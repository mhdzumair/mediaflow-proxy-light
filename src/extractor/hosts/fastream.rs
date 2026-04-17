use async_trait::async_trait;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};
use crate::extractor::packed::unpack_packed_js;

fn file_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"file:"(.*?)""#).unwrap())
}

pub struct FastreamExtractor(pub BaseExtractor);

impl FastreamExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for FastreamExtractor {
    fn host_name(&self) -> &'static str {
        "Fastream"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let mut headers = HashMap::new();
        headers.insert(
            "accept".to_string(),
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".to_string(),
        );
        headers.insert("connection".to_string(), "keep-alive".to_string());
        headers.insert("accept-language".to_string(), "en-US,en;q=0.5".to_string());
        headers.insert(
            "user-agent".to_string(),
            "Mozilla/5.0 (X11; Linux x86_64; rv:138.0) Gecko/20100101 Firefox/138.0".to_string(),
        );

        let (html, _) = self.0.get_text(url, Some(headers.clone())).await?;

        let final_url = if let Some(cap) = file_re().captures(&html) {
            cap[1].to_string()
        } else if html.contains("eval(function(p,a,c,k,e,d)") {
            let unpacked = unpack_packed_js(&html)
                .ok_or_else(|| ExtractorError::extract("Fastream: unpack failed"))?;
            file_re()
                .captures(&unpacked)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
                .ok_or_else(|| {
                    ExtractorError::extract("Fastream: file URL not found after unpack")
                })?
        } else {
            return Err(ExtractorError::extract("Fastream: file URL not found"));
        };

        let host = url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or("");

        let mut result_headers = self.0.base_headers.clone();
        result_headers.insert("referer".to_string(), format!("https://{host}/"));
        result_headers.insert("origin".to_string(), format!("https://{host}"));
        result_headers.insert("accept-language".to_string(), "en-US,en;q=0.5".to_string());
        result_headers.insert("accept".to_string(), "*/*".to_string());
        result_headers.insert(
            "user-agent".to_string(),
            "Mozilla/5.0 (X11; Linux x86_64; rv:138.0) Gecko/20100101 Firefox/138.0".to_string(),
        );

        Ok(ExtractorResult {
            destination_url: final_url,
            request_headers: result_headers,
            mediaflow_endpoint: "hls_manifest_proxy",
        })
    }
}
