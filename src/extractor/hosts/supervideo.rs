use async_trait::async_trait;
use regex::Regex;
use rquest::header::{HeaderMap, HeaderName, HeaderValue};
use scraper::{Html, Selector};
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    build_chrome_client, BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};
use crate::extractor::packed::unpack_packed_js;

fn file_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"file:"([^"]+)""#).unwrap())
}

pub struct SupervideoExtractor {
    pub base: BaseExtractor,
    chrome_client: rquest::Client,
}

impl SupervideoExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        let chrome_client = build_chrome_client(proxy_url.as_deref());
        Self {
            base: BaseExtractor::new(request_headers, proxy_url),
            chrome_client,
        }
    }

    async fn chrome_get(
        &self,
        url: &str,
        headers: HeaderMap,
    ) -> Result<(String, String), ExtractorError> {
        let resp = self
            .chrome_client
            .get(url)
            .headers(headers)
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
        let text = resp
            .text()
            .await
            .map_err(|e| ExtractorError::Network(e.to_string()))?;
        Ok((text, final_url))
    }
}

#[async_trait]
impl Extractor for SupervideoExtractor {
    fn host_name(&self) -> &'static str {
        "Supervideo"
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

        let (html, _) = self.chrome_get(url, hm).await?;

        let doc = Html::parse_document(&html);
        let sel = Selector::parse("script").unwrap();
        let origin = url.split('/').take(3).collect::<Vec<_>>().join("/");

        for script in doc.select(&sel) {
            let text = script.text().collect::<String>();
            if text.is_empty() {
                continue;
            }

            let search_in = if text.contains("eval(function(p,a,c,k,e,d)") {
                unpack_packed_js(&text).unwrap_or_else(|| text.clone())
            } else {
                text.clone()
            };

            if let Some(cap) = file_re().captures(&search_in) {
                let extracted = &cap[1];
                let final_url = if extracted.starts_with("http") {
                    extracted.to_string()
                } else {
                    format!("{origin}{extracted}")
                };

                let mut headers = self.base.base_headers.clone();
                headers.insert("referer".to_string(), url.to_string());

                return Ok(ExtractorResult {
                    destination_url: final_url,
                    request_headers: headers,
                    mediaflow_endpoint: "hls_manifest_proxy",
                });
            }
        }

        Err(ExtractorError::extract("Supervideo: file URL not found"))
    }
}
