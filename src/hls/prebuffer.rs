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
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use dashmap::{mapref::entry::Entry, DashMap};
use futures::future::join_all;
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
    /// Ordered segment queue and headers, updated atomically per playlist refresh.
    state: Mutex<PrefetcherState>,
    /// Signals that a new URL was pushed to the queue.
    wake: Notify,
    /// Updated each time the player actively fetches a segment.
    last_active: Mutex<Instant>,
    /// Shared segment cache.
    cache: LocalCache,
}

struct PrefetcherState {
    queue: VecDeque<String>,
    headers: HashMap<String, String>,
}

impl PlaylistPrefetcher {
    fn new(urls: Vec<String>, headers: HashMap<String, String>, cache: LocalCache) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(PrefetcherState {
                queue: VecDeque::from(urls),
                headers,
            }),
            wake: Notify::new(),
            last_active: Mutex::new(Instant::now()),
            cache,
        })
    }

    async fn update(&self, urls: Vec<String>, headers: HashMap<String, String>) {
        let mut state = self.state.lock().await;
        state.queue.clear();
        state.queue.extend(urls);
        state.headers = headers;
        drop(state);

        *self.last_active.lock().await = Instant::now();
        self.wake.notify_one();
    }

    /// Promote `url` to the front of the queue (player requested this segment).
    async fn prioritize(&self, url: &str) {
        let mut state = self.state.lock().await;
        if let Some(pos) = state.queue.iter().position(|u| u == url) {
            let item = state.queue.remove(pos).unwrap();
            state.queue.push_front(item);
        } else {
            state.queue.push_front(url.to_string());
        }
        drop(state);
        self.wake.notify_one();
        *self.last_active.lock().await = Instant::now();
    }

    async fn is_idle(&self, timeout_duration: Duration) -> bool {
        let last = *self.last_active.lock().await;
        last.elapsed() >= timeout_duration
    }

    async fn pop_batch_with_headers(
        &self,
        max_items: usize,
    ) -> (Vec<String>, HashMap<String, String>) {
        if max_items == 0 {
            return (Vec::new(), HashMap::new());
        }

        let mut state = self.state.lock().await;
        let mut batch = Vec::with_capacity(max_items.min(state.queue.len()));
        for _ in 0..max_items {
            let Some(url) = state.queue.pop_front() else {
                break;
            };
            batch.push(url);
        }
        let headers = state.headers.clone();
        (batch, headers)
    }

    #[cfg(test)]
    async fn queue_snapshot(&self) -> Vec<String> {
        self.state.lock().await.queue.iter().cloned().collect()
    }

    async fn headers_snapshot(&self) -> HashMap<String, String> {
        self.state.lock().await.headers.clone()
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
    #[cfg(test)]
    worker_starts: Arc<std::sync::atomic::AtomicUsize>,
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
            #[cfg(test)]
            worker_starts: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Register or update a playlist's segment queue.
    pub async fn register_playlist(
        &self,
        playlist_url: &str,
        segment_urls: Vec<String>,
        headers: HashMap<String, String>,
    ) {
        let playlist_key = playlist_url.to_string();
        let prefetcher = match self.prefetchers.entry(playlist_key.clone()) {
            Entry::Occupied(entry) => {
                let prefetcher = entry.get().clone();
                prefetcher.update(segment_urls, headers).await;
                return;
            }
            Entry::Vacant(entry) => {
                let prefetcher = PlaylistPrefetcher::new(segment_urls, headers, self.cache.clone());
                entry.insert(prefetcher.clone());
                prefetcher
            }
        };

        let cache = self.cache.clone();
        let client = self.client.clone();
        let ahead = self.config.segments_ahead;
        let inactivity = self.config.inactivity_timeout;
        let prefetchers = self.prefetchers.clone();
        #[cfg(test)]
        self.worker_starts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        tokio::spawn(async move {
            loop {
                // Evict if idle
                if prefetcher.is_idle(inactivity).await {
                    prefetchers.remove(&playlist_key);
                    break;
                }

                let (urls_to_fetch, headers) = prefetcher.pop_batch_with_headers(ahead).await;
                let headers = Arc::new(headers);

                if !urls_to_fetch.is_empty() {
                    let mut fetches = Vec::with_capacity(urls_to_fetch.len());
                    for url in urls_to_fetch {
                        let cache_key = segment_cache_key(&url, headers.as_ref());
                        if cache.get(&cache_key).await.is_some() {
                            continue;
                        }
                        fetches.push(fetch_segment(
                            client.clone(),
                            url,
                            cache_key,
                            headers.clone(),
                        ));
                    }

                    for (cache_key, bytes) in join_all(fetches).await.into_iter().flatten() {
                        cache.set(cache_key, bytes).await;
                    }
                } else {
                    // Queue is empty — wait for a wake signal or the inactivity deadline.
                    //
                    // tokio::Notify stores one pending notification: if prioritize() or
                    // update() fires between pop_batch_with_headers returning empty and
                    // notified().await being registered, notified() returns immediately
                    // on the next poll rather than sleeping. No URLs are lost.
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
    pub async fn get_cached_segment(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
    ) -> Option<Bytes> {
        self.cache.get(&segment_cache_key(url, headers)).await
    }

    /// Number of active prefetchers.
    pub fn active_count(&self) -> usize {
        self.prefetchers.len()
    }

    #[cfg(test)]
    fn worker_start_count(&self) -> usize {
        self.worker_starts
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    pub async fn queue_snapshot(&self, playlist_url: &str) -> Option<Vec<String>> {
        let entry = self.prefetchers.get(playlist_url)?;
        Some(entry.queue_snapshot().await)
    }
}

pub fn segment_cache_key(url: &str, headers: &HashMap<String, String>) -> String {
    if headers.is_empty() {
        return url.to_string();
    }

    let mut entries = headers
        .iter()
        .map(|(key, value)| (key.to_ascii_lowercase(), value.as_str()))
        .collect::<Vec<_>>();
    entries.sort_unstable_by(|(left_key, left_value), (right_key, right_value)| {
        left_key
            .cmp(right_key)
            .then_with(|| left_value.cmp(right_value))
    });

    let mut composite = String::with_capacity(url.len() + entries.len() * 32);
    composite.push_str(url);
    for (key, value) in entries {
        composite.push('\n');
        composite.push_str(&urlencoding::encode(&key));
        composite.push('=');
        composite.push_str(&urlencoding::encode(value));
    }
    composite
}

async fn fetch_segment(
    client: reqwest::Client,
    url: String,
    cache_key: String,
    headers: Arc<HashMap<String, String>>,
) -> Option<(String, Bytes)> {
    let mut req = client.get(&url);
    for (k, v) in headers.iter() {
        req = req.header(k.as_str(), v.as_str());
    }

    let resp = match req.send().await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!("HLS prebuffer: failed to fetch {url}: {err}");
            return None;
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(
            "HLS prebuffer: upstream returned {} for {url}",
            resp.status()
        );
        return None;
    }
    let bytes = match resp.bytes().await {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!("HLS prebuffer: failed to read {url}: {err}");
            return None;
        }
    };
    Some((cache_key, bytes))
}

impl Default for HlsPrebuffer {
    fn default() -> Self {
        Self::new(PrebufferConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn test_config(segments_ahead: usize) -> PrebufferConfig {
        PrebufferConfig {
            segments_ahead,
            max_prefetchers: 10,
            inactivity_timeout: Duration::from_secs(60),
            segment_cache_ttl: Duration::from_secs(60),
        }
    }

    async fn wait_for_cache(
        prebuffer: &HlsPrebuffer,
        url: &str,
        headers: &HashMap<String, String>,
    ) -> Bytes {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(bytes) = prebuffer.get_cached_segment(url, headers).await {
                return bytes;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for cached segment {url}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn start_test_server(
        response_delay: Duration,
    ) -> (String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));

        let active_server = active.clone();
        let max_active_server = max_active.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let active = active_server.clone();
                let max_active = max_active_server.clone();
                tokio::spawn(async move {
                    let now_active = active.fetch_add(1, Ordering::SeqCst) + 1;
                    max_active.fetch_max(now_active, Ordering::SeqCst);

                    let mut buf = [0_u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    tokio::time::sleep(response_delay).await;
                    let body = b"segment";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.write_all(body).await;
                    let _ = socket.shutdown().await;
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });

        (format!("http://{addr}"), active, max_active)
    }

    async fn start_fixture_server(root: PathBuf) -> (String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));

        let active_server = active.clone();
        let max_active_server = max_active.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let root = root.clone();
                let active = active_server.clone();
                let max_active = max_active_server.clone();
                tokio::spawn(async move {
                    let now_active = active.fetch_add(1, Ordering::SeqCst) + 1;
                    max_active.fetch_max(now_active, Ordering::SeqCst);

                    let mut buf = [0_u8; 2048];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]);
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let relative = path.trim_start_matches('/').split('?').next().unwrap_or("");
                    let file_path = root.join(relative);

                    let (status, body) = match tokio::fs::read(&file_path).await {
                        Ok(bytes) => ("200 OK", bytes),
                        Err(_) => ("404 Not Found", b"not found".to_vec()),
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                    let _ = socket.shutdown().await;
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });

        (format!("http://{addr}"), active, max_active)
    }

    fn fixture_root() -> (Option<TempDir>, PathBuf) {
        if let Some(path) = std::env::var_os("MEDIAFLOW_HLS_FIXTURE_DIR") {
            return (None, PathBuf::from(path));
        }

        let temp_dir = tempfile::tempdir().expect("fixture tempdir");
        generate_test_fixture(temp_dir.path());
        let fixture_path = temp_dir.path().to_path_buf();
        (Some(temp_dir), fixture_path)
    }

    fn fixture_segment_urls(root: &Path, base_url: &str, count: usize) -> Vec<String> {
        let playlist =
            std::fs::read_to_string(root.join("0640_vod.m3u8")).expect("fixture playlist");
        playlist
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .take(count)
            .map(|line| format!("{base_url}/{line}"))
            .collect()
    }

    fn generate_test_fixture(root: &Path) {
        const SEGMENT_COUNT: usize = 5;
        const VARIANTS: [&str; 8] = [
            "0150_vod.m3u8",
            "0240_vod.m3u8",
            "0440_vod.m3u8",
            "0640_vod.m3u8",
            "1240_vod.m3u8",
            "1840_vod.m3u8",
            "2540_vod.m3u8",
            "3340_vod.m3u8",
        ];

        std::fs::create_dir_all(root.join("segments")).expect("segments dir");

        let master = concat!(
            "#EXTM3U\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=165135,RESOLUTION=416x234,CODECS=\"avc1.42e00d,mp4a.40.2\"\n",
            "0150_vod.m3u8\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=262346,RESOLUTION=480x270,CODECS=\"avc1.42e015,mp4a.40.2\"\n",
            "0240_vod.m3u8\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=481677,RESOLUTION=640x360,CODECS=\"avc1.4d401e,mp4a.40.2\"\n",
            "0440_vod.m3u8\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=688301,RESOLUTION=640x360,CODECS=\"avc1.4d401e,mp4a.40.2\"\n",
            "0640_vod.m3u8\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=1308077,RESOLUTION=960x540,CODECS=\"avc1.4d401f,mp4a.40.2\"\n",
            "1240_vod.m3u8\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=1927853,RESOLUTION=1280x720,CODECS=\"avc1.4d401f,mp4a.40.2\"\n",
            "1840_vod.m3u8\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=2650941,RESOLUTION=1920x1080,CODECS=\"avc1.640028,mp4a.40.2\"\n",
            "2540_vod.m3u8\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=3477293,RESOLUTION=1920x1080,CODECS=\"avc1.640028,mp4a.40.2\"\n",
            "3340_vod.m3u8\n",
        );
        std::fs::write(root.join("sl.m3u8"), master).expect("master fixture");

        let mut variant = String::from(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n",
        );
        for idx in 0..SEGMENT_COUNT {
            variant.push_str(&format!("#EXTINF:4.000,\nsegments/seg{idx:03}.ts\n"));
        }
        variant.push_str("#EXT-X-ENDLIST\n");

        for variant_name in VARIANTS {
            std::fs::write(root.join(variant_name), &variant).expect("variant fixture");
        }

        for idx in 0..SEGMENT_COUNT {
            std::fs::write(
                root.join(format!("segments/seg{idx:03}.ts")),
                synthetic_ts_segment(idx as u8),
            )
            .expect("segment fixture");
        }
    }

    fn synthetic_ts_segment(seed: u8) -> Vec<u8> {
        const PACKET_SIZE: usize = 188;
        const PACKET_COUNT: usize = 64;

        let mut bytes = vec![0_u8; PACKET_SIZE * PACKET_COUNT];
        for (packet_index, packet) in bytes
            .as_chunks_mut::<PACKET_SIZE>()
            .0
            .iter_mut()
            .enumerate()
        {
            packet[0] = 0x47;
            packet[1] = 0x40 | ((packet_index as u8) & 0x1f);
            packet[2] = (packet_index as u8).wrapping_add(seed);
            packet[3] = 0x10;
            for (offset, byte) in packet[4..].iter_mut().enumerate() {
                *byte = seed
                    .wrapping_add(offset as u8)
                    .wrapping_add(packet_index as u8);
            }
        }
        bytes
    }

    #[tokio::test]
    async fn duplicate_registration_uses_one_prefetcher_worker() {
        let prebuffer = HlsPrebuffer::new(test_config(0));
        let playlist = "http://example.test/live.m3u8";

        prebuffer
            .register_playlist(
                playlist,
                vec!["http://example.test/one.ts".to_string()],
                HashMap::new(),
            )
            .await;
        prebuffer
            .register_playlist(
                playlist,
                vec!["http://example.test/two.ts".to_string()],
                HashMap::new(),
            )
            .await;

        assert_eq!(prebuffer.active_count(), 1);
        assert_eq!(prebuffer.worker_start_count(), 1);
    }

    #[tokio::test]
    async fn repeated_registration_replaces_queue_and_headers() {
        let prebuffer = HlsPrebuffer::new(test_config(0));
        let playlist = "http://example.test/live.m3u8";
        let mut first_headers = HashMap::new();
        first_headers.insert("referer".to_string(), "first".to_string());
        let mut second_headers = HashMap::new();
        second_headers.insert("referer".to_string(), "second".to_string());

        prebuffer
            .register_playlist(
                playlist,
                vec!["http://example.test/one.ts".to_string()],
                first_headers,
            )
            .await;
        prebuffer
            .register_playlist(
                playlist,
                vec![
                    "http://example.test/two.ts".to_string(),
                    "http://example.test/three.ts".to_string(),
                ],
                second_headers,
            )
            .await;

        let entry = prebuffer.prefetchers.get(playlist).unwrap();
        let queue = entry.queue_snapshot().await;
        assert_eq!(
            queue,
            vec![
                "http://example.test/two.ts".to_string(),
                "http://example.test/three.ts".to_string(),
            ]
        );

        let headers = entry.headers_snapshot().await;
        assert_eq!(headers.get("referer").map(String::as_str), Some("second"));
        assert_eq!(prebuffer.worker_start_count(), 1);
    }

    #[tokio::test]
    async fn initial_warmup_fetches_segments_in_parallel() {
        let (base_url, _active, max_active) = start_test_server(Duration::from_millis(250)).await;
        let prebuffer = HlsPrebuffer::new(test_config(5));
        let urls = (0..5)
            .map(|idx| format!("{base_url}/seg-{idx}.ts"))
            .collect::<Vec<_>>();

        prebuffer
            .register_playlist(
                "http://example.test/live.m3u8",
                urls.clone(),
                HashMap::new(),
            )
            .await;

        for url in &urls {
            assert_eq!(
                wait_for_cache(&prebuffer, url, &HashMap::new()).await,
                Bytes::from_static(b"segment")
            );
        }
        assert_eq!(prebuffer.worker_start_count(), 1);
        assert!(
            max_active.load(Ordering::SeqCst) > 1,
            "expected more than one concurrent prefetch request"
        );
    }

    #[tokio::test]
    async fn burst_skips_already_cached_segments() {
        let (base_url, _active, max_active) = start_test_server(Duration::from_millis(50)).await;
        let prebuffer = HlsPrebuffer::new(test_config(2));
        let cached_url = format!("{base_url}/cached.ts");
        let uncached_url = format!("{base_url}/uncached.ts");
        prebuffer
            .cache
            .set(
                segment_cache_key(&cached_url, &HashMap::new()),
                Bytes::from_static(b"already cached"),
            )
            .await;

        prebuffer
            .register_playlist(
                "http://example.test/live.m3u8",
                vec![cached_url.clone(), uncached_url.clone()],
                HashMap::new(),
            )
            .await;

        assert_eq!(
            wait_for_cache(&prebuffer, &uncached_url, &HashMap::new()).await,
            Bytes::from_static(b"segment")
        );
        assert_eq!(
            prebuffer
                .get_cached_segment(&cached_url, &HashMap::new())
                .await,
            Some(Bytes::from_static(b"already cached"))
        );
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn generated_hls_fixture_prefetches_segments_locally() {
        let (_fixture_guard, fixture_dir) = fixture_root();
        let (base_url, _active, max_active) = start_fixture_server(fixture_dir.clone()).await;
        let segment_urls = fixture_segment_urls(&fixture_dir, &base_url, 5);
        assert_eq!(segment_urls.len(), 5);

        let prebuffer = HlsPrebuffer::new(test_config(5));
        prebuffer
            .register_playlist(
                &format!("{base_url}/0640_vod.m3u8"),
                segment_urls.clone(),
                HashMap::new(),
            )
            .await;

        for url in &segment_urls {
            let bytes = wait_for_cache(&prebuffer, url, &HashMap::new()).await;
            assert_eq!(bytes.len(), 188 * 64, "unexpected segment fixture size");
        }
        assert_eq!(prebuffer.worker_start_count(), 1);
        assert!(
            max_active.load(Ordering::SeqCst) > 1,
            "expected real fixture segments to be prefetched concurrently"
        );
    }

    #[tokio::test]
    async fn cache_key_separates_header_contexts() {
        let prebuffer = HlsPrebuffer::new(test_config(1));
        let url = "http://example.test/seg.ts";
        let mut first = HashMap::new();
        first.insert("Authorization".to_string(), "Bearer first".to_string());
        let mut second = HashMap::new();
        second.insert("authorization".to_string(), "Bearer second".to_string());

        prebuffer
            .cache
            .set(
                segment_cache_key(url, &first),
                Bytes::from_static(b"first payload"),
            )
            .await;

        assert_eq!(
            prebuffer.get_cached_segment(url, &first).await,
            Some(Bytes::from_static(b"first payload"))
        );
        assert_eq!(prebuffer.get_cached_segment(url, &second).await, None);
    }

    #[tokio::test]
    async fn segment_request_prioritizes_without_starting_worker() {
        let prebuffer = HlsPrebuffer::new(test_config(0));
        let playlist = "http://example.test/live.m3u8";
        prebuffer
            .register_playlist(
                playlist,
                vec!["http://example.test/one.ts".to_string()],
                HashMap::new(),
            )
            .await;

        prebuffer
            .on_segment_request(playlist, "http://example.test/urgent.ts")
            .await;

        let entry = prebuffer.prefetchers.get(playlist).unwrap();
        let queue = entry.queue_snapshot().await;
        assert_eq!(
            queue.first().map(String::as_str),
            Some("http://example.test/urgent.ts")
        );
        assert_eq!(prebuffer.worker_start_count(), 1);
    }
}
