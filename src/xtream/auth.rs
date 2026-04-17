//! Xtream Codes username parsing and API-password verification.
//!
//! Username encodes the upstream server URL (plus optional MediaFlow API password)
//! in one of two formats:
//!
//! **New format** — entire username is base64url of `{upstream_url}:{xc_username}[:{api_password}]`
//! **Legacy format** — `{base64_upstream}:{xc_username}[:{api_password}]`
//!
//! The XC *password* field is passed through unchanged to the upstream server.

use base64::{engine::general_purpose, Engine};
use tracing::debug;

use crate::error::{AppError, AppResult};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Result of parsing a packed XC username field.
#[derive(Debug, Clone)]
pub struct XcCredentials {
    /// Upstream XC server base URL (always ends with `/`).
    pub upstream_base: String,
    /// The real XC username to present to the upstream server.
    pub actual_username: String,
    /// MediaFlow API password, if embedded in the username.
    pub api_password: Option<String>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a packed Xtream Codes username into its components.
///
/// Supports the new base64-encoded format and the legacy colon-separated format.
pub fn parse_username_with_upstream(username: &str) -> AppResult<XcCredentials> {
    // Try new format: entire username is base64url of compound string.
    if let Some(creds) = try_parse_base64_username(username) {
        return Ok(creds);
    }

    // Legacy format: {base64_upstream}:{actual_username}[:{api_password}]
    parse_legacy_username(username)
}

/// Verify the API password extracted from a username against the server config.
///
/// If `configured_password` is empty, all requests are allowed.
pub fn verify_xc_api_password(
    embedded_password: Option<&str>,
    configured_password: &str,
) -> AppResult<()> {
    // Empty server password → allow all.
    if configured_password.is_empty() {
        return Ok(());
    }

    match embedded_password {
        None => Err(AppError::Forbidden(
            "API password required. Username format: base64({upstream}:{user}:{api_password})"
                .into(),
        )),
        Some(p) if p == configured_password => Ok(()),
        Some(_) => Err(AppError::Forbidden("Invalid API password".into())),
    }
}

/// Build a base64url token that encodes upstream URL + username (+ optional api_password).
/// Used when rewriting stream URLs so IPTV players can make subsequent requests.
pub fn encode_username_token(
    upstream_base: &str,
    actual_username: &str,
    api_password: Option<&str>,
) -> String {
    let upstream_clean = upstream_base.trim_end_matches('/');
    let combined = match api_password {
        Some(p) if !p.is_empty() => format!("{upstream_clean}:{actual_username}:{p}"),
        _ => format!("{upstream_clean}:{actual_username}"),
    };
    general_purpose::URL_SAFE_NO_PAD.encode(combined.as_bytes())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn try_parse_base64_username(username: &str) -> Option<XcCredentials> {
    // Normalise URL-safe characters and add padding.
    let normalised = username.replace('-', "+").replace('_', "/");
    let padded = match normalised.len() % 4 {
        2 => format!("{normalised}=="),
        3 => format!("{normalised}="),
        _ => normalised.clone(),
    };

    let decoded = general_purpose::STANDARD.decode(&padded).ok()?;
    let decoded_str = String::from_utf8(decoded).ok()?;

    // Must look like a URL.
    if !decoded_str.contains("://") || !decoded_str.contains(':') {
        return None;
    }

    debug!("XC: decoded base64 username: {decoded_str}");

    // Split on `://` to isolate the protocol.
    let (proto, rest) = decoded_str.split_once("://")?;
    let parts: Vec<&str> = rest.splitn(4, ':').collect();

    let (upstream_url, actual_username, api_password) = match parts.as_slice() {
        [host, actual_user] => (format!("{proto}://{host}"), actual_user.to_string(), None),
        [host, second, third] => {
            if second.chars().all(|c| c.is_ascii_digit()) && second.len() <= 5 {
                // host:port:username
                (
                    format!("{proto}://{host}:{second}"),
                    third.to_string(),
                    None,
                )
            } else {
                // host:username:api_password
                (
                    format!("{proto}://{host}"),
                    second.to_string(),
                    Some(third.to_string()),
                )
            }
        }
        [host, port, actual_user, api_pwd] => (
            format!("{proto}://{host}:{port}"),
            actual_user.to_string(),
            Some(api_pwd.to_string()),
        ),
        _ => return None,
    };

    let api_password = api_password.filter(|p| !p.is_empty());
    let upstream_base = ensure_trailing_slash(upstream_url);

    Some(XcCredentials {
        upstream_base,
        actual_username,
        api_password,
    })
}

fn parse_legacy_username(username: &str) -> AppResult<XcCredentials> {
    let parts: Vec<&str> = username.splitn(3, ':').collect();
    let (upstream_encoded, actual_username, api_password) = match parts.as_slice() {
        [enc, user] => (*enc, *user, None),
        [enc, user, pwd] => (*enc, *user, Some(*pwd)),
        _ => {
            return Err(AppError::BadRequest(
                "Invalid XC username format. Expected base64url-encoded string or \
                 legacy `{base64_upstream}:{username}[:{api_password}]`."
                    .into(),
            ))
        }
    };

    let upstream_base = decode_upstream_url(upstream_encoded)?;
    let api_password = api_password.filter(|p| !p.is_empty()).map(String::from);

    Ok(XcCredentials {
        upstream_base,
        actual_username: actual_username.to_string(),
        api_password,
    })
}

fn decode_upstream_url(encoded: &str) -> AppResult<String> {
    // Try URL-safe base64 first, then standard.
    let normalised = encoded.replace('-', "+").replace('_', "/");
    let padded = match normalised.len() % 4 {
        2 => format!("{normalised}=="),
        3 => format!("{normalised}="),
        _ => normalised,
    };

    let decoded = general_purpose::STANDARD
        .decode(&padded)
        .map_err(|_| AppError::BadRequest("Invalid base64 upstream URL encoding".into()))?;

    let url = String::from_utf8(decoded)
        .map_err(|_| AppError::BadRequest("Upstream URL is not valid UTF-8".into()))?;

    Ok(ensure_trailing_slash(url))
}

fn ensure_trailing_slash(mut url: String) -> String {
    if !url.ends_with('/') {
        url.push('/');
    }
    url
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legacy_format_two_parts() {
        // base64("http://example.com") = "aHR0cDovL2V4YW1wbGUuY29t"
        let encoded = general_purpose::STANDARD.encode("http://example.com");
        let username = format!("{encoded}:myuser");
        let creds = parse_username_with_upstream(&username).unwrap();
        assert_eq!(creds.upstream_base, "http://example.com/");
        assert_eq!(creds.actual_username, "myuser");
        assert!(creds.api_password.is_none());
    }

    #[test]
    fn test_legacy_format_three_parts() {
        let encoded = general_purpose::STANDARD.encode("http://example.com:8080");
        let username = format!("{encoded}:myuser:secret");
        let creds = parse_username_with_upstream(&username).unwrap();
        assert_eq!(creds.upstream_base, "http://example.com:8080/");
        assert_eq!(creds.actual_username, "myuser");
        assert_eq!(creds.api_password.as_deref(), Some("secret"));
    }

    #[test]
    fn test_new_base64_format_no_port() {
        let combined = "http://example.com:myuser:secret";
        let encoded = general_purpose::URL_SAFE_NO_PAD.encode(combined.as_bytes());
        let creds = parse_username_with_upstream(&encoded).unwrap();
        assert_eq!(creds.upstream_base, "http://example.com/");
        assert_eq!(creds.actual_username, "myuser");
        assert_eq!(creds.api_password.as_deref(), Some("secret"));
    }

    #[test]
    fn test_encode_username_token_roundtrip() {
        let token = encode_username_token("http://upstream.tv:8888", "alice", Some("pw123"));
        let creds = parse_username_with_upstream(&token).unwrap();
        assert_eq!(creds.upstream_base, "http://upstream.tv:8888/");
        assert_eq!(creds.actual_username, "alice");
        assert_eq!(creds.api_password.as_deref(), Some("pw123"));
    }
}
