//! Acestream proxy route handlers.
//!
//! Routes (registered under `/proxy/acestream`):
//! - `GET  /proxy/acestream/manifest.m3u8` — initiate session, fetch & rewrite HLS manifest
//! - `HEAD /proxy/acestream/manifest.m3u8` — same, no body
//! - `GET  /proxy/acestream/stream`         — raw MPEG-TS stream
//! - `HEAD /proxy/acestream/stream`         — same, no body
//! - `GET  /proxy/acestream/segment.{ext}`  — individual TS/fMP4 segment proxy
//! - `GET  /proxy/acestream/status`         — session registry status
//!
//! The Acestream engine must be running locally on `acestream_port` (default 6878).
//! On first request, the handler performs a session initiation call:
//!   GET /ace/manifest.m3u8?format=json&pid=<uuid>&id=<infohash>
//! to obtain the `playback_url` (containing the engine's `access_token`).
//! Subsequent manifest/segment fetches use that URL directly.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use actix_web::{web, HttpRequest, HttpResponse};
use futures::Stream;
use reqwest::header::HeaderMap;
use urlencoding::encode as url_encode;

use crate::{
    config::Config,
    error::{AppError, AppResult},
    proxy::stream::StreamManager,
};

use super::session::AcestreamSessionManager;

// ---------------------------------------------------------------------------
// Stop-on-drop stream wrapper
//
// Wraps an upstream Stream and fires a one-shot channel when the stream
// ends normally *or* is dropped (client disconnected). The channel receiver
// is held by a background task that calls the acestream engine stop command.
// ---------------------------------------------------------------------------

struct StopNotifyStream<S> {
    inner: Pin<Box<S>>,
    stop_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl<S> StopNotifyStream<S> {
    fn new(inner: S, stop_tx: tokio::sync::oneshot::Sender<()>) -> Self {
        Self {
            inner: Box::pin(inner),
            stop_tx: Some(stop_tx),
        }
    }
    fn signal(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl<S: Stream + Unpin> Stream for StopNotifyStream<S> {
    type Item = S::Item;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<S::Item>> {
        let this = unsafe { self.get_unchecked_mut() };
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(None) => {
                this.signal();
                Poll::Ready(None)
            }
            other => other,
        }
    }
}

impl<S> Drop for StopNotifyStream<S> {
    fn drop(&mut self) {
        self.signal();
    }
}

// ---------------------------------------------------------------------------
// Manifest handler
// ---------------------------------------------------------------------------

pub async fn acestream_manifest_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    config: web::Data<Arc<Config>>,
    session_mgr: web::Data<AcestreamSessionManager>,
) -> AppResult<HttpResponse> {
    let is_head = req.method() == actix_web::http::Method::HEAD;

    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let infohash = query
        .get("id")
        .or_else(|| query.get("infohash"))
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing id/infohash param".into()))?;

    let content_id = query.get("id").cloned();
    let engine_host = &config.acestream.host;
    let engine_port = config.acestream.port;
    let engine_token = config.acestream.access_token.as_deref();

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

    tracing::info!("Acestream manifest: infohash={infohash}");

    // Get or create engine session (with access_token)
    let session = session_mgr
        .get_or_create(
            engine_host,
            engine_port,
            &infohash,
            content_id.as_deref(),
            engine_token,
        )
        .await
        .map_err(|e| {
            tracing::warn!("Acestream session init failed: {e}");
            AppError::Acestream(format!("Session init failed: {e}"))
        })?;

    tracing::debug!("Acestream playback_url: {}", session.playback_url);

    if is_head {
        return Ok(HttpResponse::Ok()
            .content_type("application/vnd.apple.mpegurl")
            .insert_header(("accept-ranges", "none"))
            .finish());
    }

    // Retry once on 403 (stale access_token)
    if let Some(attempt) = (0..2u8).next() {
        let raw = stream_manager
            .fetch_bytes(session.playback_url.clone(), HeaderMap::new())
            .await;

        match raw {
            Err(e) if attempt == 0 => {
                let msg = e.to_string();
                if msg.contains("403") || msg.contains("Forbidden") {
                    tracing::warn!("Acestream manifest 403, invalidating session");
                    session_mgr.invalidate(&infohash);
                    // Reinit session on next loop iteration
                    let new_session = session_mgr
                        .get_or_create(
                            engine_host,
                            engine_port,
                            &infohash,
                            content_id.as_deref(),
                            engine_token,
                        )
                        .await
                        .map_err(|e| AppError::Acestream(format!("Session reinit failed: {e}")))?;
                    let raw2 = stream_manager
                        .fetch_bytes(new_session.playback_url.clone(), HeaderMap::new())
                        .await
                        .map_err(|e| AppError::Acestream(format!("Manifest fetch failed: {e}")))?;
                    let manifest_text = String::from_utf8_lossy(&raw2);
                    let rewritten = rewrite_acestream_manifest(
                        &manifest_text,
                        &base_url,
                        &infohash,
                        &api_password,
                    );
                    return Ok(HttpResponse::Ok()
                        .content_type("application/vnd.apple.mpegurl")
                        .insert_header(("cache-control", "no-cache, no-store"))
                        .body(rewritten));
                }
                return Err(AppError::Acestream(format!("Manifest fetch failed: {msg}")));
            }
            Err(e) => {
                return Err(AppError::Acestream(format!("Manifest fetch failed: {e}")));
            }
            Ok(raw) => {
                let manifest_text = String::from_utf8_lossy(&raw);
                let rewritten =
                    rewrite_acestream_manifest(&manifest_text, &base_url, &infohash, &api_password);
                return Ok(HttpResponse::Ok()
                    .content_type("application/vnd.apple.mpegurl")
                    .insert_header(("cache-control", "no-cache, no-store"))
                    .body(rewritten));
            }
        }
    }

    unreachable!()
}

// ---------------------------------------------------------------------------
// Stream handler (raw MPEG-TS)
// ---------------------------------------------------------------------------

pub async fn acestream_stream_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    config: web::Data<Arc<Config>>,
    session_mgr: web::Data<AcestreamSessionManager>,
) -> AppResult<HttpResponse> {
    let is_head = req.method() == actix_web::http::Method::HEAD;

    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let infohash = query
        .get("id")
        .or_else(|| query.get("infohash"))
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing id/infohash param".into()))?;

    let content_id = query.get("id").cloned();
    let engine_host = &config.acestream.host;
    let engine_port = config.acestream.port;
    let engine_token = config.acestream.access_token.as_deref();

    tracing::info!("Acestream stream: infohash={infohash}");

    let session = session_mgr
        .get_or_create(
            engine_host,
            engine_port,
            &infohash,
            content_id.as_deref(),
            engine_token,
        )
        .await
        .map_err(|e| {
            tracing::warn!("Acestream session init failed: {e}");
            AppError::Acestream(format!("Session init failed: {e}"))
        })?;

    // In free mode the engine only supports HLS (getstream → 500).
    // Return the HLS manifest so the player can still play.
    if session.is_free_mode {
        tracing::info!("Acestream free mode: serving HLS manifest from stream endpoint");

        if is_head {
            return Ok(HttpResponse::Ok()
                .content_type("application/vnd.apple.mpegurl")
                .insert_header(("cache-control", "no-cache, no-store"))
                .finish());
        }

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
        let api_password = query.get("api_password").cloned().unwrap_or_default();

        tracing::debug!("Acestream free mode manifest URL: {}", session.playback_url);
        let raw = stream_manager
            .fetch_bytes(session.playback_url.clone(), HeaderMap::new())
            .await
            .map_err(|e| {
                tracing::warn!("Acestream free mode manifest fetch failed: {e}");
                AppError::Acestream(format!("Manifest fetch failed: {e}"))
            })?;

        let manifest_text = String::from_utf8_lossy(&raw);
        let rewritten =
            rewrite_acestream_manifest(&manifest_text, &base_url, &infohash, &api_password);

        return Ok(HttpResponse::Ok()
            .content_type("application/vnd.apple.mpegurl")
            .insert_header(("cache-control", "no-cache, no-store"))
            .body(rewritten));
    }

    // Premium mode: use getstream.
    // Mirror Python's approach: always include id/infohash + pid explicitly.
    // Also forward any access_token from the engine's playback_url for
    // premium engines that require it.
    let ts_url = playback_to_getstream(
        &session.playback_url,
        engine_host,
        engine_port,
        &infohash,
        content_id.as_deref(),
        &session.pid,
    );

    tracing::debug!("Acestream getstream url: {ts_url}");

    if is_head {
        return Ok(HttpResponse::Ok().content_type("video/mp2t").finish());
    }

    // Stream the live MPEG-TS response chunk-by-chunk.
    // Do NOT buffer with .bytes() — it will wait forever on a live stream.
    let (_status, _resp_headers, stream_opt) = stream_manager
        .create_stream(ts_url, HeaderMap::new(), false)
        .await
        .map_err(|e| AppError::Acestream(format!("Engine stream request failed: {e}")))?;

    let Some(raw_stream) = stream_opt else {
        return Ok(HttpResponse::Ok().content_type("video/mp2t").finish());
    };

    // Spawn a background task: when the stream ends or the client drops,
    // send the stop command to the acestream engine.
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let session_mgr_clone = session_mgr.clone();
    let infohash_clone = infohash.clone();
    tokio::spawn(async move {
        let _ = stop_rx.await;
        tracing::info!("Acestream client disconnected — stopping session {infohash_clone:.16}");
        session_mgr_clone.stop_session(&infohash_clone).await;
    });

    let wrapped = StopNotifyStream::new(raw_stream, stop_tx);
    let response_stream = crate::proxy::stream::ResponseStream::new(wrapped);

    Ok(HttpResponse::Ok()
        .content_type("video/mp2t")
        .insert_header(("cache-control", "no-cache, no-store"))
        .streaming(response_stream))
}

// ---------------------------------------------------------------------------
// Segment handler
// ---------------------------------------------------------------------------

pub async fn acestream_segment_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    _config: web::Data<Arc<Config>>,
) -> AppResult<HttpResponse> {
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let segment_url = query
        .get("d")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing d (segment URL) param".into()))?;

    tracing::debug!("Acestream segment: {segment_url}");

    let mime = if segment_url.contains(".m4s") || segment_url.contains(".mp4") {
        "video/mp4"
    } else {
        "video/mp2t"
    };

    let body = stream_manager
        .fetch_bytes(segment_url.clone(), HeaderMap::new())
        .await
        .map_err(|e| AppError::Acestream(format!("Segment fetch failed: {e}")))?;

    Ok(HttpResponse::Ok().content_type(mime).body(body))
}

// ---------------------------------------------------------------------------
// Status handler
// ---------------------------------------------------------------------------

pub async fn acestream_status_handler(config: web::Data<Arc<Config>>) -> AppResult<HttpResponse> {
    Ok(HttpResponse::Ok().json(serde_json::json!({
        "enabled": true,
        "engine_host": config.acestream.host,
        "engine_port": config.acestream.port,
        "engine_url": format!("http://{}:{}/", config.acestream.host, config.acestream.port),
    })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `getstream` URL from session parameters.
///
/// Mirrors the Python proxy's approach: always include the content identifier
/// (`id` or `infohash`) and `pid` explicitly.  The acestream engine needs the
/// content identifier to locate the active stream; relying only on `pid` or
/// `access_token` from the playback_url query string is not universally
/// supported across engine versions.
///
/// If the engine's `playback_url` carries an `access_token` (premium mode), it
/// is forwarded so that those engines can still authenticate the request.
fn playback_to_getstream(
    playback_url: &str,
    engine_host: &str,
    engine_port: u16,
    infohash: &str,
    content_id: Option<&str>,
    pid: &str,
) -> String {
    // Choose id= vs infohash= to match what was used during session init.
    let (id_key, id_val) = match content_id {
        Some(cid) => ("id", cid),
        None => {
            if infohash.len() == 40 && infohash.chars().all(|c| c.is_ascii_hexdigit()) {
                ("infohash", infohash)
            } else {
                ("id", infohash)
            }
        }
    };

    let mut url =
        format!("http://{engine_host}:{engine_port}/ace/getstream?{id_key}={id_val}&pid={pid}");

    // Forward access_token from the engine's playback_url if present
    // (required by premium / some Android builds).
    if let Some(token) = extract_query_param(playback_url, "access_token") {
        url.push_str(&format!("&access_token={token}"));
    }

    url
}

/// Extract a single query parameter value from a URL string.
fn extract_query_param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let qs = url.split_once('?')?.1;
    for part in qs.split('&') {
        if let Some(eq) = part.find('=') {
            if &part[..eq] == key {
                return Some(&part[eq + 1..]);
            }
        }
    }
    None
}

fn rewrite_acestream_manifest(
    manifest: &str,
    base_url: &str,
    infohash: &str,
    api_password: &str,
) -> String {
    let mut out = String::with_capacity(manifest.len() + 512);

    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            let mut params = format!(
                "d={}&infohash={}",
                url_encode(trimmed),
                url_encode(infohash)
            );
            if !api_password.is_empty() {
                params.push_str(&format!("&api_password={}", url_encode(api_password)));
            }
            out.push_str(&format!("{base_url}/proxy/acestream/segment.ts?{params}"));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }

    out
}
