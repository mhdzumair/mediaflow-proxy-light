use config::{Map, Value};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::warn;
use url::Url;

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub workers: usize,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct ProxyRouteConfig {
    #[serde(default)]
    pub proxy: bool,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default = "default_verify_ssl")]
    pub verify_ssl: bool,
}

fn default_verify_ssl() -> bool {
    true
}

// ── ProxyConfig defaults ────────────────────────────────────────────────────
//
// These apply to omitted TOML keys so older config files keep working.
fn default_request_timeout_factor() -> u64 {
    8
}
fn default_max_concurrent_per_host() -> usize {
    10
}
fn default_pool_idle_timeout() -> u64 {
    90
}
fn default_pool_max_idle_per_host() -> usize {
    100
}
fn default_body_read_timeout() -> u64 {
    60
}

#[derive(Debug, Deserialize, Clone)]
pub struct ProxyConfig {
    /// TCP handshake timeout (seconds) for a new upstream connection.
    pub connect_timeout: u64,

    /// Chunk buffer size used by the streaming pipeline.
    pub buffer_size: usize,

    /// Whether reqwest should follow 3xx redirects.
    pub follow_redirects: bool,

    /// Optional default upstream HTTP/HTTPS/SOCKS proxy URL.
    #[serde(default)]
    pub proxy_url: Option<String>,

    /// If true, route all upstream traffic through `proxy_url`.
    #[serde(default)]
    pub all_proxy: bool,

    /// Per-pattern transport overrides (proxy, SSL verification).
    #[serde(default)]
    pub transport_routes: HashMap<String, ProxyRouteConfig>,

    // ── Tunables (all have sensible defaults) ─────────────────────────────
    /// Multiplier applied to `connect_timeout` to derive the full request
    /// timeout (covers pool-wait + connect + TLS + response headers).
    ///
    /// Default: `8` → request timeout = connect_timeout × 8.
    #[serde(default = "default_request_timeout_factor")]
    pub request_timeout_factor: u64,

    /// Maximum concurrent in-flight upstream requests per origin host.
    /// Matches aiohttp's `limit_per_host=10` default; set to 0 to disable.
    /// Capping here forces HTTP/1.1 keep-alive reuse for bursty traffic
    /// and dramatically reduces per-request latency.
    ///
    /// Default: `10`.
    #[serde(default = "default_max_concurrent_per_host")]
    pub max_concurrent_per_host: usize,

    /// Seconds an idle upstream connection is kept in the pool before eviction.
    ///
    /// Default: `90`.
    #[serde(default = "default_pool_idle_timeout")]
    pub pool_idle_timeout: u64,

    /// Maximum idle upstream connections retained per host per worker.
    ///
    /// Default: `100`.
    #[serde(default = "default_pool_max_idle_per_host")]
    pub pool_max_idle_per_host: usize,

    /// Timeout (seconds) applied to fully reading a small in-memory body
    /// via `fetch_bytes()` (used for manifests, playlists, EPG data).
    ///
    /// Default: `60`.
    #[serde(default = "default_body_read_timeout")]
    pub body_read_timeout: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AuthConfig {
    pub api_password: String,
}

// ---------------------------------------------------------------------------
// HLS config
// ---------------------------------------------------------------------------

fn default_hls_prebuffer_segments() -> usize {
    5
}
fn default_hls_prebuffer_cache_size() -> usize {
    50
}
fn default_hls_segment_cache_ttl() -> u64 {
    300
}
fn default_hls_inactivity_timeout() -> u64 {
    60
}

#[derive(Debug, Deserialize, Clone)]
pub struct HlsConfig {
    /// Number of segments to pre-fetch ahead of playback position.
    #[serde(default = "default_hls_prebuffer_segments")]
    pub prebuffer_segments: usize,
    /// Maximum number of playlist prefetcher instances held in memory.
    #[serde(default = "default_hls_prebuffer_cache_size")]
    pub prebuffer_cache_size: usize,
    /// TTL in seconds for cached HLS segments.
    #[serde(default = "default_hls_segment_cache_ttl")]
    pub segment_cache_ttl: u64,
    /// Seconds of inactivity before a playlist prefetcher is evicted.
    #[serde(default = "default_hls_inactivity_timeout")]
    pub inactivity_timeout: u64,
}

impl Default for HlsConfig {
    fn default() -> Self {
        Self {
            prebuffer_segments: default_hls_prebuffer_segments(),
            prebuffer_cache_size: default_hls_prebuffer_cache_size(),
            segment_cache_ttl: default_hls_segment_cache_ttl(),
            inactivity_timeout: default_hls_inactivity_timeout(),
        }
    }
}

// ---------------------------------------------------------------------------
// MPD / DASH config
// ---------------------------------------------------------------------------

fn default_mpd_live_playlist_depth() -> usize {
    8
}
fn default_mpd_live_init_cache_ttl() -> u64 {
    60
}
fn default_mpd_remux_to_ts() -> bool {
    false
}

#[derive(Debug, Deserialize, Clone)]
pub struct MpdConfig {
    /// Number of segments to include in a live DASH → HLS media playlist.
    #[serde(default = "default_mpd_live_playlist_depth")]
    pub live_playlist_depth: usize,
    /// TTL in seconds for cached MPD init segments.
    #[serde(default = "default_mpd_live_init_cache_ttl")]
    pub live_init_cache_ttl: u64,
    /// When true, remux DASH segments to MPEG-TS instead of fMP4.
    #[serde(default = "default_mpd_remux_to_ts")]
    pub remux_to_ts: bool,
}

impl Default for MpdConfig {
    fn default() -> Self {
        Self {
            live_playlist_depth: default_mpd_live_playlist_depth(),
            live_init_cache_ttl: default_mpd_live_init_cache_ttl(),
            remux_to_ts: default_mpd_remux_to_ts(),
        }
    }
}

// ---------------------------------------------------------------------------
// DRM config
// ---------------------------------------------------------------------------

fn default_drm_key_cache_ttl() -> u64 {
    3600
}

#[derive(Debug, Deserialize, Clone)]
pub struct DrmConfig {
    /// TTL in seconds for cached ClearKey keys.
    #[serde(default = "default_drm_key_cache_ttl")]
    pub key_cache_ttl: u64,
}

impl Default for DrmConfig {
    fn default() -> Self {
        Self {
            key_cache_ttl: default_drm_key_cache_ttl(),
        }
    }
}

// ---------------------------------------------------------------------------
// Redis config
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone, Default)]
pub struct RedisConfig {
    /// Redis connection URL. Empty string disables Redis (falls back to local cache).
    #[serde(default)]
    pub url: String,
    /// Namespace prefix for all cache keys.
    #[serde(default)]
    pub cache_namespace: String,
}

impl RedisConfig {
    pub fn is_enabled(&self) -> bool {
        !self.url.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Telegram config
// ---------------------------------------------------------------------------

fn default_telegram_max_connections() -> usize {
    8
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelegramConfig {
    /// Telegram API app ID.
    #[serde(default)]
    pub api_id: i32,
    /// Telegram API app hash.
    #[serde(default)]
    pub api_hash: String,
    /// Serialized session string (from grammers-client).
    #[serde(default)]
    pub session_string: String,
    /// Maximum parallel DC connections for chunk downloads.
    #[serde(default = "default_telegram_max_connections")]
    pub max_connections: usize,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            api_id: 0,
            api_hash: String::new(),
            session_string: String::new(),
            max_connections: default_telegram_max_connections(),
        }
    }
}

// ---------------------------------------------------------------------------
// Acestream config
// ---------------------------------------------------------------------------

fn default_acestream_host() -> String {
    "localhost".to_string()
}
fn default_acestream_port() -> u16 {
    6878
}
fn default_acestream_buffer_size() -> usize {
    4 * 1024 * 1024 // 4 MB
}

#[derive(Debug, Deserialize, Clone)]
pub struct AcestreamConfig {
    /// Hostname of the local Acestream engine.
    #[serde(default = "default_acestream_host")]
    pub host: String,
    /// Port of the local Acestream engine.
    #[serde(default = "default_acestream_port")]
    pub port: u16,
    /// Internal buffer size in bytes for MPEG-TS fan-out.
    #[serde(default = "default_acestream_buffer_size")]
    pub buffer_size: usize,
    /// Static access token for the Acestream engine HTTP API (engine API key).
    /// Required on some Android builds that lock the HTTP API behind a token.
    #[serde(default)]
    pub access_token: Option<String>,
}

impl Default for AcestreamConfig {
    fn default() -> Self {
        Self {
            host: default_acestream_host(),
            port: default_acestream_port(),
            buffer_size: default_acestream_buffer_size(),
            access_token: None,
        }
    }
}

// ---------------------------------------------------------------------------
// EPG config
// ---------------------------------------------------------------------------

fn default_epg_cache_ttl() -> u64 {
    3600
}

#[derive(Debug, Deserialize, Clone)]
pub struct EpgConfig {
    /// TTL in seconds for cached EPG/XMLTV data. Default: 3600 (1 hour).
    /// Set to 0 to disable caching entirely.
    #[serde(default = "default_epg_cache_ttl")]
    pub cache_ttl: u64,
}

impl Default for EpgConfig {
    fn default() -> Self {
        Self {
            cache_ttl: default_epg_cache_ttl(),
        }
    }
}

// ---------------------------------------------------------------------------
// Transcode config
// ---------------------------------------------------------------------------

fn default_transcode_enabled() -> bool {
    true
}
fn default_transcode_prefer_gpu() -> bool {
    true
}
fn default_transcode_video_bitrate() -> String {
    "4M".to_string()
}
fn default_transcode_audio_bitrate() -> u32 {
    192_000
}

#[derive(Debug, Deserialize, Clone)]
pub struct TranscodeConfig {
    /// Enable on-the-fly transcoding endpoints.
    #[serde(default = "default_transcode_enabled")]
    pub enabled: bool,
    /// Prefer hardware-accelerated encoders when available.
    #[serde(default = "default_transcode_prefer_gpu")]
    pub prefer_gpu: bool,
    /// Target video bitrate string passed to ffmpeg (e.g. "4M").
    #[serde(default = "default_transcode_video_bitrate")]
    pub video_bitrate: String,
    /// Target audio bitrate in bits per second.
    #[serde(default = "default_transcode_audio_bitrate")]
    pub audio_bitrate: u32,
}

impl Default for TranscodeConfig {
    fn default() -> Self {
        Self {
            enabled: default_transcode_enabled(),
            prefer_gpu: default_transcode_prefer_gpu(),
            video_bitrate: default_transcode_video_bitrate(),
            audio_bitrate: default_transcode_audio_bitrate(),
        }
    }
}

// ---------------------------------------------------------------------------
// Root config
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub proxy: ProxyConfig,
    pub auth: AuthConfig,
    #[serde(default)]
    pub hls: HlsConfig,
    #[serde(default)]
    pub mpd: MpdConfig,
    #[serde(default)]
    pub drm: DrmConfig,
    #[serde(default)]
    pub redis: RedisConfig,
    #[serde(default)]
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub acestream: AcestreamConfig,
    #[serde(default)]
    pub transcode: TranscodeConfig,
    #[serde(default)]
    pub epg: EpgConfig,
    /// Log filter directive (e.g. "debug", "info", "mediaflow_proxy_light=debug,info").
    /// Overrides the RUST_LOG env var when set.
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

fn default_log_level() -> String {
    "info,actix_http::h1=off".to_string()
}

// ---------------------------------------------------------------------------
// Proxy routing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ProxyRoute {
    pub pattern: Regex,
    pub config: ProxyRouteConfig,
}

#[derive(Debug, Clone)]
pub struct ProxyRouter {
    default_proxy: Option<String>,
    all_proxy: bool,
    routes: Vec<ProxyRoute>,
}

impl ProxyRouter {
    pub fn new(
        default_proxy: Option<String>,
        all_proxy: bool,
        routes_config: HashMap<String, ProxyRouteConfig>,
    ) -> Self {
        let mut routes = Vec::new();

        for (pattern, config) in routes_config {
            let pattern = pattern
                .replace(".", "\\.")
                .replace("*", "[^/]*")
                .replace("all://", "(http|https)://");

            match Regex::new(&format!("^{}", pattern)) {
                Ok(regex) => {
                    routes.push(ProxyRoute {
                        pattern: regex,
                        config,
                    });
                }
                Err(e) => {
                    tracing::error!("Invalid route pattern '{}': {}", pattern, e);
                }
            }
        }

        // Sort routes by specificity (fewer wildcards = more specific = higher priority)
        routes.sort_by(|a, b| {
            let a_wildcards = a.pattern.as_str().matches("[^/]*").count();
            let b_wildcards = b.pattern.as_str().matches("[^/]*").count();
            b_wildcards.cmp(&a_wildcards)
        });

        Self {
            default_proxy,
            all_proxy,
            routes,
        }
    }

    pub fn from_config(config: &ProxyConfig) -> Self {
        Self::new(
            config.proxy_url.clone(),
            config.all_proxy,
            config.transport_routes.clone(),
        )
    }

    pub fn get_proxy_config(&self, url: &str) -> Option<ProxyRouteConfig> {
        match Url::parse(url) {
            Ok(parsed_url) => {
                let url_str = parsed_url.as_str();

                for route in &self.routes {
                    if route.pattern.is_match(url_str) {
                        tracing::debug!("Matched route pattern: {}", route.pattern.as_str());
                        return Some(route.config.clone());
                    }
                }

                if self.all_proxy {
                    return Some(ProxyRouteConfig {
                        proxy: true,
                        proxy_url: self.default_proxy.clone(),
                        verify_ssl: true,
                    });
                }
            }
            Err(e) => {
                tracing::error!("Failed to parse URL '{}': {}", url, e);
            }
        }

        None
    }

    pub fn default_proxy(&self) -> &Option<String> {
        &self.default_proxy
    }
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

impl Config {
    pub fn from_env() -> Result<Self, config::ConfigError> {
        let mut builder = config::Config::builder()
            // Server
            .set_default("server.host", "127.0.0.1")?
            .set_default("server.port", 8888)?
            .set_default("server.workers", 4)?
            // Proxy
            .set_default("proxy.connect_timeout", 30)?
            .set_default("proxy.buffer_size", 262144)?
            .set_default("proxy.follow_redirects", true)?
            .set_default("proxy.all_proxy", false)?
            .set_default("proxy.transport_routes", HashMap::<String, Value>::new())?
            // Auth
            .set_default("auth.api_password", "changeme")?
            // HLS
            .set_default("hls.prebuffer_segments", 5)?
            .set_default("hls.prebuffer_cache_size", 50)?
            .set_default("hls.segment_cache_ttl", 300)?
            .set_default("hls.inactivity_timeout", 60)?
            // MPD
            .set_default("mpd.live_playlist_depth", 8)?
            .set_default("mpd.live_init_cache_ttl", 60)?
            .set_default("mpd.remux_to_ts", false)?
            // DRM
            .set_default("drm.key_cache_ttl", 3600)?
            // Redis
            .set_default("redis.url", "")?
            .set_default("redis.cache_namespace", "")?
            // Telegram
            .set_default("telegram.api_id", 0)?
            .set_default("telegram.api_hash", "")?
            .set_default("telegram.session_string", "")?
            .set_default("telegram.max_connections", 8)?
            // Acestream
            .set_default("acestream.host", "localhost")?
            .set_default("acestream.port", 6878)?
            .set_default("acestream.buffer_size", 4194304)?
            .set_default("acestream.access_token", Option::<String>::None)?
            // Transcode
            .set_default("transcode.enabled", true)?
            .set_default("transcode.prefer_gpu", true)?
            .set_default("transcode.video_bitrate", "4M")?
            .set_default("transcode.audio_bitrate", 192000)?
            // EPG
            .set_default("epg.cache_ttl", 3600)?;

        if let Ok(config_path) = std::env::var("CONFIG_PATH") {
            let path = Path::new(&config_path);
            if path.exists() {
                builder = builder.add_source(config::File::with_name(&config_path));
            } else {
                warn!("Config file not found at {}", config_path);
            }
        }

        builder = builder.add_source(
            config::Environment::with_prefix("APP")
                .separator("__")
                .try_parsing(true),
        );

        if let Ok(routes_json) = std::env::var("APP__PROXY__TRANSPORT_ROUTES") {
            match serde_json::from_str::<HashMap<String, ProxyRouteConfig>>(&routes_json) {
                Ok(routes) => {
                    let routes_map = routes
                        .into_iter()
                        .map(|(k, v)| {
                            let mut inner_map = Map::new();
                            inner_map.insert("proxy".into(), Value::from(v.proxy));
                            if let Some(url) = v.proxy_url {
                                inner_map.insert("proxy_url".into(), Value::from(url));
                            }
                            inner_map.insert("verify_ssl".into(), Value::from(v.verify_ssl));
                            (k, Value::from(inner_map))
                        })
                        .collect::<Map<String, Value>>();

                    builder =
                        builder.set_override("proxy.transport_routes", Value::from(routes_map))?;
                }
                Err(e) => {
                    return Err(config::ConfigError::Message(format!(
                        "Failed to parse TRANSPORT_ROUTES: {}",
                        e
                    )));
                }
            }
        }

        let config = builder.build()?;
        config.try_deserialize()
    }
}
