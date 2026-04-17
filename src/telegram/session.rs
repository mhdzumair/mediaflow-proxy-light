//! Telegram session manager.
//!
//! Wraps a `grammers_client::Client` singleton initialized from a Telethon-format
//! session string stored in `APP__TELEGRAM__SESSION_STRING`.
//!
//! On the first request that requires Telegram access the client is connected
//! lazily and cached for the lifetime of the process.

use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;

#[cfg(feature = "telegram")]
use {
    actix_web::web::Bytes,
    async_stream::stream,
    base64::{engine::general_purpose, Engine},
    futures::Stream,
    grammers_client::session::Session,
    grammers_client::{Client, Config, InitParams},
    grammers_tl_types as tl,
    std::net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    tracing::{debug, info, warn},
};

use crate::{config::TelegramConfig, error::AppError};

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

/// Lazily-initialized Telegram client, protected by a Tokio `Mutex` so that
/// only one init attempt runs at a time.
static CLIENT: OnceLock<Arc<Mutex<Option<Arc<Client>>>>> = OnceLock::new();

#[cfg(feature = "telegram")]
fn client_mutex() -> Arc<Mutex<Option<Arc<Client>>>> {
    CLIENT.get_or_init(|| Arc::new(Mutex::new(None))).clone()
}

// ---------------------------------------------------------------------------
// Session initialisation
// ---------------------------------------------------------------------------

/// Parse a Telethon `StringSession` string into `(dc_id, SocketAddr, auth_key_256)`.
///
/// Format: `'1'` + base64url( BE u8 dc_id | BE 4-byte IPv4 | BE u16 port | 256-byte auth_key )
#[cfg(feature = "telegram")]
pub fn parse_telethon_session(s: &str) -> Option<(i32, SocketAddr, [u8; 256])> {
    if !s.starts_with('1') {
        return None;
    }
    let b64 = &s[1..];
    let data = general_purpose::URL_SAFE_NO_PAD
        .decode(b64)
        .or_else(|_| general_purpose::URL_SAFE.decode(b64))
        .ok()?;

    // 1 (dc_id) + 4 (ip) + 2 (port) + 256 (auth_key) = 263
    if data.len() < 263 {
        return None;
    }

    let dc_id = data[0] as i32;
    let ip = Ipv4Addr::new(data[1], data[2], data[3], data[4]);
    let port = u16::from_be_bytes([data[5], data[6]]);
    let mut auth_key = [0u8; 256];
    auth_key.copy_from_slice(&data[7..263]);

    let addr = SocketAddr::V4(SocketAddrV4::new(ip, port));
    Some((dc_id, addr, auth_key))
}

/// Create a grammers `Client` from a Telethon session string.
#[cfg(feature = "telegram")]
async fn init_client(cfg: &TelegramConfig) -> Result<Client, String> {
    let (dc_id, addr, auth_key) = parse_telethon_session(&cfg.session_string)
        .ok_or_else(|| "Invalid or unsupported session string format".to_string())?;

    info!("Initialising Telegram client (DC {}, {})", dc_id, addr);

    let session = Session::new();
    session.insert_dc(dc_id, addr, auth_key);

    // Tell grammers which DC to connect to initially. Without this it defaults
    // to DC 2 and generates a brand-new (unauthorised) auth key there. By
    // setting a placeholder user with the correct DC, grammers picks up the
    // Telethon auth key we just inserted and connects to the right DC.
    // The user_id value doesn't matter for file downloads; we use 0.
    session.set_user(0, dc_id, false);

    let client = Client::connect(Config {
        session,
        api_id: cfg.api_id,
        api_hash: cfg.api_hash.clone(),
        params: InitParams {
            catch_up: false,
            ..Default::default()
        },
    })
    .await
    .map_err(|e| format!("Client::connect failed: {}", e))?;

    info!("Telegram client connected.");
    Ok(client)
}

/// Return the shared `Arc<Client>`, initialising it on the first call.
#[cfg(feature = "telegram")]
pub async fn get_or_init_client(cfg: &TelegramConfig) -> Result<Arc<Client>, String> {
    let mtx = client_mutex();
    let mut guard = mtx.lock().await;

    if let Some(ref client) = *guard {
        return Ok(Arc::clone(client));
    }

    // First call — connect
    let client = init_client(cfg).await?;
    let arc = Arc::new(client);
    *guard = Some(Arc::clone(&arc));
    Ok(arc)
}

/// Drop the cached client so the next `get_or_init_client` call creates a fresh one.
///
/// Called after unrecoverable DC connection failures so that a new auth key
/// negotiation is forced on the next request.
#[cfg(feature = "telegram")]
pub async fn reset_client() {
    let mtx = client_mutex();
    let mut guard = mtx.lock().await;
    if guard.is_some() {
        warn!("Resetting Telegram client singleton due to unrecoverable DC connection failure");
        *guard = None;
    }
}

// ---------------------------------------------------------------------------
// File-reference refresh helpers
// ---------------------------------------------------------------------------

/// Resolve a chat_id string to a Telegram `InputPeer`.
///
/// Accepts:
/// - `"@username"` or `"username"` — resolved via `contacts.ResolveUsername`
/// - `"-100XXXXXXXXXX"` — supergroup / channel (access_hash set to 0 for numeric-only)
/// - `"-XXXXXXX"` — legacy group chat
/// - Positive integer string — user ID
#[cfg(feature = "telegram")]
pub async fn resolve_peer_from_str(client: &Client, chat_id: &str) -> Option<tl::enums::InputPeer> {
    let stripped = chat_id.trim_start_matches('@');

    // Try numeric parse first
    if let Ok(id) = stripped.parse::<i64>() {
        if id < 0 {
            let abs = -id;
            if abs > 1_000_000_000 {
                // -100XXXXXXXXXX → channel
                return Some(tl::enums::InputPeer::Channel(tl::types::InputPeerChannel {
                    channel_id: abs - 1_000_000_000,
                    access_hash: 0,
                }));
            } else {
                return Some(tl::enums::InputPeer::Chat(tl::types::InputPeerChat {
                    chat_id: abs,
                }));
            }
        } else {
            return Some(tl::enums::InputPeer::User(tl::types::InputPeerUser {
                user_id: id,
                access_hash: 0,
            }));
        }
    }

    // Username resolution
    let result = client
        .invoke(&tl::functions::contacts::ResolveUsername {
            username: stripped.to_string(),
        })
        .await
        .ok()?;

    let tl::enums::contacts::ResolvedPeer::Peer(resolved) = result;

    match resolved.peer {
        tl::enums::Peer::User(p) => {
            let access_hash = resolved
                .users
                .iter()
                .find_map(|u| match u {
                    tl::enums::User::User(u) if u.id == p.user_id => u.access_hash,
                    _ => None,
                })
                .unwrap_or(0);
            Some(tl::enums::InputPeer::User(tl::types::InputPeerUser {
                user_id: p.user_id,
                access_hash,
            }))
        }
        tl::enums::Peer::Channel(p) => {
            let access_hash = resolved
                .chats
                .iter()
                .find_map(|c| match c {
                    tl::enums::Chat::Channel(c) if c.id == p.channel_id => c.access_hash,
                    _ => None,
                })
                .unwrap_or(0);
            Some(tl::enums::InputPeer::Channel(tl::types::InputPeerChannel {
                channel_id: p.channel_id,
                access_hash,
            }))
        }
        tl::enums::Peer::Chat(p) => Some(tl::enums::InputPeer::Chat(tl::types::InputPeerChat {
            chat_id: p.chat_id,
        })),
    }
}

/// Fresh document info obtained from the user's own MTProto session.
/// All fields come from the same `Document` object so they're consistent.
#[cfg(feature = "telegram")]
pub struct FreshDocumentInfo {
    pub file_reference: Vec<u8>,
    pub access_hash: i64,
    pub dc_id: i32,
}

/// Extract a full `FreshDocumentInfo` for `document_id` from a `messages::Messages` result.
/// Returns ALL fields from the found document, not just file_reference, because:
/// - `access_hash` in a Bot API file_id is bot-session-specific (won't work for user MTProto)
/// - `dc_id` from the message document is authoritative
#[cfg(feature = "telegram")]
fn extract_doc_info_from_msgs(
    result: &tl::enums::messages::Messages,
    document_id: i64,
) -> Option<FreshDocumentInfo> {
    let messages: &[tl::enums::Message] = match result {
        tl::enums::messages::Messages::Messages(m) => &m.messages,
        tl::enums::messages::Messages::Slice(m) => &m.messages,
        tl::enums::messages::Messages::ChannelMessages(m) => &m.messages,
        tl::enums::messages::Messages::NotModified(_) => return None,
    };

    for msg in messages {
        if let tl::enums::Message::Message(m) = msg {
            if let Some(tl::enums::MessageMedia::Document(md)) = &m.media {
                if let Some(tl::enums::Document::Document(doc)) = &md.document {
                    if doc.id == document_id {
                        return Some(FreshDocumentInfo {
                            file_reference: doc.file_reference.clone(),
                            access_hash: doc.access_hash,
                            dc_id: doc.dc_id,
                        });
                    }
                }
            }
        }
    }
    None
}

/// Obtain fresh document info (file_reference, access_hash, dc_id) by scanning chat history.
///
/// Strategy:
/// 1. If `message_id` is provided, try `messages.GetMessages` first (fast path).
/// 2. Fall back to scanning up to `scan_limit` messages via `messages.GetHistory`.
///
/// Returns `None` if the document is not found or the chat cannot be resolved.
#[cfg(feature = "telegram")]
pub async fn get_fresh_document_info(
    client: Arc<Client>,
    chat_id: &str,
    document_id: i64,
    message_id: Option<i32>,
    scan_limit: usize,
) -> Option<FreshDocumentInfo> {
    let peer = resolve_peer_from_str(&client, chat_id).await?;

    // Fast path: direct lookup if we have the message_id
    if let Some(mid) = message_id {
        if let Ok(result) = client
            .invoke(&tl::functions::messages::GetMessages {
                id: vec![tl::enums::InputMessage::Id(tl::types::InputMessageId {
                    id: mid,
                })],
            })
            .await
        {
            if let Some(info) = extract_doc_info_from_msgs(&result, document_id) {
                info!(
                    "Got fresh doc info via GetMessages (message_id={}): fref={}, dc={}",
                    mid,
                    hex::encode(&info.file_reference),
                    info.dc_id
                );
                return Some(info);
            }
        }
    }

    // Scan history in batches of 100
    let batch = 100i32;
    let max_batches = scan_limit.div_ceil(100);
    let mut offset_id = 0i32;

    'outer: for batch_idx in 0..max_batches {
        // Retry on transient IO disconnects (grammers "read 0 bytes")
        let result = {
            let mut attempt = 0u32;
            loop {
                let r = client
                    .invoke(&tl::functions::messages::GetHistory {
                        peer: peer.clone(),
                        offset_id,
                        offset_date: 0,
                        add_offset: 0,
                        limit: batch,
                        max_id: 0,
                        min_id: 0,
                        hash: 0,
                    })
                    .await;
                match r {
                    Err(grammers_client::InvocationError::Read(_)) if attempt < 3 => {
                        attempt += 1;
                        let delay_ms = 300u64 * (1 << attempt.min(4));
                        warn!(
                            "History scan IO disconnect (batch {}); retry {}/3 in {}ms",
                            batch_idx, attempt, delay_ms
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                        continue;
                    }
                    other => break other,
                }
            }
        };

        match result {
            Ok(result) => {
                if let Some(info) = extract_doc_info_from_msgs(&result, document_id) {
                    info!(
                        "Got fresh doc info via history scan (batch {}): fref={}, dc={}, access_hash={}",
                        batch_idx,
                        hex::encode(&info.file_reference),
                        info.dc_id,
                        info.access_hash
                    );
                    return Some(info);
                }

                // Determine offset for next page
                let messages: &[tl::enums::Message] = match &result {
                    tl::enums::messages::Messages::Messages(m) => &m.messages,
                    tl::enums::messages::Messages::Slice(m) => &m.messages,
                    tl::enums::messages::Messages::ChannelMessages(m) => &m.messages,
                    tl::enums::messages::Messages::NotModified(_) => break 'outer,
                };

                if messages.len() < batch as usize {
                    break 'outer; // no more messages
                }

                // offset_id = smallest message id in this batch
                offset_id = messages
                    .iter()
                    .filter_map(|m| match m {
                        tl::enums::Message::Message(m) => Some(m.id),
                        _ => None,
                    })
                    .min()
                    .unwrap_or(0);

                if offset_id == 0 {
                    break 'outer;
                }
            }
            Err(e) => {
                warn!("History scan error (batch {}): {}", batch_idx, e);
                break 'outer;
            }
        }
    }

    warn!(
        "document_id {} not found in {} messages of chat {}",
        document_id, scan_limit, chat_id
    );
    None
}

// ---------------------------------------------------------------------------
// Streaming helpers
// ---------------------------------------------------------------------------

/// Minimum chunk alignment required by `upload.GetFile` (4 KiB).
#[cfg(feature = "telegram")]
const MIN_CHUNK: i32 = 4 * 1024;
/// Maximum chunk size accepted by `upload.GetFile` (512 KiB).
#[cfg(feature = "telegram")]
const MAX_CHUNK: i32 = 512 * 1024;

/// Round `n` up to the nearest multiple of `align`.
#[cfg(feature = "telegram")]
fn align_up(n: i32, align: i32) -> i32 {
    ((n + align - 1) / align) * align
}

/// Fetch a single aligned chunk from Telegram, with in-place reconnect on IO failures.
///
/// Returns `Ok(None)` on clean EOF (empty bytes from server).
#[cfg(feature = "telegram")]
async fn fetch_chunk(
    client: &mut Arc<Client>,
    cfg: &crate::config::TelegramConfig,
    location: &tl::enums::InputFileLocation,
    dc_id: i32,
    chunk_offset: u64,
    reconnect_count: &mut u32,
) -> Result<Option<Vec<u8>>, AppError> {
    const MAX_RECONNECTS: u32 = 6;

    loop {
        debug!(
            "tg fetch: dc={} offset={} limit={}",
            dc_id, chunk_offset, MAX_CHUNK
        );

        let request = tl::functions::upload::GetFile {
            precise: false,
            cdn_supported: false,
            location: location.clone(),
            offset: chunk_offset as i64,
            limit: MAX_CHUNK,
        };

        // Go directly to the file's DC — skip the home-DC roundtrip that always
        // returns FILE_MIGRATE_4 and doubles per-chunk latency.
        let r = match client.invoke_in_dc(&request, dc_id).await {
            Err(grammers_client::InvocationError::Read(_)) => {
                Err(grammers_client::InvocationError::Dropped)
            }
            other => other,
        };

        let is_io_err = matches!(
            &r,
            Err(grammers_client::InvocationError::Read(_))
                | Err(grammers_client::InvocationError::Dropped)
        );

        if is_io_err {
            if *reconnect_count >= MAX_RECONNECTS {
                return Err(AppError::Telegram(format!(
                    "Too many DC reconnects ({}) at offset {}",
                    MAX_RECONNECTS, chunk_offset
                )));
            }
            *reconnect_count += 1;
            warn!(
                "Telegram DC{} IO disconnect at offset {}; reconnecting ({}/{})",
                dc_id, chunk_offset, reconnect_count, MAX_RECONNECTS
            );
            reset_client().await;
            tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
            *client = get_or_init_client(cfg)
                .await
                .map_err(|e| AppError::Telegram(format!("Reconnect failed: {}", e)))?;
            info!(
                "Telegram reconnected (dc{}); retrying offset {}",
                dc_id, chunk_offset
            );
            continue; // retry same chunk with fresh client
        }

        return match r {
            Ok(tl::enums::upload::File::File(f)) => {
                debug!(
                    "tg fetch: dc={} offset={} got {} bytes",
                    dc_id,
                    chunk_offset,
                    f.bytes.len()
                );
                if f.bytes.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(f.bytes))
                }
            }
            Ok(tl::enums::upload::File::CdnRedirect(_)) => {
                Err(AppError::Telegram("CDN redirect not supported".into()))
            }
            Err(e) => Err(AppError::Telegram(e.to_string())),
        };
    }
}

/// Stream bytes `[start, end]` (both inclusive) from a Telegram document.
///
/// Sequential fetch using aligned MAX_CHUNK requests.
/// ECONNABORTED / "read 0 bytes" (common on Android): the broken client is
/// replaced in-place via `fetch_chunk` and the same chunk is retried — the
/// HTTP body keeps flowing without the player seeing a failure or having to
/// restart from scratch.
#[cfg(feature = "telegram")]
pub async fn stream_document_range(
    client: Arc<Client>,
    cfg: crate::config::TelegramConfig,
    location: tl::enums::InputFileLocation,
    dc_id: i32,
    start: u64,
    end: u64,
) -> impl Stream<Item = Result<Bytes, AppError>> {
    let aligned_start = (start / MAX_CHUNK as u64) * MAX_CHUNK as u64;
    debug!(
        "tg stream: start={} end={} total={} aligned_start={} dc={}",
        start,
        end,
        end - start + 1,
        aligned_start,
        dc_id
    );

    stream! {
        let mut current_client = client;
        let mut chunk_offset = aligned_start;
        let mut bytes_emitted: u64 = 0;
        let total_needed = end - start + 1;
        let mut reconnect_count = 0u32;

        loop {
            if bytes_emitted >= total_needed {
                debug!("tg stream: done, emitted={} needed={}", bytes_emitted, total_needed);
                break;
            }

            match fetch_chunk(&mut current_client, &cfg, &location, dc_id, chunk_offset, &mut reconnect_count).await {
                Err(e) => {
                    warn!("Telegram fetch error at offset {}: {}", chunk_offset, e);
                    yield Err(e);
                    break;
                }
                Ok(None) => {
                    debug!("tg stream: EOF at offset={} emitted={}", chunk_offset, bytes_emitted);
                    break;
                }
                Ok(Some(raw_bytes)) => {
                    let chunk_len = raw_bytes.len() as u64;
                    let eof = (chunk_len as i32) < MAX_CHUNK;

                    let chunk_file_start = chunk_offset;
                    let chunk_file_end   = chunk_offset + chunk_len - 1;

                    let slice_start = start.max(chunk_file_start);
                    let slice_end   = end.min(chunk_file_end);

                    if slice_start <= slice_end {
                        let from = (slice_start - chunk_file_start) as usize;
                        let to   = (slice_end   - chunk_file_start + 1) as usize;
                        let payload = Bytes::copy_from_slice(&raw_bytes[from..to]);
                        bytes_emitted += payload.len() as u64;
                        debug!("tg stream: yield {} bytes (offset={} slice={}..{} emitted={})",
                            payload.len(), chunk_offset, slice_start, slice_end, bytes_emitted);
                        yield Ok(payload);
                    } else {
                        debug!("tg stream: skip chunk offset={} (no overlap with {}-{})", chunk_offset, start, end);
                    }

                    chunk_offset += chunk_len;

                    if eof || chunk_offset > end {
                        break;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Legacy session-manager struct (kept for /status handler)
// ---------------------------------------------------------------------------

pub struct TelegramSessionManager {
    pub session_file: String,
    pub is_connected: bool,
}

impl TelegramSessionManager {
    pub fn new(session_file: impl Into<String>) -> Self {
        Self {
            session_file: session_file.into(),
            is_connected: false,
        }
    }

    pub fn is_authorized(&self) -> bool {
        // Check if the global client is initialised
        CLIENT
            .get()
            .and_then(|m| m.try_lock().ok())
            .map(|g| g.is_some())
            .unwrap_or(false)
    }
}

static MANAGER: OnceLock<Arc<tokio::sync::RwLock<TelegramSessionManager>>> = OnceLock::new();

pub fn get_manager() -> Arc<tokio::sync::RwLock<TelegramSessionManager>> {
    MANAGER
        .get_or_init(|| {
            Arc::new(tokio::sync::RwLock::new(TelegramSessionManager::new(
                "telegram.session",
            )))
        })
        .clone()
}
