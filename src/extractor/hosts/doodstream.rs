use async_trait::async_trait;
use regex::Regex;
use rquest::header::{HeaderMap, HeaderName, HeaderValue};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::extractor::base::{
    build_chrome_client, BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

fn pass_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(/pass_md5/[^'"<>\s]+)"#).unwrap())
}
fn token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"token=([^&\s'"]+)"#).unwrap())
}

pub struct DoodStreamExtractor {
    pub base: BaseExtractor,
    /// rquest client with Chrome TLS/HTTP2 fingerprint — needed to bypass
    /// Cloudflare bot detection on playmogo.com (the canonical DoodStream host).
    chrome_client: rquest::Client,
}

impl DoodStreamExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        let chrome_client = build_chrome_client(proxy_url.as_deref());
        Self {
            base: BaseExtractor::new(request_headers, proxy_url),
            chrome_client,
        }
    }

    /// Fetch `url` using the Chrome-impersonating rquest client and return
    /// `(body_text, final_url_after_redirects)`.
    async fn chrome_get(
        &self,
        url: &str,
        headers: HeaderMap,
    ) -> Result<(String, String), ExtractorError> {
        let resp = self
            .chrome_client
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

        let final_url = resp.url().to_string();
        let text = resp
            .text()
            .await
            .map_err(|e| ExtractorError::Network(e.to_string()))?;
        Ok((text, final_url))
    }
}

#[async_trait]
impl Extractor for DoodStreamExtractor {
    fn host_name(&self) -> &'static str {
        "Doodstream"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let video_id = url
            .trim_end_matches('/')
            .split('/')
            .next_back()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ExtractorError::extract("Doodstream: invalid URL — no video ID"))?;

        self.extract_via_embed(url, video_id).await
    }
}

impl DoodStreamExtractor {
    async fn extract_via_embed(
        &self,
        url: &str,
        video_id: &str,
    ) -> Result<ExtractorResult, ExtractorError> {
        let ua = self
            .base
            .base_headers
            .get("user-agent")
            .cloned()
            .unwrap_or_else(|| {
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36"
                    .to_string()
            });

        let embed_url = embed_url_from_raw(url, video_id);
        let origin_host = url_host(url);

        // Build initial request headers
        let mut hm = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&ua) {
            hm.insert(HeaderName::from_static("user-agent"), v);
        }
        if let Ok(v) = HeaderValue::from_str(&format!("https://{origin_host}/")) {
            hm.insert(HeaderName::from_static("referer"), v);
        }

        let (html, final_url) = self.chrome_get(&embed_url, hm).await?;
        let base_url = base_from_url(&final_url);

        if !html.contains("pass_md5") {
            if html.contains("turnstile") || html.contains("captcha_l") {
                return Err(ExtractorError::extract(
                    "Doodstream: site is serving a Turnstile CAPTCHA that requires \
                     browser interaction — cannot be bypassed automatically from this \
                     network location. Try a residential IP or a VPN/proxy.",
                ));
            }
            return Err(ExtractorError::extract(format!(
                "Doodstream: pass_md5 not found in embed HTML (final URL: {final_url})"
            )));
        }

        let pass_path = pass_re()
            .find(&html)
            .ok_or_else(|| ExtractorError::extract("Doodstream: pass_md5 path not found"))?
            .as_str();
        let pass_url = format!("{base_url}{pass_path}");

        // Build pass_md5 fetch headers
        let mut fetch_hm = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&ua) {
            fetch_hm.insert(HeaderName::from_static("user-agent"), v);
        }
        if let Ok(v) = HeaderValue::from_str(&format!("{base_url}/")) {
            fetch_hm.insert(HeaderName::from_static("referer"), v);
        }

        let (base_stream, _) = self.chrome_get(&pass_url, fetch_hm).await?;
        let base_stream = base_stream.trim().to_string();

        if base_stream.is_empty() || base_stream.contains("RELOAD") {
            return Err(ExtractorError::extract(
                "Doodstream: pass_md5 endpoint returned no stream URL \
                 (captcha session may have expired).",
            ));
        }

        let token = token_re()
            .captures(&html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| ExtractorError::extract("Doodstream: token not found in embed HTML"))?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let final_stream_url = format!("{base_stream}123456789?token={token}&expiry={now}");

        // Return headers that the proxy should use when fetching the CDN stream
        let mut result_headers = HashMap::new();
        result_headers.insert("user-agent".to_string(), ua);
        result_headers.insert("referer".to_string(), format!("{base_url}/"));

        Ok(ExtractorResult {
            destination_url: final_stream_url,
            request_headers: result_headers,
            mediaflow_endpoint: "proxy_stream_endpoint",
        })
    }
}

fn url_host(url: &str) -> &str {
    url.trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("dood.to")
}

fn embed_url_from_raw(url: &str, video_id: &str) -> String {
    if url.contains("/e/") {
        return url.to_string();
    }
    let host = url_host(url);
    format!("https://{host}/e/{video_id}")
}

fn base_from_url(url: &str) -> String {
    let stripped = url
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host = stripped.split('/').next().unwrap_or("playmogo.com");
    format!("https://{host}")
}
