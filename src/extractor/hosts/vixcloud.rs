use async_trait::async_trait;
use regex::Regex;
use scraper::{Html, Selector};
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

fn token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"'token':\s*'(\w+)'").unwrap())
}
fn expires_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"'expires':\s*'(\d+)'").unwrap())
}
fn url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"url:\s*'([^']+)'").unwrap())
}
fn fhd_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"window\.canPlayFHD\s*=\s*true").unwrap())
}

pub struct VixCloudExtractor(pub BaseExtractor);

impl VixCloudExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for VixCloudExtractor {
    fn host_name(&self) -> &'static str {
        "VixCloud"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let (html, _) = self.0.get_text(url, None).await?;

        let doc = Html::parse_document(&html);
        let script_sel = Selector::parse("body > script").unwrap();

        let script_text = doc
            .select(&script_sel)
            .next()
            .map(|el| el.text().collect::<String>())
            .ok_or_else(|| ExtractorError::extract("VixCloud: script not found"))?;

        let token = token_re()
            .captures(&script_text)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| ExtractorError::extract("VixCloud: token not found"))?;

        let expires = expires_re()
            .captures(&script_text)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| ExtractorError::extract("VixCloud: expires not found"))?;

        let server_url = url_re()
            .captures(&script_text)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .ok_or_else(|| ExtractorError::extract("VixCloud: server URL not found"))?;

        let mut final_url = if server_url.contains("?b=1") {
            format!("{server_url}&token={token}&expires={expires}")
        } else {
            format!("{server_url}?token={token}&expires={expires}")
        };

        if fhd_re().is_match(&script_text) {
            final_url.push_str("&h=1");
        }

        let mut headers = self.0.base_headers.clone();
        headers.insert("referer".to_string(), url.to_string());

        Ok(ExtractorResult {
            destination_url: final_url,
            request_headers: headers,
            mediaflow_endpoint: "hls_manifest_proxy",
        })
    }
}
