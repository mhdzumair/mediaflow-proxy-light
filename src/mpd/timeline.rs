//! Duration parsing and segment generation for DASH manifests.
//!
//! Ports the timing utilities from `mpd_utils.py`:
//! - [`parse_duration`] — ISO 8601 duration → seconds
//! - [`preprocess_timeline`] — expand `<S>` entries with repeat counts
//! - [`generate_live_segments`] — segment list for dynamic MPD streams
//! - [`generate_vod_segments`] — segment list for static (VOD) MPD streams

use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// ISO 8601 duration parsing
// ---------------------------------------------------------------------------

/// Parse an ISO 8601 duration string (e.g. `PT30S`, `PT1H30M`, `P1DT2H3M4S`) into seconds.
pub fn parse_duration(s: &str) -> f64 {
    let mut years = 0.0f64;
    let mut months = 0.0f64;
    let mut days = 0.0f64;
    let mut hours = 0.0f64;
    let mut minutes = 0.0f64;
    let mut secs = 0.0f64;

    if s.is_empty() {
        return 0.0;
    }

    let s = s.trim();
    if !s.starts_with('P') {
        return 0.0;
    }
    let s = &s[1..]; // strip leading 'P'

    let (date_part, time_part) = if let Some(t_pos) = s.find('T') {
        (&s[..t_pos], Some(&s[t_pos + 1..]))
    } else {
        (s, None)
    };

    // Parse date designators: Y M D
    let mut rest = date_part;
    while !rest.is_empty() {
        if let Some((val, rem, unit)) = next_value(rest) {
            match unit {
                'Y' => years = val,
                'M' => months = val,
                'D' => days = val,
                _ => {}
            }
            rest = rem;
        } else {
            break;
        }
    }

    // Parse time designators: H M S
    if let Some(t) = time_part {
        let mut rest = t;
        while !rest.is_empty() {
            if let Some((val, rem, unit)) = next_value(rest) {
                match unit {
                    'H' => hours = val,
                    'M' => minutes = val,
                    'S' => secs = val,
                    _ => {}
                }
                rest = rem;
            } else {
                break;
            }
        }
    }

    years * 365.0 * 86400.0
        + months * 30.0 * 86400.0
        + days * 86400.0
        + hours * 3600.0
        + minutes * 60.0
        + secs
}

/// Parse the leading numeric value and its designator letter from a string slice.
/// Returns `(value, remaining, designator)` or `None` if parsing fails.
fn next_value(s: &str) -> Option<(f64, &str, char)> {
    // Find the designator character (non-digit, non-dot)
    let end = s
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit() && *c != '.')
        .map(|(i, _)| i)?;
    let num_str = &s[..end];
    let designator = s[end..].chars().next()?;
    let val: f64 = num_str.parse().ok()?;
    Some((val, &s[end + designator.len_utf8()..], designator))
}

// ---------------------------------------------------------------------------
// Preprocessed timeline entry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TimelineEntry {
    pub number: u64,
    /// Start time in MPD timescale units.
    pub time: u64,
    /// Duration in MPD timescale units.
    pub duration: u64,
    /// Absolute start time as Unix timestamp (seconds), for live streams.
    pub start_unix: Option<f64>,
    /// Absolute end time as Unix timestamp (seconds), for live streams.
    pub end_unix: Option<f64>,
}

// ---------------------------------------------------------------------------
// Preprocess SegmentTimeline S-elements
// ---------------------------------------------------------------------------

/// Expand `<S>` elements (which may have `@r` repeat counts) into individual
/// [`TimelineEntry`] items, mirrors `preprocess_timeline()` in Python.
pub fn preprocess_timeline(
    s_elements: &[crate::mpd::parser::SElement],
    start_number: u64,
    period_start_unix: f64, // seconds since epoch
    presentation_time_offset: u64,
    timescale: u64,
) -> Vec<TimelineEntry> {
    let mut entries = Vec::new();
    let mut current_time: u64 = 0;
    let mut segment_number = start_number;

    for s in s_elements {
        let duration: u64 = s.d.parse().unwrap_or(0);
        let repeat: u64 = s.r.as_deref().and_then(|v| v.parse().ok()).unwrap_or(0);
        let start_time: u64 =
            s.t.as_deref()
                .and_then(|v| v.parse().ok())
                .unwrap_or(current_time);

        let mut t = start_time;
        for _ in 0..=repeat {
            let offset_sec = (t.saturating_sub(presentation_time_offset)) as f64 / timescale as f64;
            let start_unix = period_start_unix + offset_sec;
            let end_unix = start_unix + duration as f64 / timescale as f64;

            entries.push(TimelineEntry {
                number: segment_number,
                time: t,
                duration,
                start_unix: Some(start_unix),
                end_unix: Some(end_unix),
            });

            t += duration;
            segment_number += 1;
        }

        current_time = t;
    }

    entries
}

// ---------------------------------------------------------------------------
// Live segment generation
// ---------------------------------------------------------------------------

/// Generate live segment entries based on the current wall-clock time and the
/// MPD's `timeShiftBufferDepth`, mirrors `generate_live_segments()` in Python.
pub fn generate_live_segments(
    availability_start_unix: f64,
    time_shift_buffer_depth_sec: f64,
    segment_duration_sec: f64,
    start_number: u64,
    duration_mpd_timescale: Option<u64>,
    presentation_time_offset: u64,
) -> Vec<TimelineEntry> {
    if segment_duration_sec <= 0.0 {
        return Vec::new();
    }

    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);

    let segment_count = (time_shift_buffer_depth_sec / segment_duration_sec).ceil() as u64;

    let elapsed_segments = ((now_unix - availability_start_unix) / segment_duration_sec) as u64;
    let earliest = (start_number + elapsed_segments)
        .saturating_sub(segment_count)
        .max(start_number);

    let dur_ts = duration_mpd_timescale.unwrap_or(0);

    (earliest..earliest + segment_count)
        .map(|number| {
            let start_unix =
                availability_start_unix + (number - start_number) as f64 * segment_duration_sec;
            let time = presentation_time_offset + (number - start_number) * dur_ts;

            TimelineEntry {
                number,
                time,
                duration: dur_ts,
                start_unix: Some(start_unix),
                end_unix: Some(start_unix + segment_duration_sec),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// VOD segment generation
// ---------------------------------------------------------------------------

/// Generate segment entries for a static (VOD) MPD using a fixed `@duration`.
pub fn generate_vod_segments(
    total_duration_sec: f64,
    segment_duration_ts: u64,
    timescale: u64,
    start_number: u64,
) -> Vec<TimelineEntry> {
    if timescale == 0 || segment_duration_ts == 0 {
        return Vec::new();
    }
    let segment_duration_sec = segment_duration_ts as f64 / timescale as f64;
    let segment_count =
        (total_duration_sec * timescale as f64 / segment_duration_ts as f64).ceil() as u64;

    (0..segment_count)
        .map(|i| TimelineEntry {
            number: start_number + i,
            time: 0,
            duration: segment_duration_ts,
            start_unix: None,
            end_unix: Some(segment_duration_sec), // relative duration only
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Parse ISO 8601 datetime string to Unix timestamp
// ---------------------------------------------------------------------------

/// Parse an ISO 8601 datetime string (e.g. "2024-01-01T00:00:00Z") to a Unix
/// timestamp in seconds. Returns `None` if parsing fails.
pub fn parse_datetime_to_unix(s: &str) -> Option<f64> {
    // Normalise "Z" to "+00:00" for the time crate
    let s = s.replace('Z', "+00:00");
    time::OffsetDateTime::parse(&s, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|dt| dt.unix_timestamp() as f64)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert!((parse_duration("PT30S") - 30.0).abs() < 1e-9);
        assert!((parse_duration("PT1H30M") - 5400.0).abs() < 1e-9);
        assert!((parse_duration("P1DT2H3M4S") - 93784.0).abs() < 1e-9);
        assert!((parse_duration("PT0S") - 0.0).abs() < 1e-9);
        assert!((parse_duration("PT2M") - 120.0).abs() < 1e-9);
    }

    #[test]
    fn test_parse_datetime_to_unix() {
        // Known timestamp: 2021-01-01T00:00:00Z = 1609459200
        let ts = parse_datetime_to_unix("2021-01-01T00:00:00Z");
        assert!(ts.is_some());
        assert!((ts.unwrap() - 1609459200.0).abs() < 1.0);
    }
}
