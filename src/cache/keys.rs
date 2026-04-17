//! Cache key builders.
//!
//! Centralised so that all modules use the same namespacing convention:
//!   `{namespace}:{category}:{key_material}`

pub struct CacheKeys {
    namespace: String,
}

impl CacheKeys {
    pub fn new(namespace: &str) -> Self {
        Self {
            namespace: namespace.to_string(),
        }
    }

    fn key(&self, category: &str, material: &str) -> String {
        if self.namespace.is_empty() {
            format!("{}:{}", category, material)
        } else {
            format!("{}:{}:{}", self.namespace, category, material)
        }
    }

    // HLS / MPD segment cache
    pub fn hls_segment(&self, url: &str) -> String {
        self.key("hls_seg", url)
    }

    pub fn hls_playlist(&self, url: &str) -> String {
        self.key("hls_pl", url)
    }

    // MPD init segment cache
    pub fn mpd_init(&self, representation_id: &str, url: &str) -> String {
        self.key("mpd_init", &format!("{}:{}", representation_id, url))
    }

    pub fn mpd_manifest(&self, url: &str) -> String {
        self.key("mpd_manifest", url)
    }

    // DRM key cache
    pub fn drm_key(&self, key_id: &str) -> String {
        self.key("drm_key", key_id)
    }

    pub fn clearkey_jwks(&self, license_url: &str) -> String {
        self.key("clearkey_jwks", license_url)
    }

    // Extractor result cache
    pub fn extractor_result(&self, host: &str, url: &str) -> String {
        self.key("extractor", &format!("{}:{}", host, url))
    }

    // Rate-limit cooldown
    pub fn cooldown(&self, host: &str) -> String {
        self.key("cooldown", host)
    }
}

impl Default for CacheKeys {
    fn default() -> Self {
        Self::new("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_namespace() {
        let keys = CacheKeys::new("");
        assert_eq!(
            keys.hls_segment("https://cdn.example.com/seg.ts"),
            "hls_seg:https://cdn.example.com/seg.ts"
        );
    }

    #[test]
    fn test_with_namespace() {
        let keys = CacheKeys::new("myapp");
        assert_eq!(keys.drm_key("abc123"), "myapp:drm_key:abc123");
    }
}
