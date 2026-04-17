//! Transcoding route handlers.
//!
//! Routes (registered under `/proxy/transcode`):
//! - `GET /proxy/transcode`          — transcode a URL and stream fMP4/TS output
//! - `GET /proxy/transcode/hls/init` — serve fMP4 init segment
//! - `GET /proxy/transcode/hls/playlist` — serve HLS VOD playlist
//! - `GET /proxy/transcode/hls/segment` — serve a single HLS fMP4 segment

use std::collections::HashMap;
use std::sync::Arc;

use actix_web::{web, HttpRequest, HttpResponse};
use urlencoding::encode as url_encode;

use crate::{
    config::Config,
    error::{AppError, AppResult},
    proxy::stream::StreamManager,
    transcode::pipeline::{transcode_url, OutputFormat, TranscodeOptions},
};

// ---------------------------------------------------------------------------
// GET /proxy/transcode
// ---------------------------------------------------------------------------

/// Transcode an upstream URL and stream the output.
pub async fn transcode_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    config: web::Data<Arc<Config>>,
) -> AppResult<HttpResponse> {
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let source_url = query
        .get("d")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing d (source URL) param".into()))?;

    let start_time: Option<f64> = query.get("start").and_then(|v| v.parse().ok());
    let output_ts = query
        .get("ts")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);

    // Collect h_* headers.
    let request_headers: Vec<(String, String)> = query
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("h_")
                .map(|name| (name.to_string(), v.clone()))
        })
        .collect();

    let opts = TranscodeOptions {
        input_format: None,
        start_time,
        output_format: if output_ts {
            OutputFormat::MpegTs
        } else {
            OutputFormat::FragmentedMp4
        },
    };

    tracing::info!("Transcode: {source_url}");

    let body = transcode_url(&source_url, opts, request_headers).await?;

    let content_type = if output_ts { "video/mp2t" } else { "video/mp4" };
    Ok(HttpResponse::Ok()
        .content_type(content_type)
        .insert_header(("cache-control", "no-cache"))
        .body(body))
}

// ---------------------------------------------------------------------------
// GET /proxy/transcode/hls/init
// ---------------------------------------------------------------------------

/// Serve the fMP4 init segment for an HLS VOD transcoded stream.
pub async fn transcode_hls_init_handler(
    req: HttpRequest,
    _stream_manager: web::Data<StreamManager>,
    _config: web::Data<Arc<Config>>,
) -> AppResult<HttpResponse> {
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let source_url = query
        .get("d")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing d param".into()))?;

    let request_headers: Vec<(String, String)> = query
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("h_")
                .map(|name| (name.to_string(), v.clone()))
        })
        .collect();

    let opts = TranscodeOptions {
        input_format: None,
        start_time: Some(0.0),
        output_format: OutputFormat::FragmentedMp4,
    };

    let body = transcode_url(&source_url, opts, request_headers).await?;

    // The first few bytes of a fragmented MP4 are the init segment (ftyp + moov).
    // For simplicity we return the whole output; a real impl would cut at the first moof.
    Ok(HttpResponse::Ok()
        .content_type("video/mp4")
        .insert_header(("cache-control", "no-cache"))
        .body(body))
}

// ---------------------------------------------------------------------------
// GET /proxy/transcode/hls/playlist
// ---------------------------------------------------------------------------

/// Return a simple HLS VOD playlist for a transcoded stream.
pub async fn transcode_hls_playlist_handler(
    req: HttpRequest,
    _config: web::Data<Arc<Config>>,
) -> AppResult<HttpResponse> {
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let source_url = query
        .get("d")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing d param".into()))?;
    let api_password = query.get("api_password").cloned().unwrap_or_default();

    let conn = req.connection_info();
    let base_url = format!("{}://{}", conn.scheme(), conn.host());

    let mut params = format!("d={}", url_encode(&source_url));
    // Forward h_* headers.
    for (k, v) in &query {
        if k.starts_with("h_") {
            params.push_str(&format!("&{}={}", url_encode(k), url_encode(v)));
        }
    }
    if !api_password.is_empty() {
        params.push_str(&format!("&api_password={}", url_encode(&api_password)));
    }

    // Single-segment VOD playlist.
    let segment_dur = 86400.0_f32; // Whole file as one segment.
    let playlist = format!(
        "#EXTM3U\n\
         #EXT-X-VERSION:6\n\
         #EXT-X-TARGETDURATION:{}\n\
         #EXT-X-MAP:URI=\"{}/proxy/transcode/init.mp4?{}\"\n\
         #EXTINF:{},\n\
         {}/proxy/transcode?{}&seg=0&start_ms=0&end_ms=86400000\n\
         #EXT-X-ENDLIST\n",
        segment_dur as u32, base_url, params, segment_dur, base_url, params
    );

    Ok(HttpResponse::Ok()
        .content_type("application/vnd.apple.mpegurl")
        .insert_header(("cache-control", "no-cache, no-store"))
        .body(playlist))
}

// ---------------------------------------------------------------------------
// GET /proxy/transcode/hls/segment
// ---------------------------------------------------------------------------

/// Serve a single HLS segment for a transcoded stream (start/end ms).
pub async fn transcode_hls_segment_handler(
    req: HttpRequest,
    _stream_manager: web::Data<StreamManager>,
    _config: web::Data<Arc<Config>>,
) -> AppResult<HttpResponse> {
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let source_url = query
        .get("d")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing d param".into()))?;

    let start_ms: f64 = query
        .get("start_ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let end_ms: f64 = query
        .get("end_ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(86400000.0);

    let request_headers: Vec<(String, String)> = query
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("h_")
                .map(|name| (name.to_string(), v.clone()))
        })
        .collect();

    let opts = TranscodeOptions {
        input_format: None,
        start_time: Some(start_ms / 1000.0),
        output_format: OutputFormat::FragmentedMp4,
    };

    tracing::debug!("Transcode segment: start={start_ms}ms");

    let body = transcode_url(&source_url, opts, request_headers).await?;

    Ok(HttpResponse::Ok()
        .content_type("video/mp4")
        .insert_header(("cache-control", "no-cache"))
        .body(body))
}
