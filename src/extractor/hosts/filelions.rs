use async_trait::async_trait;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};
use crate::extractor::packed::unpack_packed_js;

fn patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            Regex::new(r#"sources:\s*\[\{file:\s*["'](?P<url>[^"']+)"#).unwrap(),
            Regex::new(r#"["']hls4["']:\s*["'](?P<url>[^"']+)"#).unwrap(),
            Regex::new(r#"["']hls2["']:\s*["'](?P<url>[^"']+)"#).unwrap(),
        ]
    })
}

pub struct FileLionsExtractor(pub BaseExtractor);

impl FileLionsExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for FileLionsExtractor {
    fn host_name(&self) -> &'static str {
        "FileLions"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let (html, _) = self.0.get_text(url, None).await?;

        let search_in = if html.contains("eval(function(p,a,c,k,e,d)") {
            unpack_packed_js(&html).unwrap_or_else(|| html.clone())
        } else {
            html.clone()
        };

        let mut video_url = None;
        for re in patterns() {
            if let Some(cap) = re.captures(&search_in) {
                if let Some(u) = cap.name("url").or_else(|| cap.get(1)) {
                    video_url = Some(u.as_str().to_string());
                    break;
                }
            }
        }

        let video_url =
            video_url.ok_or_else(|| ExtractorError::extract("FileLions: stream URL not found"))?;

        let mut headers = self.0.base_headers.clone();
        headers.insert("referer".to_string(), url.to_string());

        Ok(ExtractorResult {
            destination_url: video_url,
            request_headers: headers,
            mediaflow_endpoint: "hls_manifest_proxy",
        })
    }
}
