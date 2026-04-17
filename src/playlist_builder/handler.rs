//! Playlist builder handler.
//!
//! `GET /playlist/builder?url=<m3u_url>&api_password=...`
//!
//! Fetches an M3U/M3U8 playlist from `url`, rewrites all stream URLs to route
//! through this MediaFlow proxy, and streams the rewritten content back.
//!
//! Rewrite rules (port from mediaflow_proxy/routes/playlist_builder.py):
//! - Non-HTTP lines are passed through unchanged.
//! - `#EXTVLCOPT:` / `#EXTHTTP:` / `#KODIPROP:` — accumulate headers, pass through.
//! - HTTP stream lines — rewrite to `/proxy/stream?d=<enc>&h_*=<headers>&api_password=<>`.
//! - vavoo.to links → `/proxy/hls/manifest?d=<enc>`.
//! - M3U8 links → `/proxy/hls/manifest?d=<enc>`.

use std::collections::HashMap;
use std::sync::Arc;

use actix_web::{web, HttpRequest, HttpResponse};
use reqwest::header::HeaderMap;
use urlencoding::encode as url_encode;

use crate::{
    config::Config,
    error::{AppError, AppResult},
    proxy::stream::StreamManager,
};

pub async fn playlist_builder_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    _config: web::Data<Arc<Config>>,
) -> AppResult<HttpResponse> {
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let m3u_url = query
        .get("url")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing url param".into()))?;

    let api_password = query.get("api_password").cloned().unwrap_or_default();

    let conn = req.connection_info();
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
    let base_url = format!("{scheme}://{host}");

    tracing::info!("Playlist builder: fetching {m3u_url}");

    let raw = stream_manager
        .fetch_bytes(m3u_url.clone(), HeaderMap::new())
        .await?;

    let content = String::from_utf8_lossy(&raw);
    let rewritten = rewrite_m3u(&content, &base_url, &api_password);

    Ok(HttpResponse::Ok()
        .content_type("application/x-mpegurl; charset=utf-8")
        .insert_header(("cache-control", "no-cache, no-store"))
        .body(rewritten))
}

// ---------------------------------------------------------------------------
// M3U rewriting
// ---------------------------------------------------------------------------

fn rewrite_m3u(content: &str, base_url: &str, api_password: &str) -> String {
    let mut out = String::with_capacity(content.len() + 1024);
    let mut current_headers: HashMap<String, String> = HashMap::new();
    let mut current_kodi: HashMap<String, String> = HashMap::new();

    for line in content.lines() {
        let logical = line.trim();

        if logical.starts_with("#EXTVLCOPT:") {
            out.push_str(line);
            out.push('\n');
            // Parse http-* headers
            if let Some(opt) = logical.strip_prefix("#EXTVLCOPT:") {
                if let Some((k, v)) = opt.split_once('=') {
                    let k = k.trim();
                    let v = v.trim();
                    if k == "http-header" {
                        if let Some((hk, hv)) = v.split_once(':') {
                            current_headers.insert(hk.trim().to_lowercase(), hv.trim().to_string());
                        }
                    } else if let Some(hname) = k.strip_prefix("http-") {
                        current_headers.insert(hname.to_lowercase(), v.to_string());
                    }
                }
            }
            continue;
        }

        if logical.starts_with("#EXTHTTP:") {
            out.push_str(line);
            out.push('\n');
            if let Some(json_str) = logical.strip_prefix("#EXTHTTP:") {
                if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(json_str) {
                    current_headers = map
                        .iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.to_lowercase(), s.to_string())))
                        .collect();
                }
            }
            continue;
        }

        if logical.starts_with("#KODIPROP:") {
            out.push_str(line);
            out.push('\n');
            if let Some(prop) = logical.strip_prefix("#KODIPROP:") {
                if let Some((k, v)) = prop.split_once('=') {
                    current_kodi.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
            continue;
        }

        // Pass-through all non-URL lines unchanged.
        if logical.is_empty() || logical.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Rewrite stream URL.
        if logical.starts_with("http://") || logical.starts_with("https://") {
            let rewritten_url =
                rewrite_stream_url(logical, base_url, api_password, &current_headers);
            out.push_str(&rewritten_url);
            out.push('\n');
            // Reset headers after consuming them.
            current_headers.clear();
            current_kodi.clear();
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }

    out
}

fn rewrite_stream_url(
    url: &str,
    base_url: &str,
    api_password: &str,
    headers: &HashMap<String, String>,
) -> String {
    // Skip pluto.tv — already CDN, no rewrite needed.
    if url.contains("pluto.tv") {
        return url.to_string();
    }

    // vavoo.to → HLS manifest proxy.
    if url.contains("vavoo.to") {
        let mut result = format!("{base_url}/proxy/hls/manifest?d={}", url_encode(url));
        if !api_password.is_empty() {
            result.push_str(&format!("&api_password={}", url_encode(api_password)));
        }
        return result;
    }

    // M3U8 streams → HLS manifest proxy.
    if url.contains(".m3u8") || url.contains("m3u8") {
        let mut params = format!("d={}", url_encode(url));
        for (k, v) in headers {
            params.push_str(&format!("&h_{}={}", url_encode(k), url_encode(v)));
        }
        if !api_password.is_empty() {
            params.push_str(&format!("&api_password={}", url_encode(api_password)));
        }
        return format!("{base_url}/proxy/hls/manifest?{params}");
    }

    // MPD streams → MPD manifest proxy.
    if url.ends_with(".mpd") || url.contains(".mpd?") {
        let mut params = format!("d={}", url_encode(url));
        for (k, v) in headers {
            params.push_str(&format!("&h_{}={}", url_encode(k), url_encode(v)));
        }
        if !api_password.is_empty() {
            params.push_str(&format!("&api_password={}", url_encode(api_password)));
        }
        return format!("{base_url}/proxy/mpd/manifest?{params}");
    }

    // Generic stream → raw stream proxy.
    let mut params = format!("d={}", url_encode(url));
    for (k, v) in headers {
        params.push_str(&format!("&h_{}={}", url_encode(k), url_encode(v)));
    }
    if !api_password.is_empty() {
        params.push_str(&format!("&api_password={}", url_encode(api_password)));
    }
    format!("{base_url}/proxy/stream?{params}")
}
