use actix_web::web::Bytes;
use dashmap::DashMap;
use futures::{Stream, StreamExt};
use reqwest::{Client, Proxy, Response};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::{timeout, Duration};
use tracing::error;

use crate::{
    config::{ProxyConfig, ProxyRouteConfig, ProxyRouter},
    error::{AppError, AppResult},
};

#[derive(Clone)]
pub struct StreamManager {
    /// Fallback shared client (used if per-thread lookup fails).
    shared_client: Client,
    /// Per-thread clients — each actix-web worker thread gets its own reqwest
    /// Client and connection pool.  This eliminates pool contention at high
    /// concurrency: c=50 across 8 workers becomes c≈6 per pool instead of
    /// one shared pool serialising 50 simultaneous checkout/connect operations.
    per_thread_clients: Arc<DashMap<std::thread::ThreadId, Client>>,
    /// Per-route client cache — keyed by a hash of the route config so we
    /// never create more than one client per unique (proxy_url, verify_ssl)
    /// combination, even under high concurrency.
    route_clients: Arc<DashMap<String, Client>>,
    /// Per-host concurrency limiter — caps simultaneous in-flight upstream
    /// requests to the same origin.  Forces HTTP/1.1 keep-alive reuse when
    /// many requests target the same host, which dramatically reduces
    /// per-request latency for bursts to a single upstream.
    per_host_semaphores: Arc<DashMap<String, Arc<Semaphore>>>,
    config: ProxyConfig,
    proxy_router: ProxyRouter,
}

impl StreamManager {
    pub fn new(config: ProxyConfig) -> Self {
        let proxy_router = ProxyRouter::from_config(&config);
        let shared_client = Self::create_default_client(&config, &proxy_router);

        Self {
            shared_client,
            per_thread_clients: Arc::new(DashMap::new()),
            route_clients: Arc::new(DashMap::new()),
            per_host_semaphores: Arc::new(DashMap::new()),
            config,
            proxy_router,
        }
    }

    /// Get or create the semaphore limiting concurrent in-flight upstream
    /// requests to a given host.  Returns `None` when the limiter is disabled
    /// (config value 0).
    fn host_semaphore(&self, host: &str) -> Option<Arc<Semaphore>> {
        if self.config.max_concurrent_per_host == 0 {
            return None;
        }
        if let Some(s) = self.per_host_semaphores.get(host) {
            return Some(s.clone());
        }
        let sem = Arc::new(Semaphore::new(self.config.max_concurrent_per_host));
        self.per_host_semaphores
            .insert(host.to_string(), sem.clone());
        Some(sem)
    }

    /// Get or create the reqwest Client for the current worker thread.
    fn thread_client(&self) -> Client {
        let tid = std::thread::current().id();
        if let Some(c) = self.per_thread_clients.get(&tid) {
            return c.clone();
        }
        let c = Self::create_default_client(&self.config, &self.proxy_router);
        self.per_thread_clients.insert(tid, c.clone());
        c
    }

    /// Build the default shared client used when no transport route matches.
    ///
    /// Connection pooling is ENABLED (up to 100 idle connections per host).
    /// This is the most important performance lever: re-using TCP connections
    /// eliminates per-request handshake latency and dramatically reduces
    /// overhead at high concurrency.
    fn create_default_client(config: &ProxyConfig, proxy_router: &ProxyRouter) -> Client {
        let follow_redirects = config.follow_redirects;
        let request_timeout =
            Duration::from_secs(config.connect_timeout * config.request_timeout_factor);
        let mut builder = Client::builder()
            // TCP connect timeout (handshake only — does NOT include pool-acquisition wait).
            .connect_timeout(Duration::from_secs(config.connect_timeout))
            // Overall request timeout: covers pool-wait + connect + TLS + response headers.
            // Body streaming is NOT limited by this — only the time until headers arrive.
            .timeout(request_timeout)
            // Keep idle connections alive for reuse.
            .pool_idle_timeout(Duration::from_secs(config.pool_idle_timeout))
            // Cap idle connections retained per upstream host.
            .pool_max_idle_per_host(config.pool_max_idle_per_host)
            // Use a browser-like User-Agent so CDNs (e.g. Akamai) don't block the request.
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
            // Disable auto-decompression so we stream the raw bytes unchanged.
            .no_brotli()
            .no_deflate()
            .no_gzip()
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if follow_redirects {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }));

        if let Some(default_proxy) = proxy_router.default_proxy() {
            if let Ok(proxy) = Proxy::all(default_proxy) {
                builder = builder.proxy(proxy);
            }
        }

        builder.build().expect("Failed to create HTTP client")
    }

    /// Build a client for a specific transport route config.
    ///
    /// Called at most once per unique route config (results are cached in
    /// `self.route_clients`).  Never called in the common case where
    /// `all_proxy = false` and no transport route patterns match.
    fn build_route_client(
        config: &ProxyConfig,
        route_config: &ProxyRouteConfig,
    ) -> AppResult<Client> {
        let request_timeout =
            Duration::from_secs(config.connect_timeout * config.request_timeout_factor);
        let mut builder = Client::builder()
            .connect_timeout(Duration::from_secs(config.connect_timeout))
            .timeout(request_timeout)
            .pool_idle_timeout(Duration::from_secs(config.pool_idle_timeout))
            .pool_max_idle_per_host(config.pool_max_idle_per_host)
            .no_brotli()
            .no_deflate()
            .no_gzip()
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36");

        if route_config.proxy {
            if let Some(proxy_url) = route_config.proxy_url.as_ref() {
                match Proxy::all(proxy_url) {
                    Ok(proxy) => {
                        tracing::info!("Building route client with proxy {}", proxy_url);
                        builder = builder.proxy(proxy);
                    }
                    Err(e) => {
                        error!("Failed to create proxy for route: {}", e);
                        return Err(AppError::Internal(format!(
                            "Failed to create route proxy client: {}",
                            e
                        )));
                    }
                }
            }
        }

        if !route_config.verify_ssl {
            tracing::warn!("SSL verification disabled for route");
            builder = builder.danger_accept_invalid_certs(true);
        }

        builder
            .build()
            .map_err(|e| AppError::Internal(format!("Failed to build route client: {}", e)))
    }

    /// Extract the host (and port) from a URL for per-host limiting.
    fn url_host_key(url: &str) -> String {
        url::Url::parse(url)
            .ok()
            .and_then(|u| {
                u.host_str().map(|h| match u.port() {
                    Some(p) => format!("{}:{}", h, p),
                    None => h.to_string(),
                })
            })
            .unwrap_or_else(|| "_unknown_".to_string())
    }

    pub async fn make_request(
        &self,
        url: String,
        headers: reqwest::header::HeaderMap,
    ) -> AppResult<Response> {
        // Hold the permit for the full call — when the returned Response is
        // consumed by the caller (e.g. `.bytes()` in fetch_bytes), the permit
        // still applies since we already acquired it here and it drops when
        // this function returns.  For streaming callers, prefer
        // `create_stream` which carries the permit with the body.
        let _permit = self.acquire_host_permit(&url).await?;
        self.make_request_raw(url, headers).await
    }

    /// Acquire a permit from the per-host semaphore.  Returns an owned permit
    /// that can be carried by a streaming body so the slot stays reserved
    /// until the body is fully consumed (or the client disconnects).
    ///
    /// Returns `Ok(None)` when the limiter is disabled (`max_concurrent_per_host = 0`).
    pub async fn acquire_host_permit(
        &self,
        url: &str,
    ) -> AppResult<Option<tokio::sync::OwnedSemaphorePermit>> {
        let host = Self::url_host_key(url);
        let Some(sem) = self.host_semaphore(&host) else {
            return Ok(None);
        };
        sem.acquire_owned()
            .await
            .map(Some)
            .map_err(|_| AppError::Proxy("Upstream semaphore closed".to_string()))
    }

    /// Inner request without the per-host limiter — used when the caller
    /// has already acquired a permit and wants to carry it with the body.
    async fn make_request_raw(
        &self,
        url: String,
        headers: reqwest::header::HeaderMap,
    ) -> AppResult<Response> {
        let proxy_config = self.proxy_router.get_proxy_config(&url);

        let client = if let Some(route_config) = proxy_config {
            let cache_key = format!(
                "{}|{}|{}",
                route_config.proxy,
                route_config.proxy_url.as_deref().unwrap_or(""),
                route_config.verify_ssl,
            );

            if let Some(cached) = self.route_clients.get(&cache_key) {
                cached.clone()
            } else {
                let new_client = Self::build_route_client(&self.config, &route_config)?;
                self.route_clients.insert(cache_key, new_client.clone());
                new_client
            }
        } else {
            self.thread_client()
        };

        let response = client
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| AppError::Proxy(format!("Failed to connect to upstream: {}", e)))?;

        // Accept 2xx (including 206 Partial Content) and 3xx redirects that reqwest follows.
        // Reject only genuine error status codes (4xx, 5xx).
        if response.status().is_client_error() || response.status().is_server_error() {
            return Err(AppError::Upstream(format!(
                "Upstream returned error status: {}",
                response.status()
            )));
        }

        Ok(response)
    }

    /// Fetch all response bytes into memory (for small resources like M3U8 playlists / MPD manifests).
    ///
    /// A separate read timeout (`config.body_read_timeout`, default 60s) is
    /// applied to `.bytes()` so CDN throttling (e.g. Akamai drip-feeding a
    /// large MPD) doesn't hang the handler indefinitely.  The connection-
    /// level timeout in `make_request` only covers the TCP + TLS + response-
    /// headers phase; body reading has no built-in limit.
    pub async fn fetch_bytes(
        &self,
        url: String,
        headers: reqwest::header::HeaderMap,
    ) -> AppResult<bytes::Bytes> {
        let body_timeout = Duration::from_secs(self.config.body_read_timeout);
        let response = self.make_request(url, headers).await?;
        timeout(body_timeout, response.bytes())
            .await
            .map_err(|_| {
                AppError::Proxy(format!(
                    "Body read timeout after {} s",
                    self.config.body_read_timeout
                ))
            })?
            .map_err(|e| AppError::Proxy(format!("Failed to read response body: {}", e)))
    }

    pub async fn create_stream(
        &self,
        url: String,
        headers: reqwest::header::HeaderMap,
        is_head: bool,
    ) -> AppResult<(
        reqwest::StatusCode,
        reqwest::header::HeaderMap,
        Option<impl Stream<Item = Result<Bytes, AppError>>>,
    )> {
        // Acquire the per-host slot BEFORE dispatching.  For streaming, the
        // permit will be moved into the body stream closure so it lives until
        // the body is fully read (or the client disconnects and the stream
        // is dropped).  This caps parallel in-flight upstream connections per
        // host at `config.max_concurrent_per_host`, forcing HTTP/1.1 keep-alive
        // reuse when many requests target the same origin — eliminating
        // repeated TCP handshake + TLS cost for warm connections.
        //
        // Returns `None` when the limiter is disabled (max_concurrent_per_host = 0).
        let permit = self.acquire_host_permit(&url).await?;
        let response = self.make_request_raw(url, headers).await?;
        let status = response.status();
        let response_headers = response.headers().clone();

        if is_head {
            return Ok((status, response_headers, None));
        }

        // Zero-copy streaming: chunks from reqwest are forwarded directly to
        // actix-web without any intermediate buffering.  Memory usage stays
        // bounded regardless of response size or concurrency.  The permit
        // is held inside the map closure so it drops (releasing the slot)
        // only when the stream ends or is dropped.
        let stream = response.bytes_stream().map(move |result| {
            // Keep the permit alive for each chunk's poll — it drops
            // when the stream is exhausted or cancelled.
            let _keep = &permit;
            result.map_err(|e| AppError::Proxy(format!("Stream error: {}", e)))
        });
        Ok((status, response_headers, Some(stream)))
    }

    /// Resolve the proxy URL (if any) that should be used for the given destination URL.
    ///
    /// Uses the already-built `ProxyRouter` stored in `self` — zero regex compilation
    /// or allocation on the hot path.  Returns `None` when no proxy applies.
    pub fn get_proxy_url_for(&self, url: &str) -> Option<String> {
        self.proxy_router
            .get_proxy_config(url)
            .filter(|pc| pc.proxy)
            .and_then(|pc| {
                pc.proxy_url
                    .clone()
                    .or_else(|| self.config.proxy_url.clone())
            })
    }

    /// Wrap a stream so that errors are logged with the byte offset.
    ///
    /// Not async — avoids a state-machine allocation.
    /// Does NOT Box::pin — the caller's ResponseStream::new() handles that.
    pub fn stream_with_progress<S>(&self, stream: S) -> impl Stream<Item = Result<Bytes, AppError>>
    where
        S: Stream<Item = Result<Bytes, AppError>>,
    {
        let mut total_bytes = 0usize;

        stream.map(move |chunk| match chunk {
            Ok(bytes) => {
                total_bytes += bytes.len();
                Ok(bytes)
            }
            Err(e) => {
                error!("Streaming error after {} bytes: {}", total_bytes, e);
                Err(e)
            }
        })
    }
}

pub struct ResponseStream<S> {
    inner: Pin<Box<S>>,
}

impl<S> ResponseStream<S>
where
    S: Stream<Item = Result<Bytes, AppError>>,
{
    pub fn new(stream: S) -> Self {
        Self {
            inner: Box::pin(stream),
        }
    }
}

impl<S> Stream for ResponseStream<S>
where
    S: Stream<Item = Result<Bytes, AppError>>,
{
    type Item = Result<Bytes, AppError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}
