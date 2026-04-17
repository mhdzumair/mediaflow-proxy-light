use async_trait::async_trait;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

fn sources_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"sources\s*:\s*\[\s*\{\s*file\s*:\s*['"]([^'"]+)"#).unwrap())
}

pub struct VidmolyExtractor(pub BaseExtractor);

impl VidmolyExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for VidmolyExtractor {
    fn host_name(&self) -> &'static str {
        "Vidmoly"
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
             (KHTML, like Gecko) Chrome/120 Safari/537.36"
                .to_string(),
        );
        headers.insert("referer".to_string(), url.to_string());
        headers.insert("sec-fetch-dest".to_string(), "iframe".to_string());

        let (html, _) = self.0.get_text(url, Some(headers.clone())).await?;

        let master_url = sources_re()
            .captures(&html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| ExtractorError::extract("Vidmoly: stream URL not found"))?;

        let final_url = if master_url.starts_with("http") {
            master_url
        } else {
            let origin = url.split('/').take(3).collect::<Vec<_>>().join("/");
            format!("{origin}{master_url}")
        };

        Ok(ExtractorResult {
            destination_url: final_url,
            request_headers: headers,
            mediaflow_endpoint: "hls_manifest_proxy",
        })
    }
}
