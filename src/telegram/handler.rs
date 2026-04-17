//! Telegram MTProto proxy route handlers.
//!
//! Routes (registered under `/proxy/telegram`):
//! - `GET  /proxy/telegram/stream`             — stream Telegram media by file_id
//! - `HEAD /proxy/telegram/stream`             — same, no body
//! - `GET  /proxy/telegram/stream/{filename}`  — same, cosmetic filename variant
//! - `HEAD /proxy/telegram/stream/{filename}`  — same, no body
//! - `GET  /proxy/telegram/info`               — get metadata
//! - `GET  /proxy/telegram/status`             — session status

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use actix_web::{
    body::SizedStream,
    web::{self, Bytes},
    HttpRequest, HttpResponse,
};
use futures::stream;
use futures::StreamExt;

use crate::{
    config::Config,
    error::{AppError, AppResult},
    metrics::AppMetrics,
    telegram::{
        media_ref::{decode_file_id, parse_telegram_url},
        session::get_manager,
    },
};

// ---------------------------------------------------------------------------
// Stream handler (GET + HEAD)
// ---------------------------------------------------------------------------

/// Stream or probe a Telegram document identified by `file_id` + `file_size`.
///
/// Required query params:
/// - `file_id`   — Bot API file_id
/// - `file_size` — total byte size of the file (required for Range support)
///
/// Optional:
/// - `chat_id`     — chat/channel ID or username (informational only at this layer)
/// - `document_id` — Telegram document ID (informational only)
/// - `message_id`  — message ID (informational only)
pub async fn telegram_stream_handler(
    req: HttpRequest,
    config: web::Data<Arc<Config>>,
    metrics: web::Data<Arc<AppMetrics>>,
) -> AppResult<HttpResponse> {
    metrics.inc_request();
    metrics.telegram_requests.fetch_add(1, Ordering::Relaxed);
    let is_head = req.method() == actix_web::http::Method::HEAD;

    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    // --- Resolve file_id -------------------------------------------------
    let file_id_str = query
        .get("file_id")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing required parameter: file_id".into()))?;

    let file_size: u64 = query
        .get("file_size")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| {
            AppError::BadRequest("Missing or invalid required parameter: file_size".into())
        })?;

    if file_size == 0 {
        return Err(AppError::BadRequest("file_size must be > 0".into()));
    }

    let decoded = decode_file_id(&file_id_str)
        .ok_or_else(|| AppError::BadRequest("Cannot decode file_id".into()))?;

    // Optional params for file_reference refresh
    let chat_id_opt = query.get("chat_id").cloned();
    let document_id_opt: Option<i64> = query.get("document_id").and_then(|v| v.parse().ok());
    let message_id_opt: Option<i32> = query.get("message_id").and_then(|v| v.parse().ok());

    // --- Parse Range header -----------------------------------------------
    let range_header = req
        .headers()
        .get("range")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let (start, end) = parse_range(range_header.as_deref(), file_size);

    tracing::debug!(
        "tg handler: file_size={} range_header={:?} parsed start={} end={} content_length={}",
        file_size,
        range_header,
        start,
        end,
        end - start + 1
    );

    // --- Check Telegram config -------------------------------------------
    let tg_cfg = &config.telegram;
    if tg_cfg.api_id == 0 || tg_cfg.api_hash.is_empty() || tg_cfg.session_string.is_empty() {
        return Err(AppError::Telegram(
            "Telegram not configured. Set APP__TELEGRAM__API_ID, \
             APP__TELEGRAM__API_HASH and APP__TELEGRAM__SESSION_STRING."
                .into(),
        ));
    }

    // --- Initialise (or reuse) the Telegram client -----------------------
    #[cfg(not(feature = "telegram"))]
    return Err(AppError::Telegram(
        "Telegram feature not compiled in.".into(),
    ));

    #[cfg(feature = "telegram")]
    {
        use crate::proxy::stream::ResponseStream;
        use crate::telegram::session::{
            get_fresh_document_info, get_or_init_client, stream_document_range,
        };
        use grammers_tl_types as tl;

        let client = get_or_init_client(tg_cfg)
            .await
            .map_err(|e| AppError::Telegram(format!("Telegram connect failed: {}", e)))?;

        // --- Resolve fresh document info when chat context is provided ------
        // The file_reference AND access_hash embedded in a Bot API file_id are
        // bot-session-specific and expire.  When chat_id + document_id are supplied
        // (e.g. by MediaFusion / Stremio), we scan the chat history to obtain
        // current values from our own user MTProto session.
        let (file_reference, access_hash, dc_id) =
            if let (Some(chat_id), Some(doc_id)) = (&chat_id_opt, document_id_opt) {
                match get_fresh_document_info(client.clone(), chat_id, doc_id, message_id_opt, 200)
                    .await
                {
                    Some(info) => (info.file_reference, info.access_hash, info.dc_id),
                    None => {
                        tracing::warn!(
                            "Could not find document_id={} in chat history; \
                             falling back to file_id embedded values",
                            doc_id
                        );
                        (
                            decoded.file_reference.clone(),
                            decoded.access_hash,
                            decoded.dc_id,
                        )
                    }
                }
            } else {
                (
                    decoded.file_reference.clone(),
                    decoded.access_hash,
                    decoded.dc_id,
                )
            };

        // --- Build file location -----------------------------------------
        let location = tl::enums::InputFileLocation::InputDocumentFileLocation(
            tl::types::InputDocumentFileLocation {
                id: decoded.id,
                access_hash,
                file_reference,
                thumb_size: String::new(),
            },
        );

        // --- Determine MIME type from path / file_id type ----------------
        let filename = req.match_info().get("filename").unwrap_or("stream.mkv");
        let mime = mime_guess::from_path(filename)
            .first_or_octet_stream()
            .to_string();

        // --- Build response headers --------------------------------------
        let content_length = end - start + 1;
        let is_range_request = range_header.is_some();

        let status_code = if is_range_request { 206u16 } else { 200u16 };

        let mut response = HttpResponse::build(
            actix_web::http::StatusCode::from_u16(status_code)
                .unwrap_or(actix_web::http::StatusCode::OK),
        );
        response.insert_header(("content-type", mime.as_str()));
        response.insert_header(("accept-ranges", "bytes"));
        response.insert_header(("content-length", content_length.to_string()));
        if is_range_request {
            response.insert_header((
                "content-range",
                format!("bytes {}-{}/{}", start, end, file_size),
            ));
        }

        // HEAD: return headers only, no body
        if is_head {
            let empty = Box::pin(stream::empty::<Result<Bytes, std::io::Error>>());
            return Ok(response
                .no_chunking(content_length)
                .body(SizedStream::new(content_length, empty)));
        }

        // GET: stream body
        let byte_stream =
            stream_document_range(client, tg_cfg.clone(), location, dc_id, start, end).await;
        let metrics_clone = Arc::clone(&metrics);
        let byte_stream = byte_stream.map(move |chunk| {
            if let Ok(ref b) = chunk {
                metrics_clone.add_bytes_out(b.len() as u64);
            }
            chunk
        });
        let response_stream = ResponseStream::new(byte_stream);

        Ok(response
            .no_chunking(content_length)
            .body(SizedStream::new(content_length, response_stream)))
    }
}

// ---------------------------------------------------------------------------
// Info handler
// ---------------------------------------------------------------------------

pub async fn telegram_info_handler(
    req: HttpRequest,
    _config: web::Data<Arc<Config>>,
) -> AppResult<HttpResponse> {
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let url = query
        .get("url")
        .or_else(|| query.get("d"))
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing url/d param".into()))?;

    let media_ref = parse_telegram_url(&url)
        .ok_or_else(|| AppError::BadRequest(format!("Cannot parse Telegram URL: {url}")))?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "url": url,
        "parsed": format!("{media_ref:?}"),
    })))
}

// ---------------------------------------------------------------------------
// Status handler
// ---------------------------------------------------------------------------

pub async fn telegram_status_handler() -> AppResult<HttpResponse> {
    let manager = get_manager();
    let mgr = manager.read().await;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "connected": mgr.is_authorized(),
        "session_file": mgr.session_file,
    })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse an HTTP `Range: bytes=start-end` header.
///
/// Returns `(start, end)` (both inclusive) clamped to `[0, file_size-1]`.
fn parse_range(header: Option<&str>, file_size: u64) -> (u64, u64) {
    let header = match header {
        Some(h) => h,
        None => return (0, file_size.saturating_sub(1)),
    };

    // Expected format: "bytes=start-end" or "bytes=-suffix_len"
    let bytes_part = header.strip_prefix("bytes=").unwrap_or(header);
    let (start_str, end_str) = match bytes_part.split_once('-') {
        Some(pair) => pair,
        None => return (0, file_size.saturating_sub(1)),
    };

    if start_str.is_empty() {
        // Suffix-range: "bytes=-N" means last N bytes.
        // end_str is the count (N), NOT an end offset.
        let suffix_len = end_str.parse::<u64>().unwrap_or(0).min(file_size);
        let start = file_size.saturating_sub(suffix_len);
        let end = file_size.saturating_sub(1);
        return (start, end);
    }

    let start = start_str.parse::<u64>().unwrap_or(0);

    let end = if end_str.is_empty() {
        file_size.saturating_sub(1)
    } else {
        end_str
            .parse::<u64>()
            .unwrap_or(file_size.saturating_sub(1))
            .min(file_size.saturating_sub(1))
    };

    if start > end {
        return (0, file_size.saturating_sub(1));
    }

    (start, end)
}
