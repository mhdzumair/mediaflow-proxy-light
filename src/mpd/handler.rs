//! Actix-web route handlers for DASH/MPD proxy endpoints.
//!
//! Routes (registered under `/proxy/mpd` in `main.rs`):
//! - `GET /manifest` — fetch MPD, convert to HLS master manifest
//! - `GET /playlist`  — fetch MPD, build HLS media playlist for a specific profile
//! - `GET /segment`   — proxy a DASH media segment (fMP4 or TS remux)
//! - `GET /segment.{ext}` — same as above with explicit extension
//! - `GET /init`      — proxy a DASH init segment (EXT-X-MAP)

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use actix_web::{web, HttpRequest, HttpResponse};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::str::FromStr;

use crate::{
    auth::encryption::ProxyData,
    cache::local::LocalCache,
    config::Config,
    error::{AppError, AppResult},
    metrics::AppMetrics,
    mpd::{
        parser::parse_mpd,
        processor::{
            build_hls_master, build_hls_media_playlist, parse_mpd_document, parse_sidx_fragments,
            MpdProfile, MpdProxyParams, MpdSegment,
        },
    },
    proxy::stream::StreamManager,
};

// ---------------------------------------------------------------------------
// Helper: extract the proxy base URL from the request
// ---------------------------------------------------------------------------

fn proxy_base_url(req: &HttpRequest) -> String {
    let conn = req.connection_info();
    format!("{}://{}", conn.scheme(), conn.host())
}

// ---------------------------------------------------------------------------
// Helper: build a HeaderMap from the h_* pass-headers in ProxyData
// ---------------------------------------------------------------------------

fn build_request_headers(proxy_data: &ProxyData) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(map) = proxy_data
        .request_headers
        .as_ref()
        .and_then(|v| v.as_object())
    {
        for (k, v) in map {
            if let Some(val_str) = v.as_str() {
                if let (Ok(name), Ok(value)) =
                    (HeaderName::from_str(k), HeaderValue::from_str(val_str))
                {
                    headers.insert(name, value);
                }
            }
        }
    }
    headers
}

// ---------------------------------------------------------------------------
// Helper: build MpdProxyParams from query + config
// ---------------------------------------------------------------------------

fn build_proxy_params(
    proxy_data: &ProxyData,
    query: &HashMap<String, String>,
    config: &Config,
) -> MpdProxyParams {
    let pass_headers: HashMap<String, String> = proxy_data
        .request_headers
        .as_ref()
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    // Support combined kid:key,kid:key format passed as a single key= param.
    // e.g. key=010304...:5228...,0100...:a947...  →  key_id=010304...,0100...  key=5228...,a947...
    let mut key_id_opt = query.get("key_id").filter(|v| !v.is_empty()).cloned();
    let mut key_opt = query.get("key").filter(|v| !v.is_empty()).cloned();
    if key_id_opt.is_none() {
        if let Some(ref k) = key_opt.clone() {
            if k.contains(':') {
                let pairs: Vec<&str> = k.split(',').collect();
                if pairs.iter().all(|p| p.contains(':')) {
                    key_id_opt = Some(
                        pairs
                            .iter()
                            .map(|p| p.split(':').next().unwrap_or(""))
                            .collect::<Vec<_>>()
                            .join(","),
                    );
                    key_opt = Some(
                        pairs
                            .iter()
                            .map(|p| p.split_once(':').map(|x| x.1).unwrap_or(""))
                            .collect::<Vec<_>>()
                            .join(","),
                    );
                }
            }
        }
    }

    MpdProxyParams {
        api_password: config.auth.api_password.clone(),
        pass_headers,
        key_id: key_id_opt,
        key: key_opt,
        resolution: query.get("resolution").cloned(),
        skip: query.get("skip").cloned(),
        remux_to_ts: query
            .get("remux_to_ts")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(config.mpd.remux_to_ts),
    }
}

// ---------------------------------------------------------------------------
// SegmentBase SIDX expansion
// ---------------------------------------------------------------------------

/// If `profile` has exactly one segment with an `index_range`, fetch the SIDX box
/// from the CDN and replace the single large-file segment with per-fragment entries.
///
/// This converts a SegmentBase MPD's "one segment = entire movie" into N HLS segments
/// (one per SIDX fragment), enabling efficient seeking without downloading from byte 0.
///
/// Modifies `profile.segments` in-place; on any failure the original single-segment
/// entry is kept as a fallback so playback still works (just without seeking).
async fn expand_segment_base_segments(
    profile: &mut MpdProfile,
    sm: &StreamManager,
    headers: HeaderMap,
) {
    if profile.segments.len() != 1 {
        return;
    }

    let seg = &profile.segments[0];
    let index_range = match seg.index_range.as_deref().filter(|r| !r.is_empty()) {
        Some(r) => r.to_string(),
        None => return,
    };
    let media_url = seg.media.clone();
    let init_range = seg.init_range.clone();

    let index_range_start: u64 = index_range
        .split('-')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    match fetch_with_range(sm, &media_url, Some(&index_range), headers).await {
        Ok(sidx_bytes) => {
            let fragments = parse_sidx_fragments(&sidx_bytes, index_range_start);

            if fragments.is_empty() {
                tracing::warn!("SIDX parse returned no fragments for {media_url}, keeping single-segment fallback");
                return;
            }

            let new_segments: Vec<MpdSegment> = fragments
                .iter()
                .enumerate()
                .map(|(i, frag)| MpdSegment {
                    media: media_url.clone(),
                    number: (i + 1) as u64,
                    extinf: frag.duration_timescale as f64 / frag.timescale as f64,
                    time: None,
                    duration_mpd_timescale: None,
                    start_unix: None,
                    program_date_time: None,
                    media_range: Some(format!("{}-{}", frag.start, frag.end)),
                    init_range: init_range.clone(),
                    index_range: None,
                })
                .collect();

            tracing::info!(
                "SegmentBase SIDX expanded {:?} → {} fragments ({:.3}s each)",
                media_url,
                new_segments.len(),
                new_segments.first().map(|s| s.extinf).unwrap_or(0.0),
            );

            profile.segments = new_segments;
        }
        Err(e) => {
            tracing::warn!(
                "SIDX fetch failed for {media_url} ({index_range}): {e} — \
                 keeping single-segment fallback"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// GET /proxy/mpd/manifest
// ---------------------------------------------------------------------------

/// Fetch the upstream MPD, parse it, and return an HLS master manifest.
pub async fn mpd_manifest_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    proxy_data: web::ReqData<ProxyData>,
    config: web::Data<Arc<Config>>,
    metrics: web::Data<Arc<AppMetrics>>,
    mpd_cache: web::Data<LocalCache>,
) -> AppResult<HttpResponse> {
    metrics.inc_request();
    metrics.mpd_requests.fetch_add(1, Ordering::Relaxed);
    let destination = proxy_data.destination.clone();
    let proxy_base = proxy_base_url(&req);
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let params = build_proxy_params(&proxy_data, &query, &config);
    let request_headers = build_request_headers(&proxy_data);

    // Fetch the upstream MPD (cached to avoid re-fetching for every parallel playlist request)
    let mpd_bytes =
        fetch_mpd_cached(&stream_manager, &mpd_cache, &destination, request_headers).await?;

    // Parse the MPD
    let doc =
        parse_mpd(&mpd_bytes).map_err(|e| AppError::Mpd(format!("Failed to parse MPD: {e}")))?;

    // Convert to ParsedMpd (no segment parsing at master level)
    let parsed = parse_mpd_document(&doc, &destination, None);

    // Build HLS master manifest
    let hls = build_hls_master(&parsed, &proxy_base, &destination, &params);

    metrics.add_bytes_out(hls.len() as u64);
    Ok(HttpResponse::Ok()
        .content_type("application/vnd.apple.mpegurl")
        .insert_header(("cache-control", "no-cache, no-store"))
        .body(hls))
}

// ---------------------------------------------------------------------------
// GET /proxy/mpd/playlist
// ---------------------------------------------------------------------------

/// Fetch the upstream MPD, parse segments for `profile_id`, and return an HLS
/// media playlist.
pub async fn mpd_playlist_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    proxy_data: web::ReqData<ProxyData>,
    config: web::Data<Arc<Config>>,
    metrics: web::Data<Arc<AppMetrics>>,
    mpd_cache: web::Data<LocalCache>,
) -> AppResult<HttpResponse> {
    metrics.inc_request();
    metrics.mpd_requests.fetch_add(1, Ordering::Relaxed);
    let destination = proxy_data.destination.clone();
    let proxy_base = proxy_base_url(&req);
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let profile_id = query
        .get("profile_id")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing profile_id".into()))?;

    let params = build_proxy_params(&proxy_data, &query, &config);
    let request_headers = build_request_headers(&proxy_data);

    // Fetch MPD (cached — all 40+ parallel playlist requests share one upstream fetch)
    let mpd_bytes = fetch_mpd_cached(
        &stream_manager,
        &mpd_cache,
        &destination,
        request_headers.clone(),
    )
    .await?;

    // Parse with segment generation for the specific profile
    let doc =
        parse_mpd(&mpd_bytes).map_err(|e| AppError::Mpd(format!("Failed to parse MPD: {e}")))?;

    let mut parsed = parse_mpd_document(&doc, &destination, Some(&profile_id));

    // For SegmentBase profiles with a SIDX index range, expand the single large-file
    // segment into per-fragment segments so seeking works without re-downloading from
    // the start of the file.
    if let Some(profile_mut) = parsed.profiles.iter_mut().find(|p| p.id == profile_id) {
        expand_segment_base_segments(profile_mut, &stream_manager, request_headers.clone()).await;
    }

    // Find the profile (immutable borrow for playlist building)
    let profile = parsed
        .profiles
        .iter()
        .find(|p| p.id == profile_id)
        .ok_or_else(|| AppError::Mpd(format!("Profile {profile_id} not found in MPD")))?;

    // Parse start_offset
    let start_offset: Option<f64> = query.get("start_offset").and_then(|v| v.parse().ok());

    let hls = build_hls_media_playlist(
        &parsed,
        profile,
        &proxy_base,
        &destination,
        &params,
        &[], // skip_ranges — TODO parse from query
        start_offset,
        config.mpd.live_playlist_depth,
    );

    metrics.add_bytes_out(hls.len() as u64);
    Ok(HttpResponse::Ok()
        .content_type("application/vnd.apple.mpegurl")
        .insert_header(("cache-control", "no-cache, no-store"))
        .body(hls))
}

// ---------------------------------------------------------------------------
// GET /proxy/mpd/segment  &  /proxy/mpd/segment.{ext}
// ---------------------------------------------------------------------------

/// Proxy a DASH media segment.
/// Query params: `init_url`, `segment_url`, `mime_type`, `is_live`, `use_map`,
/// `key_id`, `key`, and `h_*` headers.
pub async fn mpd_segment_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    proxy_data: web::ReqData<ProxyData>,
    _config: web::Data<Arc<Config>>,
    metrics: web::Data<Arc<AppMetrics>>,
) -> AppResult<HttpResponse> {
    metrics.inc_request();
    metrics.mpd_requests.fetch_add(1, Ordering::Relaxed);
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let init_url = query
        .get("init_url")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing init_url".into()))?;
    let segment_url = query
        .get("segment_url")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing segment_url".into()))?;
    let mime_type = query
        .get("mime_type")
        .cloned()
        .unwrap_or_else(|| "video/mp4".to_string());
    let use_map = query
        .get("use_map")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    let _is_live = query
        .get("is_live")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    // SegmentBase: byte range for the media portion (everything after the init segment)
    let segment_range = query
        .get("segment_range")
        .filter(|v| !v.is_empty())
        .cloned();

    let request_headers = build_request_headers(&proxy_data);

    // Always fetch the (small) init segment up-front — needed for DRM state setup
    // and for non-DRM concatenation.  The init range is typically < 1 KB.
    let init_bytes = fetch_with_range(
        &stream_manager,
        &init_url,
        query.get("init_range").map(String::as_str),
        request_headers.clone(),
    )
    .await
    .unwrap_or_default();

    let key_id = query.get("key_id").filter(|v| !v.is_empty()).cloned();
    let key_val = query.get("key").filter(|v| !v.is_empty()).cloned();

    // -----------------------------------------------------------------------
    // DRM + SegmentBase: stream-decrypt the large media file instead of
    // buffering it entirely.  For a typical 2-hour movie the audio track is
    // ~120 MB; buffering before serving causes mpv/ffmpeg to time out.
    // -----------------------------------------------------------------------
    #[cfg(feature = "drm")]
    if let (Some(ref kid), Some(ref k)) = (&key_id, &key_val) {
        if let Some(ref range) = segment_range {
            // Move CDN connection setup inside the stream generator so that
            // HTTP response headers are sent to ffmpeg immediately.  If we
            // .await create_stream() before returning an HttpResponse, ffmpeg's
            // open-timeout fires ("Immediate exit requested") on slow CDNs.
            // We also send Connection: close so actix doesn't try to reuse a
            // keep-alive connection that will have expired by the time this
            // long-running response finishes (prevents EOF on the next request).
            let kid_owned = kid.clone();
            let k_owned = k.clone();
            let range_owned = range.clone();
            let sm = stream_manager.clone();
            let seg_url = segment_url.clone();
            let req_hdrs = request_headers.clone();
            let use_map_val = use_map;
            let init_owned = init_bytes.clone();

            let out_stream = async_stream::stream! {
                use futures::StreamExt as _;
                let mut stream_headers = req_hdrs;
                let range_val = format!("bytes={}", range_owned);
                if let Ok(v) = HeaderValue::from_str(&range_val) {
                    stream_headers.insert(reqwest::header::RANGE, v);
                }
                let raw_stream = match sm.create_stream(seg_url, stream_headers, false).await {
                    Ok((_, _, Some(s))) => s,
                    Ok((_, _, None)) => {
                        yield Err(AppError::Proxy("CDN returned empty body".to_string()));
                        return;
                    }
                    Err(e) => { yield Err(e); return; }
                };
                let decrypted = crate::drm::cenc::decrypt_segment_streaming(
                    init_owned, raw_stream, kid_owned, k_owned, !use_map_val,
                );
                let mut pinned = std::pin::pin!(decrypted);
                while let Some(item) = pinned.next().await {
                    yield item;
                }
            };

            return Ok(HttpResponse::Ok()
                .content_type(mime_type_to_content_type(&mime_type, &query))
                .force_close()
                .streaming(Box::pin(out_stream)));
        }
    }

    // -----------------------------------------------------------------------
    // Non-streaming path: buffer the segment then decrypt / concatenate.
    // Used for normal fragmented DASH (each segment is already small).
    // -----------------------------------------------------------------------
    let segment_bytes = fetch_with_range(
        &stream_manager,
        &segment_url,
        segment_range.as_deref(),
        request_headers.clone(),
    )
    .await?;

    #[cfg(feature = "drm")]
    if let (Some(ref kid), Some(ref k)) = (&key_id, &key_val) {
        let decrypted =
            crate::drm::cenc::decrypt_segment(&init_bytes, &segment_bytes, kid, k, !use_map)
                .map_err(AppError::Drm)?;
        metrics.add_bytes_out(decrypted.len() as u64);
        return Ok(HttpResponse::Ok()
            .content_type(mime_type_to_content_type(&mime_type, &query))
            .body(bytes::Bytes::from(decrypted)));
    }

    // Non-DRM path
    let body: bytes::Bytes = if use_map || init_bytes.is_empty() {
        segment_bytes
    } else {
        let mut combined = bytes::BytesMut::with_capacity(init_bytes.len() + segment_bytes.len());
        combined.extend_from_slice(&init_bytes);
        combined.extend_from_slice(&segment_bytes);
        combined.freeze()
    };

    let content_type = mime_type_to_content_type(&mime_type, &query);
    metrics.add_bytes_out(body.len() as u64);
    Ok(HttpResponse::Ok().content_type(content_type).body(body))
}

// ---------------------------------------------------------------------------
// GET /proxy/mpd/init
// ---------------------------------------------------------------------------

/// Proxy a DASH initialization segment (EXT-X-MAP).
pub async fn mpd_init_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    proxy_data: web::ReqData<ProxyData>,
    _config: web::Data<Arc<Config>>,
    metrics: web::Data<Arc<AppMetrics>>,
) -> AppResult<HttpResponse> {
    metrics.inc_request();
    metrics.mpd_requests.fetch_add(1, Ordering::Relaxed);
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let init_url = query
        .get("init_url")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing init_url".into()))?;
    let mime_type = query
        .get("mime_type")
        .cloned()
        .unwrap_or_else(|| "video/mp4".to_string());

    let request_headers = build_request_headers(&proxy_data);

    let raw_init = fetch_with_range(
        &stream_manager,
        &init_url,
        query.get("init_range").map(String::as_str),
        request_headers,
    )
    .await?;

    let key_id = query.get("key_id").filter(|v| !v.is_empty()).cloned();
    let key_val = query.get("key").filter(|v| !v.is_empty()).cloned();

    #[cfg(feature = "drm")]
    let init_bytes: bytes::Bytes = if let (Some(ref kid), Some(ref k)) = (&key_id, &key_val) {
        let processed =
            crate::drm::cenc::process_drm_init_segment(&raw_init, kid, k).map_err(AppError::Drm)?;
        bytes::Bytes::from(processed)
    } else {
        raw_init
    };

    #[cfg(not(feature = "drm"))]
    let init_bytes: bytes::Bytes = raw_init;

    let content_type = mime_type_to_content_type(&mime_type, &query);

    metrics.add_bytes_out(init_bytes.len() as u64);
    Ok(HttpResponse::Ok()
        .content_type(content_type)
        .insert_header(("cache-control", "no-cache"))
        .body(init_bytes))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Fetch an MPD document, returning cached bytes on subsequent calls for the same URL.
///
/// This avoids re-downloading the (often 1 MB+) MPD for every parallel playlist
/// request that mpv opens simultaneously (one per audio/video track).
async fn fetch_mpd_cached(
    sm: &StreamManager,
    cache: &LocalCache,
    url: &str,
    headers: reqwest::header::HeaderMap,
) -> AppResult<bytes::Bytes> {
    if let Some(cached) = cache.get(url).await {
        return Ok(cached);
    }
    let fetched = sm
        .fetch_bytes(url.to_string(), headers)
        .await
        .map_err(|e| {
            tracing::warn!("Failed to fetch MPD from {}: {}", url, e);
            e
        })?;
    cache.set(url.to_string(), fetched.clone()).await;
    Ok(fetched)
}

/// Fetch a resource, optionally restricted to a byte range via `Range` header.
async fn fetch_with_range(
    sm: &StreamManager,
    url: &str,
    range: Option<&str>,
    mut headers: HeaderMap,
) -> AppResult<bytes::Bytes> {
    if let Some(r) = range {
        // Convert MPD byte-range "start-end" to HTTP "bytes=start-end"
        let range_header = format!("bytes={r}");
        if let Ok(v) = HeaderValue::from_str(&range_header) {
            headers.insert(reqwest::header::RANGE, v);
        }
    }
    sm.fetch_bytes(url.to_string(), headers).await
}

/// Map a DASH MIME type to an HTTP content-type string.
fn mime_type_to_content_type(mime_type: &str, query: &HashMap<String, String>) -> &'static str {
    // TS remux requested?
    if query
        .get("remux_to_ts")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
    {
        return "video/mp2t";
    }
    if mime_type.contains("mp2t") || mime_type.contains("mpeg-ts") {
        "video/mp2t"
    } else {
        "video/mp4"
    }
}
