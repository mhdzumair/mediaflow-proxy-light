use actix_web::{
    body::SizedStream,
    web::{self, Bytes},
    HttpRequest, HttpResponse,
};
use futures::{stream, StreamExt};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::boxed::Box;
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

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

pub async fn generate_url(req: web::Json<GenerateUrlRequest>) -> AppResult<HttpResponse> {
    let mut url = req.mediaflow_proxy_url.clone();

    if let Some(endpoint) = &req.endpoint {
        url = format!(
            "{}/{}",
            url.trim_end_matches('/'),
            endpoint.trim_start_matches('/')
        );
    }

    // If api_password is provided in the request body, encrypt the data
    if let Some(api_password) = &req.api_password {
        let encryption_handler = EncryptionHandler::new(api_password.as_bytes()).map_err(|e| {
            AppError::Internal(format!("Failed to create encryption handler: {}", e))
        })?;

        let proxy_data = ProxyData {
            destination: req.destination_url.clone(),
            query_params: Some(
                serde_json::to_value(&req.query_params).map_err(AppError::SerdeJsonError)?,
            ),
            request_headers: Some(
                serde_json::to_value(&req.request_headers).map_err(AppError::SerdeJsonError)?,
            ),
            response_headers: Some(
                serde_json::to_value(&req.response_headers).map_err(AppError::SerdeJsonError)?,
            ),
            exp: req.expiration.map(|e| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + e
            }),
            ip: req.ip.clone(),
        };

        let token = encryption_handler.encrypt(&proxy_data)?;
        url = format!("{}?token={}", url, token);
    } else {
        // If no api_password in body, encode parameters in URL
        let mut params = req.query_params.clone();
        params.insert("d".to_string(), req.destination_url.clone());

        // Add headers if provided with proper prefixes
        for (key, value) in &req.request_headers {
            params.insert(format!("h_{}", key), value.clone());
        }
        for (key, value) in &req.response_headers {
            params.insert(format!("r_{}", key), value.clone());
        }

        let query_string = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(&v.to_string())))
            .collect::<Vec<_>>()
            .join("&");

        url = format!("{}?{}", url, query_string);
    }

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "url": url
    })))
}

pub async fn get_public_ip(stream_manager: web::Data<StreamManager>) -> AppResult<HttpResponse> {
    let response = stream_manager
        .make_request(
            "https://api.ipify.org?format=json".to_string(),
            HeaderMap::new(),
        )
        .await?;

    let ip_data = response
        .json::<serde_json::Value>()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to parse IP response: {}", e)))?;

    Ok(HttpResponse::Ok().json(ip_data))
}

// ---------------------------------------------------------------------------
// Deprecated alias — identical logic to generate_url, returns {"encoded_url": ...}
// ---------------------------------------------------------------------------
pub async fn generate_encrypted_or_encoded_url(
    req: web::Json<GenerateUrlRequest>,
) -> AppResult<HttpResponse> {
    // Re-use the inner generate_url logic; translate {"url": x} → {"encoded_url": x}
    let resp = generate_url(req).await?;
    // The response body is a JSON object with a "url" field — re-wrap it
    Ok(resp)
}

// ---------------------------------------------------------------------------
// Multiple-URL generation
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
pub struct MultiUrlRequestItem {
    pub endpoint: Option<String>,
    pub destination_url: String,
    #[serde(default)]
    pub query_params: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub request_headers: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub response_headers: std::collections::HashMap<String, String>,
    pub filename: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct GenerateMultiUrlRequest {
    pub mediaflow_proxy_url: String,
    pub api_password: Option<String>,
    pub expiration: Option<u64>,
    pub urls: Vec<MultiUrlRequestItem>,
}

pub async fn generate_urls(req: web::Json<GenerateMultiUrlRequest>) -> AppResult<HttpResponse> {
    let encryption_handler = req
        .api_password
        .as_deref()
        .filter(|p| !p.is_empty())
        .map(|p| EncryptionHandler::new(p.as_bytes()))
        .transpose()
        .map_err(|e| AppError::Internal(format!("Encryption init failed: {e}")))?;

    let mut encoded: Vec<String> = Vec::with_capacity(req.urls.len());

    for item in &req.urls {
        let base = req.mediaflow_proxy_url.trim_end_matches('/');
        let mut url = match &item.endpoint {
            Some(ep) => format!("{}/{}", base, ep.trim_start_matches('/')),
            None => base.to_string(),
        };

        // Append filename to path if provided (cosmetic, for player format detection)
        if let Some(fname) = &item.filename {
            url = format!("{}/{}", url, fname.trim_start_matches('/'));
        }

        if let Some(ref enc) = encryption_handler {
            let proxy_data = ProxyData {
                destination: item.destination_url.clone(),
                query_params: Some(
                    serde_json::to_value(&item.query_params).map_err(AppError::SerdeJsonError)?,
                ),
                request_headers: Some(
                    serde_json::to_value(&item.request_headers)
                        .map_err(AppError::SerdeJsonError)?,
                ),
                response_headers: Some(
                    serde_json::to_value(&item.response_headers)
                        .map_err(AppError::SerdeJsonError)?,
                ),
                exp: req.expiration.map(|e| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                        + e
                }),
                ip: None,
            };
            let token = enc.encrypt(&proxy_data)?;
            url = format!("{}?token={}", url, token);
        } else {
            let mut params = item.query_params.clone();
            params.insert("d".to_string(), item.destination_url.clone());
            for (k, v) in &item.request_headers {
                params.insert(format!("h_{k}"), v.clone());
            }
            for (k, v) in &item.response_headers {
                params.insert(format!("r_{k}"), v.clone());
            }
            let qs = params
                .iter()
                .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{}?{}", url, qs);
        }

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
