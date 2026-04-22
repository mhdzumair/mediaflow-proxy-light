use actix_web::HttpMessage;
use actix_web::{
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    Error,
};
use futures::future::LocalBoxFuture;
use serde_json::Value;
use std::future::{ready, Ready};
use std::rc::Rc;
use std::sync::Arc;

use crate::auth::encryption::{EncryptionHandler, ProxyData};
use crate::error::AppError;

const OPEN_ENDPOINTS: &[&str] = &[
    "/proxy/generate_url",
    // URL-generation handlers authenticate via `api_password` in the JSON body,
    // not query string — so the middleware must let these through untouched.
    "/generate_url",
    "/generate_urls",
    "/generate_encrypted_or_encoded_url",
    "/health",
    // Web UI navigation paths (redirect to .html pages)
    "/speedtest",
    "/url-generator",
    "/playlist/builder",
];

/// Static web UI assets (html/js/css/images) are public — they contain no secrets.
fn is_static_asset(path: &str) -> bool {
    if path == "/" {
        return true;
    }
    matches!(
        std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str()),
        Some(
            "html"
                | "js"
                | "css"
                | "png"
                | "jpg"
                | "jpeg"
                | "ico"
                | "svg"
                | "woff"
                | "woff2"
                | "ttf"
        )
    )
}

#[derive(Clone)]
pub struct AuthMiddleware {
    encryption_handler: Option<Arc<EncryptionHandler>>,
    api_password: String,
}

impl AuthMiddleware {
    pub fn new(api_password: String) -> Self {
        let encryption_handler = if !api_password.is_empty() {
            Some(Arc::new(
                EncryptionHandler::new(api_password.as_bytes())
                    .expect("Failed to create encryption handler"),
            ))
        } else {
            None
        };

        Self {
            encryption_handler,
            api_password,
        }
    }

    fn extract_query_params(query_string: &str) -> serde_json::Map<String, Value> {
        let mut params = serde_json::Map::new();
        for pair in query_string.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                if !key.is_empty() && !value.is_empty() {
                    params.insert(
                        key.to_string(),
                        Value::String(
                            urlencoding::decode(value)
                                .unwrap_or_else(|_| value.into())
                                .into_owned(),
                        ),
                    );
                }
            }
        }
        params
    }
}

impl<S, B> Transform<S, ServiceRequest> for AuthMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = AuthMiddlewareService<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(AuthMiddlewareService {
            service: Rc::new(service),
            encryption_handler: self.encryption_handler.clone(),
            api_password: self.api_password.clone(),
        }))
    }
}

pub struct AuthMiddlewareService<S> {
    service: Rc<S>,
    encryption_handler: Option<Arc<EncryptionHandler>>,
    api_password: String,
}

impl<S, B> Service<ServiceRequest> for AuthMiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = self.service.clone();
        let encryption_handler = self.encryption_handler.clone();
        let api_password = self.api_password.clone();

        Box::pin(async move {
            // Check if path is in open endpoints or is a static web UI asset
            if OPEN_ENDPOINTS.iter().any(|path| req.path() == *path) || is_static_asset(req.path())
            {
                return service.call(req).await;
            }

            // If API password is not set, allow all requests
            if api_password.is_empty() {
                return service.call(req).await;
            }

            let query_string = req.query_string().to_owned();
            let query_params = AuthMiddleware::extract_query_params(&query_string);

            // Check for encrypted token
            if let Some(token) = query_params.get("token").and_then(|v| v.as_str()) {
                if let Some(handler) = encryption_handler {
                    // Get client IP if needed for validation
                    let client_ip = req
                        .connection_info()
                        .realip_remote_addr()
                        .map(|s| s.to_string());

                    // Decrypt the token. A successful decryption is itself proof
                    // of authentication: the AES-256 key is derived from
                    // `api_password`, so only a caller that knows the password
                    // can produce a ciphertext that decrypts to valid JSON
                    // ProxyData (wrong key → Pkcs7 padding error or non-JSON
                    // garbage). No further api_password check inside the
                    // decrypted payload is needed, and requiring one breaks
                    // Python-proxy compatibility: neither the Python
                    // mediaflow-proxy nor our own `generate_url(s)` handlers
                    // embed api_password inside the encrypted payload, so the
                    // old post-decryption check rejected every token those
                    // code paths produced with a silent 401.
                    let proxy_data = handler
                        .decrypt(token, client_ip.as_deref())
                        .map_err(Error::from)?;

                    // Store proxy data in request extensions
                    req.extensions_mut().insert(proxy_data);
                    return service.call(req).await;
                }
            }

            // Check for direct API password
            if let Some(password) = query_params.get("api_password").and_then(|v| v.as_str()) {
                if password == api_password {
                    // Accept both "d" (canonical) and "url" (Python-proxy compat) as destination.
                    // Endpoints like /proxy/mpd/segment have no "d=" param — use empty string so
                    // ReqData<ProxyData> extraction never fails with a 500.
                    let destination = query_params
                        .get("d")
                        .or_else(|| query_params.get("url"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    // Build h_* and r_* sub-maps in a single pass over query_params,
                    // then move the full map into ProxyData (no clone).
                    let mut request_headers = serde_json::Map::new();
                    let mut response_headers = serde_json::Map::new();
                    for (k, v) in &query_params {
                        if let Some(stripped) = k.strip_prefix("h_") {
                            request_headers.insert(stripped.to_string(), v.clone());
                        } else if let Some(stripped) = k.strip_prefix("r_") {
                            response_headers.insert(stripped.to_string(), v.clone());
                        }
                    }

                    let proxy_data = ProxyData {
                        destination,
                        query_params: Some(Value::Object(query_params)),
                        request_headers: Some(Value::Object(request_headers)),
                        response_headers: Some(Value::Object(response_headers)),
                        exp: None,
                        ip: None,
                    };

                    req.extensions_mut().insert(proxy_data);
                    return service.call(req).await;
                }
            }

            Err(AppError::Auth("Invalid or missing authentication".to_string()).into())
        })
    }
}
