use async_trait::async_trait;
use base64::{engine::general_purpose, Engine};
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

fn redirect_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"window\.location\.href\s*=\s*'([^']+)").unwrap())
}
fn code_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"json">\["([^"]+)"\]</script>\s*<script\s*src="([^"]+)"#).unwrap()
    })
}
fn luts_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(\[(?:'\W{2}'[,\]]){1,9})").unwrap())
}

pub struct VoeExtractor(pub BaseExtractor);

impl VoeExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for VoeExtractor {
    fn host_name(&self) -> &'static str {
        "Voe"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        self.extract_inner(url, false).await
    }
}

impl VoeExtractor {
    async fn extract_inner(
        &self,
        url: &str,
        redirected: bool,
    ) -> Result<ExtractorResult, ExtractorError> {
        let (html, final_url) = self.0.get_text(url, None).await?;

        // Handle JS redirect.
        if let Some(cap) = redirect_re().captures(&html) {
            if redirected {
                return Err(ExtractorError::extract("VOE: too many redirects"));
            }
            return Box::pin(self.extract_inner(&cap[1], true)).await;
        }

        // Locate obfuscated payload + script URL.
        let cap = code_re()
            .captures(&html)
            .ok_or_else(|| ExtractorError::extract("VOE: obfuscated payload not found"))?;
        let code = cap[1].to_string();
        let script_url = if cap[2].starts_with("http") {
            cap[2].to_string()
        } else {
            let origin = final_url.split('/').take(3).collect::<Vec<_>>().join("/");
            format!("{origin}{}", &cap[2])
        };

        let (script, _) = self.0.get_text(&script_url, None).await?;

        let luts = luts_re()
            .find(&script)
            .ok_or_else(|| ExtractorError::extract("VOE: LUTs not found in script"))?
            .as_str();

        let data = voe_decode(&code, luts)
            .map_err(|e| ExtractorError::extract(format!("VOE: decode failed: {e}")))?;

        let source = data["source"]
            .as_str()
            .ok_or_else(|| ExtractorError::extract("VOE: source not found in decoded data"))?
            .to_string();

        let mut headers = self.0.base_headers.clone();
        headers.insert("referer".to_string(), url.to_string());

        Ok(ExtractorResult {
            destination_url: source,
            request_headers: headers,
            mediaflow_endpoint: "hls_manifest_proxy",
        })
    }
}

/// Port of `VoeExtractor.voe_decode` from Python.
fn voe_decode(ct: &str, luts: &str) -> Result<serde_json::Value, String> {
    // Parse the LUTs array of 2-character strings, e.g. ['xx','yy',...]
    let inner = luts.trim_start_matches('[').trim_end_matches(']');
    let lut_items: Vec<String> = inner
        .split("','")
        .map(|s| s.trim_matches('\'').to_string())
        .collect();

    // ROT-13-like transform
    let mut txt = String::new();
    for ch in ct.chars() {
        let x = ch as u32;
        let new_x = if x > 64 && x < 91 {
            (x - 52) % 26 + 65
        } else if x > 96 && x < 123 {
            (x - 84) % 26 + 97
        } else {
            x
        };
        txt.push(char::from_u32(new_x).unwrap_or(ch));
    }

    // Strip LUT characters (regex-escaped)
    for lut in &lut_items {
        let escaped = regex::escape(lut);
        if let Ok(re) = Regex::new(&escaped) {
            txt = re.replace_all(&txt, "").to_string();
        }
    }

    // base64 decode
    let decoded = general_purpose::STANDARD
        .decode(&txt)
        .map_err(|e| format!("base64 decode 1: {e}"))?;
    let decoded_str = String::from_utf8_lossy(&decoded);

    // Shift chars by -3
    let shifted: String = decoded_str
        .chars()
        .map(|c| char::from_u32(c as u32 - 3).unwrap_or(c))
        .collect();

    // Reverse + base64 decode
    let reversed: String = shifted.chars().rev().collect();
    let final_decoded = general_purpose::STANDARD
        .decode(&reversed)
        .map_err(|e| format!("base64 decode 2: {e}"))?;
    let final_str = String::from_utf8_lossy(&final_decoded);

    serde_json::from_str(&final_str).map_err(|e| format!("JSON parse: {e}"))
}
