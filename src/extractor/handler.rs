//! Route handler for `GET /extractor/video`.
//!
//! Query params:
//! - `host`            — extractor key (e.g. `"VixCloud"`)
//! - `d`               — source page URL to extract from (alias: `url`)
//! - `redirect_stream` — if `"true"`, return 302 redirect to the proxy endpoint
//! - `api_password`    — (validated by middleware)
//! - `h_*`             — extra request headers forwarded to the extractor

use std::collections::HashMap;
use std::sync::Arc;

use actix_web::{web, HttpRequest, HttpResponse};

use crate::{
    auth::encryption::ProxyData,
    config::Config,
    error::{AppError, AppResult},
    extractor::{base::ExtraParams, factory::get_extractor},
    proxy::stream::StreamManager,
};

pub async fn extractor_video_handler(
    req: HttpRequest,
    _proxy_data: web::ReqData<ProxyData>,
    config: web::Data<Arc<Config>>,
    stream_manager: web::Data<StreamManager>,
) -> AppResult<HttpResponse> {
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let host = query
        .get("host")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing host param".into()))?;

    // Accept "d" (Python/canonical convention) or "url" as the source page URL.
    let url = query
        .get("d")
        .or_else(|| query.get("url"))
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing url/d param".into()))?;

    let redirect_stream = query
        .get("redirect_stream")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    // Build request headers from h_* query params.
    let request_headers: HashMap<String, String> = query
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("h_")
                .map(|name| (name.to_string(), v.clone()))
        })
        .collect();

    let extra = ExtraParams {
        raw: query
            .iter()
            .filter(|(k, _)| {
                !["host", "url", "d", "api_password", "redirect_stream"].contains(&k.as_str())
                    && !k.starts_with("h_")
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    };

    // Use the already-built ProxyRouter inside StreamManager — avoids recompiling
    // all route regexes on every extraction request.
    let proxy_url: Option<String> = stream_manager.get_proxy_url_for(&url);

    let extractor = get_extractor(&host, request_headers, proxy_url)
        .map_err(|e| AppError::Extractor(e.to_string()))?;

    tracing::info!("Extractor: host={host}, url={url}");

    let result = extractor.extract(&url, &extra).await.map_err(|e| {
        tracing::error!("Extractor error: host={host}, url={url}, error={e}");
        AppError::Extractor(e.to_string())
    })?;

    if redirect_stream {
        // Map extractor endpoint to the corresponding proxy path.
        let proxy_path = match result.mediaflow_endpoint {
            "hls_manifest_proxy" => "/proxy/hls/manifest.m3u8",
            "hls_key_proxy" => "/proxy/hls/key_proxy/manifest.m3u8",
            "mpd_manifest_proxy" => "/proxy/mpd/manifest.m3u8",
            _ => "/proxy/stream", // proxy_stream_endpoint + fallback
        };

        // Build query string: d=<url>&api_password=<pw>&h_<name>=<val>...
        let mut params: Vec<String> = vec![
            format!("d={}", urlencoding::encode(&result.destination_url)),
            format!(
                "api_password={}",
                urlencoding::encode(&config.auth.api_password)
            ),
        ];
        for (name, value) in &result.request_headers {
            params.push(format!(
                "h_{}={}",
                urlencoding::encode(name),
                urlencoding::encode(value),
            ));
        }

        let conn = req.connection_info();
        let redirect_url = format!(
            "{}://{}{}?{}",
            conn.scheme(),
            conn.host(),
            proxy_path,
            params.join("&"),
        );

        tracing::info!("Extractor redirect: {redirect_url}");

        return Ok(HttpResponse::TemporaryRedirect()
            .append_header(("Location", redirect_url))
            .finish());
    }

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "destination_url": result.destination_url,
        "request_headers": result.request_headers,
        "mediaflow_endpoint": result.mediaflow_endpoint,
    })))
}
