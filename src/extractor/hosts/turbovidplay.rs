use async_trait::async_trait;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

fn urlplay_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?:urlPlay|data-hash)\s*=\s*['"]([^'"]+)"#).unwrap())
}
fn m3u8_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"https?://[^\s']+\.m3u8").unwrap())
}

pub struct TurboVidPlayExtractor(pub BaseExtractor);

impl TurboVidPlayExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for TurboVidPlayExtractor {
    fn host_name(&self) -> &'static str {
        "TurboVidPlay"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let (html, final_url) = self.0.get_text(url, None).await?;

        let media_url = urlplay_re()
            .captures(&html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| ExtractorError::extract("TurboVidPlay: media URL not found"))?;

        let origin = final_url.split('/').take(3).collect::<Vec<_>>().join("/");
        let media_url = if media_url.starts_with("//") {
            let scheme = final_url.split(':').next().unwrap_or("https");
            format!("{scheme}:{media_url}")
        } else if media_url.starts_with('/') {
            format!("{origin}{media_url}")
        } else {
            media_url.to_string()
        };

        let mut h = HashMap::new();
        h.insert("referer".to_string(), url.to_string());

        let (playlist, _) = self.0.get_text(&media_url, Some(h)).await?;

        let real_m3u8 = m3u8_re()
            .find(&playlist)
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| {
                ExtractorError::extract("TurboVidPlay: m3u8 URL not found in playlist")
            })?;

        let mut headers = HashMap::new();
        headers.insert("origin".to_string(), origin);

        Ok(ExtractorResult {
            destination_url: real_m3u8,
            request_headers: headers,
            mediaflow_endpoint: "hls_manifest_proxy",
        })
    }
}
