//! Parse an [`MpdDocument`] into typed [`ParsedMpd`] / [`MpdProfile`] structures
//! and render HLS master + media playlists from them.
//!
//! Ports `parse_mpd_dict`, `build_hls`, and `build_hls_playlist` from
//! `mpd_processor.py` / `mpd_utils.py`.

use std::collections::HashMap;
use tracing::warn;

use crate::hls::skip_filter::{SkipRange, SkipSegmentFilter};
use crate::mpd::parser::{
    AdaptationSet, ContentProtection, MpdDocument, Representation, SegmentBase, SegmentList,
    SegmentTemplate,
};
use crate::mpd::segment::{expand_template, resolve_url};
use crate::mpd::timeline::{
    generate_live_segments, generate_vod_segments, parse_datetime_to_unix, parse_duration,
    preprocess_timeline, TimelineEntry,
};

// ---------------------------------------------------------------------------
// Parsed MPD types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ParsedMpd {
    pub is_live: bool,
    pub profiles: Vec<MpdProfile>,
    pub minimum_update_period_sec: Option<f64>,
    pub time_shift_buffer_depth_sec: f64,
    pub availability_start_unix: Option<f64>,
    pub period_start_sec: f64,
    pub media_presentation_duration_sec: Option<f64>,
    pub drm_info: DrmInfo,
}

#[derive(Debug, Clone, Default)]
pub struct DrmInfo {
    pub is_drm_protected: bool,
    pub drm_system: Option<String>,
    pub key_id: Option<String>,
    pub la_url: Option<String>,
    pub pssh: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MpdProfile {
    /// Globally unique profile identifier used in playlist URLs: `{adapt_id}_{rep_id}_{bandwidth}`.
    pub id: String,
    /// Raw representation ID from the MPD XML (used for `$RepresentationID$` template expansion).
    pub rep_id: String,
    pub mime_type: String,
    pub codecs: String,
    pub bandwidth: u64,
    pub lang: Option<String>,
    /// For video profiles.
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub sar: String,
    /// For audio profiles.
    pub audio_sampling_rate: Option<String>,
    pub channels: String,

    pub init_url: Option<String>,
    pub init_range: Option<String>,
    pub segments: Vec<MpdSegment>,

    pub segment_template_start_number: u64,
    pub segment_template_start_number_explicit: bool,
    pub segment_template_timescale: u64,
    pub nominal_duration_mpd_timescale: Option<u64>,
}

impl MpdProfile {
    pub fn is_video(&self) -> bool {
        self.mime_type.contains("video")
    }
    pub fn is_audio(&self) -> bool {
        self.mime_type.contains("audio")
    }
}

#[derive(Debug, Clone)]
pub struct MpdSegment {
    /// Absolute URL for this segment.
    pub media: String,
    pub number: u64,
    /// Duration in seconds (used for `#EXTINF`).
    pub extinf: f64,
    /// Start time in MPD timescale units (for live sequence calculation).
    pub time: Option<u64>,
    /// Duration in MPD timescale units.
    pub duration_mpd_timescale: Option<u64>,
    /// Absolute start time (Unix seconds).
    pub start_unix: Option<f64>,
    /// ISO 8601 datetime string for EXT-X-PROGRAM-DATE-TIME.
    pub program_date_time: Option<String>,
    /// Byte range for SegmentBase/SegmentList.
    pub media_range: Option<String>,
    pub init_range: Option<String>,
    pub index_range: Option<String>,
}

// ---------------------------------------------------------------------------
// MPD → ParsedMpd
// ---------------------------------------------------------------------------

/// Parse an [`MpdDocument`] into a [`ParsedMpd`], optionally restricting
/// segment parsing to a single `parse_segment_profile_id`.
pub fn parse_mpd_document(
    doc: &MpdDocument,
    mpd_url: &str,
    parse_segment_profile_id: Option<&str>,
) -> ParsedMpd {
    let is_live = doc
        .stream_type
        .as_deref()
        .map(|t| t.eq_ignore_ascii_case("dynamic"))
        .unwrap_or(false);

    let media_presentation_duration_sec = doc
        .media_presentation_duration
        .as_deref()
        .map(parse_duration);

    let minimum_update_period_sec = doc.minimum_update_period.as_deref().map(parse_duration);

    let time_shift_buffer_depth_sec = doc
        .time_shift_buffer_depth
        .as_deref()
        .map(parse_duration)
        .unwrap_or(120.0); // default 2 minutes

    let availability_start_unix = doc
        .availability_start_time
        .as_deref()
        .and_then(parse_datetime_to_unix);

    // Collect all content protection elements for DRM info
    let mut all_cp: Vec<ContentProtection> = Vec::new();
    for period in &doc.periods {
        for ad in &period.adaptation_sets {
            for cp in &ad.content_protection {
                all_cp.push(cp.clone());
            }
            for rep in &ad.representations {
                for cp in &rep.content_protection {
                    all_cp.push(cp.clone());
                }
            }
        }
    }
    let drm_info = extract_drm_info(&all_cp, mpd_url);

    let mut profiles = Vec::new();
    for period in &doc.periods {
        let period_start_sec = period.start.as_deref().map(parse_duration).unwrap_or(0.0);

        let period_avail_start = availability_start_unix.unwrap_or(0.0) + period_start_sec;

        for adaptation in &period.adaptation_sets {
            for representation in &adaptation.representations {
                if let Some(profile) = parse_representation(
                    doc,
                    representation,
                    adaptation,
                    mpd_url,
                    is_live,
                    period_avail_start,
                    period_start_sec,
                    time_shift_buffer_depth_sec,
                    media_presentation_duration_sec,
                    parse_segment_profile_id,
                ) {
                    profiles.push(profile);
                }
            }
        }
    }

    ParsedMpd {
        is_live,
        profiles,
        minimum_update_period_sec,
        time_shift_buffer_depth_sec,
        availability_start_unix,
        period_start_sec: 0.0,
        media_presentation_duration_sec,
        drm_info,
    }
}

// ---------------------------------------------------------------------------
// Representation → MpdProfile
// ---------------------------------------------------------------------------

fn parse_representation(
    _doc: &MpdDocument,
    rep: &Representation,
    adaptation: &AdaptationSet,
    mpd_url: &str,
    is_live: bool,
    period_avail_start_unix: f64,
    period_start_sec: f64,
    time_shift_buffer_depth_sec: f64,
    media_presentation_duration_sec: Option<f64>,
    parse_segment_profile_id: Option<&str>,
) -> Option<MpdProfile> {
    // MIME type - prefer representation, fall back to adaptation set
    let mime_type = rep
        .mime_type
        .as_deref()
        .or(adaptation.mime_type.as_deref())
        .unwrap_or_else(|| {
            // Guess from codecs
            rep.codecs
                .as_deref()
                .or(adaptation.codecs.as_deref())
                .map(|c| {
                    if c.contains("avc") || c.contains("hvc") || c.contains("vp") {
                        "video/mp4"
                    } else {
                        "audio/mp4"
                    }
                })
                .unwrap_or("video/mp4")
        })
        .to_string();

    // Only handle video/audio
    if !mime_type.contains("video") && !mime_type.contains("audio") {
        return None;
    }

    // raw rep.id from XML — used for $RepresentationID$ template expansion
    let rep_id = rep.id.as_deref().unwrap_or("0").to_string();
    // globally unique profile ID = "{adapt_id}_{rep_id}_{bandwidth}"
    // Multiple reps within the same AdaptationSet can share rep_id (e.g. same quality tier,
    // different codec), so we disambiguate with bandwidth.
    let adapt_id = adaptation.id.as_deref().unwrap_or("0");
    // bandwidth isn't parsed yet, compute it here for the ID
    let bandwidth_for_id: u64 = rep
        .bandwidth
        .as_deref()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let id = format!("{adapt_id}_{rep_id}_{bandwidth_for_id}");

    let codecs = rep
        .codecs
        .as_deref()
        .or(adaptation.codecs.as_deref())
        .unwrap_or("")
        .to_string();

    // prefer rep bandwidth; fall back to adaptation bandwidth (already computed above for id)
    let bandwidth: u64 = rep
        .bandwidth
        .as_deref()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            adaptation
                .bandwidth
                .as_deref()
                .and_then(|v| v.parse().ok())
                .unwrap_or(bandwidth_for_id)
        });

    let lang = rep.lang.clone().or_else(|| adaptation.lang.clone());

    // Video-specific
    let width: u32 = rep
        .width
        .as_deref()
        .or(adaptation.width.as_deref())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let height: u32 = rep
        .height
        .as_deref()
        .or(adaptation.height.as_deref())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let frame_rate_str = rep
        .frame_rate
        .as_deref()
        .or(adaptation.max_frame_rate.as_deref())
        .or(adaptation.frame_rate.as_deref())
        .unwrap_or("30000/1001");

    let frame_rate = parse_frame_rate(frame_rate_str);

    let sar = rep.sar.as_deref().unwrap_or("1:1").to_string();

    // Audio-specific
    let audio_sampling_rate = rep
        .audio_sampling_rate
        .clone()
        .or_else(|| adaptation.audio_sampling_rate.clone());

    let channels = rep
        .audio_channel_config
        .as_ref()
        .and_then(|a| a.value.as_deref())
        .unwrap_or("2")
        .to_string();

    // Segment template metadata (without generating segments yet)
    let seg_tmpl = rep
        .segment_template
        .as_ref()
        .or(adaptation.segment_template.as_ref());

    let (start_number, start_number_explicit, timescale) = if let Some(t) = seg_tmpl {
        let sn: u64 = t
            .start_number
            .as_deref()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let explicit = t.start_number.is_some();
        let ts: u64 = t
            .timescale
            .as_deref()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        (sn, explicit, ts)
    } else {
        (1, false, 1)
    };

    // Base URL (may be a relative path prefix inside the representation)
    let base_url_str = rep
        .base_url
        .as_ref()
        .and_then(|b| b.value.as_deref())
        .unwrap_or("");

    // Compute initUrl from template / list / base even when not fully parsing segments
    // Use rep_id (raw XML id) for $RepresentationID$ template expansion, not the unique profile id.
    let (init_url, init_range) =
        compute_init_url(rep, adaptation, mpd_url, base_url_str, &rep_id, bandwidth);

    let mut profile = MpdProfile {
        id: id.clone(),
        rep_id: rep_id.clone(),
        mime_type,
        codecs,
        bandwidth,
        lang,
        width,
        height,
        frame_rate,
        sar,
        audio_sampling_rate,
        channels,
        init_url,
        init_range,
        segments: Vec::new(),
        segment_template_start_number: start_number,
        segment_template_start_number_explicit: start_number_explicit,
        segment_template_timescale: timescale,
        nominal_duration_mpd_timescale: None,
    };

    // Only generate segments when explicitly requested (performance optimisation)
    if parse_segment_profile_id.is_none() || parse_segment_profile_id == Some(&id) {
        profile.segments = generate_segments(
            &profile,
            rep,
            adaptation,
            mpd_url,
            base_url_str,
            is_live,
            period_avail_start_unix,
            period_start_sec,
            time_shift_buffer_depth_sec,
            media_presentation_duration_sec,
            start_number,
            timescale,
        );

        // Derive nominal duration for live sequence calculation
        if profile.nominal_duration_mpd_timescale.is_none() {
            profile.nominal_duration_mpd_timescale = resolve_nominal_duration(&profile.segments);
        }
    }

    Some(profile)
}

// ---------------------------------------------------------------------------
// Segment generation
// ---------------------------------------------------------------------------

fn generate_segments(
    profile: &MpdProfile,
    rep: &Representation,
    adaptation: &AdaptationSet,
    mpd_url: &str,
    base_url_str: &str,
    is_live: bool,
    period_avail_start_unix: f64,
    _period_start_sec: f64,
    time_shift_buffer_depth_sec: f64,
    media_presentation_duration_sec: Option<f64>,
    start_number: u64,
    timescale: u64,
) -> Vec<MpdSegment> {
    let seg_tmpl = rep
        .segment_template
        .as_ref()
        .or(adaptation.segment_template.as_ref());
    let seg_list = rep
        .segment_list
        .as_ref()
        .or(adaptation.segment_list.as_ref());
    let seg_base = rep.segment_base.as_ref();

    if let Some(tmpl) = seg_tmpl {
        generate_from_template(
            profile,
            tmpl,
            mpd_url,
            base_url_str,
            is_live,
            period_avail_start_unix,
            time_shift_buffer_depth_sec,
            media_presentation_duration_sec,
            start_number,
            timescale,
        )
    } else if let Some(list) = seg_list {
        generate_from_list(list, rep, mpd_url, timescale)
    } else if let Some(base) = seg_base {
        generate_from_base(base, rep, mpd_url, media_presentation_duration_sec)
    } else {
        Vec::new()
    }
}

fn generate_from_template(
    profile: &MpdProfile,
    tmpl: &SegmentTemplate,
    mpd_url: &str,
    base_url_str: &str,
    is_live: bool,
    period_avail_start_unix: f64,
    time_shift_buffer_depth_sec: f64,
    media_presentation_duration_sec: Option<f64>,
    start_number: u64,
    timescale: u64,
) -> Vec<MpdSegment> {
    let _media_tmpl = match tmpl.media.as_deref() {
        Some(m) => m,
        None => return Vec::new(),
    };
    let presentation_time_offset: u64 = tmpl
        .presentation_time_offset
        .as_deref()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // SegmentTimeline mode
    if let Some(timeline) = &tmpl.segment_timeline {
        let entries = preprocess_timeline(
            &timeline.segments,
            start_number,
            period_avail_start_unix,
            presentation_time_offset,
            timescale,
        );
        return entries
            .iter()
            .map(|e| timeline_entry_to_segment(e, tmpl, profile, mpd_url, base_url_str, timescale))
            .collect();
    }

    // Fixed @duration mode
    if let Some(duration_str) = &tmpl.duration {
        let duration_ts: u64 = duration_str.parse().unwrap_or(0);
        if duration_ts == 0 {
            return Vec::new();
        }
        let segment_duration_sec = duration_ts as f64 / timescale as f64;

        let entries = if is_live {
            generate_live_segments(
                period_avail_start_unix,
                time_shift_buffer_depth_sec,
                segment_duration_sec,
                start_number,
                Some(duration_ts),
                presentation_time_offset,
            )
        } else {
            let total_dur = media_presentation_duration_sec.unwrap_or(0.0);
            generate_vod_segments(total_dur, duration_ts, timescale, start_number)
        };

        return entries
            .iter()
            .map(|e| timeline_entry_to_segment(e, tmpl, profile, mpd_url, base_url_str, timescale))
            .collect();
    }

    Vec::new()
}

fn timeline_entry_to_segment(
    entry: &TimelineEntry,
    tmpl: &SegmentTemplate,
    profile: &MpdProfile,
    mpd_url: &str,
    base_url_str: &str,
    timescale: u64,
) -> MpdSegment {
    let media_tmpl = tmpl.media.as_deref().unwrap_or("");
    let expanded = expand_template(
        media_tmpl,
        &profile.rep_id, // use raw rep.id for $RepresentationID$ expansion
        profile.bandwidth,
        entry.number,
        if entry.time > 0 {
            Some(entry.time)
        } else {
            None
        },
    );
    let media_path = if !base_url_str.is_empty() {
        format!("{base_url_str}{expanded}")
    } else {
        expanded
    };
    let media_url = resolve_url(mpd_url, &media_path);

    let extinf = if let (Some(start), Some(end)) = (entry.start_unix, entry.end_unix) {
        end - start
    } else {
        entry.duration as f64 / timescale.max(1) as f64
    };

    let program_date_time = entry.start_unix.and_then(unix_to_iso8601);

    MpdSegment {
        media: media_url,
        number: entry.number,
        extinf,
        time: if entry.time > 0 {
            Some(entry.time)
        } else {
            None
        },
        duration_mpd_timescale: if entry.duration > 0 {
            Some(entry.duration)
        } else {
            None
        },
        start_unix: entry.start_unix,
        program_date_time,
        media_range: None,
        init_range: None,
        index_range: None,
    }
}

fn generate_from_list(
    list: &SegmentList,
    rep: &Representation,
    mpd_url: &str,
    timescale: u64,
) -> Vec<MpdSegment> {
    let list_timescale: u64 = list
        .timescale
        .as_deref()
        .and_then(|v| v.parse().ok())
        .unwrap_or(timescale.max(1));
    let duration: u64 = list
        .duration
        .as_deref()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let segment_duration_sec = if list_timescale > 0 && duration > 0 {
        duration as f64 / list_timescale as f64
    } else {
        1.0
    };

    let base_url = rep
        .base_url
        .as_ref()
        .and_then(|b| b.value.as_deref())
        .unwrap_or("");

    list.segment_urls
        .iter()
        .enumerate()
        .filter_map(|(i, seg_url)| {
            let media_url = if let Some(media) = seg_url.media.as_deref() {
                resolve_url(mpd_url, media)
            } else if !base_url.is_empty() {
                resolve_url(mpd_url, base_url)
            } else {
                return None;
            };

            Some(MpdSegment {
                media: media_url,
                number: (i + 1) as u64,
                extinf: segment_duration_sec,
                time: None,
                duration_mpd_timescale: if duration > 0 { Some(duration) } else { None },
                start_unix: None,
                program_date_time: None,
                media_range: seg_url.media_range.clone(),
                init_range: None,
                index_range: None,
            })
        })
        .collect()
}

fn generate_from_base(
    base: &SegmentBase,
    rep: &Representation,
    mpd_url: &str,
    total_duration_sec: Option<f64>,
) -> Vec<MpdSegment> {
    let base_url = rep
        .base_url
        .as_ref()
        .and_then(|b| b.value.as_deref())
        .unwrap_or("");
    let media_url = resolve_url(mpd_url, base_url);

    let extinf = total_duration_sec.unwrap_or(1.0).max(1.0);
    let init_range = base.initialization.as_ref().and_then(|i| i.range.clone());

    // Derive media byte range: media data starts immediately after the init segment.
    // init_range is "start-end" (e.g. "0-657"), so media begins at end+1.
    // This lets the segment handler fetch only the media bytes via Range header,
    // avoiding full-file downloads on CDNs that gate whole-file responses.
    let media_range = init_range.as_ref().and_then(|r| {
        r.split('-')
            .nth(1)
            .and_then(|end| end.trim().parse::<u64>().ok())
            .map(|end_byte| format!("{}-", end_byte + 1))
    });

    vec![MpdSegment {
        media: media_url,
        number: 1,
        extinf,
        time: None,
        duration_mpd_timescale: None,
        start_unix: None,
        program_date_time: None,
        media_range,
        init_range,
        index_range: base.index_range.clone(),
    }]
}

// ---------------------------------------------------------------------------
// Init URL computation
// ---------------------------------------------------------------------------

fn compute_init_url(
    rep: &Representation,
    adaptation: &AdaptationSet,
    mpd_url: &str,
    base_url_str: &str,
    profile_id: &str,
    bandwidth: u64,
) -> (Option<String>, Option<String>) {
    // SegmentTemplate @initialization
    let tmpl = rep
        .segment_template
        .as_ref()
        .or(adaptation.segment_template.as_ref());
    if let Some(t) = tmpl {
        if let Some(init) = t.initialization.as_deref() {
            let expanded = expand_template(init, profile_id, bandwidth, 1, None);
            let path = if !base_url_str.is_empty() {
                format!("{base_url_str}{expanded}")
            } else {
                expanded
            };
            return (Some(resolve_url(mpd_url, &path)), None);
        }
    }

    // SegmentList Initialization @sourceURL
    let seg_list = rep
        .segment_list
        .as_ref()
        .or(adaptation.segment_list.as_ref());
    if let Some(list) = seg_list {
        if let Some(init) = &list.initialization {
            if let Some(source_url) = init.source_url.as_deref() {
                return (Some(resolve_url(mpd_url, source_url)), None);
            }
            if let Some(range) = init.range.as_deref() {
                let base = rep
                    .base_url
                    .as_ref()
                    .and_then(|b| b.value.as_deref())
                    .unwrap_or("");
                return (Some(resolve_url(mpd_url, base)), Some(range.to_string()));
            }
        }
    }

    // SegmentBase
    if let Some(sb) = &rep.segment_base {
        let base = rep
            .base_url
            .as_ref()
            .and_then(|b| b.value.as_deref())
            .unwrap_or("");
        let init_url = resolve_url(mpd_url, base);
        let init_range = sb.initialization.as_ref().and_then(|i| i.range.clone());
        return (Some(init_url), init_range);
    }

    // Fallback: BaseURL
    let base = rep
        .base_url
        .as_ref()
        .and_then(|b| b.value.as_deref())
        .unwrap_or("");
    if !base.is_empty() {
        return (Some(resolve_url(mpd_url, base)), None);
    }

    (None, None)
}

// ---------------------------------------------------------------------------
// DRM info extraction
// ---------------------------------------------------------------------------

fn extract_drm_info(cps: &[ContentProtection], mpd_url: &str) -> DrmInfo {
    let mut info = DrmInfo::default();
    for cp in cps {
        info.is_drm_protected = true;
        let scheme = cp.scheme_id_uri.as_deref().unwrap_or("").to_lowercase();

        if scheme.contains("clearkey") {
            info.drm_system
                .get_or_insert_with(|| "clearkey".to_string());
            if let Some(laurl) = cp.clearkey_laurl.as_ref().and_then(|l| l.value.as_deref()) {
                if info.la_url.is_none() {
                    info.la_url = Some(laurl.to_string());
                }
            }
        } else if scheme.contains("widevine")
            || scheme.contains("edef8ba9-79d6-4ace-a3c8-27dcd51d21ed")
        {
            info.drm_system
                .get_or_insert_with(|| "widevine".to_string());
            if let Some(pssh) = cp.cenc_pssh.as_ref().and_then(|p| p.value.as_deref()) {
                info.pssh.get_or_insert_with(|| pssh.to_string());
            }
        } else if scheme.contains("playready")
            || scheme.contains("9a04f079-9840-4286-ab92-e65be0885f95")
        {
            info.drm_system
                .get_or_insert_with(|| "playready".to_string());
            if let Some(la) = cp.ms_laurl.as_ref().and_then(|m| m.license_url.as_deref()) {
                if info.la_url.is_none() {
                    info.la_url = Some(la.to_string());
                }
            }
        }

        if let Some(kid) = cp.cenc_default_kid.as_deref() {
            let kid_clean = kid.replace('-', "");
            info.key_id.get_or_insert(kid_clean);
        }
    }

    // Resolve relative LA URL
    if let Some(la) = info.la_url.as_deref() {
        if !la.starts_with("http://") && !la.starts_with("https://") {
            let resolved = resolve_url(mpd_url, la);
            info.la_url = Some(resolved);
        }
    }

    info
}

// ---------------------------------------------------------------------------
// Live sequence / depth helpers (ported from mpd_processor.py)
// ---------------------------------------------------------------------------

fn resolve_nominal_duration(segments: &[MpdSegment]) -> Option<u64> {
    let durations: Vec<u64> = segments
        .iter()
        .filter_map(|s| s.duration_mpd_timescale)
        .filter(|&d| d > 0)
        .collect();
    if durations.is_empty() {
        return None;
    }
    let mut sorted = durations.clone();
    sorted.sort_unstable();
    Some(sorted[sorted.len() / 2]) // median_low
}

/// Compute the HLS `#EXT-X-MEDIA-SEQUENCE` value for a live playlist.
pub fn compute_live_media_sequence(
    first_segment: &MpdSegment,
    profile: &MpdProfile,
    segments: &[MpdSegment],
) -> u64 {
    if profile.segment_template_start_number_explicit {
        return first_segment.number.max(1);
    }

    let nominal = profile
        .nominal_duration_mpd_timescale
        .or_else(|| resolve_nominal_duration(segments));

    if let (Some(t), Some(nom)) = (first_segment.time, nominal) {
        if nom > 0 {
            return (t / nom).max(1);
        }
    }

    first_segment.number.max(1)
}

/// Compute the live playlist depth (how many segments to include).
pub fn compute_live_playlist_depth(
    is_ts_mode: bool,
    effective_start_offset: Option<f64>,
    configured_depth: usize,
    extinf_values: &[f64],
) -> usize {
    let depth_floor = if is_ts_mode { 20 } else { 15 };
    let mut depth = configured_depth.max(depth_floor);

    if let Some(offset) = effective_start_offset {
        if offset < 0.0 {
            let segment_duration = if !extinf_values.is_empty() {
                let mut sorted: Vec<f64> = extinf_values.to_vec();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                sorted[sorted.len() / 2].max(0.1)
            } else {
                4.0
            };
            let segs_behind = (offset.abs() / segment_duration).ceil() as usize;
            let safety = if is_ts_mode { 10 } else { 12 };
            depth = depth.max(segs_behind + safety);
        }
    }

    depth.max(1)
}

// ---------------------------------------------------------------------------
// HLS master manifest generation (build_hls)
// ---------------------------------------------------------------------------

/// Parameters passed through into proxied HLS URLs.
#[derive(Debug, Clone, Default)]
pub struct MpdProxyParams {
    pub api_password: String,
    /// `h_*` headers to forward.
    pub pass_headers: HashMap<String, String>,
    /// DRM key_id (hex).
    pub key_id: Option<String>,
    /// DRM key (hex).
    pub key: Option<String>,
    /// Resolution filter (e.g. "1080p").
    pub resolution: Option<String>,
    /// Skip segment time ranges.
    pub skip: Option<String>,
    /// Whether to remux to TS (per-request override).
    pub remux_to_ts: bool,
}

/// Build the HLS master manifest from a [`ParsedMpd`].
/// Returns the manifest as a UTF-8 string.
pub fn build_hls_master(
    parsed: &ParsedMpd,
    proxy_base: &str,
    mpd_url: &str,
    params: &MpdProxyParams,
) -> String {
    let version = if params.remux_to_ts { 3 } else { 6 };
    let mut hls = vec!["#EXTM3U".to_string(), format!("#EXT-X-VERSION:{version}")];

    let audio_profiles: Vec<&MpdProfile> =
        parsed.profiles.iter().filter(|p| p.is_audio()).collect();
    let mut video_profiles: Vec<&MpdProfile> =
        parsed.profiles.iter().filter(|p| p.is_video()).collect();

    // Resolution filter
    if let Some(res) = &params.resolution {
        video_profiles = filter_by_resolution(video_profiles, res);
    }

    // TS mode: only highest quality video
    if params.remux_to_ts && !video_profiles.is_empty() {
        let max_h = video_profiles.iter().map(|p| p.height).max().unwrap_or(0);
        video_profiles.retain(|p| p.height >= max_h);
    }

    // Sort highest bandwidth first so dedup keeps best codec per quality tier.
    video_profiles.sort_by(|a, b| b.bandwidth.cmp(&a.bandwidth));
    let mut seen_rep_ids = std::collections::HashSet::new();
    video_profiles.retain(|p| seen_rep_ids.insert(p.rep_id.clone()));

    if params.resolution.is_some() || params.remux_to_ts {
        // Explicit resolution or TS mode: single matching variant.
        video_profiles.truncate(1);
    } else {
        // Limit ABR ladder: one entry per unique height, capped at MAX_VIDEO_VARIANTS.
        // libavformat (mpv/ffmpeg) fetches ALL #EXT-X-STREAM-INF media playlists and
        // their init segments before playback regardless of BANDWIDTH hints —
        // N variants = N × ~2 s of init-segment probing at startup.
        // Height-dedup keeps the full quality range (4K/1080p/720p/…) while
        // eliminating multiple-bitrate entries at the same resolution.
        const MAX_VIDEO_VARIANTS: usize = 5;
        let mut seen_heights: std::collections::HashSet<u32> = std::collections::HashSet::new();
        video_profiles.retain(|p| seen_heights.insert(p.height));
        video_profiles.truncate(MAX_VIDEO_VARIANTS);
    }

    // Determine the default audio profile (English preferred, else highest bandwidth).
    // This is used both for the CODECS attribute on EXT-X-STREAM-INF and to mark
    // which audio track gets DEFAULT=YES,AUTOSELECT=YES.
    let default_audio_id: Option<String> = {
        let preferred = audio_profiles
            .iter()
            .filter(|p| {
                p.lang
                    .as_deref()
                    .map(|l| l.starts_with("en"))
                    .unwrap_or(false)
            })
            .max_by_key(|p| p.bandwidth)
            .or_else(|| audio_profiles.iter().max_by_key(|p| p.bandwidth));
        preferred.map(|p| p.id.clone())
    };

    let first_audio_codec: Option<&str> = {
        // Use the default audio track's codec for the CODECS string on video variants.
        audio_profiles
            .iter()
            .find(|p| Some(&p.id) == default_audio_id.as_ref())
            .or_else(|| audio_profiles.first())
            .map(|p| p.codecs.as_str())
            .filter(|s| !s.is_empty())
    };

    // Audio tracks: one entry per unique language, capped at MAX_AUDIO_TRACKS.
    // Sort default track first so it is always within the cap, then by bandwidth
    // descending so the highest-quality codec per language wins.
    // libavformat probes every #EXT-X-MEDIA entry regardless of DEFAULT/AUTOSELECT;
    // capping at 4 keeps language selection while bounding startup probing.
    {
        const MAX_AUDIO_TRACKS: usize = 4;
        let mut seen_langs: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut sorted_audio = audio_profiles.clone();
        sorted_audio.sort_by(|a, b| {
            let a_def = Some(&a.id) == default_audio_id.as_ref();
            let b_def = Some(&b.id) == default_audio_id.as_ref();
            b_def.cmp(&a_def).then(b.bandwidth.cmp(&a.bandwidth))
        });
        let mut count = 0;
        for profile in sorted_audio {
            if count >= MAX_AUDIO_TRACKS {
                break;
            }
            let lang = profile.lang.as_deref().unwrap_or("und");
            if !seen_langs.insert(lang.to_string()) {
                continue;
            }
            let is_default = Some(&profile.id) == default_audio_id.as_ref();
            let default_attr = if is_default { "YES" } else { "NO" };
            let autoselect_attr = if is_default { "YES" } else { "NO" };
            let name = format!("Audio {lang} ({})", profile.bandwidth);
            let playlist_url = build_playlist_url(proxy_base, mpd_url, &profile.id, params);
            hls.push(format!(
                "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"{name}\",DEFAULT={default_attr},AUTOSELECT={autoselect_attr},LANGUAGE=\"{lang}\",URI=\"{playlist_url}\""
            ));
            count += 1;
        }
    }

    // Video tracks
    for profile in &video_profiles {
        let audio_attr = if !audio_profiles.is_empty() {
            ",AUDIO=\"audio\"".to_string()
        } else {
            String::new()
        };

        let codecs = if let Some(ac) = first_audio_codec {
            if !audio_attr.is_empty() {
                format!("{},{ac}", profile.codecs)
            } else {
                profile.codecs.clone()
            }
        } else {
            profile.codecs.clone()
        };

        let playlist_url = build_playlist_url(proxy_base, mpd_url, &profile.id, params);

        if params.remux_to_ts {
            hls.push(format!(
                "#EXT-X-STREAM-INF:BANDWIDTH={},RESOLUTION={}x{},CODECS=\"{codecs}\"{audio_attr}",
                profile.bandwidth, profile.width, profile.height
            ));
        } else {
            hls.push(format!(
                "#EXT-X-STREAM-INF:BANDWIDTH={},RESOLUTION={}x{},CODECS=\"{codecs}\",FRAME-RATE={:.3}{audio_attr}",
                profile.bandwidth, profile.width, profile.height, profile.frame_rate
            ));
        }
        hls.push(playlist_url);
    }

    hls.join("\n")
}

/// Build the HLS media playlist for a specific profile.
pub fn build_hls_media_playlist(
    parsed: &ParsedMpd,
    profile: &MpdProfile,
    proxy_base: &str,
    _mpd_url: &str,
    params: &MpdProxyParams,
    skip_ranges: &[SkipRange],
    start_offset: Option<f64>,
    configured_depth: usize,
) -> String {
    let is_ts_mode = params.remux_to_ts;
    let version = if is_ts_mode { 3 } else { 6 };
    let is_live = parsed.is_live;

    let mut hls = vec!["#EXTM3U".to_string(), format!("#EXT-X-VERSION:{version}")];

    let segments = &profile.segments;
    if segments.is_empty() {
        warn!("No segments for profile {}", profile.id);
        return hls.join("\n");
    }

    // Determine effective start offset
    let effective_start_offset = if is_ts_mode && is_live && start_offset.is_none() {
        Some(-30.0f64)
    } else {
        start_offset.or_else(|| if is_live { Some(-30.0) } else { None })
    };

    if let Some(offset) = effective_start_offset {
        let precise = if is_ts_mode { "NO" } else { "YES" };
        hls.push(format!(
            "#EXT-X-START:TIME-OFFSET={offset:.1},PRECISE={precise}"
        ));
    }

    // Trim to live playlist depth
    let trimmed: &[MpdSegment] = if is_live {
        let extinf_vals: Vec<f64> = segments
            .iter()
            .filter_map(|s| if s.extinf > 0.0 { Some(s.extinf) } else { None })
            .collect();
        let depth = compute_live_playlist_depth(
            is_ts_mode,
            effective_start_offset,
            configured_depth,
            &extinf_vals,
        );
        let start = segments.len().saturating_sub(depth);
        &segments[start..]
    } else {
        segments
    };

    if trimmed.is_empty() {
        return hls.join("\n");
    }

    // TARGETDURATION and MEDIA-SEQUENCE
    let extinf_vals: Vec<f64> = trimmed
        .iter()
        .filter_map(|s| if s.extinf > 0.0 { Some(s.extinf) } else { None })
        .collect();
    let target_duration = if is_ts_mode {
        extinf_vals.iter().cloned().fold(0.0f64, f64::max).ceil() as u64 + 1
    } else {
        extinf_vals.iter().cloned().fold(0.0f64, f64::max).ceil() as u64
    };

    let sequence = if is_live {
        compute_live_media_sequence(&trimmed[0], profile, trimmed)
    } else {
        trimmed[0].number.max(1)
    };

    hls.push(format!("#EXT-X-TARGETDURATION:{target_duration}"));
    hls.push(format!("#EXT-X-MEDIA-SEQUENCE:{sequence}"));
    if !is_live {
        hls.push("#EXT-X-PLAYLIST-TYPE:VOD".to_string());
    }

    // EXT-X-MAP for all fMP4 streams (live and VOD).
    // Serving init via a dedicated /init endpoint avoids re-sending the moov
    // on every segment request and lets ffmpeg/players cache the init separately.
    let use_map = !is_ts_mode;
    if use_map {
        if let Some(init_url) = &profile.init_url {
            let map_url = build_init_url(
                proxy_base,
                init_url,
                &profile.mime_type,
                profile.init_range.as_deref(),
                params,
            );
            hls.push(format!("#EXT-X-MAP:URI=\"{map_url}\""));
        }
    }

    // Segment lines
    let mut skip_filter = if !skip_ranges.is_empty() {
        Some(SkipSegmentFilter::new(skip_ranges.to_vec()))
    } else {
        None
    };
    let mut need_discontinuity = false;

    for segment in trimmed {
        let duration = segment.extinf;

        // Skip filter
        if let Some(ref mut sf) = skip_filter {
            if sf.check_and_advance(duration) {
                need_discontinuity = true;
                continue;
            }
        }

        if need_discontinuity {
            hls.push("#EXT-X-DISCONTINUITY".to_string());
            need_discontinuity = false;
        }

        if let Some(pdt) = &segment.program_date_time {
            if !is_ts_mode {
                hls.push(format!("#EXT-X-PROGRAM-DATE-TIME:{pdt}"));
            }
        }

        hls.push(format!("#EXTINF:{duration:.3},"));

        let seg_url = build_segment_url(
            proxy_base,
            profile.init_url.as_deref().unwrap_or(""),
            &segment.media,
            &profile.mime_type,
            is_live,
            use_map && !is_ts_mode,
            profile.init_range.as_deref(),
            segment.init_range.as_deref(),
            segment.media_range.as_deref(),
            params,
        );
        hls.push(seg_url);
    }

    if !is_live {
        hls.push("#EXT-X-ENDLIST".to_string());
    }

    hls.join("\n")
}

// ---------------------------------------------------------------------------
// URL builders
// ---------------------------------------------------------------------------

fn build_playlist_url(
    proxy_base: &str,
    mpd_url: &str,
    profile_id: &str,
    params: &MpdProxyParams,
) -> String {
    let encoded_mpd = urlencoding::encode(mpd_url);
    let mut url = format!(
        "{proxy_base}/proxy/mpd/playlist?d={encoded_mpd}&profile_id={profile_id}&api_password={}",
        urlencoding::encode(&params.api_password)
    );
    if let Some(kid) = &params.key_id {
        url.push_str(&format!("&key_id={}", urlencoding::encode(kid)));
    }
    if let Some(key) = &params.key {
        url.push_str(&format!("&key={}", urlencoding::encode(key)));
    }
    if let Some(skip) = &params.skip {
        url.push_str(&format!("&skip={}", urlencoding::encode(skip)));
    }
    if params.remux_to_ts {
        url.push_str("&remux_to_ts=1");
    }
    for (k, v) in &params.pass_headers {
        url.push_str(&format!(
            "&h_{}={}",
            urlencoding::encode(k),
            urlencoding::encode(v)
        ));
    }
    url
}

fn build_segment_url(
    proxy_base: &str,
    init_url: &str,
    segment_url: &str,
    mime_type: &str,
    is_live: bool,
    use_map: bool,
    init_range: Option<&str>,
    seg_init_range: Option<&str>,
    segment_range: Option<&str>,
    params: &MpdProxyParams,
) -> String {
    let ext = if params.remux_to_ts { "ts" } else { "mp4" };
    let mut url = format!(
        "{proxy_base}/proxy/mpd/segment.{ext}?init_url={}&segment_url={}&mime_type={}&is_live={}&api_password={}",
        urlencoding::encode(init_url),
        urlencoding::encode(segment_url),
        urlencoding::encode(mime_type),
        if is_live { "true" } else { "false" },
        urlencoding::encode(&params.api_password)
    );
    if use_map {
        url.push_str("&use_map=true");
    }
    let effective_init_range = seg_init_range.or(init_range);
    if let Some(range) = effective_init_range {
        url.push_str(&format!("&init_range={}", urlencoding::encode(range)));
    }
    if let Some(range) = segment_range {
        url.push_str(&format!("&segment_range={}", urlencoding::encode(range)));
    }
    if let Some(kid) = &params.key_id {
        url.push_str(&format!("&key_id={}", urlencoding::encode(kid)));
    }
    if let Some(key) = &params.key {
        url.push_str(&format!("&key={}", urlencoding::encode(key)));
    }
    for (k, v) in &params.pass_headers {
        url.push_str(&format!(
            "&h_{}={}",
            urlencoding::encode(k),
            urlencoding::encode(v)
        ));
    }
    url
}

fn build_init_url(
    proxy_base: &str,
    init_url: &str,
    mime_type: &str,
    init_range: Option<&str>,
    params: &MpdProxyParams,
) -> String {
    let mut url = format!(
        "{proxy_base}/proxy/mpd/init?init_url={}&mime_type={}&api_password={}",
        urlencoding::encode(init_url),
        urlencoding::encode(mime_type),
        urlencoding::encode(&params.api_password)
    );
    if let Some(range) = init_range {
        url.push_str(&format!("&init_range={}", urlencoding::encode(range)));
    }
    if let Some(kid) = &params.key_id {
        url.push_str(&format!("&key_id={}", urlencoding::encode(kid)));
    }
    if let Some(key) = &params.key {
        url.push_str(&format!("&key={}", urlencoding::encode(key)));
    }
    for (k, v) in &params.pass_headers {
        url.push_str(&format!(
            "&h_{}={}",
            urlencoding::encode(k),
            urlencoding::encode(v)
        ));
    }
    url
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_frame_rate(s: &str) -> f64 {
    if s.contains('/') {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() == 2 {
            let num: f64 = parts[0].parse().unwrap_or(30000.0);
            let den: f64 = parts[1].parse().unwrap_or(1001.0);
            if den > 0.0 {
                return (num / den * 1000.0).round() / 1000.0;
            }
        }
    }
    s.parse().unwrap_or(29.97)
}

fn filter_by_resolution<'a>(profiles: Vec<&'a MpdProfile>, target: &str) -> Vec<&'a MpdProfile> {
    let target_h: u32 = target.trim_end_matches('p').parse().unwrap_or(1080);
    let mut valid: Vec<&MpdProfile> = profiles.iter().copied().filter(|p| p.height > 0).collect();
    if valid.is_empty() {
        return profiles;
    }
    // Find the profile at or below target height
    valid.sort_by(|a, b| b.height.cmp(&a.height));

    let selected = valid
        .iter()
        .copied()
        .find(|p| p.height <= target_h)
        .or_else(|| valid.last().copied());

    if let Some(s) = selected {
        vec![s]
    } else {
        profiles
    }
}

fn unix_to_iso8601(unix: f64) -> Option<String> {
    let ts = time::OffsetDateTime::from_unix_timestamp(unix as i64).ok()?;
    let fmt = time::format_description::well_known::Rfc3339;
    ts.format(&fmt).ok()
}

// ---------------------------------------------------------------------------
// SIDX (Segment Index) parser
// ---------------------------------------------------------------------------

/// One media subsegment described by a SIDX box.
pub struct SidxFragment {
    /// First byte of this fragment in the original media file.
    pub start: u64,
    /// Last byte of this fragment in the original media file (inclusive).
    pub end: u64,
    /// Subsegment duration in SIDX timescale ticks.
    pub duration_timescale: u64,
    /// SIDX timescale (ticks per second).
    pub timescale: u32,
}

/// Parse a SIDX box from `data` (which may contain other boxes before it, e.g. `styp`)
/// and return per-fragment byte-range descriptors.
///
/// `index_range_start` is the file offset of `data[0]` (i.e. the start byte of the
/// `@indexRange` attribute value).  Fragment file offsets are computed relative to the
/// end of the SIDX box itself:
///
/// ```text
/// first_fragment_start = sidx_box_end_in_file + first_offset
/// ```
///
/// Returns an empty `Vec` if the box cannot be parsed (bad data, missing SIDX, etc.).
pub fn parse_sidx_fragments(data: &[u8], index_range_start: u64) -> Vec<SidxFragment> {
    inner(data, index_range_start).unwrap_or_default()
}

fn inner(data: &[u8], index_range_start: u64) -> Option<Vec<SidxFragment>> {
    fn r8(d: &[u8], o: usize) -> Option<u8> {
        d.get(o).copied()
    }
    fn r16(d: &[u8], o: usize) -> Option<u16> {
        Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
    }
    fn r32(d: &[u8], o: usize) -> Option<u32> {
        Some(u32::from_be_bytes([
            *d.get(o)?,
            *d.get(o + 1)?,
            *d.get(o + 2)?,
            *d.get(o + 3)?,
        ]))
    }
    fn r64(d: &[u8], o: usize) -> Option<u64> {
        Some(u64::from_be_bytes([
            *d.get(o)?,
            *d.get(o + 1)?,
            *d.get(o + 2)?,
            *d.get(o + 3)?,
            *d.get(o + 4)?,
            *d.get(o + 5)?,
            *d.get(o + 6)?,
            *d.get(o + 7)?,
        ]))
    }

    // Scan forward to find the `sidx` box (may be preceded by `styp` or others).
    let mut scan = 0usize;
    let sidx_local_start = loop {
        if scan + 8 > data.len() {
            return None;
        }
        let bsize = r32(data, scan)? as usize;
        if bsize < 8 {
            return None;
        }
        if data.get(scan + 4..scan + 8) == Some(b"sidx") {
            break scan;
        }
        scan += bsize;
    };

    let sidx_file_start = index_range_start + sidx_local_start as u64;
    let sidx_box_size = r32(data, sidx_local_start)? as u64;
    let sidx_file_end = sidx_file_start + sidx_box_size; // first byte *after* sidx

    let mut off = sidx_local_start + 8; // skip box-size + box-type

    let version = r8(data, off)?;
    off += 4; // version (1) + flags (3)

    off += 4; // reference_id

    let timescale = r32(data, off)?;
    off += 4;

    let first_offset = if version == 0 {
        off += 4; // earliest_presentation_time (32-bit)
        let fo = r32(data, off)? as u64;
        off += 4;
        fo
    } else {
        off += 8; // earliest_presentation_time (64-bit)
        let fo = r64(data, off)?;
        off += 8;
        fo
    };

    off += 2; // reserved
    let reference_count = r16(data, off)? as usize;
    off += 2;

    let mut frag_start = sidx_file_end + first_offset;
    let mut fragments = Vec::with_capacity(reference_count);

    for _ in 0..reference_count {
        if off + 12 > data.len() {
            break;
        }

        let ref_field = r32(data, off)?;
        off += 4;
        let ref_type = (ref_field >> 31) & 1;
        let referenced_size = (ref_field & 0x7FFF_FFFF) as u64;

        let duration = r32(data, off)? as u64;
        off += 4;
        off += 4; // SAP field (ignored)

        if ref_type == 0 {
            // media reference; skip index-of-indexes (type 1)
            fragments.push(SidxFragment {
                start: frag_start,
                end: frag_start + referenced_size - 1,
                duration_timescale: duration,
                timescale,
            });
        }

        frag_start += referenced_size;
    }

    Some(fragments)
}
