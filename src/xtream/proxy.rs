//! Xtream Codes upstream forwarding and URL rewriting.
//!
//! `forward_api_request` fetches an upstream XC API URL (player_api.php,
//! panel_api.php, etc.), rewrites all stream URLs in the JSON response to
//! route through *this* MediaFlow instance, and returns the response.

use actix_web::HttpRequest;
use reqwest::header::HeaderMap;

use crate::{
    error::{AppError, AppResult},
    proxy::stream::StreamManager,
    xtream::auth::encode_username_token,
};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Forward an XC API request to upstream and rewrite stream URLs.
pub async fn forward_api_request(
    upstream_url: &str,
    req: &HttpRequest,
    stream_manager: &StreamManager,
    upstream_base: &str,
    actual_username: &str,
    api_password: Option<&str>,
) -> AppResult<(Vec<u8>, String)> {
    let raw = stream_manager
        .fetch_bytes(upstream_url.to_string(), HeaderMap::new())
        .await
        .map_err(|e| {
            tracing::warn!("XC upstream error for {upstream_url}: {e}");
            e
        })?;

    let content_type = "application/json; charset=utf-8".to_string();

    let body_str = match std::str::from_utf8(&raw) {
        Ok(s) => s,
        Err(_) => return Ok((raw.to_vec(), content_type)),
    };

    // Only rewrite JSON bodies.
    let mediaflow_base = proxy_base_url(req);
    let rewritten = rewrite_urls_for_api(
        body_str,
        upstream_base,
        &mediaflow_base,
        actual_username,
        api_password,
    );

    Ok((rewritten.into_bytes(), content_type))
}

/// Get the public-facing base URL of this MediaFlow instance.
pub fn proxy_base_url(req: &HttpRequest) -> String {
    let conn = req.connection_info();
    // Prefer X-Forwarded-Proto / X-Forwarded-Host if behind a reverse proxy.
    let scheme = req
        .headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_else(|| conn.scheme())
        .to_string();
    let host = req
        .headers()
        .get("x-forwarded-host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_else(|| conn.host())
        .to_string();
    format!("{scheme}://{host}")
}

// ---------------------------------------------------------------------------
// URL rewriting
// ---------------------------------------------------------------------------

/// Rewrite all upstream stream URLs in an XC API JSON response so that they
/// route through this MediaFlow proxy, and replace the `username` field so
/// that subsequent player API calls are also routed correctly.
pub fn rewrite_urls_for_api(
    content: &str,
    upstream_base: &str,
    mediaflow_base: &str,
    actual_username: &str,
    api_password: Option<&str>,
) -> String {
    // Strip the trailing slash for origin matching.
    let upstream_origin = upstream_base.trim_end_matches('/');

    let encoded_username = encode_username_token(upstream_base, actual_username, api_password);

    // Parse upstream origin for hostname-only fallback.
    let upstream_host_only: Option<String> = {
        if let Some(host_part) = upstream_origin.split("://").nth(1) {
            // host_part may be  "host:port"
            if let Some(host) = host_part.split(':').next() {
                // Only relevant when there IS a non-standard port.
                if host_part.contains(':') {
                    let scheme = upstream_origin.split("://").next().unwrap_or("http");
                    Some(format!("{scheme}://{host}"))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    };

    let mut out = content.to_string();

    // 1. Replace upstream origin → mediaflow_base in plain URLs.
    out = out.replace(upstream_origin, mediaflow_base);

    // 2. Replace escaped JSON variant (`\/` instead of `/`).
    let escaped_upstream = upstream_origin.replace('/', "\\/");
    let escaped_mediaflow = mediaflow_base.replace('/', "\\/");
    out = out.replace(&escaped_upstream, &escaped_mediaflow);

    // 3. Handle hostname-only variant (without non-standard port).
    if let Some(ref host_only) = upstream_host_only {
        out = out.replace(host_only.as_str(), mediaflow_base);
        let escaped_host_only = host_only.replace('/', "\\/");
        out = out.replace(&escaped_host_only, &escaped_mediaflow);
    }

    // 4. Replace the actual username with the encoded token in stream paths.
    //    Patterns: /live/{user}/, /movie/{user}/, /series/{user}/, /{user}/ (short).
    //    We do a simple string replacement of the username segment.
    let user_slash = format!("/{actual_username}/");
    let token_slash = format!("/{encoded_username}/");
    out = out.replace(&user_slash, &token_slash);

    // Escaped JSON variant.
    let escaped_user_slash = format!("\\/{actual_username}\\/");
    let escaped_token_slash = format!("\\/{encoded_username}\\/");
    out = out.replace(&escaped_user_slash, &escaped_token_slash);

    // 5. Rewrite `"username":"actual_username"` in user_info so that IPTV
    //    players (e.g. Tivimate) use the token for subsequent API calls.
    let old_username_field = format!(r#""username":"{}""#, actual_username);
    let new_username_field = format!(r#""username":"{}""#, encoded_username);
    out = out.replace(&old_username_field, &new_username_field);

    out
}

// ---------------------------------------------------------------------------
// Stream proxy helper
// ---------------------------------------------------------------------------

/// Proxy a raw upstream stream (live, VOD, series, timeshift) to the client.
/// Returns `(response_headers, body_bytes)`.
pub async fn proxy_upstream_stream(
    upstream_url: &str,
    request_headers: HeaderMap,
    stream_manager: &StreamManager,
) -> AppResult<(reqwest::header::HeaderMap, bytes::Bytes)> {
    let response = stream_manager
        .make_request(upstream_url.to_string(), request_headers)
        .await?;

    let headers = response.headers().clone();
    let body = response
        .bytes()
        .await
        .map_err(|e| AppError::Proxy(format!("Failed to read upstream stream: {e}")))?;

    Ok((headers, body))
}

// ---------------------------------------------------------------------------
// Build upstream URL helpers
// ---------------------------------------------------------------------------

/// Construct a full upstream URL by joining a base URL with a relative path.
pub fn build_upstream_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}
