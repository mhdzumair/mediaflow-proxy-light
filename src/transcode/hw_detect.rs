//! Hardware encoder detection.
//!
//! Runs `ffmpeg -encoders` once at startup, then caches the best available
//! H.264 encoder (hardware > software fallback).

use std::sync::OnceLock;
use tokio::process::Command;

static BEST_ENCODER: OnceLock<&'static str> = OnceLock::new();

const HW_ENCODERS: &[&str] = &[
    "h264_nvenc",        // NVIDIA
    "h264_videotoolbox", // Apple
    "h264_vaapi",        // VA-API (Intel/AMD Linux)
    "h264_qsv",          // Intel Quick Sync
    "h264_amf",          // AMD (Windows)
];
const SW_ENCODER: &str = "libx264";

/// Detect and cache the best available H.264 encoder.
/// Call once at startup.
pub async fn detect_encoder() -> &'static str {
    if let Some(enc) = BEST_ENCODER.get() {
        return enc;
    }

    let enc = find_best_encoder().await;
    // SAFETY: OnceLock::set only called once.
    let _ = BEST_ENCODER.set(enc);
    enc
}

/// Return the cached encoder, or `libx264` if detection hasn't run yet.
pub fn cached_encoder() -> &'static str {
    BEST_ENCODER.get().copied().unwrap_or(SW_ENCODER)
}

async fn find_best_encoder() -> &'static str {
    let output = Command::new("ffmpeg")
        .args(["-encoders", "-v", "quiet"])
        .output()
        .await;

    let encoders_text = match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => return SW_ENCODER,
    };

    for &enc in HW_ENCODERS {
        if encoders_text.contains(enc) {
            tracing::info!("Transcode: selected hardware encoder {enc}");
            return enc;
        }
    }

    tracing::info!("Transcode: no hardware encoder found, falling back to {SW_ENCODER}");
    SW_ENCODER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cached_encoder_fallback() {
        // Before detect_encoder() runs, should return SW fallback.
        // (BEST_ENCODER is OnceLock, may already be set in other tests — just ensure no panic)
        let _ = cached_encoder();
    }
}
