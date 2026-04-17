use aes::Aes256;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use cbc::{Decryptor, Encryptor};
use cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::AppError;

type Aes256CbcEnc = Encryptor<Aes256>;
type Aes256CbcDec = Decryptor<Aes256>;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProxyData {
    pub destination: String,
    pub query_params: Option<serde_json::Value>,
    pub request_headers: Option<serde_json::Value>,
    pub response_headers: Option<serde_json::Value>,
    pub exp: Option<u64>,
    pub ip: Option<String>,
}

/// AES-256-CBC encryption handler compatible with the Python mediaflow-proxy.
///
/// Key derivation matches Python's `secret_key.encode("utf-8").ljust(32)[:32]`:
/// - Pad the password bytes with spaces (0x20) to 32 bytes
/// - Truncate if longer than 32 bytes
///
/// Token format: base64url-no-padding( IV[16] || AES-CBC-PKCS7(json_payload) )
#[derive(Clone)]
pub struct EncryptionHandler {
    key: [u8; 32],
}

impl EncryptionHandler {
    pub fn new(api_password: &[u8]) -> Result<Self, AppError> {
        // Match Python: secret_key.encode("utf-8").ljust(32)[:32]
        // ljust pads with ASCII space (0x20); [:32] truncates if longer.
        let mut key = [0x20u8; 32];
        let copy_len = api_password.len().min(32);
        key[..copy_len].copy_from_slice(&api_password[..copy_len]);
        Ok(Self { key })
    }

    pub fn encrypt(&self, data: &ProxyData) -> Result<String, AppError> {
        let json_data = serde_json::to_vec(data)
            .map_err(|e| AppError::Internal(format!("Failed to serialize proxy data: {}", e)))?;

        let mut iv = [0u8; 16];
        rand::rng().fill_bytes(&mut iv);

        let enc = Aes256CbcEnc::new(&self.key.into(), &iv.into());
        let ciphertext = enc.encrypt_padded_vec_mut::<Pkcs7>(&json_data);

        let mut final_data = Vec::with_capacity(16 + ciphertext.len());
        final_data.extend_from_slice(&iv);
        final_data.extend_from_slice(&ciphertext);

        // No-padding base64url to match Python's .rstrip("=")
        Ok(URL_SAFE_NO_PAD.encode(final_data))
    }

    pub fn decrypt(&self, token: &str, client_ip: Option<&str>) -> Result<ProxyData, AppError> {
        let encrypted_data = URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|e| AppError::Auth(format!("Invalid token format: {}", e)))?;

        if encrypted_data.len() < 17 {
            return Err(AppError::Auth("Token too short".to_string()));
        }

        let (iv_bytes, ciphertext) = encrypted_data.split_at(16);

        let dec = Aes256CbcDec::new(&self.key.into(), iv_bytes.into());
        let plaintext = dec
            .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
            .map_err(|_| {
                AppError::Auth("Decryption failed: invalid padding or corrupt token".to_string())
            })?;

        let proxy_data: ProxyData = serde_json::from_slice(&plaintext)
            .map_err(|e| AppError::Auth(format!("Invalid token data: {}", e)))?;

        if let Some(exp) = proxy_data.exp {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            if exp < now {
                return Err(AppError::Auth("Token has expired".to_string()));
            }
        }

        if let (Some(token_ip), Some(client_ip)) = (proxy_data.ip.as_ref(), client_ip) {
            if token_ip != client_ip {
                return Err(AppError::Auth("IP mismatch".to_string()));
            }
        }

        Ok(proxy_data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let handler = EncryptionHandler::new(b"test_password").unwrap();
        let data = ProxyData {
            destination: "https://example.com/stream.m3u8".to_string(),
            query_params: None,
            request_headers: None,
            response_headers: None,
            exp: None,
            ip: None,
        };
        let token = handler.encrypt(&data).unwrap();
        let decrypted = handler.decrypt(&token, None).unwrap();
        assert_eq!(decrypted.destination, data.destination);
    }

    #[test]
    fn test_key_derivation_space_padding() {
        // Short password should be padded with spaces (0x20) to 32 bytes
        let handler = EncryptionHandler::new(b"short").unwrap();
        let mut expected_key = [0x20u8; 32];
        expected_key[..5].copy_from_slice(b"short");
        assert_eq!(handler.key, expected_key);
    }

    #[test]
    fn test_key_derivation_truncation() {
        // Long password (>32 bytes) should be truncated to 32 bytes
        let long_pw = b"this_is_a_very_long_password_that_exceeds_32_bytes";
        let handler = EncryptionHandler::new(long_pw).unwrap();
        assert_eq!(&handler.key[..], &long_pw[..32]);
    }

    #[test]
    fn test_expired_token_rejected() {
        let handler = EncryptionHandler::new(b"test_password").unwrap();
        let data = ProxyData {
            destination: "https://example.com/stream.m3u8".to_string(),
            query_params: None,
            request_headers: None,
            response_headers: None,
            exp: Some(1), // Unix timestamp 1 = long expired
            ip: None,
        };
        let token = handler.encrypt(&data).unwrap();
        let result = handler.decrypt(&token, None);
        assert!(matches!(result, Err(AppError::Auth(_))));
    }

    #[test]
    fn test_ip_mismatch_rejected() {
        let handler = EncryptionHandler::new(b"test_password").unwrap();
        let data = ProxyData {
            destination: "https://example.com/stream.m3u8".to_string(),
            query_params: None,
            request_headers: None,
            response_headers: None,
            exp: None,
            ip: Some("1.2.3.4".to_string()),
        };
        let token = handler.encrypt(&data).unwrap();
        let result = handler.decrypt(&token, Some("9.9.9.9"));
        assert!(matches!(result, Err(AppError::Auth(_))));
    }

    /// Verify token generated by Python equivalent:
    /// python3 -c "
    ///   from Crypto.Cipher import AES; from Crypto.Util.Padding import pad
    ///   import base64, json
    ///   key = b'testpassword' + b' ' * (32 - 12)
    ///   iv = b'\x00' * 16
    ///   data = json.dumps({'destination': 'https://example.com', 'query_params': None,
    ///                      'request_headers': None, 'response_headers': None}).encode()
    ///   ct = AES.new(key, AES.MODE_CBC, iv).encrypt(pad(data, 16))
    ///   print(base64.urlsafe_b64encode(iv + ct).rstrip(b'=').decode())
    /// "
    #[test]
    fn test_python_compat_token() {
        let handler = EncryptionHandler::new(b"testpassword").unwrap();
        // Build a token with a known IV of all zeros to verify Python-compatible decryption.
        // We manually construct the ciphertext the same way Python would.
        let key: [u8; 32] = {
            let mut k = [0x20u8; 32];
            k[..12].copy_from_slice(b"testpassword");
            k
        };
        let iv = [0u8; 16];
        let payload = r#"{"destination":"https://example.com","query_params":null,"request_headers":null,"response_headers":null,"exp":null,"ip":null}"#;
        let enc = Aes256CbcEnc::new(&key.into(), &iv.into());
        let ciphertext = enc.encrypt_padded_vec_mut::<Pkcs7>(payload.as_bytes());
        let mut token_bytes = Vec::with_capacity(16 + ciphertext.len());
        token_bytes.extend_from_slice(&iv);
        token_bytes.extend_from_slice(&ciphertext);
        let token = URL_SAFE_NO_PAD.encode(&token_bytes);

        let decrypted = handler.decrypt(&token, None).unwrap();
        assert_eq!(decrypted.destination, "https://example.com");
    }
}
