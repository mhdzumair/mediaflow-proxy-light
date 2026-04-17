pub mod auth;
pub mod config;
pub mod error;
pub mod metrics;
pub mod models;
pub mod proxy;

// Phase 1: HLS processing
#[cfg(feature = "hls")]
pub mod hls;

// Phase 2: DASH/MPD processing
#[cfg(feature = "mpd")]
pub mod mpd;

// Phase 3: DRM decryption (ClearKey / CENC)
#[cfg(feature = "drm")]
pub mod drm;

// Shared cache (local + Redis)
pub mod cache;

// Shared utilities
pub mod utils;

// Phase 4: Xtream Codes API
#[cfg(feature = "xtream")]
pub mod xtream;

// Phase 5: Content extractors
#[cfg(feature = "extractors")]
pub mod extractor;

// Phase 6: Playlist builder, speedtest, web UI
pub mod playlist_builder;
pub mod speedtest;
#[cfg(feature = "web-ui")]
pub mod web_ui;

// Phase 7: Acestream proxy
#[cfg(feature = "acestream")]
pub mod acestream;

// Phase 8: On-the-fly transcoding
#[cfg(feature = "transcode")]
pub mod transcode;

// Phase 9: Telegram MTProto streaming
#[cfg(feature = "telegram")]
pub mod telegram;

// Phase 10: iOS C FFI bridge
#[cfg(feature = "ffi")]
pub mod ffi;
