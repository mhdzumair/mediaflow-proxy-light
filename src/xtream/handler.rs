//! Actix-web route handlers for the Xtream Codes (XC) API proxy.
//!
//! Routes are registered at the root scope (not under `/proxy`) so that
//! any IPTV player pointing at this server just works.
//!
//! Registered in `main.rs` under the `xtream` feature flag.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use actix_web::{web, HttpRequest, HttpResponse};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::str::FromStr;
use urlencoding::encode as url_encode;

use crate::{
    config::Config,
    error::{AppError, AppResult},
    metrics::AppMetrics,
    proxy::stream::StreamManager,
    xtream::{
        auth::{parse_username_with_upstream, verify_xc_api_password},
        proxy::{build_upstream_url, forward_api_request, proxy_base_url},
    },
};

// ---------------------------------------------------------------------------
// Helper: extract upstream headers from client request
// ---------------------------------------------------------------------------

fn pass_through_headers(req: &HttpRequest) -> HeaderMap {
    const PASS: &[&str] = &["range", "if-range", "if-modified-since", "if-none-match"];
    let mut headers = HeaderMap::new();
    for name in PASS {
        if let Some(v) = req.headers().get(*name) {
            if let (Ok(n), Ok(val)) = (
                HeaderName::from_str(name),
                HeaderValue::try_from(v.as_bytes()),
            ) {
                headers.insert(n, val);
            }
        }
    }
    headers
}

// ---------------------------------------------------------------------------
// /player_api.php
// ---------------------------------------------------------------------------

pub async fn player_api_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    config: web::Data<Arc<Config>>,
) -> AppResult<HttpResponse> {
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let username = query
        .get("username")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing username".into()))?;
    let password = query
        .get("password")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing password".into()))?;
    let action = query.get("action").cloned();

    let creds = parse_username_with_upstream(&username)?;
    verify_xc_api_password(creds.api_password.as_deref(), &config.auth.api_password)?;

    // Forward query params with real username.
    let mut params: Vec<(String, String)> = vec![
        ("username".into(), creds.actual_username.clone()),
        ("password".into(), password),
    ];
    if let Some(act) = action {
        params.push(("action".into(), act));
    }
    for (k, v) in &query {
        if !["username", "password", "action", "api_password"].contains(&k.as_str()) {
            params.push((k.clone(), v.clone()));
        }
    }
    let qs = build_query_string(&params);
    let upstream_url = format!("{}player_api.php?{qs}", creds.upstream_base);

    tracing::info!(
        "XC player_api.php: upstream={}, user={}",
        creds.upstream_base,
        creds.actual_username
    );

    let (body, content_type) = forward_api_request(
        &upstream_url,
        &req,
        &stream_manager,
        &creds.upstream_base,
        &creds.actual_username,
        creds.api_password.as_deref(),
    )
    .await?;

    Ok(HttpResponse::Ok().content_type(content_type).body(body))
}

// ---------------------------------------------------------------------------
// /xmltv.php
// ---------------------------------------------------------------------------

pub async fn xmltv_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    config: web::Data<Arc<Config>>,
) -> AppResult<HttpResponse> {
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let username = query
        .get("username")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing username".into()))?;
    let password = query
        .get("password")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing password".into()))?;

    let creds = parse_username_with_upstream(&username)?;
    verify_xc_api_password(creds.api_password.as_deref(), &config.auth.api_password)?;

    let mut params: Vec<(String, String)> = vec![
        ("username".into(), creds.actual_username.clone()),
        ("password".into(), password),
    ];
    for (k, v) in &query {
        if !["username", "password", "api_password"].contains(&k.as_str()) {
            params.push((k.clone(), v.clone()));
        }
    }
    let qs = build_query_string(&params);
    let upstream_url = format!("{}xmltv.php?{qs}", creds.upstream_base);

    tracing::info!("XC xmltv.php: upstream={}", creds.upstream_base);

    let raw = stream_manager
        .fetch_bytes(upstream_url, HeaderMap::new())
        .await?;

    Ok(HttpResponse::Ok()
        .content_type("application/xml; charset=utf-8")
        .body(raw))
}

// ---------------------------------------------------------------------------
// /get.php  →  redirect to HLS manifest proxy
// ---------------------------------------------------------------------------

pub async fn get_playlist_handler(
    req: HttpRequest,
    config: web::Data<Arc<Config>>,
) -> AppResult<HttpResponse> {
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let username = query
        .get("username")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing username".into()))?;
    let password = query
        .get("password")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing password".into()))?;
    let output_type = query
        .get("type")
        .cloned()
        .unwrap_or_else(|| "m3u_plus".into());
    let output = query.get("output").cloned().unwrap_or_else(|| "ts".into());

    let creds = parse_username_with_upstream(&username)?;
    verify_xc_api_password(creds.api_password.as_deref(), &config.auth.api_password)?;

    let mut params: Vec<(String, String)> = vec![
        ("username".into(), creds.actual_username.clone()),
        ("password".into(), password),
        ("type".into(), output_type),
        ("output".into(), output),
    ];
    for (k, v) in &query {
        if !["username", "password", "type", "output", "api_password"].contains(&k.as_str()) {
            params.push((k.clone(), v.clone()));
        }
    }
    let qs = build_query_string(&params);
    let upstream_url = format!("{}get.php?{qs}", creds.upstream_base);

    let mediaflow_base = proxy_base_url(&req);
    let mut hls_params = format!("d={}", url_encode(&upstream_url));
    if let Some(ref pwd) = creds.api_password {
        hls_params.push_str(&format!("&api_password={}", url_encode(pwd)));
    }

    let redirect_url = format!("{mediaflow_base}/proxy/hls/manifest?{hls_params}");
    Ok(HttpResponse::Found()
        .insert_header(("location", redirect_url))
        .finish())
}

// ---------------------------------------------------------------------------
// /panel_api.php
// ---------------------------------------------------------------------------

pub async fn panel_api_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    config: web::Data<Arc<Config>>,
) -> AppResult<HttpResponse> {
    let query: HashMap<String, String> =
        web::Query::<HashMap<String, String>>::from_query(req.query_string())
            .map(|q| q.into_inner())
            .unwrap_or_default();

    let username = query
        .get("username")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing username".into()))?;
    let password = query
        .get("password")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Missing password".into()))?;

    let creds = parse_username_with_upstream(&username)?;
    verify_xc_api_password(creds.api_password.as_deref(), &config.auth.api_password)?;

    let mut params: Vec<(String, String)> = vec![
        ("username".into(), creds.actual_username.clone()),
        ("password".into(), password),
    ];
    for (k, v) in &query {
        if !["username", "password", "api_password"].contains(&k.as_str()) {
            params.push((k.clone(), v.clone()));
        }
    }
    let qs = build_query_string(&params);
    let upstream_url = format!("{}panel_api.php?{qs}", creds.upstream_base);

    tracing::info!("XC panel_api.php: upstream={}", creds.upstream_base);

    let (body, content_type) = forward_api_request(
        &upstream_url,
        &req,
        &stream_manager,
        &creds.upstream_base,
        &creds.actual_username,
        creds.api_password.as_deref(),
    )
    .await?;

    Ok(HttpResponse::Ok().content_type(content_type).body(body))
}

// ---------------------------------------------------------------------------
// /live/{username}/{password}/{stream_id}.{ext}
// ---------------------------------------------------------------------------

pub async fn live_stream_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    config: web::Data<Arc<Config>>,
    metrics: web::Data<Arc<AppMetrics>>,
    path: web::Path<(String, String, String, String)>,
) -> AppResult<HttpResponse> {
    metrics.inc_request();
    metrics
        .proxy_stream_requests
        .fetch_add(1, Ordering::Relaxed);

    let (username, password, stream_id, ext) = path.into_inner();

    let creds = parse_username_with_upstream(&username)?;
    verify_xc_api_password(creds.api_password.as_deref(), &config.auth.api_password)?;

    let stream_path = format!(
        "live/{}/{}/{}.{}",
        creds.actual_username, password, stream_id, ext
    );
    let upstream_url = build_upstream_url(&creds.upstream_base, &stream_path);

    tracing::info!("XC live: {stream_path}");

    // m3u8 → redirect to HLS proxy.
    if ext == "m3u8" || ext == "m3u" {
        let mediaflow_base = proxy_base_url(&req);
        let mut hls_params = format!("d={}", url_encode(&upstream_url));
        if let Some(ref pwd) = creds.api_password {
            hls_params.push_str(&format!("&api_password={}", url_encode(pwd)));
        }
        let redirect_url = format!("{mediaflow_base}/proxy/hls/manifest?{hls_params}");
        return Ok(HttpResponse::Found()
            .insert_header(("location", redirect_url))
            .finish());
    }

    proxy_stream_response(&upstream_url, &req, &stream_manager, &metrics).await
}

// ---------------------------------------------------------------------------
// /movie/{username}/{password}/{stream_id}.{ext}
// ---------------------------------------------------------------------------

pub async fn movie_stream_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    config: web::Data<Arc<Config>>,
    metrics: web::Data<Arc<AppMetrics>>,
    path: web::Path<(String, String, String, String)>,
) -> AppResult<HttpResponse> {
    metrics.inc_request();
    metrics
        .proxy_stream_requests
        .fetch_add(1, Ordering::Relaxed);

    let (username, password, stream_id, ext) = path.into_inner();

    let creds = parse_username_with_upstream(&username)?;
    verify_xc_api_password(creds.api_password.as_deref(), &config.auth.api_password)?;

    let stream_path = format!(
        "movie/{}/{}/{}.{}",
        creds.actual_username, password, stream_id, ext
    );
    let upstream_url = build_upstream_url(&creds.upstream_base, &stream_path);

    tracing::info!("XC movie: {stream_path}");

    proxy_stream_response(&upstream_url, &req, &stream_manager, &metrics).await
}

// ---------------------------------------------------------------------------
// /series/{username}/{password}/{stream_id}/{season}/{episode}.{ext}
// ---------------------------------------------------------------------------

pub async fn series_stream_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    config: web::Data<Arc<Config>>,
    metrics: web::Data<Arc<AppMetrics>>,
    path: web::Path<(String, String, String, String, String, String)>,
) -> AppResult<HttpResponse> {
    metrics.inc_request();
    metrics
        .proxy_stream_requests
        .fetch_add(1, Ordering::Relaxed);

    let (username, password, stream_id, season, episode, ext) = path.into_inner();

    let creds = parse_username_with_upstream(&username)?;
    verify_xc_api_password(creds.api_password.as_deref(), &config.auth.api_password)?;

    let stream_path = format!(
        "series/{}/{}/{}/{}/{}.{}",
        creds.actual_username, password, stream_id, season, episode, ext
    );
    let upstream_url = build_upstream_url(&creds.upstream_base, &stream_path);

    tracing::info!("XC series: {stream_path}");

    proxy_stream_response(&upstream_url, &req, &stream_manager, &metrics).await
}

// ---------------------------------------------------------------------------
// /timeshift/{username}/{password}/{duration}/{start}/{stream_id}.ts
// ---------------------------------------------------------------------------

pub async fn timeshift_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    config: web::Data<Arc<Config>>,
    metrics: web::Data<Arc<AppMetrics>>,
    path: web::Path<(String, String, String, String, String)>,
) -> AppResult<HttpResponse> {
    metrics.inc_request();
    metrics
        .proxy_stream_requests
        .fetch_add(1, Ordering::Relaxed);

    let (username, password, duration, start, stream_id) = path.into_inner();

    let creds = parse_username_with_upstream(&username)?;
    verify_xc_api_password(creds.api_password.as_deref(), &config.auth.api_password)?;

    let stream_path = format!(
        "timeshift/{}/{}/{}/{}/{}.ts",
        creds.actual_username, password, duration, start, stream_id
    );
    let upstream_url = build_upstream_url(&creds.upstream_base, &stream_path);

    tracing::info!("XC timeshift: {stream_path}");

    proxy_stream_response(&upstream_url, &req, &stream_manager, &metrics).await
}

// ---------------------------------------------------------------------------
// Short-form  /{username}/{password}/{stream_id}[.{ext}]
// ---------------------------------------------------------------------------

pub async fn short_stream_handler(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    config: web::Data<Arc<Config>>,
    metrics: web::Data<Arc<AppMetrics>>,
    path: web::Path<(String, String, String)>,
) -> AppResult<HttpResponse> {
    metrics.inc_request();
    metrics
        .proxy_stream_requests
        .fetch_add(1, Ordering::Relaxed);

    let (username, password, stream_id) = path.into_inner();

    let creds = parse_username_with_upstream(&username)?;
    verify_xc_api_password(creds.api_password.as_deref(), &config.auth.api_password)?;

    let stream_path = format!("{}/{}/{}", creds.actual_username, password, stream_id);
    let upstream_url = build_upstream_url(&creds.upstream_base, &stream_path);

    tracing::info!("XC short stream: {stream_path}");

    proxy_stream_response(&upstream_url, &req, &stream_manager, &metrics).await
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

async fn proxy_stream_response(
    upstream_url: &str,
    req: &HttpRequest,
    stream_manager: &StreamManager,
    metrics: &AppMetrics,
) -> AppResult<HttpResponse> {
    let headers = pass_through_headers(req);
    let response = stream_manager
        .make_request(upstream_url.to_string(), headers)
        .await?;

    let status = response.status();
    let resp_headers = response.headers().clone();

    // Stream the body using chunked transfer.
    let body_bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::Proxy(format!("Failed to read upstream body: {e}")))?;

    metrics.add_bytes_out(body_bytes.len() as u64);

    let mut resp = HttpResponse::build(
        actix_web::http::StatusCode::from_u16(status.as_u16())
            .unwrap_or(actix_web::http::StatusCode::OK),
    );

    for (name, value) in &resp_headers {
        // Forward content-type and content-length only.
        let n = name.as_str();
        if n == "content-type" || n == "content-length" || n == "accept-ranges" {
            if let Ok(v) = actix_web::http::header::HeaderValue::from_bytes(value.as_bytes()) {
                resp.insert_header((n, v));
            }
        }
    }

    Ok(resp.body(body_bytes))
}

fn build_query_string(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}
