//! Telegram media URL parsing and Bot API file_id decoding.

use base64::{engine::general_purpose, Engine};
use regex::Regex;

/// Decoded Bot API file_id structure.
#[derive(Debug, Clone)]
pub struct DecodedFileId {
    pub type_id: i32,
    pub dc_id: i32,
    pub id: i64,
    pub access_hash: i64,
    pub file_reference: Vec<u8>,
}

/// A parsed reference to a Telegram media file (legacy enum, kept for info handler).
#[derive(Debug, Clone)]
pub enum TelegramMediaRef {
    /// Public/private message: `t.me/c/{chat_id}/{msg_id}` or `t.me/{username}/{msg_id}`.
    Message { chat: TelegramChat, message_id: i64 },
    /// Bot API file ID.
    FileId(String),
}

#[derive(Debug, Clone)]
pub enum TelegramChat {
    /// Numeric (private) chat: channel ID or negative group ID.
    Id(i64),
    /// Username (public) channel/group.
    Username(String),
}

/// Parse a Telegram URL or file_id string into a `TelegramMediaRef` (legacy helper).
pub fn parse_telegram_url(url: &str) -> Option<TelegramMediaRef> {
    // t.me/c/{chat_id}/{msg_id}
    let private_re = Regex::new(r"t\.me/c/(\d+)/(\d+)").unwrap();
    if let Some(cap) = private_re.captures(url) {
        let chat_id: i64 = cap[1].parse().ok()?;
        let msg_id: i64 = cap[2].parse().ok()?;
        return Some(TelegramMediaRef::Message {
            chat: TelegramChat::Id(chat_id),
            message_id: msg_id,
        });
    }

    // t.me/{username}/{msg_id}
    let public_re = Regex::new(r"t\.me/([A-Za-z][A-Za-z0-9_]+)/(\d+)").unwrap();
    if let Some(cap) = public_re.captures(url) {
        let username = cap[1].to_string();
        let msg_id: i64 = cap[2].parse().ok()?;
        return Some(TelegramMediaRef::Message {
            chat: TelegramChat::Username(username),
            message_id: msg_id,
        });
    }

    // Treat anything else that looks like a Bot API file_id (base64url-ish).
    if !url.contains('/') && !url.contains('.') && url.len() > 20 {
        return Some(TelegramMediaRef::FileId(url.to_string()));
    }

    None
}

/// Decode a Bot API file_id into its components.
///
/// Ported from Python's `decode_file_id` in `mediaflow_proxy/utils/telegram.py`.
pub fn decode_file_id(file_id: &str) -> Option<DecodedFileId> {
    // Telegram uses URL-safe base64 without padding; try both with and without padding
    let decoded = general_purpose::URL_SAFE_NO_PAD
        .decode(file_id)
        .or_else(|_| general_purpose::URL_SAFE.decode(file_id))
        .ok()?;

    // RLE-decode the raw bytes
    let data = rle_decode(&decoded);

    if data.len() < 20 {
        return None;
    }

    let mut pos = 0usize;

    // type_id: 4 bytes little-endian i32
    let type_id_raw = i32::from_le_bytes(data[pos..pos + 4].try_into().ok()?);
    pos += 4;

    const TYPE_ID_FILE_REFERENCE_FLAG: i32 = 1 << 25;
    let has_reference = (type_id_raw & TYPE_ID_FILE_REFERENCE_FLAG) != 0;
    let type_id = type_id_raw & 0x00FF_FFFF;

    // dc_id: 4 bytes little-endian i32
    if pos + 4 > data.len() {
        return None;
    }
    let dc_id = i32::from_le_bytes(data[pos..pos + 4].try_into().ok()?);
    pos += 4;

    // Optional file_reference (TL byte-string)
    let file_reference = if has_reference {
        if pos >= data.len() {
            return None;
        }
        let (fref, consumed) = read_tl_bytes(&data[pos..])?;
        pos += consumed;
        fref
    } else {
        vec![]
    };

    // id: 8 bytes LE i64
    // access_hash: 8 bytes LE i64
    if pos + 16 > data.len() {
        return None;
    }
    let id = i64::from_le_bytes(data[pos..pos + 8].try_into().ok()?);
    pos += 8;
    let access_hash = i64::from_le_bytes(data[pos..pos + 8].try_into().ok()?);

    Some(DecodedFileId {
        type_id,
        dc_id,
        id,
        access_hash,
        file_reference,
    })
}

/// RLE-decode Telegram's file_id encoding.
///
/// `\x00\xNN` → `NN` zero bytes.
fn rle_decode(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == 0 && i + 1 < data.len() {
            let count = data[i + 1] as usize;
            result.extend(std::iter::repeat_n(0u8, count));
            i += 2;
        } else {
            result.push(data[i]);
            i += 1;
        }
    }
    result
}

/// Read a TL byte-string (length-prefixed) from a byte slice.
///
/// Returns `(bytes, bytes_consumed_from_slice)`.
fn read_tl_bytes(data: &[u8]) -> Option<(Vec<u8>, usize)> {
    if data.is_empty() {
        return None;
    }
    let (len, header_size) = if data[0] == 254 {
        // Long format: next 3 bytes are the length, little-endian
        if data.len() < 4 {
            return None;
        }
        let len = u32::from_le_bytes([data[1], data[2], data[3], 0]) as usize;
        (len, 4)
    } else {
        (data[0] as usize, 1)
    };

    let end = header_size + len;
    if end > data.len() {
        return None;
    }
    let content = data[header_size..end].to_vec();

    // TL strings are padded to 4-byte alignment
    let padded = (end + 3) & !3;
    let consumed = if padded <= data.len() { padded } else { end };

    Some((content, consumed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_private_link() {
        let r = parse_telegram_url("https://t.me/c/1234567890/42");
        assert!(matches!(
            r,
            Some(TelegramMediaRef::Message {
                chat: TelegramChat::Id(1234567890),
                message_id: 42
            })
        ));
    }

    #[test]
    fn test_parse_public_link() {
        let r = parse_telegram_url("https://t.me/someChannel/100");
        assert!(matches!(
            r,
            Some(TelegramMediaRef::Message {
                chat: TelegramChat::Username(_),
                message_id: 100
            })
        ));
    }

    #[test]
    fn test_rle_decode_empty() {
        assert_eq!(rle_decode(&[]), Vec::<u8>::new());
    }

    #[test]
    fn test_rle_decode_run() {
        // \x00\x03 → three zero bytes
        assert_eq!(rle_decode(&[0x00, 0x03]), vec![0, 0, 0]);
    }
}
