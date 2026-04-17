//! Speed test route handlers.
//!
//! Routes:
//! - `GET  /speedtest`        — redirect to Web UI speedtest page
//! - `POST /speedtest/config` — return test URLs for a given provider

use std::sync::Arc;

use actix_web::{web, HttpResponse};
use serde::Deserialize;

use crate::{
    config::Config,
    error::{AppError, AppResult},
    speedtest::providers::{all_debrid_config, real_debrid_config},
};

#[derive(Debug, Deserialize)]
pub struct SpeedTestRequest {
    pub provider: String,
    pub api_key: Option<String>,
}

pub async fn speedtest_redirect_handler() -> HttpResponse {
    HttpResponse::Found()
        .insert_header(("location", "/speedtest.html"))
        .finish()
}

pub async fn speedtest_config_handler(
    body: web::Json<SpeedTestRequest>,
    _config: web::Data<Arc<Config>>,
) -> AppResult<HttpResponse> {
    let config = match body.provider.to_lowercase().as_str() {
        "real_debrid" | "realdebrid" => real_debrid_config(),
        "all_debrid" | "alldebrid" => {
            let api_key = body
                .api_key
                .as_deref()
                .filter(|k| !k.is_empty())
                .ok_or_else(|| AppError::BadRequest("api_key required for AllDebrid".into()))?;
            all_debrid_config(api_key)
                .await
                .map_err(AppError::Extractor)?
        }
        other => return Err(AppError::BadRequest(format!("Unknown provider: {other}"))),
    };

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "provider": body.provider,
        "test_duration_secs": config.test_duration_secs,
        "test_urls": config.test_urls,
        "user_info": config.user_info,
    })))
}
