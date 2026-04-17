//! Mixdrop extractor — uses P,A,C,K,E,D eval unpacker.
use async_trait::async_trait;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};
use crate::extractor::packed::unpack_packed_js;

fn wurl_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"MDCore\.wurl\s*=\s*"(.*?)""#).unwrap())
}

pub struct MixdropExtractor(pub BaseExtractor);

impl MixdropExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for MixdropExtractor {
    fn host_name(&self) -> &'static str {
        "Mixdrop"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let url = if url.contains("club") {
            url.replace("club", "ps")
                .split("/2")
                .next()
                .unwrap_or(url)
                .to_string()
        } else {
            url.to_string()
        };

        let mut extra_headers = HashMap::new();
        extra_headers.insert("accept-language".to_string(), "en-US,en;q=0.5".to_string());

        let (html, _) = self.0.get_text(&url, Some(extra_headers)).await?;

        let video_url = if let Some(cap) = wurl_re().captures(&html) {
            cap[1].to_string()
        } else {
            let unpacked = unpack_packed_js(&html)
                .ok_or_else(|| ExtractorError::extract("Mixdrop: could not unpack JS"))?;
            wurl_re()
                .captures(&unpacked)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
                .ok_or_else(|| ExtractorError::extract("Mixdrop: wurl not found"))?
        };

        let final_url = if video_url.starts_with("//") {
            format!("https:{video_url}")
        } else {
            video_url
        };

        let mut headers = self.0.base_headers.clone();
        headers.insert("referer".to_string(), url.to_string());

        Ok(ExtractorResult {
            destination_url: final_url,
            request_headers: headers,
            mediaflow_endpoint: "proxy_stream_endpoint",
        })
    }
}
