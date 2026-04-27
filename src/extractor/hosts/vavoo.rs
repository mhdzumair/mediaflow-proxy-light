//! Vavoo extractor — resolves vavoo.to links via the Vavoo auth API.
use async_trait::async_trait;

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::extractor::base::{
    BaseExtractor, ExtraParams, Extractor, ExtractorError, ExtractorResult,
};

const API_UA: &str = "okhttp/4.11.0";
const RESOLVE_UA: &str = "MediaHubMX/2";
const AUTH_TOKEN: &str = "ldCvE092e7gER0rVIajfsXIvRhwlrAzP6_1oEJ4q6HH89QHt24v6NNL_jQJO219hiLOXF2hqEfsUuEWitEIGN4EaHHEHb7Cd7gojc5SQYRFzU3XWo_kMeryAUbcwWnQrnf0-";

pub struct VavooExtractor(pub BaseExtractor);

impl VavooExtractor {
    pub fn new(request_headers: HashMap<String, String>, proxy_url: Option<String>) -> Self {
        Self(BaseExtractor::new(request_headers, proxy_url))
    }
}

#[async_trait]
impl Extractor for VavooExtractor {
    fn host_name(&self) -> &'static str {
        "Vavoo"
    }

    async fn extract(
        &self,
        url: &str,
        _extra: &ExtraParams,
    ) -> Result<ExtractorResult, ExtractorError> {
        let unique_id = &Uuid::new_v4().to_string().replace('-', "")[..16];
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        // Full ping body matching the Python extractor — omitting any field causes
        // lokke.app to reject the request or return a non-JSON response.
        let ping_body = serde_json::json!({
            "token": AUTH_TOKEN,
            "reason": "app-blur",
            "locale": "de",
            "theme": "dark",
            "metadata": {
                "device": {
                    "type": "Handset",
                    "brand": "google",
                    "model": "Nexus",
                    "name": "21081111RG",
                    "uniqueId": unique_id
                },
                "os": { "name": "android", "version": "7.1.2", "abis": ["arm64-v8a"], "host": "android" },
                "app": {
                    "platform": "android",
                    "version": "1.1.0",
                    "buildId": "97215000",
                    "engine": "hbc85",
                    "signatures": ["6e8a975e3cbf07d5de823a760d4c2547f86c1403105020adee5de67ac510999e"],
                    "installer": "com.android.vending"
                },
                "version": { "package": "app.lokke.main", "binary": "1.1.0", "js": "1.1.0" },
                "platform": {
                    "isAndroid": true,
                    "isIOS": false,
                    "isTV": false,
                    "isWeb": false,
                    "isMobile": true,
                    "isWebTV": false,
                    "isElectron": false
                }
            },
            "appFocusTime": 0,
            "playerActive": false,
            "playDuration": 0,
            "devMode": true,
            "hasAddon": true,
            "castConnected": false,
            "package": "app.lokke.main",
            "version": "1.1.0",
            "process": "app",
            "firstAppStart": now_ms - 86400000u64,
            "lastAppStart": now_ms,
            "ipLocation": null,
            "adblockEnabled": false,
            "proxy": {
                "supported": ["ss", "openvpn"],
                "engine": "openvpn",
                "ssVersion": 1,
                "enabled": false,
                "autoServer": true,
                "id": "fi-hel"
            },
            "iap": { "supported": true }
        });

        // Note: no explicit accept-encoding header — reqwest handles gzip automatically
        // via the `gzip` Cargo feature on the reqwest dependency.
        let auth_resp = self
            .0
            .client
            .post("https://www.lokke.app/api/app/ping")
            .header("user-agent", API_UA)
            .header("accept", "application/json")
            .header("content-type", "application/json; charset=utf-8")
            .json(&ping_body)
            .send()
            .await
            .map_err(|e| ExtractorError::Network(e.to_string()))?;

        let auth_data: serde_json::Value = auth_resp
            .json()
            .await
            .map_err(|e| ExtractorError::extract(format!("Vavoo: auth parse error: {e}")))?;

        // Python extractor uses "addonSig", not "signature".
        let signature = auth_data["addonSig"].as_str().ok_or_else(|| {
            ExtractorError::extract(format!(
                "Vavoo: addonSig not found in auth response: {auth_data}"
            ))
        })?;

        // Resolve the Vavoo URL via mediahubmx-resolve.json (matching Python extractor).
        let resolve_body = serde_json::json!({
            "language": "de",
            "region": "AT",
            "url": url,
            "clientVersion": "3.0.2",
        });

        let resolve_resp = self
            .0
            .client
            .post("https://vavoo.to/mediahubmx-resolve.json")
            .header("user-agent", RESOLVE_UA)
            .header("accept", "application/json")
            .header("content-type", "application/json; charset=utf-8")
            .header("mediahubmx-signature", signature)
            .json(&resolve_body)
            .send()
            .await
            .map_err(|e| ExtractorError::Network(e.to_string()))?;

        let resolve_data: serde_json::Value = resolve_resp
            .json()
            .await
            .map_err(|e| ExtractorError::extract(format!("Vavoo: resolve parse error: {e}")))?;

        // Response can be a JSON array or object
        let final_url = if let Some(arr) = resolve_data.as_array() {
            arr.first()
                .and_then(|v| v["url"].as_str())
                .map(String::from)
        } else {
            resolve_data["url"]
                .as_str()
                .or_else(|| resolve_data["data"]["url"].as_str())
                .map(String::from)
        }
        .ok_or_else(|| {
            ExtractorError::extract(format!(
                "Vavoo: no URL found in resolve response: {resolve_data}"
            ))
        })?;

        let mut headers = HashMap::new();
        headers.insert(
            "User-Agent".to_string(),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".to_string(),
        );
        headers.insert("Referer".to_string(), "https://vavoo.to".to_string());
        headers.insert("Origin".to_string(), "https://vavoo.to".to_string());
        headers.insert(
            "X-EasyProxy-Disable-SSL".to_string(),
            "1".to_string(),
        );

        // If the resolved URL is an HLS manifest, route it through hls_manifest_proxy
        // so segment URLs inside the playlist get rewritten.  Raw TS streams go
        // through the stream proxy as-is.
        let endpoint = {
            let lower = final_url.to_lowercase();
            if lower.contains(".m3u8") || lower.contains(".m3u") || lower.contains(".m3u_plus") {
                "hls_manifest_proxy"
            } else {
                "proxy_stream_endpoint"
            }
        };

        Ok(ExtractorResult {
            destination_url: final_url,
            request_headers: headers,
            mediaflow_endpoint: endpoint,
        })
    }
}
