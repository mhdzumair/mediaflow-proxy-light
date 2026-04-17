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
    RE.get_or_init(|| Regex::new(r#"iframe.*?src=["']([^"']+)["']"#).unwrap())
}
fn file_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"file:"([^"]+)""#).unwrap())
}

pub struct FileMoonExtractor(pub BaseExtractor);

impl FileMoonExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for FileMoonExtractor {
    fn host_name(&self) -> &'static str {
        "FileMoon"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let (html, final_url) = self.0.get_text(url, None).await?;

        // Extract iframe URL.
        let iframe_url = iframe_re()
            .captures(&html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| ExtractorError::extract("FileMoon: iframe not found"))?;

        let origin = final_url.split('/').take(3).collect::<Vec<_>>().join("/");
        let iframe_url = if iframe_url.starts_with("//") {
            let scheme = final_url.split(':').next().unwrap_or("https");
            format!("{scheme}:{iframe_url}")
        } else if !iframe_url.starts_with("http") {
            format!("{origin}{iframe_url}")
        } else {
            iframe_url.to_string()
        };

        let mut iframe_headers = HashMap::new();
        iframe_headers.insert("referer".to_string(), url.to_string());

        let (iframe_html, _) = self
            .0
            .get_text(&iframe_url, Some(iframe_headers.clone()))
            .await?;

        let video_url = if let Some(cap) = file_re().captures(&iframe_html) {
            cap[1].to_string()
        } else if iframe_html.contains("eval(function(p,a,c,k,e,d)") {
            let unpacked = unpack_packed_js(&iframe_html)
                .ok_or_else(|| ExtractorError::extract("FileMoon: unpack failed"))?;
            file_re()
                .captures(&unpacked)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
                .ok_or_else(|| {
                    ExtractorError::extract("FileMoon: file URL not found after unpack")
                })?
        } else {
            return Err(ExtractorError::extract("FileMoon: file URL not found"));
        };

        let mut result_headers = self.0.base_headers.clone();
        result_headers.insert("referer".to_string(), url.to_string());

        Ok(ExtractorResult {
            destination_url: video_url,
            request_headers: result_headers,
            mediaflow_endpoint: "hls_manifest_proxy",
        })
    }
}
