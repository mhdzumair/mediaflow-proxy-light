//! Extractor for vidfast.pro (movies and TV).
//!
//! URL formats:
//!   https://vidfast.pro/movie/{tmdb_id}
//!   https://vidfast.pro/tv/{tmdb_id}/{season}/{episode}
//!
//! Extraction flow:
//!   1. Parse TMDB ID from the URL path.
//!   2. GET https://ythd.org/embed/{tmdb_id}  →  grab the first data-hash.
//!      (Hashes are time-limited: cloudnestra.com only accepts a hash returned
//!      by the most-recent ythd.org page load.)
//!   3. GET https://cloudnestra.com/rcp/{hash} (Referer: ythd.org)
//!      →  grab the /prorcp/{hash} from the inline iframe src.
//!   4. GET https://cloudnestra.com/prorcp/{prorcp_hash}
//!      →  parse the Playerjs `file:` value (HLS master URL with {v1} CDN placeholder).
//!   5. Substitute {v1} with cloudnestra.com and return the resolved HLS URL.

use async_trait::async_trait;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

// Static compiled regexes — compiled once on first use, reused on all subsequent calls.
fn hash_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"data-hash="([^"]+)""#).unwrap())
}
fn prorcp_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"src:\s*'/prorcp/([^']+)'").unwrap())
}
fn file_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"file:\s*"(https://[^"]+)""#).unwrap())
}

pub struct VidFastExtractor {
    base: BaseExtractor,
}

impl VidFastExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self {
            base: BaseExtractor::new(request_headers, proxy_url),
        }
    }

    /// Build a HeaderMap from the extractor's base headers plus extra pairs.
    fn make_headers(&self, extra: &[(&str, &str)]) -> HeaderMap {
        let mut hm = HeaderMap::new();
        for (k, v) in &self.base.base_headers {
            if let (Ok(n), Ok(val)) = (HeaderName::from_str(k), HeaderValue::from_str(v)) {
                hm.insert(n, val);
            }
        }
        for &(k, v) in extra {
            if let (Ok(n), Ok(val)) = (HeaderName::from_str(k), HeaderValue::from_str(v)) {
                hm.insert(n, val);
            }
        }
        hm
    }

    /// GET url → response body as String.
    async fn get_text(&self, url: &str, headers: HeaderMap) -> Result<String, ExtractorError> {
        let resp = self
            .base
            .client
            .get(url)
            .headers(headers)
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

        resp.text()
            .await
            .map_err(|e| ExtractorError::Network(e.to_string()))
    }
}

#[async_trait]
impl Extractor for VidFastExtractor {
    fn host_name(&self) -> &'static str {
        "VidFast"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        // ── Parse TMDB ID from URL path ────────────────────────────────────
        // Expected: /movie/{tmdb_id} or /tv/{tmdb_id}/{season}/{episode}
        let path = url
            .split_once("://")
            .and_then(|(_, rest)| rest.split_once('/'))
            .map(|(_, p)| p)
            .unwrap_or("");

        let parts: Vec<&str> = path.trim_start_matches('/').splitn(3, '/').collect();
        if parts.len() < 2 || parts[1].is_empty() {
            return Err(ExtractorError::extract(format!(
                "VidFast: cannot parse TMDB ID from URL: {url}"
            )));
        }
        let tmdb_id = parts[1];
        let ythd_url = format!("https://ythd.org/embed/{tmdb_id}");

        // ── Step 1: ythd.org embed page ───────────────────────────────────
        let ythd_html = self.get_text(&ythd_url, self.make_headers(&[])).await?;

        let data_hash = hash_re()
            .captures(&ythd_html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
            .ok_or_else(|| ExtractorError::extract("VidFast: no data-hash on ythd.org page"))?;

        // ── Step 2: cloudnestra /rcp/ ─────────────────────────────────────
        let rcp_url = format!("https://cloudnestra.com/rcp/{data_hash}");
        let rcp_html = self
            .get_text(&rcp_url, self.make_headers(&[("referer", &ythd_url)]))
            .await?;

        let prorcp_hash = prorcp_re()
            .captures(&rcp_html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
            .ok_or_else(|| ExtractorError::extract("VidFast: /prorcp/ hash not found"))?;

        // ── Step 3: cloudnestra /prorcp/ ─────────────────────────────────
        let prorcp_url = format!("https://cloudnestra.com/prorcp/{prorcp_hash}");
        let prorcp_html = self
            .get_text(&prorcp_url, self.make_headers(&[("referer", &rcp_url)]))
            .await?;

        // ── Step 4: parse Playerjs file URL ──────────────────────────────
        let full_file = file_re()
            .captures(&prorcp_html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| ExtractorError::extract("VidFast: Playerjs file URL not found"))?;

        // Multiple CDN fallbacks separated by " or "; use the first.
        let first_url = full_file.split(" or ").next().unwrap_or("").trim();

        // {v1} → cloudnestra.com (primary CDN for the tmstr4 proxy)
        let stream_url = first_url.replace("{v1}", "cloudnestra.com");

        if !stream_url.starts_with("https://") {
            return Err(ExtractorError::extract(format!(
                "VidFast: unexpected stream URL: {}",
                &stream_url[..stream_url.len().min(120)]
            )));
        }

        // ── Result headers ────────────────────────────────────────────────
        let mut result_headers = HashMap::new();
        if let Some(ua) = self.base.base_headers.get("user-agent") {
            result_headers.insert("user-agent".to_string(), ua.clone());
        }
        result_headers.insert(
            "referer".to_string(),
            "https://cloudnestra.com/".to_string(),
        );

        Ok(ExtractorResult {
            destination_url: stream_url,
            request_headers: result_headers,
            mediaflow_endpoint: "hls_manifest_proxy",
        })
    }
}
