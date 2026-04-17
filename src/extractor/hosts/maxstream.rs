use async_trait::async_trait;
use regex::Regex;
use scraper::{Html, Selector};
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

fn packed_js_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\}\('(.+)',.+,'(.+)'\.split").unwrap())
}

pub struct MaxstreamExtractor(pub BaseExtractor);

impl MaxstreamExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for MaxstreamExtractor {
    fn host_name(&self) -> &'static str {
        "Maxstream"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        // Get intermediate URL
        let fixed_url = if url.contains("msf") {
            url.replace("msf", "mse")
        } else {
            url.to_string()
        };
        let (html1, _) = self.0.get_text(&fixed_url, None).await?;

        // Extract href before any await — scraper::Html is not Send.
        let maxstream_url = {
            let doc = Html::parse_document(&html1);
            let sel = Selector::parse("a").unwrap();
            doc.select(&sel)
                .next()
                .and_then(|el| el.value().attr("href"))
                .ok_or_else(|| ExtractorError::extract("Maxstream: intermediate link not found"))?
                .to_string()
        };

        let mut headers = HashMap::new();
        headers.insert("accept-language".to_string(), "en-US,en;q=0.5".to_string());

        let (html2, _) = self
            .0
            .get_text(&maxstream_url, Some(headers.clone()))
            .await?;

        let cap = packed_js_re()
            .captures(&html2)
            .ok_or_else(|| ExtractorError::extract("Maxstream: packed JS not found"))?;

        let s1 = &cap[2];
        let terms: Vec<&str> = s1.split('|').collect();

        let urlset_idx = terms
            .iter()
            .position(|&t| t == "urlset")
            .ok_or_else(|| ExtractorError::extract("Maxstream: urlset not found"))?;
        let hls_idx = terms
            .iter()
            .position(|&t| t == "hls")
            .ok_or_else(|| ExtractorError::extract("Maxstream: hls not found"))?;
        let sources_idx = terms
            .iter()
            .position(|&t| t == "sources")
            .ok_or_else(|| ExtractorError::extract("Maxstream: sources not found"))?;

        let reversed_elements: Vec<&str> = terms[urlset_idx + 1..hls_idx]
            .iter()
            .rev()
            .cloned()
            .collect();
        let first_part: Vec<&str> = terms[hls_idx + 1..sources_idx]
            .iter()
            .rev()
            .cloned()
            .collect();

        let mut first_url_part = String::new();
        for part in &first_part {
            if part.contains('0') {
                first_url_part.push_str(part);
            } else {
                first_url_part.push_str(part);
                first_url_part.push('-');
            }
        }

        let base = format!("https://{first_url_part}.host-cdn.net/hls/");
        let final_url = if reversed_elements.len() == 1 {
            format!("{base},{}.urlset/master.m3u8", reversed_elements[0])
        } else {
            let mut b = base.clone();
            for (i, el) in reversed_elements.iter().enumerate() {
                b.push_str(el);
                b.push(',');
                if i == reversed_elements.len() - 1 {
                    b.push_str(".urlset/master.m3u8");
                }
            }
            b
        };

        let mut result_headers = self.0.base_headers.clone();
        result_headers.insert("referer".to_string(), url.to_string());

        Ok(ExtractorResult {
            destination_url: final_url,
            request_headers: result_headers,
            mediaflow_endpoint: "hls_manifest_proxy",
        })
    }
}
