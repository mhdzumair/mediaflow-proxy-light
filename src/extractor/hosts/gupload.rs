use async_trait::async_trait;
use base64::{engine::general_purpose, Engine};
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

fn decode_payload_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"decodePayload\('([^']+)'\)").unwrap())
}

pub struct GuploadExtractor(pub BaseExtractor);

impl GuploadExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for GuploadExtractor {
    fn host_name(&self) -> &'static str {
        "Gupload"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let mut headers = HashMap::new();
        headers.insert(
            "user-agent".to_string(),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/144 Safari/537.36"
                .to_string(),
        );
        headers.insert("referer".to_string(), "https://gupload.xyz/".to_string());
        headers.insert("origin".to_string(), "https://gupload.xyz".to_string());

        let (html, _) = self.0.get_text(url, Some(headers.clone())).await?;

        let encoded = decode_payload_re()
            .captures(&html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim().to_string())
            .ok_or_else(|| ExtractorError::extract("Gupload: payload not found"))?;

        let decoded = general_purpose::STANDARD
            .decode(&encoded)
            .map_err(|e| ExtractorError::extract(format!("Gupload: base64 decode failed: {e}")))?;
        let decoded_str = String::from_utf8_lossy(&decoded);

        let json_part = decoded_str
            .split_once('|')
            .map(|x| x.1)
            .ok_or_else(|| ExtractorError::extract("Gupload: payload format invalid"))?;
        let payload: serde_json::Value = serde_json::from_str(json_part)
            .map_err(|e| ExtractorError::extract(format!("Gupload: JSON parse error: {e}")))?;

        let hls_url = payload["videoUrl"]
            .as_str()
            .ok_or_else(|| ExtractorError::extract("Gupload: videoUrl missing"))?
            .to_string();

        Ok(ExtractorResult {
            destination_url: hls_url,
            request_headers: headers,
            mediaflow_endpoint: "hls_manifest_proxy",
        })
    }
}
