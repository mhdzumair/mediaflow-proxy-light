/// HLS segment proxy handler.
///
/// Routes `GET /proxy/hls/segment.{ext}` to the upstream origin, optionally
/// decrypting DRM-protected bytes (Phase 3).
use actix_web::{
    body::SizedStream,
    web::{self, Bytes},
    HttpRequest, HttpResponse,
};
use futures::stream;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::str::FromStr;

use crate::{
    auth::encryption::ProxyData,
    error::{AppError, AppResult},
    proxy::stream::StreamManager,
};

/// `GET /proxy/hls/segment.{ext}` — proxy an HLS segment to the client.
///
/// The destination URL is carried in `proxy_data.destination` (set by the
/// auth middleware after decrypting the `d` query parameter or `token`).
pub async fn hls_segment_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    proxy_data: web::ReqData<ProxyData>,
    metrics: web::Data<std::sync::Arc<crate::metrics::AppMetrics>>,
) -> AppResult<HttpResponse> {
    metrics.inc_request();
    metrics
        .hls_requests
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut request_headers = HeaderMap::new();

    // Forward the `Range` header so players can seek within segments
    if let Some(range) = req.headers().get("range") {
        request_headers.insert(
            HeaderName::from_static("range"),
            HeaderValue::try_from(range.as_bytes())
                .map_err(|e| AppError::Internal(format!("Invalid Range header: {}", e)))?,
        );
    }

    // Merge custom request headers from proxy data (h_* params)
    if let Some(custom) = &proxy_data.request_headers {
        for (k, v) in custom.as_object().unwrap_or(&serde_json::Map::new()) {
            if let Some(v_str) = v.as_str() {
                if let (Ok(name), Ok(value)) =
                    (HeaderName::from_str(k), HeaderValue::from_str(v_str))
                {
                    request_headers.insert(name, value);
                }
            }
        }
    }

    let (upstream_status, upstream_headers, stream_opt) = stream_manager
        .create_stream(proxy_data.destination.clone(), request_headers, false)
        .await?;

    // Mirror the upstream status (200 or 206 Partial Content)
    let mut resp = HttpResponse::build(
        actix_web::http::StatusCode::from_u16(upstream_status.as_u16())
            .unwrap_or(actix_web::http::StatusCode::OK),
    );

    // Forward content-type, content-length, content-range, accept-ranges
    for header_name in &[
        "content-type",
        "content-length",
        "content-range",
        "accept-ranges",
        "cache-control",
    ] {
        if let Some(v) = upstream_headers.get(*header_name) {
            if let Ok(val) =
                actix_web::http::header::HeaderValue::from_str(v.to_str().unwrap_or_default())
            {
                resp.insert_header((*header_name, val));
            }
        }
    }

    let content_length: u64 = upstream_headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    // Custom response headers from proxy data (r_* / rp_* params)
    if let Some(custom) = &proxy_data.response_headers {
        for (k, v) in custom.as_object().unwrap_or(&serde_json::Map::new()) {
            if let Some(v_str) = v.as_str() {
                if let (Ok(name), Ok(val)) = (
                    actix_web::http::header::HeaderName::from_str(k),
                    actix_web::http::header::HeaderValue::from_str(v_str),
                ) {
                    resp.insert_header((name, val));
                }
            }
        }
    }

    // Tell actix-web to close the TCP connection after this response.
    // HLS segments are independent one-shot fetches; reusing a keepalive
    // connection across segments causes "Error reading HTTP response: End of
    // file" in FFmpeg/mpv whenever the proxy's keepalive timer fires between
    // fetches (actix-web default: 5 s).  force_close() is the correct
    // actix-web 4 API — inserting a raw "Connection: close" header is silently
    // ignored because actix-web manages that hop-by-hop header internally.
    resp.force_close();

    match stream_opt {
        Some(stream) => {
            use futures::StreamExt;
            let metrics_clone = std::sync::Arc::clone(&metrics);
            let stream_with_progress = stream_manager.stream_with_progress(stream).map(
                move |chunk: Result<actix_web::web::Bytes, crate::error::AppError>| {
                    if let Ok(ref b) = chunk {
                        metrics_clone.add_bytes_out(b.len() as u64);
                    }
                    chunk
                },
            );
            let response_stream = crate::proxy::stream::ResponseStream::new(stream_with_progress);
            if content_length > 0 {
                Ok(resp
                    .no_chunking(content_length)
                    .body(SizedStream::new(content_length, response_stream)))
            } else {
                Ok(resp.streaming(response_stream))
            }
        }
        None => {
            let empty = Box::pin(stream::empty::<Result<Bytes, std::io::Error>>());
            Ok(resp
                .no_chunking(content_length)
                .body(SizedStream::new(content_length, empty)))
        }
    }
}
