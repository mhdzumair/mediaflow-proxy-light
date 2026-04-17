/// In-process TTL cache backed by `moka`.
///
/// Used as the no-Redis fallback for all caching operations.
use bytes::Bytes;
use moka::future::Cache;
use std::time::Duration;

/// A named, bounded in-memory cache with per-entry TTL.
#[derive(Clone)]
pub struct LocalCache {
    inner: Cache<String, Bytes>,
}

impl LocalCache {
    /// Create a new cache with `max_capacity` entries and a default `ttl`.
    pub fn new(max_capacity: u64, ttl: Duration) -> Self {
        let inner = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(ttl)
            .build();
        Self { inner }
    }

    /// Store `value` under `key`.
    pub async fn set(&self, key: String, value: Bytes) {
        self.inner.insert(key, value).await;
    }

    /// Retrieve the value stored under `key`, if present and not expired.
    pub async fn get(&self, key: &str) -> Option<Bytes> {
        self.inner.get(key).await
    }

    /// Remove `key` from the cache.
    pub async fn remove(&self, key: &str) {
        self.inner.remove(key).await;
    }

    /// Number of entries currently in the cache (approximate).
    pub fn len(&self) -> u64 {
        self.inner.entry_count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_set_get() {
        let cache = LocalCache::new(100, Duration::from_secs(60));
        cache
            .set("key1".to_string(), Bytes::from_static(b"hello"))
            .await;
        let val = cache.get("key1").await;
        assert_eq!(val, Some(Bytes::from_static(b"hello")));
    }

    #[tokio::test]
    async fn test_miss() {
        let cache = LocalCache::new(100, Duration::from_secs(60));
        assert!(cache.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_remove() {
        let cache = LocalCache::new(100, Duration::from_secs(60));
        cache
            .set("key1".to_string(), Bytes::from_static(b"val"))
            .await;
        cache.remove("key1").await;
        assert!(cache.get("key1").await.is_none());
    }
}
