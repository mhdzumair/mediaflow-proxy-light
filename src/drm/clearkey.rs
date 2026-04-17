//! ClearKey JWKS key fetching.
//!
//! Fetches a ClearKey license server URL that returns JWKS (JSON Web Key Set)
//! format, decodes the base64url-encoded key material, and returns a map of
//! KID (bytes) → key (bytes).

use std::collections::HashMap;

use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

// ---------------------------------------------------------------------------
// JWKS response structures
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<JwksKey>,
}

#[derive(Debug, Deserialize)]
struct JwksKey {
    /// Key type — always "oct" for ClearKey.
    #[serde(default)]
    kty: String,
    /// Key ID, base64url-encoded (no padding).
    kid: String,
    /// Key value, base64url-encoded (no padding).
    k: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Fetch ClearKey keys from a JWKS endpoint.
///
/// Returns a map of KID bytes → key bytes, or an error string.
pub async fn fetch_clearkey_keys(
    la_url: &str,
    client: &Client,
    key_ids: &[Vec<u8>],
) -> Result<HashMap<Vec<u8>, Vec<u8>>, String> {
    // Build the ClearKey license request body
    let kid_strings: Vec<String> = key_ids.iter().map(|kid| base64_url_encode(kid)).collect();

    let request_body = serde_json::json!({
        "kids": kid_strings,
        "type": "temporary"
    });

    debug!("Requesting ClearKey license from {}", la_url);

    let response = client
        .post(la_url)
        .header("content-type", "application/json")
        .json(&request_body)
        .send()
        .await
        .map_err(|e| format!("ClearKey request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "ClearKey server returned {}: {}",
            response.status(),
            la_url
        ));
    }

    let jwks: JwksResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse JWKS response: {e}"))?;

    let mut key_map = HashMap::new();
    for key in &jwks.keys {
        let kid_bytes = base64_url_decode(&key.kid)
            .map_err(|e| format!("Failed to decode kid '{}': {e}", key.kid))?;
        let key_bytes =
            base64_url_decode(&key.k).map_err(|e| format!("Failed to decode key material: {e}"))?;
        debug!("ClearKey: loaded key for KID {}", hex::encode(&kid_bytes));
        key_map.insert(kid_bytes, key_bytes);
    }

    Ok(key_map)
}

// ---------------------------------------------------------------------------
// Helper: Base64URL encode/decode (no padding)
// ---------------------------------------------------------------------------

/// Encode bytes as base64url (no padding), matching Python's `base64.urlsafe_b64encode(...).rstrip(b"=")`.
pub fn base64_url_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// Decode base64url string (with or without padding) to bytes.
pub fn base64_url_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    // Add padding if needed
    let padded = match s.len() % 4 {
        2 => format!("{s}=="),
        3 => format!("{s}="),
        _ => s.to_string(),
    };
    base64::engine::general_purpose::URL_SAFE
        .decode(&padded)
        .map_err(|e| format!("base64url decode error: {e}"))
}

// ---------------------------------------------------------------------------
// Parse key_id / key hex strings to key_map
// ---------------------------------------------------------------------------

/// Build a `KID bytes → key bytes` map from (possibly comma-separated) hex strings.
/// This mirrors `_build_key_map()` in the Python implementation.
pub fn build_key_map_from_hex(key_id: &str, key: &str) -> HashMap<Vec<u8>, Vec<u8>> {
    let key_ids: Vec<&str> = key_id
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let keys: Vec<&str> = key
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    key_ids
        .iter()
        .zip(keys.iter())
        .filter_map(|(kid_hex, key_hex)| {
            let kid = hex::decode(kid_hex.replace('-', "")).ok()?;
            let k = hex::decode(key_hex.replace('-', "")).ok()?;
            Some((kid, k))
        })
        .collect()
}
