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

        // Resolve the Vavoo URL.
        let resolve_resp = self
            .0
            .client
            .get(url)
            .header("user-agent", RESOLVE_UA)
            .header("referer", "https://vavoo.to/")
            .header("x-signature-v2", signature)
            .send()
            .await
            .map_err(|e| ExtractorError::Network(e.to_string()))?;

        let final_url = resolve_resp.url().to_string();

        let mut headers = HashMap::new();
        headers.insert("user-agent".to_string(), RESOLVE_UA.to_string());
        headers.insert("referer".to_string(), "https://vavoo.to/".to_string());
        headers.insert("x-signature-v2".to_string(), signature.to_string());

        // If the resolved URL is an HLS manifest, route it through hls_manifest_proxy
        // so segment URLs inside the playlist get rewritten.  Raw TS streams go
        // through the stream proxy as-is.
        let path = resolve_resp.url().path().to_lowercase();
        let endpoint =
            if path.ends_with(".m3u8") || path.ends_with(".m3u") || path.ends_with(".m3u_plus") {
                "hls_manifest_proxy"
            } else {
                "proxy_stream_endpoint"
            };

        Ok(ExtractorResult {
            destination_url: final_url,
            request_headers: headers,
            mediaflow_endpoint: endpoint,
        })
    }
}
