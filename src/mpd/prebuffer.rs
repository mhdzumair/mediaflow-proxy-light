//! Background DASH segment pre-fetcher, mirroring `src/hls/prebuffer.rs`.
//!
//! A [`DashPrebuffer`] manages a map of active prefetchers keyed by MPD URL.
//! When a playlist is registered, a background task fetches upcoming segments
//! into a local cache so they are already warm when the player requests them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use dashmap::DashMap;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::cache::local::LocalCache;

// ---------------------------------------------------------------------------
// Per-playlist prefetcher
// ---------------------------------------------------------------------------

struct PlaylistPrefetcher {
    /// URLs of segments to prefetch, in order.
    segment_urls: Mutex<std::collections::VecDeque<String>>,
    /// Headers to include when fetching segments.
    headers: reqwest::header::HeaderMap,
    /// Timestamp of the last client request for this playlist.
    last_request: Mutex<Instant>,
}

impl PlaylistPrefetcher {
    fn new(urls: Vec<String>, headers: reqwest::header::HeaderMap) -> Self {
        let queue: std::collections::VecDeque<String> = urls.into();
        Self {
            segment_urls: Mutex::new(queue),
            headers,
            last_request: Mutex::new(Instant::now()),
        }
    }

    async fn touch(&self) {
        *self.last_request.lock().await = Instant::now();
    }

    async fn is_stale(&self, timeout_secs: u64) -> bool {
        let last = *self.last_request.lock().await;
        last.elapsed() > Duration::from_secs(timeout_secs)
    }

    async fn pop_next(&self) -> Option<String> {
        self.segment_urls.lock().await.pop_front()
    }

    async fn push_front(&self, url: String) {
        self.segment_urls.lock().await.push_front(url);
    }
}

// ---------------------------------------------------------------------------
// DashPrebuffer
// ---------------------------------------------------------------------------

/// Shared pre-buffer registry.  Cheap to clone (backed by `Arc`).
#[derive(Clone)]
pub struct DashPrebuffer {
    inner: Arc<DashPrebufferInner>,
}

struct DashPrebufferInner {
    cache: LocalCache,
    prefetchers: DashMap<String, Arc<PlaylistPrefetcher>>,
    inactivity_timeout_secs: u64,
    client: reqwest::Client,
}

impl DashPrebuffer {
    pub fn new(cache: LocalCache, inactivity_timeout_secs: u64) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to create prebuffer HTTP client");

        Self {
            inner: Arc::new(DashPrebufferInner {
                cache,
                prefetchers: DashMap::new(),
                inactivity_timeout_secs,
                client,
            }),
        }
    }

    /// Register a playlist and its next segment URLs for pre-fetching.
    pub fn register_playlist(
        &self,
        playlist_url: String,
        segment_urls: Vec<String>,
        headers: reqwest::header::HeaderMap,
    ) {
        let prefetcher = Arc::new(PlaylistPrefetcher::new(segment_urls, headers));
        self.inner
            .prefetchers
            .insert(playlist_url.clone(), Arc::clone(&prefetcher));

        let inner = Arc::clone(&self.inner);
        let playlist_url_clone = playlist_url.clone();
        let prefetcher_clone = Arc::clone(&prefetcher);

        tokio::spawn(async move {
            inner
                .run_prefetcher(playlist_url_clone, prefetcher_clone)
                .await;
        });
    }

    /// Called when a segment is requested — touch the prefetcher to reset
    /// the inactivity timer.
    pub fn on_segment_request(&self, playlist_url: &str, _segment_url: &str) {
        if let Some(p) = self.inner.prefetchers.get(playlist_url) {
            let pref = Arc::clone(p.value());
            tokio::spawn(async move {
                pref.touch().await;
            });
        }
    }

    /// Try to retrieve a segment from the cache.
    pub async fn get_segment(&self, url: &str) -> Option<Bytes> {
        self.inner.cache.get(url).await
    }
}

impl DashPrebufferInner {
    async fn run_prefetcher(&self, playlist_url: String, prefetcher: Arc<PlaylistPrefetcher>) {
        loop {
            if prefetcher.is_stale(self.inactivity_timeout_secs).await {
                info!(
                    "DASH prebuffer: removing stale prefetcher for {}",
                    playlist_url
                );
                self.prefetchers.remove(&playlist_url);
                break;
            }

            if let Some(url) = prefetcher.pop_next().await {
                // Skip if already cached
                if self.cache.get(&url).await.is_some() {
                    debug!("DASH prebuffer: cache hit for {}", url);
                    continue;
                }

                let headers = prefetcher.headers.clone();
                match timeout(
                    Duration::from_secs(15),
                    self.client.get(&url).headers(headers).send(),
                )
                .await
                {
                    Ok(Ok(resp)) if resp.status().is_success() => {
                        if let Ok(bytes) = resp.bytes().await {
                            self.cache.set(url.clone(), bytes).await;
                            debug!("DASH prebuffer: fetched {}", url);
                        }
                    }
                    Ok(Ok(resp)) => {
                        warn!(
                            "DASH prebuffer: upstream error {} for {}",
                            resp.status(),
                            url
                        );
                    }
                    Ok(Err(e)) => {
                        warn!("DASH prebuffer: fetch error for {}: {}", url, e);
                    }
                    Err(_) => {
                        warn!("DASH prebuffer: timeout fetching {}", url);
                    }
                }
            } else {
                // Queue empty — sleep briefly and wait for new registrations
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}
