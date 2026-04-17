use actix_cors::Cors;
use actix_web::middleware::Logger;
use actix_web::{web, App, HttpServer};
use std::sync::Arc;
use tracing_subscriber::{fmt::format::FmtSpan, layer::SubscriberExt, util::SubscriberInitExt};

mod auth;
mod cache;
mod config;
mod error;
mod metrics;
mod models;
mod proxy;
mod utils;

#[cfg(feature = "hls")]
mod hls;

#[cfg(feature = "mpd")]
mod mpd;

#[cfg(feature = "drm")]
mod drm;

#[cfg(feature = "xtream")]
mod xtream;

#[cfg(feature = "extractors")]
mod extractor;

#[cfg(feature = "acestream")]
mod acestream;

#[cfg(feature = "transcode")]
mod transcode;

#[cfg(feature = "telegram")]
mod telegram;

#[cfg(feature = "ffi")]
mod ffi;

mod epg;
mod playlist_builder;
mod speedtest;

#[cfg(feature = "web-ui")]
mod web_ui;

use auth::middleware::AuthMiddleware;
use config::Config;
use metrics::AppMetrics;
use proxy::{handler, stream::StreamManager};

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Load config first so log_level from APP__LOG_LEVEL is available.
    let config = Config::from_env().expect("Failed to load configuration");

    // RUST_LOG takes precedence; fall back to config.log_level.
    let log_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| config.log_level.clone());

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(log_filter))
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_thread_ids(false)
                .with_thread_names(false)
                .with_file(false)
                .with_line_number(false)
                .with_span_events(FmtSpan::NONE),
        )
        .try_init()
        .expect("Failed to initialize logging");
    let auth_middleware = AuthMiddleware::new(config.auth.api_password.clone());
    let stream_manager = StreamManager::new(config.proxy.clone());
    let server_config = Arc::new(config.clone());
    let app_metrics = AppMetrics::new();

    // MPD bytes cache: avoid re-fetching the (often 1 MB+) MPD for every parallel
    // playlist request (mpv opens one per audio/video track simultaneously).
    // 30-second TTL balances freshness for live streams with CDN request reduction.
    #[cfg(feature = "mpd")]
    let mpd_bytes_cache = web::Data::new(cache::local::LocalCache::new(
        200,
        std::time::Duration::from_secs(30),
    ));

    // EPG cache: XMLTV data is large but rarely changes — use a 1-hour TTL by default.
    // The newtype wrapper prevents Actix DI conflicts with the MPD LocalCache above.
    let epg_cache = web::Data::new(epg::handler::EpgCache(cache::local::LocalCache::new(
        50,
        std::time::Duration::from_secs(config.epg.cache_ttl.max(1)),
    )));

    tracing::info!(
        "Starting MediaFlow Proxy Light on {}:{}",
        server_config.server.host,
        server_config.server.port
    );

    HttpServer::new(move || {
        let config = Arc::clone(&server_config);
        let cors = Cors::permissive();
        let metrics = app_metrics.clone();

        // IMPORTANT: Specific /proxy/* sub-scopes MUST be registered before the
        // generic /proxy scope.  Actix-web matches services in registration order
        // using prefix matching, so a generic /proxy scope registered first would
        // swallow all /proxy/hls/*, /proxy/mpd/*, /proxy/telegram/* requests and
        // return 404 without reaching the more specific scopes.

        #[cfg(feature = "acestream")]
        let acestream_session_mgr =
            web::Data::new(acestream::session::AcestreamSessionManager::new());

        let mut app = App::new()
            .wrap(cors)
            .wrap(Logger::new("%a - \"%r\" %s"))
            // Compress::default() is intentionally omitted: the proxy streams
            // binary media (video/audio) that is already compressed.  Applying
            // gzip/brotli to such data burns CPU with zero gain and — worse —
            // actix's Compress wrapper buffers each streaming chunk before
            // encoding, which badly hurts streaming throughput.
            .wrap(auth_middleware.clone())
            .app_data(web::Data::new(stream_manager.clone()))
            .app_data(web::Data::new(config.clone()))
            .app_data(web::Data::new(metrics))
            .app_data(epg_cache.clone());

        // MPD bytes cache — register before route setup so handlers can extract it
        #[cfg(feature = "mpd")]
        {
            app = app.app_data(mpd_bytes_cache.clone());
        }

        let mut app = app
            // Root-level URL generation (Python-compatible paths)
            .route("/generate_url", web::post().to(handler::generate_url))
            .route("/generate_urls", web::post().to(handler::generate_urls))
            .route(
                "/generate_encrypted_or_encoded_url",
                web::post().to(handler::generate_url),
            )
            // Base64 utilities
            .route("/base64/encode", web::post().to(handler::base64_encode))
            .route("/base64/decode", web::post().to(handler::base64_decode))
            .route("/base64/check", web::get().to(handler::base64_check))
            // Health check
            .service(web::scope("/health").route("", web::get().to(|| async { "OK" })))
            // Usage metrics (unauthenticated — data is not sensitive)
            .route("/metrics", web::get().to(metrics::metrics_handler));

        // HLS routes (Phase 1)
        // Both semantic paths (/manifest) and Python-style .m3u8 paths are registered.
        #[cfg(feature = "hls")]
        {
            use hls::{handler as hls_handler, segment::hls_segment_handler};
            app = app.service(
                web::scope("/proxy/hls")
                    .route(
                        "/manifest",
                        web::get().to(hls_handler::hls_manifest_handler),
                    )
                    .route(
                        "/manifest",
                        web::head().to(hls_handler::hls_manifest_handler),
                    )
                    .route(
                        "/manifest.m3u8",
                        web::get().to(hls_handler::hls_manifest_handler),
                    )
                    .route(
                        "/manifest.m3u8",
                        web::head().to(hls_handler::hls_manifest_handler),
                    )
                    .route(
                        "/playlist",
                        web::get().to(hls_handler::hls_playlist_handler),
                    )
                    .route(
                        "/playlist",
                        web::head().to(hls_handler::hls_playlist_handler),
                    )
                    .route(
                        "/playlist.m3u8",
                        web::get().to(hls_handler::hls_playlist_handler),
                    )
                    .route(
                        "/playlist.m3u8",
                        web::head().to(hls_handler::hls_playlist_handler),
                    )
                    .route(
                        "/key_proxy/manifest.m3u8",
                        web::get().to(hls_handler::hls_manifest_handler),
                    )
                    .route(
                        "/key_proxy/manifest.m3u8",
                        web::head().to(hls_handler::hls_manifest_handler),
                    )
                    .route("/segment", web::get().to(hls_segment_handler))
                    .route("/segment", web::head().to(hls_segment_handler))
                    .route("/segment.{ext}", web::get().to(hls_segment_handler))
                    .route("/segment.{ext}", web::head().to(hls_segment_handler)),
            );
        }

        // MPD / DASH routes (Phase 2)
        #[cfg(feature = "mpd")]
        {
            use mpd::handler::{
                mpd_init_handler, mpd_manifest_handler, mpd_playlist_handler, mpd_segment_handler,
            };
            app = app.service(
                web::scope("/proxy/mpd")
                    .route("/manifest", web::get().to(mpd_manifest_handler))
                    .route("/manifest", web::head().to(mpd_manifest_handler))
                    .route("/manifest.m3u8", web::get().to(mpd_manifest_handler))
                    .route("/manifest.m3u8", web::head().to(mpd_manifest_handler))
                    .route("/manifest.mpd", web::get().to(mpd_manifest_handler))
                    .route("/manifest.mpd", web::head().to(mpd_manifest_handler))
                    .route("/playlist", web::get().to(mpd_playlist_handler))
                    .route("/playlist", web::head().to(mpd_playlist_handler))
                    .route("/playlist.m3u8", web::get().to(mpd_playlist_handler))
                    .route("/playlist.m3u8", web::head().to(mpd_playlist_handler))
                    .route("/segment", web::get().to(mpd_segment_handler))
                    .route("/segment", web::head().to(mpd_segment_handler))
                    .route("/segment.mp4", web::get().to(mpd_segment_handler))
                    .route("/segment.mp4", web::head().to(mpd_segment_handler))
                    .route("/segment.ts", web::get().to(mpd_segment_handler))
                    .route("/segment.ts", web::head().to(mpd_segment_handler))
                    .route("/segment.{ext}", web::get().to(mpd_segment_handler))
                    .route("/segment.{ext}", web::head().to(mpd_segment_handler))
                    .route("/init", web::get().to(mpd_init_handler))
                    .route("/init", web::head().to(mpd_init_handler))
                    .route("/init.mp4", web::get().to(mpd_init_handler))
                    .route("/init.mp4", web::head().to(mpd_init_handler)),
            );
        }

        // ── /proxy/* sub-scopes ─────────────────────────────────────────────────
        // These MUST all be registered before the Xtream root-level catch-all
        // routes (/{username}/{password}/{stream_id}) because actix-web matches
        // in registration order and the 3-segment catch-all would otherwise
        // shadow paths like /proxy/telegram/status.

        // Telegram routes (Phase 9) — before Xtream catch-all
        #[cfg(feature = "telegram")]
        {
            use telegram::handler::{
                telegram_info_handler, telegram_status_handler, telegram_stream_handler,
            };
            app = app.service(
                web::scope("/proxy/telegram")
                    .route("/stream", web::get().to(telegram_stream_handler))
                    .route("/stream", web::head().to(telegram_stream_handler))
                    .route(
                        "/stream/{filename:.*}",
                        web::get().to(telegram_stream_handler),
                    )
                    .route(
                        "/stream/{filename:.*}",
                        web::head().to(telegram_stream_handler),
                    )
                    .route("/info", web::get().to(telegram_info_handler))
                    .route("/status", web::get().to(telegram_status_handler)),
            );
        }

        // Transcode routes (Phase 8) — before Xtream catch-all
        #[cfg(feature = "transcode")]
        {
            use transcode::handler::{
                transcode_handler, transcode_hls_init_handler, transcode_hls_playlist_handler,
                transcode_hls_segment_handler,
            };
            app = app.service(
                web::scope("/proxy/transcode")
                    .route("", web::get().to(transcode_handler))
                    .route("/init.mp4", web::get().to(transcode_hls_init_handler))
                    .route(
                        "/hls/playlist",
                        web::get().to(transcode_hls_playlist_handler),
                    )
                    .route("/hls/segment", web::get().to(transcode_hls_segment_handler))
                    .route("/hls/init", web::get().to(transcode_hls_init_handler)),
            );
        }

        // Acestream routes (Phase 7) — MUST be before Xtream catch-all.
        // Without this, /{username}/{password}/{stream_id}.{ext} matches
        // /proxy/acestream/manifest.m3u8 (proxy=u, acestream=p, manifest=id, m3u8=ext).
        #[cfg(feature = "acestream")]
        {
            use acestream::handler::{
                acestream_manifest_handler, acestream_segment_handler, acestream_status_handler,
                acestream_stream_handler,
            };
            app = app.app_data(acestream_session_mgr.clone()).service(
                web::scope("/proxy/acestream")
                    .route("/manifest.m3u8", web::get().to(acestream_manifest_handler))
                    .route("/manifest.m3u8", web::head().to(acestream_manifest_handler))
                    .route("/stream", web::get().to(acestream_stream_handler))
                    .route("/stream", web::head().to(acestream_stream_handler))
                    .route("/segment.ts", web::get().to(acestream_segment_handler))
                    .route("/segment.{ext}", web::get().to(acestream_segment_handler))
                    .route("/status", web::get().to(acestream_status_handler)),
            );
        }

        // Content Extractor routes (Phase 5)
        #[cfg(feature = "extractors")]
        {
            use extractor::handler::extractor_video_handler;
            app = app.service(
                web::scope("/extractor")
                    .route("/video", web::get().to(extractor_video_handler))
                    .route("/video", web::head().to(extractor_video_handler))
                    .route("/video.{ext}", web::get().to(extractor_video_handler))
                    .route("/video.{ext}", web::head().to(extractor_video_handler)),
            );
        }

        // ── Xtream Codes routes (Phase 4) ───────────────────────────────────────
        // MUST come after all /proxy/* sub-scopes: the short-stream catch-all
        // /{username}/{password}/{stream_id} matches any 3-segment path at root.
        #[cfg(feature = "xtream")]
        {
            use xtream::handler::{
                get_playlist_handler, live_stream_handler, movie_stream_handler, panel_api_handler,
                player_api_handler, series_stream_handler, short_stream_handler, timeshift_handler,
                xmltv_handler,
            };
            app = app
                .route("/player_api.php", web::get().to(player_api_handler))
                .route("/xmltv.php", web::get().to(xmltv_handler))
                .route("/get.php", web::get().to(get_playlist_handler))
                .route("/panel_api.php", web::get().to(panel_api_handler))
                // Live streams
                .route(
                    "/live/{username}/{password}/{stream_id}.{ext}",
                    web::get().to(live_stream_handler),
                )
                .route(
                    "/live/{username}/{password}/{stream_id}.{ext}",
                    web::head().to(live_stream_handler),
                )
                .route(
                    "/live/{username}/{password}/{stream_id}",
                    web::get().to(live_stream_handler),
                )
                .route(
                    "/live/{username}/{password}/{stream_id}",
                    web::head().to(live_stream_handler),
                )
                // Movies
                .route(
                    "/movie/{username}/{password}/{stream_id}.{ext}",
                    web::get().to(movie_stream_handler),
                )
                .route(
                    "/movie/{username}/{password}/{stream_id}.{ext}",
                    web::head().to(movie_stream_handler),
                )
                .route(
                    "/movie/{username}/{password}/{stream_id}",
                    web::get().to(movie_stream_handler),
                )
                .route(
                    "/movie/{username}/{password}/{stream_id}",
                    web::head().to(movie_stream_handler),
                )
                // Series
                .route(
                    "/series/{username}/{password}/{stream_id}/{season}/{episode}.{ext}",
                    web::get().to(series_stream_handler),
                )
                .route(
                    "/series/{username}/{password}/{stream_id}/{season}/{episode}.{ext}",
                    web::head().to(series_stream_handler),
                )
                .route(
                    "/series/{username}/{password}/{stream_id}",
                    web::get().to(series_stream_handler),
                )
                .route(
                    "/series/{username}/{password}/{stream_id}",
                    web::head().to(series_stream_handler),
                )
                // Timeshift
                .route(
                    "/timeshift/{username}/{password}/{duration}/{start}/{stream_id}.ts",
                    web::get().to(timeshift_handler),
                )
                .route(
                    "/timeshift/{username}/{password}/{duration}/{start}/{stream_id}.ts",
                    web::head().to(timeshift_handler),
                )
                .route(
                    "/timeshift/{username}/{password}/{duration}/{start}/{stream_id}.{ext}",
                    web::get().to(timeshift_handler),
                )
                .route(
                    "/timeshift/{username}/{password}/{duration}/{start}/{stream_id}.{ext}",
                    web::head().to(timeshift_handler),
                )
                // Alternative timeshift path (Stalker portal style)
                .route("/streaming/timeshift.php", web::get().to(timeshift_handler))
                .route(
                    "/streaming/timeshift.php",
                    web::head().to(timeshift_handler),
                )
                // HLS token-based streams
                .route(
                    "/hls/{token}/{stream_id}.m3u8",
                    web::get().to(live_stream_handler),
                )
                .route(
                    "/hls/{token}/{stream_id}.m3u8",
                    web::head().to(live_stream_handler),
                )
                .route(
                    "/hlsr/{token}/{username}/{password}/{channel_id}/{start}/{end}/index.m3u8",
                    web::get().to(live_stream_handler),
                )
                .route(
                    "/hlsr/{token}/{username}/{password}/{channel_id}/{start}/{end}/index.m3u8",
                    web::head().to(live_stream_handler),
                )
                // Short streams (with and without extension) — must be last (catch-all)
                .route(
                    "/{username}/{password}/{stream_id}.{ext}",
                    web::get().to(short_stream_handler),
                )
                .route(
                    "/{username}/{password}/{stream_id}.{ext}",
                    web::head().to(short_stream_handler),
                )
                .route(
                    "/{username}/{password}/{stream_id}",
                    web::get().to(short_stream_handler),
                )
                .route(
                    "/{username}/{password}/{stream_id}",
                    web::head().to(short_stream_handler),
                );
        }

        // Playlist builder (Phase 6)
        {
            use playlist_builder::handler::playlist_builder_handler;
            app = app.service(
                web::scope("/playlist").route("/builder", web::get().to(playlist_builder_handler)),
            );
        }

        // Speedtest (Phase 6)
        {
            use speedtest::handler::{speedtest_config_handler, speedtest_redirect_handler};
            app = app.service(
                web::scope("/speedtest")
                    .route("", web::get().to(speedtest_redirect_handler))
                    .route("/config", web::post().to(speedtest_config_handler)),
            );
        }

        // Web UI (Phase 6)
        // `GET /` serves index.html; all other static files (html, js, png, …) are served
        // via the default_service fallback so that links like `/url_generator.html` work.
        #[cfg(feature = "web-ui")]
        {
            use web_ui::handler::index_handler;
            app = app.route("/", web::get().to(index_handler));
        }

        // Generic /proxy scope MUST come LAST — it prefix-matches all /proxy/*
        // paths, so it must only run after all specific /proxy/hls, /proxy/mpd,
        // /proxy/telegram etc. scopes have already been given a chance to match.
        {
            use epg::handler::epg_proxy_handler;
            app = app.service(
                web::scope("/proxy")
                    .route("/stream", web::get().to(handler::proxy_stream_get))
                    .route("/stream", web::head().to(handler::proxy_stream_head))
                    .route(
                        "/stream/{filename:.*}",
                        web::get().to(handler::proxy_stream_get),
                    )
                    .route(
                        "/stream/{filename:.*}",
                        web::head().to(handler::proxy_stream_head),
                    )
                    .route("/generate_url", web::post().to(handler::generate_url))
                    .route("/ip", web::get().to(handler::get_public_ip))
                    // EPG proxy — XMLTV pass-through with caching (Channels DVR & all providers)
                    .route("/epg", web::get().to(epg_proxy_handler))
                    .route("/epg", web::head().to(epg_proxy_handler)),
            );
        }

        app
            // Default: try to serve a static asset; fall back to 404 JSON
            .default_service(web::route().to(|req: actix_web::HttpRequest| async move {
                let path = req.path().trim_start_matches('/');

                #[cfg(feature = "web-ui")]
                if let Some(content) = web_ui::handler::StaticAssets::get(path) {
                    let mime = mime_guess::from_path(path).first_or_octet_stream();
                    return actix_web::HttpResponse::Ok()
                        .content_type(mime.as_ref())
                        .body(content.data.into_owned());
                }

                actix_web::HttpResponse::NotFound().json(serde_json::json!({
                    "error": "Not Found"
                }))
            }))
    })
    .workers(config.server.workers)
    .bind((config.server.host.as_str(), config.server.port))?
    .run()
    .await
}
