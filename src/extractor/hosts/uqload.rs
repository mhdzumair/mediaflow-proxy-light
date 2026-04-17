use async_trait::async_trait;
use regex::Regex;
use rquest::header::{HeaderMap, HeaderName, HeaderValue};
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    build_chrome_client, BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

fn sources_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"sources:\s*\["(https?://[^"]+)""#).unwrap())
}

pub struct UqloadExtractor {
    pub base: BaseExtractor,
    chrome_client: rquest::Client,
}

impl UqloadExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        let chrome_client = build_chrome_client(proxy_url.as_deref());
        Self {
            base: BaseExtractor::new(request_headers, proxy_url),
            chrome_client,
        }
    }
}

#[async_trait]
impl Extractor for UqloadExtractor {
    fn host_name(&self) -> &'static str {
        "Uqload"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let ua = self
            .base
            .base_headers
            .get("user-agent")
            .cloned()
            .unwrap_or_else(|| {
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36"
                    .to_string()
            });

        let mut hm = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&ua) {
            hm.insert(HeaderName::from_static("user-agent"), v);
        }

        let resp = self
            .chrome_client
            .get(url)
            .headers(hm)
            .send()
            .await
            .map_err(|e| ExtractorError::Network(e.to_string()))?;

        let status = resp.status().as_u16();
        if status >= 400 {
            return Err(ExtractorError::Http {
                status,
                message: format!("HTTP {status} from {url}"),
            });
        }

        let final_url = resp.url().to_string();
        let html = resp
            .text()
            .await
            .map_err(|e| ExtractorError::Network(e.to_string()))?;

        let video_url = sources_re()
            .captures(&html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| ExtractorError::extract("Uqload: video URL not found"))?;

        // Use the final URL after redirects for the referer origin
        let origin = final_url.split('/').take(3).collect::<Vec<_>>().join("/");
        let mut headers = self.base.base_headers.clone();
        headers.insert("referer".to_string(), format!("{origin}/"));

        Ok(ExtractorResult {
            destination_url: video_url,
            request_headers: headers,
            mediaflow_endpoint: "proxy_stream_endpoint",
        })
    }
}
