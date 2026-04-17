//! Embedded static Web UI assets via `rust-embed`.

use actix_web::{HttpRequest, HttpResponse};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "static/"]
pub struct StaticAssets;

/// Serve a static asset from the embedded `static/` directory.
pub async fn static_asset_handler(req: HttpRequest) -> HttpResponse {
    let path = req.match_info().get("path").unwrap_or("index.html");

    match StaticAssets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            HttpResponse::Ok()
                .content_type(mime.as_ref())
                .body(content.data.into_owned())
        }
        None => HttpResponse::NotFound().body("Not Found"),
    }
}

/// Serve index.html at the root.
pub async fn index_handler() -> HttpResponse {
    match StaticAssets::get("index.html") {
        Some(content) => HttpResponse::Ok()
            .content_type("text/html; charset=utf-8")
            .body(content.data.into_owned()),
        None => HttpResponse::NotFound().body("Web UI not available"),
    }
}
