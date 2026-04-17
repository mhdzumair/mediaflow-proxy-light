//! FFmpeg transcoding pipeline.
//!
//! Spawns an `ffmpeg` subprocess that reads from stdin (`pipe:0`) and writes
//! fragmented MP4 or MPEG-TS to stdout (`pipe:1`), returning the output as a
//! `bytes::Bytes` stream.

use bytes::Bytes;
use futures::Stream;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::io::ReaderStream;

/// Return the ffmpeg binary path: $FFMPEG_PATH env var, or fall back to "ffmpeg".
fn ffmpeg_bin() -> String {
    std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_string())
}

use crate::error::{AppError, AppResult};
use crate::transcode::hw_detect::cached_encoder;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TranscodeOptions {
    /// Input format hint (e.g. `"mpegts"`, `"matroska"`). `None` = auto-detect.
    pub input_format: Option<String>,
    /// Seek to this position (seconds) before transcoding.
    pub start_time: Option<f64>,
    /// Output container format.
    pub output_format: OutputFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Fragmented MP4 (fMP4 with `frag_keyframe+empty_moov`).
    FragmentedMp4,
    /// MPEG-TS.
    MpegTs,
}

impl Default for TranscodeOptions {
    fn default() -> Self {
        Self {
            input_format: None,
            start_time: None,
            output_format: OutputFormat::FragmentedMp4,
        }
    }
}

// ---------------------------------------------------------------------------
// Transcode API
// ---------------------------------------------------------------------------

/// Fetch `input_url` and transcode it via ffmpeg, returning output as `Bytes`.
///
/// This is the simplest form — download then transcode.  For large files the
/// streaming form should be used.
pub async fn transcode_url(
    input_url: &str,
    opts: TranscodeOptions,
    request_headers: Vec<(String, String)>,
) -> AppResult<Bytes> {
    let encoder = cached_encoder();

    let (output_format_flag, content_type) = match opts.output_format {
        OutputFormat::FragmentedMp4 => ("mp4", "video/mp4"),
        OutputFormat::MpegTs => ("mpegts", "video/mp2t"),
    };

    let movflags = if opts.output_format == OutputFormat::FragmentedMp4 {
        "frag_keyframe+empty_moov"
    } else {
        ""
    };

    let mut args: Vec<String> = vec!["-hide_banner".into(), "-loglevel".into(), "warning".into()];

    // Seek
    if let Some(ss) = opts.start_time {
        args.push("-ss".into());
        args.push(ss.to_string());
    }

    // Input format hint
    if let Some(ref fmt) = opts.input_format {
        args.push("-f".into());
        args.push(fmt.clone());
    }

    // Pass headers if any
    if !request_headers.is_empty() {
        let header_str: String = request_headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect();
        args.push("-headers".into());
        args.push(header_str);
    }

    args.extend([
        "-i".into(),
        input_url.to_string(),
        "-c:v".into(),
        encoder.to_string(),
        "-c:a".into(),
        "aac".into(),
    ]);

    if !movflags.is_empty() {
        args.push("-movflags".into());
        args.push(movflags.into());
    }

    args.extend(["-f".into(), output_format_flag.to_string(), "pipe:1".into()]);

    tracing::debug!("ffmpeg {}", args.join(" "));

    let output = Command::new(ffmpeg_bin())
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|e| AppError::Transcode(format!("Failed to spawn ffmpeg: {e}")))?;

    if !output.status.success() {
        return Err(AppError::Transcode(format!(
            "ffmpeg exited with status {}",
            output.status
        )));
    }

    Ok(Bytes::from(output.stdout))
}

/// Spawn ffmpeg with `pipe:0` as input and stream its stdout.
///
/// The caller must write input bytes to `input_data` and close stdin.
pub async fn transcode_bytes_to_stream(
    input_data: Bytes,
    opts: TranscodeOptions,
) -> AppResult<impl Stream<Item = Result<Bytes, std::io::Error>>> {
    let encoder = cached_encoder();

    let (output_format_flag, _) = match opts.output_format {
        OutputFormat::FragmentedMp4 => ("mp4", "video/mp4"),
        OutputFormat::MpegTs => ("mpegts", "video/mp2t"),
    };

    let movflags = if opts.output_format == OutputFormat::FragmentedMp4 {
        "frag_keyframe+empty_moov"
    } else {
        ""
    };

    let mut args: Vec<String> = vec!["-hide_banner".into(), "-loglevel".into(), "warning".into()];

    if let Some(ref fmt) = opts.input_format {
        args.push("-f".into());
        args.push(fmt.clone());
    }

    args.extend([
        "-i".into(),
        "pipe:0".into(),
        "-c:v".into(),
        encoder.to_string(),
        "-c:a".into(),
        "aac".into(),
    ]);

    if !movflags.is_empty() {
        args.push("-movflags".into());
        args.push(movflags.into());
    }

    args.extend(["-f".into(), output_format_flag.to_string(), "pipe:1".into()]);

    let mut child = Command::new(ffmpeg_bin())
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::Transcode(format!("Failed to spawn ffmpeg: {e}")))?;

    // Write input to stdin then close it.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&input_data)
            .await
            .map_err(|e| AppError::Transcode(format!("Failed to write to ffmpeg stdin: {e}")))?;
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Transcode("ffmpeg stdout not available".into()))?;

    Ok(ReaderStream::new(stdout))
}
