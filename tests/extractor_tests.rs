//! Integration tests for the extractor subsystem.
//!
//! Each test makes real outbound HTTP requests.  A test is **skipped**
//! (passes without asserting anything) when its URL is not configured in
//! `config.toml` under `[test_urls]`.
//!
//! ## Running
//!
//! ```text
//! # Run all extractor tests (skips those without a URL configured):
//! cargo test --test extractor_tests -- --nocapture
//!
//! # Run a single extractor:
//! cargo test --test extractor_tests test_vixcloud -- --nocapture
//! ```
//!
//! ## Configuration
//!
//! Add test URLs to `config.toml` under the `[test_urls]` table:
//!
//! ```toml
//! [test_urls]
//! vixcloud = "https://vixsrc.to/movie/1234"
//! voe      = "https://lauradaydo.com/1234"
//! ```

#![cfg(feature = "extractors")]

use std::collections::HashMap;

use mediaflow_proxy_light::extractor::{base::ExtraParams, factory::get_extractor};

// ---------------------------------------------------------------------------
// Valid endpoint values (mirrors Python VALID_ENDPOINTS)
// ---------------------------------------------------------------------------

const VALID_ENDPOINTS: &[&str] = &[
    "proxy_stream_endpoint",
    "hls_manifest_proxy",
    "hls_key_proxy",
    "mpd_manifest_proxy",
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Load test config from config.toml (without going through the full app Config).
fn load_test_config() -> config::Config {
    config::Config::builder()
        .add_source(
            config::File::with_name(&format!("{}/config", env!("CARGO_MANIFEST_DIR")))
                .required(false),
        )
        .build()
        .unwrap_or_default()
}

/// Look up a test URL from `config.toml` → `[test_urls].<name>`.
/// Returns `None` when the key is absent or empty, causing the test to skip.
fn test_url(name: &str) -> Option<String> {
    load_test_config()
        .get_string(&format!("test_urls.{}", name.to_lowercase()))
        .ok()
        .filter(|v| !v.is_empty())
}

/// Resolve the proxy URL from config.toml `[proxy]` section.
fn test_proxy_url() -> Option<String> {
    let cfg = load_test_config();
    let all_proxy: bool = cfg.get_bool("proxy.all_proxy").unwrap_or(false);
    if all_proxy {
        cfg.get_string("proxy.proxy_url")
            .ok()
            .filter(|v| !v.is_empty())
    } else {
        None
    }
}

/// Core validation: create extractor → extract → check response contract.
async fn assert_extractor(host: &str, url: &str) {
    let proxy_url = test_proxy_url();
    let extractor = get_extractor(host, HashMap::new(), proxy_url)
        .unwrap_or_else(|e| panic!("[{host}] failed to create extractor: {e}"));

    let result = extractor
        .extract(url, &ExtraParams::default())
        .await
        .unwrap_or_else(|e| panic!("[{host}] extraction failed for {url}: {e}"));

    assert!(
        !result.destination_url.is_empty(),
        "[{host}] destination_url is empty"
    );
    assert!(
        result.destination_url.starts_with("http"),
        "[{host}] destination_url must start with 'http', got: {}",
        result.destination_url
    );
    assert!(
        VALID_ENDPOINTS.contains(&result.mediaflow_endpoint),
        "[{host}] unknown mediaflow_endpoint '{}', expected one of {VALID_ENDPOINTS:?}",
        result.mediaflow_endpoint
    );

    println!(
        "[{host}] OK  endpoint={}  url={}…",
        result.mediaflow_endpoint,
        &result.destination_url[..result.destination_url.len().min(80)]
    );
}

// ---------------------------------------------------------------------------
// Factory smoke tests (no network required)
// ---------------------------------------------------------------------------

#[test]
fn test_factory_all_hosts_registered() {
    let known_hosts = [
        "city",
        "doodstream",
        "fastream",
        "filelions",
        "filemoon",
        "f16px",
        "gupload",
        "livetv",
        "lulustream",
        "maxstream",
        "mixdrop",
        "okru",
        "sportsonline",
        "streamtape",
        "streamwish",
        "supervideo",
        "turbovidplay",
        "uqload",
        "vavoo",
        "vidmoly",
        "vidoza",
        "vixcloud",
        "voe",
    ];
    for host in known_hosts {
        get_extractor(host, HashMap::new(), None)
            .unwrap_or_else(|e| panic!("factory rejected known host '{host}': {e}"));
    }
}

#[test]
fn test_factory_unknown_host_errors() {
    let result = get_extractor("nonexistent_host_xyz", HashMap::new(), None);
    assert!(result.is_err(), "expected Err for unknown host");
}

#[test]
fn test_factory_case_insensitive() {
    // Keys are stored in lower-case; the factory must normalise the input.
    get_extractor("DoodStream", HashMap::new(), None).expect("case-insensitive match failed");
    get_extractor("STREAMTAPE", HashMap::new(), None).expect("case-insensitive match failed");
    get_extractor("VixCloud", HashMap::new(), None).expect("case-insensitive match failed");
}

// ---------------------------------------------------------------------------
// Per-extractor integration tests
// ---------------------------------------------------------------------------

macro_rules! extractor_test {
    ($fn_name:ident, $host:literal) => {
        #[tokio::test]
        async fn $fn_name() {
            let Some(url) = test_url($host) else {
                println!(
                    "[{}] SKIPPED — test_urls.{} not set in config.toml",
                    $host, $host
                );
                return;
            };
            assert_extractor($host, &url).await;
        }
    };
}

extractor_test!(test_city, "city");
extractor_test!(test_doodstream, "doodstream");
extractor_test!(test_fastream, "fastream");
extractor_test!(test_filelions, "filelions");
extractor_test!(test_filemoon, "filemoon");
extractor_test!(test_f16px, "f16px");
extractor_test!(test_gupload, "gupload");
extractor_test!(test_livetv, "livetv");
extractor_test!(test_lulustream, "lulustream");
extractor_test!(test_maxstream, "maxstream");
extractor_test!(test_mixdrop, "mixdrop");
extractor_test!(test_okru, "okru");
extractor_test!(test_sportsonline, "sportsonline");
extractor_test!(test_streamtape, "streamtape");
extractor_test!(test_streamwish, "streamwish");
extractor_test!(test_supervideo, "supervideo");
extractor_test!(test_turbovidplay, "turbovidplay");
extractor_test!(test_uqload, "uqload");
extractor_test!(test_vavoo, "vavoo");
extractor_test!(test_vidmoly, "vidmoly");
extractor_test!(test_vidoza, "vidoza");
extractor_test!(test_vixcloud, "vixcloud");
extractor_test!(test_voe, "voe");
