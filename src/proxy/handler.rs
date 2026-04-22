use actix_web::{
    body::SizedStream,
    web::{self, Bytes},
    HttpRequest, HttpResponse,
};
use futures::{stream, StreamExt};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::boxed::Box;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::{
    auth::{encryption::ProxyData, EncryptionHandler},
    error::{AppError, AppResult},
    metrics::AppMetrics,
    models::request::{GenerateUrlRequest, SUPPORTED_REQUEST_HEADERS, SUPPORTED_RESPONSE_HEADERS},
    proxy::stream::{ResponseStream, StreamManager},
    utils::base64_url::{decode_base64_url, encode_url_to_base64, is_base64_url},
};

/// RAII guard: increments active_connections on creation, decrements on drop.
struct ConnectionGuard(Arc<AppMetrics>);
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.connection_close();
    }
}

async fn handle_proxy_request(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    proxy_data: web::ReqData<ProxyData>,
    metrics: web::Data<Arc<AppMetrics>>,
    is_head: bool,
) -> AppResult<HttpResponse> {
    // Track active connection for the lifetime of this request
    metrics.connection_open();
    let _conn_guard = ConnectionGuard(Arc::clone(&metrics));
    // Prepare headers
    let mut request_headers = HeaderMap::new();

    // Add supported headers from original request
    for &header_name in SUPPORTED_REQUEST_HEADERS {
        if let Some(value) = req.headers().get(header_name) {
            request_headers.insert(
                HeaderName::from_str(header_name)
                    .map_err(|e| AppError::Internal(format!("Invalid header name: {}", e)))?,
                HeaderValue::try_from(value.as_bytes())
                    .map_err(|e| AppError::Internal(format!("Invalid header value: {}", e)))?,
            );
        }
    }

    // Add custom headers from proxy data
    if let Some(custom_headers) = &proxy_data.request_headers {
        for (key, value) in custom_headers
            .as_object()
            .unwrap_or(&serde_json::Map::new())
        {
            if let Some(value_str) = value.as_str() {
                request_headers.insert(
                    HeaderName::from_str(key)
                        .map_err(|e| AppError::Internal(format!("Invalid header name: {}", e)))?,
                    HeaderValue::from_str(value_str)
                        .map_err(|e| AppError::Internal(format!("Invalid header value: {}", e)))?,
                );
            }
        }
    }

    tracing::debug!("Request headers: {:?}", request_headers);

    // Create the stream — also get the upstream status code so we can mirror 206 etc.
    let (upstream_status, upstream_headers, stream_opt) = stream_manager
        .create_stream(proxy_data.destination.clone(), request_headers, is_head)
        .await?;

    tracing::debug!(
        "Upstream status: {}, headers: {:?}",
        upstream_status,
        upstream_headers
    );

    // Mirror the upstream status code (200 OK or 206 Partial Content for seeks)
    let mut response = HttpResponse::build(
        actix_web::http::StatusCode::from_u16(upstream_status.as_u16())
            .unwrap_or(actix_web::http::StatusCode::OK),
    );

    // Add supported headers from upstream response
    for &header_name in SUPPORTED_RESPONSE_HEADERS {
        if let Some(value) = upstream_headers.get(header_name) {
            if let Ok(converted_value) =
                actix_web::http::header::HeaderValue::from_str(value.to_str().unwrap_or_default())
            {
                response.insert_header((header_name, converted_value));
            }
        }
    }

    // Get content length from headers
    let content_length = upstream_headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    // Add custom response headers from proxy data
    if let Some(custom_headers) = &proxy_data.response_headers {
        for (key, value) in custom_headers
            .as_object()
            .unwrap_or(&serde_json::Map::new())
        {
            if let Some(value_str) = value.as_str() {
                response.insert_header((
                    actix_web::http::header::HeaderName::from_str(key)
                        .map_err(|e| AppError::Internal(format!("Invalid header name: {}", e)))?,
                    actix_web::http::header::HeaderValue::from_str(value_str)
                        .map_err(|e| AppError::Internal(format!("Invalid header value: {}", e)))?,
                ));
            }
        }
    }

    if is_head {
        let empty_stream = Box::pin(stream::empty::<Result<Bytes, std::io::Error>>());
        Ok(response
            .no_chunking(content_length)
            .body(SizedStream::new(content_length, empty_stream)))
    } else if let Some(stream) = stream_opt {
        // Wrap stream to count bytes served for metrics
        let metrics_clone = Arc::clone(&metrics);
        let counted_stream = stream_manager
            .stream_with_progress(stream)
            .map(move |chunk| {
                if let Ok(ref bytes) = chunk {
                    metrics_clone.add_bytes_out(bytes.len() as u64);
                }
                chunk
            });
        let response_stream = ResponseStream::new(counted_stream);
        if content_length > 0 {
            Ok(response
                .no_chunking(content_length)
                .body(SizedStream::new(content_length, response_stream)))
        } else {
            Ok(response.streaming(response_stream))
        }
    } else {
        Err(AppError::Internal("Stream not available".to_string()))
    }
}

pub async fn proxy_stream_get(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    proxy_data: web::ReqData<ProxyData>,
    metrics: web::Data<Arc<AppMetrics>>,
) -> AppResult<HttpResponse> {
    metrics.inc_request();
    metrics
        .proxy_stream_requests
        .fetch_add(1, Ordering::Relaxed);
    handle_proxy_request(req, stream_manager, proxy_data, metrics, false).await
}

pub async fn proxy_stream_head(
    req: HttpRequest,
    stream_manager: web::Data<StreamManager>,
    proxy_data: web::ReqData<ProxyData>,
    metrics: web::Data<Arc<AppMetrics>>,
) -> AppResult<HttpResponse> {
    metrics.inc_request();
    metrics
        .proxy_stream_requests
        .fetch_add(1, Ordering::Relaxed);
    handle_proxy_request(req, stream_manager, proxy_data, metrics, true).await
}

/// Shared URL-building logic used by generate_url and generate_encrypted_or_encoded_url.
///
/// Encrypted tokens use Python's path format: `{base}/_token_{token}{endpoint_path}`.
/// Unencrypted tokens use flat query params matching Python's encode_mediaflow_proxy_url.
pub fn build_proxy_url(
    mediaflow_proxy_url: &str,
    endpoint: Option<&str>,
    destination_url: &str,
    query_params: &HashMap<String, String>,
    request_headers: &HashMap<String, String>,
    response_headers: &HashMap<String, String>,
    propagate_response_headers: &HashMap<String, String>,
    remove_response_headers: &[String],
    stream_transformer: Option<&str>,
    filename: Option<&str>,
    api_password: Option<&str>,
    expiration: Option<u64>,
    ip: Option<&str>,
) -> AppResult<String> {
    let base = mediaflow_proxy_url.trim_end_matches('/');
    let endpoint_path = endpoint
        .filter(|ep| !ep.is_empty())
        .map(|ep| format!("/{}", ep.trim_start_matches('/')))
        .unwrap_or_default();

    if let Some(password) = api_password.filter(|p| !p.is_empty()) {
        let handler = EncryptionHandler::new(password.as_bytes()).map_err(|e| {
            AppError::Internal(format!("Failed to create encryption handler: {}", e))
        })?;

        let proxy_data = ProxyData {
            destination: destination_url.to_string(),
            query_params: Some(
                serde_json::to_value(query_params).map_err(AppError::SerdeJsonError)?,
            ),
            request_headers: Some(
                serde_json::to_value(request_headers).map_err(AppError::SerdeJsonError)?,
            ),
            response_headers: Some(
                serde_json::to_value(response_headers).map_err(AppError::SerdeJsonError)?,
            ),
            exp: expiration.map(|e| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + e
            }),
            ip: ip.map(|s| s.to_string()),
        };

        let token = handler.encrypt(&proxy_data)?;

        // Python format: {scheme}://{authority}/_token_{token}{endpoint_path}[/{filename}]
        // Split base URL into scheme+authority and any existing path.
        let (scheme_authority, existing_path) = split_base_and_path(base);
        let mut url = format!(
            "{}/_token_{}{}{}",
            scheme_authority, token, existing_path, endpoint_path
        );
        if let Some(fname) = filename {
            url = format!("{}/{}", url, urlencoding::encode(fname));
        }
        Ok(url)
    } else {
        // Unencrypted: flat query params matching Python's encode_mediaflow_proxy_url.
        let mut url = format!("{}{}", base, endpoint_path);
        if let Some(fname) = filename {
            url = format!("{}/{}", url, urlencoding::encode(fname));
        }

        let mut params: Vec<(String, String)> =
            vec![("d".to_string(), destination_url.to_string())];

        for (k, v) in query_params {
            if !v.is_empty() {
                params.push((k.clone(), v.clone()));
            }
        }

        for (k, v) in request_headers {
            if v.is_empty() {
                continue;
            }
            // Skip per-request dynamic headers (range, if-range) — baking them into
            // the URL would override the player's actual seek headers on playback.
            let k_lower = k.to_lowercase();
            let bare = k_lower.strip_prefix("h_").unwrap_or(&k_lower);
            if SUPPORTED_REQUEST_HEADERS.contains(&bare) {
                continue;
            }
            let prefixed = if k.starts_with("h_") {
                k.clone()
            } else {
                format!("h_{}", k)
            };
            params.push((prefixed, v.clone()));
        }

        for (k, v) in response_headers {
            if v.is_empty() {
                continue;
            }
            let prefixed = if k.starts_with("r_") {
                k.clone()
            } else {
                format!("r_{}", k)
            };
            params.push((prefixed, v.clone()));
        }

        for (k, v) in propagate_response_headers {
            if v.is_empty() {
                continue;
            }
            let prefixed = if k.starts_with("rp_") {
                k.clone()
            } else {
                format!("rp_{}", k)
            };
            params.push((prefixed, v.clone()));
        }

        if !remove_response_headers.is_empty() {
            params.push(("x_headers".to_string(), remove_response_headers.join(",")));
        }

        if let Some(transformer) = stream_transformer {
            params.push(("transformer".to_string(), transformer.to_string()));
        }

        let qs = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        Ok(format!("{}?{}", url, qs))
    }
}

/// Split `"https://host/path"` → `("https://host", "/path")`.
fn split_base_and_path(url: &str) -> (&str, &str) {
    if let Some(scheme_end) = url.find("://") {
        let after_scheme = &url[scheme_end + 3..];
        if let Some(path_pos) = after_scheme.find('/') {
            let split = scheme_end + 3 + path_pos;
            return (&url[..split], &url[split..]);
        }
    }
    (url, "")
}

pub async fn generate_url(req: web::Json<GenerateUrlRequest>) -> AppResult<HttpResponse> {
    let url = build_proxy_url(
        &req.mediaflow_proxy_url,
        req.endpoint.as_deref(),
        &req.destination_url,
        &req.query_params,
        &req.request_headers,
        &req.response_headers,
        &req.propagate_response_headers,
        &req.remove_response_headers,
        req.stream_transformer.as_deref(),
        req.filename.as_deref(),
        req.api_password.as_deref(),
        req.expiration,
        req.ip.as_deref(),
    )?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "url": url })))
}

// Mirrors Python's IP_LOOKUP_SERVICES — tried in order; first success wins.
const IP_LOOKUP_SERVICES: &[(&str, &str)] = &[
    ("https://api.ipify.org?format=json", "ip"),
    ("https://ipinfo.io/json", "ip"),
    ("https://httpbin.org/ip", "origin"),
];

pub async fn get_public_ip(stream_manager: web::Data<StreamManager>) -> AppResult<HttpResponse> {
    for (url, key) in IP_LOOKUP_SERVICES {
        match stream_manager
            .make_request((*url).to_string(), HeaderMap::new())
            .await
        {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(data) => {
                    if let Some(ip) = data.get(*key).and_then(|v| v.as_str()) {
                        let ip = ip.trim();
                        if !ip.is_empty() {
                            return Ok(HttpResponse::Ok().json(serde_json::json!({ "ip": ip })));
                        }
                    }
                    tracing::warn!("IP lookup {} returned no '{}' field", url, key);
                }
                Err(e) => tracing::warn!("IP lookup {} body parse failed: {}", url, e),
            },
            Err(e) => tracing::warn!("IP lookup {} request failed: {}", url, e),
        }
    }

    Err(AppError::Upstream(
        "Failed to retrieve public IP from all services".to_string(),
    ))
}

// Deprecated alias — same logic as generate_url but returns {"encoded_url": ...}
pub async fn generate_encrypted_or_encoded_url(
    req: web::Json<GenerateUrlRequest>,
) -> AppResult<HttpResponse> {
    let url = build_proxy_url(
        &req.mediaflow_proxy_url,
        req.endpoint.as_deref(),
        &req.destination_url,
        &req.query_params,
        &req.request_headers,
        &req.response_headers,
        &req.propagate_response_headers,
        &req.remove_response_headers,
        req.stream_transformer.as_deref(),
        req.filename.as_deref(),
        req.api_password.as_deref(),
        req.expiration,
        req.ip.as_deref(),
    )?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "encoded_url": url })))
}

// ---------------------------------------------------------------------------
// Multiple-URL generation
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
pub struct MultiUrlRequestItem {
    pub endpoint: Option<String>,
    pub destination_url: String,
    #[serde(default)]
    pub query_params: HashMap<String, String>,
    #[serde(default)]
    pub request_headers: HashMap<String, String>,
    #[serde(default)]
    pub response_headers: HashMap<String, String>,
    #[serde(default)]
    pub propagate_response_headers: HashMap<String, String>,
    #[serde(default)]
    pub remove_response_headers: Vec<String>,
    pub stream_transformer: Option<String>,
    pub filename: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct GenerateMultiUrlRequest {
    pub mediaflow_proxy_url: String,
    pub api_password: Option<String>,
    pub expiration: Option<u64>,
    pub ip: Option<String>,
    pub urls: Vec<MultiUrlRequestItem>,
}

pub async fn generate_urls(req: web::Json<GenerateMultiUrlRequest>) -> AppResult<HttpResponse> {
    let mut encoded: Vec<String> = Vec::with_capacity(req.urls.len());

    for item in &req.urls {
        let url = build_proxy_url(
            &req.mediaflow_proxy_url,
            item.endpoint.as_deref(),
            &item.destination_url,
            &item.query_params,
            &item.request_headers,
            &item.response_headers,
            &item.propagate_response_headers,
            &item.remove_response_headers,
            item.stream_transformer.as_deref(),
            item.filename.as_deref(),
            req.api_password.as_deref(),
            req.expiration,
            req.ip.as_deref(),
        )?;
        encoded.push(url);
    }

    Ok(HttpResponse::Ok().json(serde_json::json!({ "urls": encoded })))
}

// ---------------------------------------------------------------------------
// Base64 utilities
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct Base64Query {
    pub url: Option<String>,
    pub encoded_url: Option<String>,
}

pub async fn base64_encode(query: web::Query<Base64Query>) -> HttpResponse {
    let url = match &query.url {
        Some(u) => u.as_str(),
        None => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "missing `url` query param"}))
        }
    };
    let encoded = encode_url_to_base64(url);
    HttpResponse::Ok().json(serde_json::json!({"encoded_url": encoded, "original_url": url}))
}

pub async fn base64_decode(query: web::Query<Base64Query>) -> HttpResponse {
    let enc = match &query.encoded_url {
        Some(e) => e.as_str(),
        None => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "missing `encoded_url` query param"}))
        }
    };
    match decode_base64_url(enc) {
        Some(decoded) => {
            HttpResponse::Ok().json(serde_json::json!({"decoded_url": decoded, "encoded_url": enc}))
        }
        None => HttpResponse::BadRequest().json(serde_json::json!({"error": "invalid base64 URL"})),
    }
}

pub async fn base64_check(query: web::Query<Base64Query>) -> HttpResponse {
    let url = match &query.url {
        Some(u) => u.as_str(),
        None => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "missing `url` query param"}))
        }
    };
    let is_b64 = is_base64_url(url);
    let mut result = serde_json::json!({"url": url, "is_base64": is_b64});
    if is_b64 {
        if let Some(decoded) = decode_base64_url(url) {
            result["decoded_url"] = serde_json::Value::String(decoded);
        }
    }
    HttpResponse::Ok().json(result)
}
