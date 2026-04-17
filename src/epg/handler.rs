//! EPG Proxy — fetch and cache XMLTV/EPG data from upstream sources.
//!
//! Route: `GET /proxy/epg?d=<url>&api_password=<key>`
//!
//! Compatible with Channels DVR, Plex, Emby, and all XMLTV-based EPG clients.

use actix_web::{web, HttpRequest, HttpResponse};
use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use crate::{
    cache::local::LocalCache,
    config::Config,
    error::{AppError, AppResult},
    proxy::stream::StreamManager,
    utils::base64_url::decode_base64_url,
};

/// Newtype wrapper so this cache can coexist with the MPD `LocalCache` in Actix DI.
pub struct EpgCache(pub LocalCache);

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub async fn epg_proxy_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    config: web::Data<Arc<Config>>,
    epg_cache: web::Data<EpgCache>,
) -> AppResult<HttpResponse> {
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    // --- Resolve destination URL -----------------------------------------
    let raw_dest = query
        .get("d")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing 'd' (destination URL) parameter".into()))?;

    let destination = if raw_dest.starts_with("http://") || raw_dest.starts_with("https://") {
        raw_dest.clone()
    } else {
        // Try base64 decode
        decode_base64_url(&raw_dest).ok_or_else(|| {
            AppError::BadRequest("Invalid destination URL or base64 encoding".into())
        })?
    };

    if !destination.starts_with("http://") && !destination.starts_with("https://") {
        return Err(AppError::BadRequest(
            "Destination must be an http:// or https:// URL".into(),
        ));
    }

    // --- Cache TTL --------------------------------------------------------
    // Per-request override via `cache_ttl` param; 0 disables caching.
    let effective_ttl: u64 = query
        .get("cache_ttl")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(config.epg.cache_ttl);

    // --- Build upstream request headers from h_<name> params -------------
    let mut upstream_headers = HeaderMap::new();
    for (key, value) in &query {
        if let Some(header_name) = key.strip_prefix("h_") {
            if let (Ok(name), Ok(val)) = (
                HeaderName::from_str(header_name),
                HeaderValue::from_str(value),
            ) {
                upstream_headers.insert(name, val);
            }
        }
    }

    // --- Cache key --------------------------------------------------------
    let cache_key = if upstream_headers.is_empty() {
        destination.clone()
    } else {
        // Mix a short hash of auth-bearing headers into the key so different
        // credentials don't collide.
        let header_repr: String = upstream_headers
            .iter()
            .map(|(k, v)| format!("{}:{}", k, v.to_str().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("|");
        format!("{destination}|{}", djb2_short(&header_repr))
    };

    // --- Cache read -------------------------------------------------------
    if effective_ttl > 0 {
        if let Some(cached) = epg_cache.0.get(&cache_key).await {
            tracing::debug!("[epg_proxy] Cache HIT: {}", destination);
            return Ok(build_response(cached, "HIT", effective_ttl));
        }
    }

    // --- Upstream fetch ---------------------------------------------------
    tracing::info!("[epg_proxy] Fetching EPG from: {}", destination);

    let body: Bytes = stream_manager
        .fetch_bytes(destination.clone(), upstream_headers)
        .await?;

    // Store in cache
    if effective_ttl > 0 {
        epg_cache.0.set(cache_key, body.clone()).await;
        tracing::info!(
            "[epg_proxy] Cached {} bytes from {} (TTL={}s)",
            body.len(),
            destination,
            effective_ttl
        );
    }

    Ok(build_response(body, "MISS", effective_ttl))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_response(body: Bytes, cache_status: &str, ttl: u64) -> HttpResponse {
    HttpResponse::Ok()
        .content_type("application/xml; charset=utf-8")
        .insert_header(("X-EPG-Cache", cache_status))
        .insert_header(("Cache-Control", format!("public, max-age={ttl}")))
        .body(body)
}

/// Short (8-char) non-cryptographic hash for cache key differentiation.
fn djb2_short(input: &str) -> String {
    let mut hash: u64 = 5381;
    for b in input.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as u64);
    }
    format!("{hash:016x}")[..8].to_string()
}
