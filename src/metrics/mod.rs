//! Lightweight request/traffic metrics using atomic counters.
//!
//! `AppMetrics` is created once and stored in actix-web `Data<AppMetrics>`.
//! Every handler increments the relevant counters; `GET /metrics` returns them as JSON.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use actix_web::{web, HttpResponse};
use serde::Serialize;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct AppMetrics {
    pub total_requests: AtomicU64,
    pub active_connections: AtomicU64,
    pub bytes_out: AtomicU64,

    // Per-endpoint counters
    pub proxy_stream_requests: AtomicU64,
    pub hls_requests: AtomicU64,
    pub mpd_requests: AtomicU64,
    pub telegram_requests: AtomicU64,
    pub extractor_requests: AtomicU64,

    /// Unix timestamp (seconds) when the process started.
    pub start_time: AtomicU64,
}

impl AppMetrics {
    pub fn new() -> Arc<Self> {
        let m = Arc::new(Self::default());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        m.start_time.store(now, Ordering::Relaxed);
        m
    }

    pub fn inc_request(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_bytes_out(&self, n: u64) {
        self.bytes_out.fetch_add(n, Ordering::Relaxed);
    }

    pub fn connection_open(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn connection_close(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// JSON response shape
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct MetricsResponse {
    uptime_seconds: u64,
    total_requests: u64,
    active_connections: u64,
    bytes_out: u64,
    bytes_out_human: String,
    proxy_stream_requests: u64,
    hls_requests: u64,
    mpd_requests: u64,
    telegram_requests: u64,
    extractor_requests: u64,
}

fn human_bytes(b: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut val = b as f64;
    let mut i = 0;
    while val >= 1024.0 && i < UNITS.len() - 1 {
        val /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{val:.0} {}", UNITS[i])
    } else {
        format!("{val:.2} {}", UNITS[i])
    }
}

// ---------------------------------------------------------------------------
// HTTP handler
// ---------------------------------------------------------------------------

pub async fn metrics_handler(metrics: web::Data<Arc<AppMetrics>>) -> HttpResponse {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let started = metrics.start_time.load(Ordering::Relaxed);
    let uptime = now.saturating_sub(started);
    let bytes_out = metrics.bytes_out.load(Ordering::Relaxed);

    HttpResponse::Ok().json(MetricsResponse {
        uptime_seconds: uptime,
        total_requests: metrics.total_requests.load(Ordering::Relaxed),
        active_connections: metrics.active_connections.load(Ordering::Relaxed),
        bytes_out,
        bytes_out_human: human_bytes(bytes_out),
        proxy_stream_requests: metrics.proxy_stream_requests.load(Ordering::Relaxed),
        hls_requests: metrics.hls_requests.load(Ordering::Relaxed),
        mpd_requests: metrics.mpd_requests.load(Ordering::Relaxed),
        telegram_requests: metrics.telegram_requests.load(Ordering::Relaxed),
        extractor_requests: metrics.extractor_requests.load(Ordering::Relaxed),
    })
}
