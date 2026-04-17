use async_trait::async_trait;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};
use crate::extractor::packed::unpack_packed_js;

fn iframe_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"<iframe[^>]+src=["']([^"']+)["']"#).unwrap())
}
fn m3u8_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(https?://[^"'\s]+\.m3u8[^"'\s]*)"#).unwrap())
}

pub struct StreamWishExtractor(pub BaseExtractor);

impl StreamWishExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for StreamWishExtractor {
    fn host_name(&self) -> &'static str {
        "StreamWish"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let origin = url.split('/').take(3).collect::<Vec<_>>().join("/");
        let referer = self
            .0
            .base_headers
            .get("referer")
            .cloned()
            .unwrap_or_else(|| format!("{origin}/"));

        let mut h = HashMap::new();
        h.insert("referer".to_string(), referer.clone());

        let (html, final_url) = self.0.get_text(url, Some(h.clone())).await?;

        // Check for iframe redirect.
        let (html, iframe_url) = if let Some(cap) = iframe_re().captures(&html) {
            let iframe_src = resolve_url(&final_url, &cap[1]);
            let (inner, u) = self.0.get_text(&iframe_src, Some(h.clone())).await?;
            (inner, u)
        } else {
            (html, final_url)
        };
        let _ = iframe_url;

        // Try to extract m3u8 directly.
        let final_url_str = if let Some(cap) = m3u8_re().captures(&html) {
            cap[1].to_string()
        } else if html.contains("eval(function(p,a,c,k,e,d)") {
            let unpacked = unpack_packed_js(&html)
                .ok_or_else(|| ExtractorError::extract("StreamWish: unpack failed"))?;
            m3u8_re().captures(&unpacked).ok_or_else(|| {
                ExtractorError::extract("StreamWish: m3u8 not found in unpacked JS")
            })?[1]
                .to_string()
        } else {
            return Err(ExtractorError::extract("StreamWish: m3u8 not found"));
        };

        let parsed_referer = url.split('/').take(3).collect::<Vec<_>>().join("/");
        let mut headers = self.0.base_headers.clone();
        headers.insert("referer".to_string(), referer);
        headers.insert("origin".to_string(), parsed_referer);

        Ok(ExtractorResult {
            destination_url: final_url_str,
            request_headers: headers,
            mediaflow_endpoint: "hls_manifest_proxy",
        })
    }
}

fn resolve_url(base: &str, relative: &str) -> String {
    if relative.starts_with("http") {
        return relative.to_string();
    }
    let origin = base.split('/').take(3).collect::<Vec<_>>().join("/");
    if relative.starts_with('/') {
        format!("{origin}{relative}")
    } else {
        let dir = base.rsplitn(2, '/').last().unwrap_or(base);
        format!("{dir}/{relative}")
    }
}
