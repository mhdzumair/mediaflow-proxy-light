use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

fn atob_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"atob\(\s*['"]([A-Za-z0-9+/=]+)['"]\s*\)"#).unwrap())
}
fn file_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"file\s*:\s*['"]([^'"]+)['"]"#).unwrap())
}

/// Pre-encoded login cookie for cinemacity.cc
const COOKIE_B64: &str =
    "ZGxlX3VzZXJfaWQ9MzI3Mjk7IGRsZV9wYXNzd29yZD04OTQxNzFjNmE4ZGFiMThlZTU5NGQ1YzY1MjAwOWEzNTs=";

pub struct CityExtractor(pub BaseExtractor);

impl CityExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }

    /// Decode a base64 string (mirrors JS `atob()`).
    fn atob_fixed(data: &str) -> Option<String> {
        BASE64
            .decode(data.trim())
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Find the first JSON array rooted at a `file:` or `sources:` key.
    fn extract_json_array(decoded: &str) -> Option<String> {
        let start = decoded.find("file:").or_else(|| decoded.find("sources:"))?;
        let bracket_start = decoded[start..].find('[').map(|i| start + i)?;

        let mut depth: i32 = 0;
        for (i, ch) in decoded[bracket_start..].char_indices() {
            match ch {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(decoded[bracket_start..bracket_start + i + 1].to_string());
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Pick a stream URL from the parsed JSON data structure.
    ///
    /// Handles three shapes:
    /// 1. Plain string  → returned as-is.
    /// 2. Flat episode list  `[{file:"…"}, …]`  → index by `episode-1`.
    /// 3. Season/episode tree  `[{title:"Season N", folder:[{file:"…"}]}]`.
    fn pick_stream(data: &Value, season: usize, episode: usize) -> Option<String> {
        match data {
            Value::String(s) => Some(s.clone()),
            Value::Array(arr) => {
                // Flat episode list: all items have a "file" key
                if arr.iter().all(|x| x.get("file").is_some()) {
                    let idx = episode.saturating_sub(1);
                    return arr
                        .get(idx)
                        .or_else(|| arr.first())?
                        .get("file")?
                        .as_str()
                        .map(str::to_string);
                }

                // Season/episode tree
                let season_str = season.to_string();
                let selected = arr
                    .iter()
                    .find(|s| {
                        let title = s
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_lowercase();
                        let pattern = format!(r"(season|s)\s*0*{}\b", season_str);
                        Regex::new(&pattern)
                            .map(|re| re.is_match(&title))
                            .unwrap_or(false)
                    })
                    .or_else(|| arr.first())?;

                let folder = selected.get("folder")?.as_array()?;
                let idx = episode.saturating_sub(1);
                folder
                    .get(idx)
                    .or_else(|| folder.first())?
                    .get("file")?
                    .as_str()
                    .map(str::to_string)
            }
            _ => None,
        }
    }
}

#[async_trait]
impl Extractor for CityExtractor {
    fn host_name(&self) -> &'static str {
        "City"
    }

    async fn extract(
        &self,
        url: &str,
        extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        // Parse season/episode from URL query params or ExtraParams
        let parsed_url = url::Url::parse(url)
            .map_err(|e| ExtractorError::extract(format!("City: invalid URL: {e}")))?;

        let query: HashMap<String, String> = parsed_url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();

        let season: usize = extra
            .raw
            .get("season")
            .or_else(|| query.get("s"))
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);

        let episode: usize = extra
            .raw
            .get("episode")
            .or_else(|| query.get("e"))
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);

        // Strip query string — cinemacity requires the clean movie URL
        let clean_url = format!(
            "{}://{}{}",
            parsed_url.scheme(),
            parsed_url.host_str().unwrap_or(""),
            parsed_url.path()
        );

        // Decode login cookie
        let cookie = BASE64
            .decode(COOKIE_B64)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();

        let mut extra_headers = HashMap::new();
        extra_headers.insert("referer".to_string(), clean_url.clone());
        extra_headers.insert("cookie".to_string(), cookie);

        let (html, _) = self.0.get_text(&clean_url, Some(extra_headers)).await?;

        let doc = Html::parse_document(&html);
        let sel = Selector::parse("script").unwrap();

        let mut file_data: Option<Value> = None;

        'outer: for script in doc.select(&sel) {
            let text = script.text().collect::<String>();
            if !text.contains("atob") {
                continue;
            }
            for cap in atob_re().captures_iter(&text) {
                let encoded = &cap[1];
                let decoded = match Self::atob_fixed(encoded) {
                    Some(d) if !d.is_empty() => d,
                    _ => continue,
                };

                // Primary: JSON array at file: / sources:
                if let Some(raw_json) = Self::extract_json_array(&decoded) {
                    // Unescape backslash sequences then try to parse
                    let cleaned = raw_json.replace("\\/", "/");
                    if let Ok(v) = serde_json::from_str::<Value>(&cleaned) {
                        file_data = Some(v);
                        break 'outer;
                    }
                }

                // Fallback: plain `file: 'url'`
                if let Some(fm) = file_re().captures(&decoded) {
                    file_data = Some(Value::String(fm[1].to_string()));
                    break 'outer;
                }
            }
        }

        let data =
            file_data.ok_or_else(|| ExtractorError::extract("City: no stream found in page"))?;

        let stream_url = Self::pick_stream(&data, season, episode).ok_or_else(|| {
            ExtractorError::extract(format!(
                "City: stream extraction failed (season={season}, episode={episode})"
            ))
        })?;

        let ua = self
            .0
            .base_headers
            .get("user-agent")
            .cloned()
            .unwrap_or_else(|| {
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
                    .to_string()
            });

        let mut result_headers = HashMap::new();
        result_headers.insert("referer".to_string(), clean_url);
        result_headers.insert("user-agent".to_string(), ua);

        Ok(ExtractorResult {
            destination_url: stream_url,
            request_headers: result_headers,
            mediaflow_endpoint: "hls_manifest_proxy",
        })
    }
}
