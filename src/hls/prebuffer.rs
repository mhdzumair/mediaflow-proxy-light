/// HLS segment pre-buffer.
///
/// When a client fetches a playlist, the pre-buffer background-fetches the next
/// N segments so they are warm in the local cache when the player requests them.
///
/// Design:
/// - One `PlaylistPrefetcher` per unique playlist URL (shared across clients).
/// - All prefetchers are held in a `DashMap`; inactive ones are evicted after
///   a configurable timeout.
/// - Priority queue: the segment the player just requested jumps to the front.
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use dashmap::DashMap;
use tokio::sync::{Mutex, Notify};
use tokio::time::timeout;

use crate::cache::local::LocalCache;

/// Configuration for the global HLS pre-buffer pool.
#[derive(Debug, Clone)]
pub struct PrebufferConfig {
    /// Number of segments to pre-fetch ahead.
    pub segments_ahead: usize,
    /// Maximum number of prefetchers held simultaneously.
    pub max_prefetchers: usize,
    /// Evict a prefetcher if idle for this duration.
    pub inactivity_timeout: Duration,
    /// TTL for cached segment bytes.
    pub segment_cache_ttl: Duration,
}

impl Default for PrebufferConfig {
    fn default() -> Self {
        Self {
            segments_ahead: 5,
            max_prefetchers: 50,
            inactivity_timeout: Duration::from_secs(60),
            segment_cache_ttl: Duration::from_secs(300),
        }
    }
}

/// A single prefetcher instance for one playlist URL.
struct PlaylistPrefetcher {
    /// Ordered queue of segment URLs to fetch.
    queue: Mutex<VecDeque<String>>,
    /// Signals that a new URL was pushed to the queue.
    wake: Notify,
    /// Updated each time the player actively fetches a segment.
    last_active: Mutex<Instant>,
    /// Shared segment cache.
    cache: LocalCache,
    /// Request headers to use when pre-fetching.
    headers: std::collections::HashMap<String, String>,
}

impl PlaylistPrefetcher {
    fn new(
        urls: Vec<String>,
        headers: std::collections::HashMap<String, String>,
        cache: LocalCache,
    ) -> Arc<Self> {
        let queue = VecDeque::from(urls);
        Arc::new(Self {
            queue: Mutex::new(queue),
            wake: Notify::new(),
            last_active: Mutex::new(Instant::now()),
            cache,
            headers,
        })
    }

    /// Promote `url` to the front of the queue (player requested this segment).
    async fn prioritize(&self, url: &str) {
        let mut q = self.queue.lock().await;
        if let Some(pos) = q.iter().position(|u| u == url) {
            let item = q.remove(pos).unwrap();
            q.push_front(item);
        } else {
            q.push_front(url.to_string());
        }
        drop(q);
        self.wake.notify_one();
        *self.last_active.lock().await = Instant::now();
    }

    async fn is_idle(&self, timeout_duration: Duration) -> bool {
        let last = *self.last_active.lock().await;
        last.elapsed() >= timeout_duration
    }
}

/// Global pool of playlist prefetchers.
pub struct HlsPrebuffer {
    prefetchers: Arc<DashMap<String, Arc<PlaylistPrefetcher>>>,
    config: PrebufferConfig,
    /// Shared segment cache (same instance used by the segment handler).
    cache: LocalCache,
    /// HTTP client for prefetching.
    client: reqwest::Client,
}

impl HlsPrebuffer {
    pub fn new(config: PrebufferConfig) -> Self {
        let cache = LocalCache::new(
            config.max_prefetchers as u64 * config.segments_ahead as u64 * 4,
            config.segment_cache_ttl,
        );
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to build reqwest client for HLS prebuffer");
        Self {
            prefetchers: Arc::new(DashMap::new()),
            config,
            cache,
            client,
        }
    }

    /// Register or update a playlist's segment queue.
    pub async fn register_playlist(
        &self,
        playlist_url: &str,
        segment_urls: Vec<String>,
        headers: std::collections::HashMap<String, String>,
    ) {
        let entry = self
            .prefetchers
            .entry(playlist_url.to_string())
            .or_insert_with(|| {
                PlaylistPrefetcher::new(segment_urls.clone(), headers.clone(), self.cache.clone())
            });

        // Update queue with fresh segment list
        let mut q = entry.queue.lock().await;
        q.clear();
        q.extend(segment_urls);
        drop(q);
        entry.wake.notify_one();

        // Spawn prefetch loop for new entries
        let prefetcher = entry.clone();
        let cache = self.cache.clone();
        let client = self.client.clone();
        let ahead = self.config.segments_ahead;
        let inactivity = self.config.inactivity_timeout;
        let prefetchers = self.prefetchers.clone();
        let playlist_key = playlist_url.to_string();

        tokio::spawn(async move {
            loop {
                // Evict if idle
                if prefetcher.is_idle(inactivity).await {
                    prefetchers.remove(&playlist_key);
                    break;
                }

                let url_to_fetch = {
                    let mut q = prefetcher.queue.lock().await;
                    // Only keep `ahead` items ahead
                    while q.len() > ahead {
                        q.pop_back();
                    }
                    q.front().cloned()
                };

                if let Some(url) = url_to_fetch {
                    // Skip if already cached
                    if cache.get(&url).await.is_none() {
                        let req = {
                            let mut r = client.get(&url);
                            for (k, v) in &prefetcher.headers {
                                r = r.header(k.as_str(), v.as_str());
                            }
                            r
                        };
                        if let Ok(resp) = req.send().await {
                            if resp.status().is_success() {
                                if let Ok(bytes) = resp.bytes().await {
                                    cache.set(url.clone(), bytes).await;
                                }
                            }
                        }
                    }

                    // Remove from queue after fetch attempt
                    let mut q = prefetcher.queue.lock().await;
                    if q.front().map(|u| u == &url).unwrap_or(false) {
                        q.pop_front();
                    }
                } else {
                    // Queue is empty — wait for signal or inactivity check
                    let _ = timeout(Duration::from_secs(5), prefetcher.wake.notified()).await;
                }
            }
        });
    }

    /// Notify the prefetcher that the player requested `segment_url`.
    pub async fn on_segment_request(&self, playlist_url: &str, segment_url: &str) {
        if let Some(entry) = self.prefetchers.get(playlist_url) {
            entry.prioritize(segment_url).await;
        }
    }

    /// Try to retrieve a pre-fetched segment from cache.
    pub async fn get_cached_segment(&self, url: &str) -> Option<Bytes> {
        self.cache.get(url).await
    }

    /// Number of active prefetchers.
    pub fn active_count(&self) -> usize {
        self.prefetchers.len()
    }
}

impl Default for HlsPrebuffer {
    fn default() -> Self {
        Self::new(PrebufferConfig::default())
    }
}
