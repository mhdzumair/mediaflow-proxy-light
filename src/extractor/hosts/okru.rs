use async_trait::async_trait;
use scraper::{Html, Selector};
use std::collections::HashMap;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

pub struct OkruExtractor(pub BaseExtractor);

impl OkruExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for OkruExtractor {
    fn host_name(&self) -> &'static str {
        "Okru"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let (html, _) = self.0.get_text(url, None).await?;

        let doc = Html::parse_document(&html);
        let sel = Selector::parse("div[data-module='OKVideo']").unwrap();

        let data_options = doc
            .select(&sel)
            .next()
            .and_then(|el| el.value().attr("data-options"))
            .ok_or_else(|| ExtractorError::extract("Okru: OKVideo div not found"))?;

        let data: serde_json::Value = serde_json::from_str(data_options)
            .map_err(|e| ExtractorError::extract(format!("Okru: invalid JSON: {e}")))?;

        let metadata_str = data["flashvars"]["metadata"]
            .as_str()
            .ok_or_else(|| ExtractorError::extract("Okru: metadata not found"))?;

        let metadata: serde_json::Value = serde_json::from_str(metadata_str)
            .map_err(|e| ExtractorError::extract(format!("Okru: metadata parse error: {e}")))?;

        let final_url = metadata["hlsMasterPlaylistUrl"]
            .as_str()
            .or_else(|| metadata["hlsManifestUrl"].as_str())
            .or_else(|| metadata["ondemandHls"].as_str())
            .ok_or_else(|| ExtractorError::extract("Okru: HLS URL not found in metadata"))?
            .to_string();

        let mut headers = self.0.base_headers.clone();
        headers.insert("referer".to_string(), url.to_string());

        Ok(ExtractorResult {
            destination_url: final_url,
            request_headers: headers,
            mediaflow_endpoint: "hls_manifest_proxy",
        })
    }
}
