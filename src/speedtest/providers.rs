//! Speed test provider implementations.

use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Common types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeedTestConfig {
    pub test_duration_secs: u32,
    /// Location name → test file URL.
    pub test_urls: HashMap<String, String>,
    pub user_info: Option<UserInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub ip: Option<String>,
    pub isp: Option<String>,
    pub country: Option<String>,
}

// ---------------------------------------------------------------------------
// Real-Debrid
// ---------------------------------------------------------------------------

pub fn real_debrid_config() -> SpeedTestConfig {
    let mut rng = rand::rng();
    let r: f64 = rng.random();

    let base_urls: &[(&str, &str)] = &[
        (
            "AMS",
            "https://45.download.real-debrid.com/speedtest/testDefault.rar/",
        ),
        (
            "RBX",
            "https://rbx.download.real-debrid.com/speedtest/test.rar/",
        ),
        (
            "LON1",
            "https://lon1.download.real-debrid.com/speedtest/test.rar/",
        ),
        (
            "HKG1",
            "https://hkg1.download.real-debrid.com/speedtest/test.rar/",
        ),
        (
            "SGP1",
            "https://sgp1.download.real-debrid.com/speedtest/test.rar/",
        ),
        (
            "SGPO1",
            "https://sgpo1.download.real-debrid.com/speedtest/test.rar/",
        ),
        (
            "TYO1",
            "https://tyo1.download.real-debrid.com/speedtest/test.rar/",
        ),
        (
            "LAX1",
            "https://lax1.download.real-debrid.com/speedtest/test.rar/",
        ),
        (
            "TLV1",
            "https://tlv1.download.real-debrid.com/speedtest/test.rar/",
        ),
        (
            "MUM1",
            "https://mum1.download.real-debrid.com/speedtest/test.rar/",
        ),
        (
            "JKT1",
            "https://jkt1.download.real-debrid.com/speedtest/test.rar/",
        ),
        (
            "Cloudflare",
            "https://45.download.real-debrid.cloud/speedtest/testCloudflare.rar/",
        ),
    ];

    let test_urls: HashMap<String, String> = base_urls
        .iter()
        .map(|(name, base)| (name.to_string(), format!("{base}{r:.16}")))
        .collect();

    SpeedTestConfig {
        test_duration_secs: 10,
        test_urls,
        user_info: None,
    }
}

// ---------------------------------------------------------------------------
// AllDebrid
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AllDebridResponse {
    status: String,
    data: Option<AllDebridData>,
}

#[derive(Debug, Deserialize)]
struct AllDebridData {
    ip: Option<String>,
    isp: Option<String>,
    country: Option<String>,
    servers: Vec<AllDebridServer>,
}

#[derive(Debug, Deserialize)]
struct AllDebridServer {
    name: String,
    url: String,
}

pub async fn all_debrid_config(api_key: &str) -> Result<SpeedTestConfig, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://alldebrid.com/internalapi/v4/speedtest")
        .header("user-agent", "MediaFlowProxy/1.0")
        .query(&[
            ("agent", "service"),
            ("version", "1.0-363869a7"),
            ("apikey", api_key),
        ])
        .send()
        .await
        .map_err(|e| format!("AllDebrid request failed: {e}"))?;

    let data: AllDebridResponse = resp
        .json()
        .await
        .map_err(|e| format!("AllDebrid JSON parse error: {e}"))?;

    if data.status != "success" {
        return Err("AllDebrid API returned non-success status".into());
    }

    let inner = data.data.ok_or("AllDebrid: empty data")?;

    let mut rng = rand::rng();
    let r: f64 = rng.random::<f64>() + 1.0;
    let rand_str = format!("{r:.24}").replace('.', "");

    let test_urls: HashMap<String, String> = inner
        .servers
        .iter()
        .map(|s| (s.name.clone(), format!("{}/speedtest/{}", s.url, rand_str)))
        .collect();

    let user_info = UserInfo {
        ip: inner.ip,
        isp: inner.isp,
        country: inner.country,
    };

    Ok(SpeedTestConfig {
        test_duration_secs: 10,
        test_urls,
        user_info: Some(user_info),
    })
}
