//! LiveTV extractor — supports M3U8 and MPD streams via player API.
use async_trait::async_trait;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

fn source_m3u8_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"source:\s*['"]([^'"]*\.m3u8[^'"]*)['"]"#).unwrap())
}
fn any_m3u8_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"['"]?(https?://[^'">\s]*\.m3u8(?:\?[^'">\s]*)?)['"]?"#).unwrap()
    })
}
fn mpd_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"['"]?(https?://[^'">\s]*\.mpd(?:\?[^'">\s]*)?)['"]?"#).unwrap())
}

pub struct LiveTVExtractor(pub BaseExtractor);

impl LiveTVExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for LiveTVExtractor {
    fn host_name(&self) -> &'static str {
        "LiveTV"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let origin = url.split('/').take(3).collect::<Vec<_>>().join("/");
        let referer = format!("{origin}/");

        let mut headers = self.0.base_headers.clone();
        headers.insert("referer".to_string(), referer.clone());

        let (html, _) = self.0.get_text(url, Some(headers.clone())).await?;

        if let Some(cap) = source_m3u8_re().captures(&html) {
            return Ok(ExtractorResult {
                destination_url: cap[1].to_string(),
                request_headers: headers,
                mediaflow_endpoint: "hls_manifest_proxy",
            });
        }

        if let Some(cap) = any_m3u8_re().captures(&html) {
            return Ok(ExtractorResult {
                destination_url: cap[1].to_string(),
                request_headers: headers,
                mediaflow_endpoint: "hls_manifest_proxy",
            });
        }

        if let Some(cap) = mpd_re().captures(&html) {
            return Ok(ExtractorResult {
                destination_url: cap[1].to_string(),
                request_headers: headers,
                mediaflow_endpoint: "proxy_mpd_manifest",
            });
        }

        Err(ExtractorError::extract("LiveTV: stream URL not found"))
    }
}
