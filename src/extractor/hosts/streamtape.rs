use async_trait::async_trait;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

fn id_re() -> &'static Regex {
    // Matches `id=<anything-except-single-quote>`.  Equivalent to the lookahead
    // form `id=.*?(?=')` but compatible with the non-look-around `regex` crate.
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"id=[^']*").unwrap())
}

pub struct StreamtapeExtractor(pub BaseExtractor);

impl StreamtapeExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for StreamtapeExtractor {
    fn host_name(&self) -> &'static str {
        "Streamtape"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let (html, _) = self.0.get_text(url, None).await?;

        let matches: Vec<&str> = id_re().find_iter(&html).map(|m| m.as_str()).collect();
        if matches.is_empty() {
            return Err(ExtractorError::extract("Streamtape: no id param found"));
        }

        let mut final_url = String::new();
        for i in 1..matches.len() {
            if matches[i - 1] == matches[i] && matches[i].contains("ip=") {
                final_url = format!("https://streamtape.com/get_video?{}", matches[i]);
            }
        }
        if final_url.is_empty() {
            return Err(ExtractorError::extract(
                "Streamtape: failed to build final URL",
            ));
        }

        let mut headers = self.0.base_headers.clone();
        headers.insert("referer".to_string(), url.to_string());

        Ok(ExtractorResult {
            destination_url: final_url,
            request_headers: headers,
            mediaflow_endpoint: "proxy_stream_endpoint",
        })
    }
}
