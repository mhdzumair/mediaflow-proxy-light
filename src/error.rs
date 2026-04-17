use actix_web::{HttpResponse, ResponseError};
use serde_json::json;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Proxy error: {0}")]
    Proxy(String),

    #[error("Internal server error: {0}")]
    Internal(String),

    #[error("Upstream service error: {0}")]
    Upstream(String),

    #[error("HLS processing error: {0}")]
    Hls(String),

    #[error("DASH/MPD processing error: {0}")]
    Mpd(String),

    #[error("DRM decryption error: {0}")]
    Drm(String),

    #[error("Extractor error: {0}")]
    Extractor(String),

    #[error("Cache error: {0}")]
    Cache(String),

    #[error("Transcode error: {0}")]
    Transcode(String),

    #[error("Xtream Codes error: {0}")]
    Xtream(String),

    #[error("Telegram error: {0}")]
    Telegram(String),

    #[error("Acestream error: {0}")]
    Acestream(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Serde JSON error: {0}")]
    SerdeJsonError(#[from] serde_json::Error),
}

impl ResponseError for AppError {
    fn error_response(&self) -> HttpResponse {
        match self {
            AppError::Auth(msg) => HttpResponse::Unauthorized().json(json!({ "error": msg })),
            AppError::Proxy(msg) => HttpResponse::BadGateway().json(json!({ "error": msg })),
            AppError::Internal(msg) => {
                HttpResponse::InternalServerError().json(json!({ "error": msg }))
            }
            AppError::Upstream(msg) => HttpResponse::BadGateway().json(json!({ "error": msg })),
            AppError::Hls(msg) => HttpResponse::BadGateway().json(json!({ "error": msg })),
            AppError::Mpd(msg) => HttpResponse::BadGateway().json(json!({ "error": msg })),
            AppError::Drm(msg) => HttpResponse::InternalServerError().json(json!({ "error": msg })),
            AppError::Extractor(msg) => HttpResponse::BadGateway().json(json!({ "error": msg })),
            AppError::Cache(msg) => {
                HttpResponse::InternalServerError().json(json!({ "error": msg }))
            }
            AppError::Transcode(msg) => {
                HttpResponse::InternalServerError().json(json!({ "error": msg }))
            }
            AppError::Xtream(msg) => HttpResponse::BadGateway().json(json!({ "error": msg })),
            AppError::Telegram(msg) => HttpResponse::BadGateway().json(json!({ "error": msg })),
            AppError::Acestream(msg) => HttpResponse::BadGateway().json(json!({ "error": msg })),
            AppError::BadRequest(msg) => HttpResponse::BadRequest().json(json!({ "error": msg })),
            AppError::NotFound(msg) => HttpResponse::NotFound().json(json!({ "error": msg })),
            AppError::Forbidden(msg) => HttpResponse::Forbidden().json(json!({ "error": msg })),
            AppError::SerdeJsonError(err) => {
                HttpResponse::InternalServerError().json(json!({ "error": err.to_string() }))
            }
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;
