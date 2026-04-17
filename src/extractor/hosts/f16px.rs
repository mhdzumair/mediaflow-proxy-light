//! F16Px extractor — API-based with optional AES-GCM encrypted sources.
use async_trait::async_trait;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

fn embed_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/e/([A-Za-z0-9]+)").unwrap())
}

pub struct F16PxExtractor(pub BaseExtractor);

impl F16PxExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for F16PxExtractor {
    fn host_name(&self) -> &'static str {
        "F16Px"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let origin = url.split('/').take(3).collect::<Vec<_>>().join("/");
        let host = url.split('/').nth(2).unwrap_or("");

        let media_id = embed_id_re()
            .captures(url)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| ExtractorError::extract("F16Px: invalid embed URL"))?;

        let api_url = format!("https://{host}/api/videos/{media_id}/embed/playback");

        let mut headers = self.0.base_headers.clone();
        headers.insert("referer".to_string(), format!("{origin}/"));

        let (json_str, _) = self.0.get_text(&api_url, Some(headers.clone())).await?;
        let data: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| ExtractorError::extract(format!("F16Px: JSON parse error: {e}")))?;

        // Case 1: plain sources array.
        if let Some(sources) = data["sources"].as_array() {
            if !sources.is_empty() {
                let src = sources[0]["url"]
                    .as_str()
                    .ok_or_else(|| ExtractorError::extract("F16Px: empty source URL"))?;
                return Ok(ExtractorResult {
                    destination_url: src.to_string(),
                    request_headers: headers,
                    mediaflow_endpoint: "hls_manifest_proxy",
                });
            }
        }

        // Case 2: encrypted sources — not supported without AES-GCM key material.
        Err(ExtractorError::extract(
            "F16Px: encrypted sources not supported in this build",
        ))
    }
}
