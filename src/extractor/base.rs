//! Base extractor trait and shared types.

use dashmap::DashMap;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rquest_util::Emulation;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::OnceLock;
use tokio::time::Duration;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Result returned by every extractor.
#[derive(Debug, Clone)]
pub struct ExtractorResult {
    /// The final (direct) media URL.
    pub destination_url: String,
    /// HTTP headers needed when fetching `destination_url`.
    pub request_headers: HashMap<String, String>,
    /// Which MediaFlow endpoint should proxy this URL.
    /// One of `"proxy_stream_endpoint"` or `"hls_manifest_proxy"`.
    pub mediaflow_endpoint: &'static str,
}

/// Extra parameters that can be passed from the HTTP query string.
#[derive(Debug, Default, Clone)]
pub struct ExtraParams {
    pub raw: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Extractor trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
pub trait Extractor: Send + Sync {
    /// The canonical host name (e.g. `"Streamtape"`).
    fn host_name(&self) -> &'static str;

    /// Extract the final URL from `url`.
    async fn extract(
        &self,
        url: &str,
        extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError>;
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ExtractorError {
    #[error("Extractor error: {0}")]
    Extract(String),

    #[error("HTTP error {status}: {message}")]
    Http { status: u16, message: String },

    #[error("Network error: {0}")]
    Network(String),
}

impl ExtractorError {
    pub fn extract(msg: impl Into<String>) -> Self {
        ExtractorError::Extract(msg.into())
    }
}

// ---------------------------------------------------------------------------
// Global shared HTTP client pool for extractors
// ---------------------------------------------------------------------------
//
// reqwest::Client wraps an Arc internally and is cheap to clone.  Creating a
// new client per extraction request throws away the warm TCP connection pool
// and pays TLS + TCP handshake overhead on every request.  Instead we keep:
//
//  - ONE default client (no proxy)
//  - ONE client per unique proxy URL (rare — typically < 5 in any deployment)
//
// Both caches are populated on first use and never evicted (client configs
// never change at runtime).

fn get_shared_extractor_client(proxy_url: Option<&str>) -> reqwest::Client {
    static DEFAULT: OnceLock<reqwest::Client> = OnceLock::new();
    static PROXIED: OnceLock<DashMap<String, reqwest::Client>> = OnceLock::new();

    match proxy_url {
        None => DEFAULT.get_or_init(|| build_reqwest_client(None)).clone(),
        Some(url) => {
            let map = PROXIED.get_or_init(DashMap::new);
            if let Some(c) = map.get(url) {
                return c.clone();
            }
            let client = build_reqwest_client(Some(url));
            map.insert(url.to_string(), client.clone());
            client
        }
    }
}

fn build_reqwest_client(proxy_url: Option<&str>) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .pool_max_idle_per_host(20)
        .pool_idle_timeout(Duration::from_secs(90))
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        );

    if let Some(url) = proxy_url {
        match reqwest::Proxy::all(url) {
            Ok(p) => {
                builder = builder.proxy(p);
            }
            Err(e) => {
                tracing::warn!("build_reqwest_client: invalid proxy URL '{}': {}", url, e);
            }
        }
    }

    builder.build().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Global shared Chrome-impersonating rquest client pool
// ---------------------------------------------------------------------------

fn get_shared_chrome_client(proxy_url: Option<&str>) -> rquest::Client {
    static DEFAULT: OnceLock<rquest::Client> = OnceLock::new();
    static PROXIED: OnceLock<DashMap<String, rquest::Client>> = OnceLock::new();

    match proxy_url {
        None => DEFAULT
            .get_or_init(|| build_chrome_client_inner(None))
            .clone(),
        Some(url) => {
            let map = PROXIED.get_or_init(DashMap::new);
            if let Some(c) = map.get(url) {
                return c.clone();
            }
            let client = build_chrome_client_inner(Some(url));
            map.insert(url.to_string(), client.clone());
            client
        }
    }
}

fn build_chrome_client_inner(proxy_url: Option<&str>) -> rquest::Client {
    let mut builder = rquest::Client::builder()
        .emulation(Emulation::Chrome133)
        .timeout(Duration::from_secs(30))
        .redirect(rquest::redirect::Policy::limited(5));

    if let Some(url) = proxy_url {
        match rquest::Proxy::all(url) {
            Ok(proxy) => {
                builder = builder.proxy(proxy);
            }
            Err(e) => {
                tracing::warn!("build_chrome_client: invalid proxy URL '{}': {}", url, e);
            }
        }
    }

    builder.build().unwrap_or_default()
}

/// Build an `rquest::Client` that impersonates Chrome 133 (TLS + HTTP/2 fingerprint).
///
/// Returns a globally-cached client so the warm connection pool is reused
/// across all extraction requests with the same proxy configuration.
pub fn build_chrome_client(proxy_url: Option<&str>) -> rquest::Client {
    get_shared_chrome_client(proxy_url)
}

// ---------------------------------------------------------------------------
// BaseExtractor helper struct (shared HTTP client logic)
// ---------------------------------------------------------------------------

pub struct BaseExtractor {
    pub client: reqwest::Client,
    pub base_headers: HashMap<String, String>,
    pub mediaflow_endpoint: &'static str,
}

impl BaseExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        let mut base_headers = HashMap::new();
        base_headers.insert(
            "user-agent".to_string(),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
                .to_string(),
        );
        base_headers.extend(request_headers);

        // Reuse a globally-cached client instead of allocating a new one per request.
        let client = get_shared_extractor_client(proxy_url.as_deref());

        Self {
            client,
            base_headers,
            mediaflow_endpoint: "proxy_stream_endpoint",
        }
    }

    /// Make an HTTP GET request and return the response body as a string.
    pub async fn get_text(
        &self,
        url: &str,
        extra_headers: Option<HashMap<String, String>>,
    ) -> Result<(String, String), ExtractorError> {
        let mut hm = HeaderMap::new();
        let mut merged = self.base_headers.clone();
        if let Some(extra) = extra_headers {
            merged.extend(extra);
        }
        for (k, v) in &merged {
            if let (Ok(n), Ok(val)) = (HeaderName::from_str(k), HeaderValue::from_str(v)) {
                hm.insert(n, val);
            }
        }

        let resp = self
            .client
            .get(url)
            .headers(hm)
            .send()
            .await
            .map_err(|e| ExtractorError::Network(e.to_string()))?;

        let status = resp.status().as_u16();
        if status >= 400 {
            return Err(ExtractorError::Http {
                status,
                message: format!("HTTP {status} from {url}"),
            });
        }

        let final_url = resp.url().to_string();
        let text = resp
            .text()
            .await
            .map_err(|e| ExtractorError::Network(e.to_string()))?;
        Ok((text, final_url))
    }
}
