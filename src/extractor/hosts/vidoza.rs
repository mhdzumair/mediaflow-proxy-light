use async_trait::async_trait;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

fn jwplayer_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(
        r#"(?:file|src)\s*[:=,]\s*["'](?P<url>https?://[^"']+)["'][^}>\]]{0,200}(?:res|label)\s*[:=]"#,
    ).unwrap())
}

pub struct VidozaExtractor(pub BaseExtractor);

impl VidozaExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for VidozaExtractor {
    fn host_name(&self) -> &'static str {
        "Vidoza"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let origin = url.split('/').take(3).collect::<Vec<_>>().join("/");

        let mut headers = HashMap::new();
        headers.insert("referer".to_string(), format!("{origin}/"));
        headers.insert(
            "user-agent".to_string(),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
                .to_string(),
        );
        headers.insert("accept".to_string(), "*/*".to_string());
        headers.insert("accept-language".to_string(), "en-US,en;q=0.9".to_string());

        let (html, _) = self.0.get_text(url, Some(headers.clone())).await?;

        let video_url = jwplayer_re()
            .captures(&html)
            .and_then(|c| c.name("url"))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| ExtractorError::extract("Vidoza: video URL not found"))?;

        let final_url = if video_url.starts_with("//") {
            format!("https:{video_url}")
        } else {
            video_url
        };

        Ok(ExtractorResult {
            destination_url: final_url,
            request_headers: headers,
            mediaflow_endpoint: "proxy_stream_endpoint",
        })
    }
}
