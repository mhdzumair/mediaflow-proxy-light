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
    RE.get_or_init(|| Regex::new(r#"<iframe[^>]+src=["']([^"']+)["']"#).unwrap())
}
fn packed_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"eval\(function\(p,a,c,k,e,[dr]\).*?\}\(.*?\)\)").unwrap())
}
fn src_m3u8_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"var\s+src\s*=\s*["']([^"']*\.m3u8[^"']*)["']"#).unwrap())
}
fn generic_m3u8_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"["'](https?://[^"']*\.m3u8[^"']*)["']"#).unwrap())
}

pub struct SportsonlineExtractor(pub BaseExtractor);

impl SportsonlineExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for SportsonlineExtractor {
    fn host_name(&self) -> &'static str {
        "Sportsonline"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let (html, final_url) = self.0.get_text(url, None).await?;

        // Find first iframe.
        let iframe_url = iframe_re()
            .captures(&html)
            .and_then(|c| c.get(1))
            .map(|m| {
                let src = m.as_str();
                if src.starts_with("http") {
                    src.to_string()
                } else {
                    let origin = final_url.split('/').take(3).collect::<Vec<_>>().join("/");
                    format!("{origin}{src}")
                }
            })
            .ok_or_else(|| ExtractorError::extract("Sportsonline: iframe not found"))?;

        let origin = final_url.split('/').take(3).collect::<Vec<_>>().join("/");
        let iframe_origin = iframe_url.split('/').take(3).collect::<Vec<_>>().join("/");

        let mut h = HashMap::new();
        h.insert("referer".to_string(), format!("{origin}/"));
        h.insert("origin".to_string(), origin.clone());

        let (iframe_html, _) = self.0.get_text(&iframe_url, Some(h)).await?;

        // Find packed eval blocks.
        let packed_blocks: Vec<&str> = packed_re()
            .find_iter(&iframe_html)
            .map(|m| m.as_str())
            .collect();

        let search_block = if packed_blocks.len() >= 2 {
            packed_blocks[1]
        } else if !packed_blocks.is_empty() {
            packed_blocks[0]
        } else {
            &iframe_html
        };

        let unpacked = unpack_packed_js(search_block).unwrap_or_else(|| search_block.to_string());

        let m3u8_url = src_m3u8_re()
            .captures(&unpacked)
            .and_then(|c| c.get(1))
            .or_else(|| generic_m3u8_re().captures(&unpacked).and_then(|c| c.get(1)))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| ExtractorError::extract("Sportsonline: m3u8 URL not found"))?;

        let mut headers = self.0.base_headers.clone();
        headers.insert("referer".to_string(), format!("{iframe_origin}/"));
        headers.insert("origin".to_string(), iframe_origin);

        Ok(ExtractorResult {
            destination_url: m3u8_url,
            request_headers: headers,
            mediaflow_endpoint: "hls_manifest_proxy",
        })
    }
}
