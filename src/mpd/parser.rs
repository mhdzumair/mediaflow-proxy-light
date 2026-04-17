//! MPD (MPEG-DASH) XML document parser using quick-xml + serde.
//!
//! The top-level entry point is [`parse_mpd`] which deserializes raw MPD bytes
//! into a [`MpdDocument`] that mirrors the structure of the XML tree.

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Top-level document
// ---------------------------------------------------------------------------

/// Root `<MPD>` element.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct MpdDocument {
    /// Stream type: "static" (default) or "dynamic" (live).
    #[serde(rename = "@type", default)]
    pub stream_type: Option<String>,

    /// ISO 8601 datetime string for the availability start time (live streams).
    #[serde(rename = "@availabilityStartTime", default)]
    pub availability_start_time: Option<String>,

    /// ISO 8601 duration for the minimum MPD update period (live streams).
    #[serde(rename = "@minimumUpdatePeriod", default)]
    pub minimum_update_period: Option<String>,

    /// ISO 8601 duration for how much time-shifted content is available (live).
    #[serde(rename = "@timeShiftBufferDepth", default)]
    pub time_shift_buffer_depth: Option<String>,

    /// ISO 8601 duration for total presentation duration (VOD).
    #[serde(rename = "@mediaPresentationDuration", default)]
    pub media_presentation_duration: Option<String>,

    /// ISO 8601 datetime string for when the MPD was published (live streams).
    #[serde(rename = "@publishTime", default)]
    pub publish_time: Option<String>,

    #[serde(rename = "Period", default)]
    pub periods: Vec<Period>,
}

// ---------------------------------------------------------------------------
// Period
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Period {
    #[serde(rename = "@id", default)]
    pub id: Option<String>,

    /// ISO 8601 duration for the period start offset (default PT0S).
    #[serde(rename = "@start", default)]
    pub start: Option<String>,

    /// ISO 8601 duration for the period duration.
    #[serde(rename = "@duration", default)]
    pub duration: Option<String>,

    #[serde(rename = "AdaptationSet", default)]
    pub adaptation_sets: Vec<AdaptationSet>,
}

// ---------------------------------------------------------------------------
// AdaptationSet
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default, Clone)]
pub struct AdaptationSet {
    #[serde(rename = "@id", default)]
    pub id: Option<String>,

    #[serde(rename = "@mimeType", default)]
    pub mime_type: Option<String>,

    #[serde(rename = "@codecs", default)]
    pub codecs: Option<String>,

    #[serde(rename = "@lang", default)]
    pub lang: Option<String>,

    #[serde(rename = "@width", default)]
    pub width: Option<String>,

    #[serde(rename = "@height", default)]
    pub height: Option<String>,

    #[serde(rename = "@bandwidth", default)]
    pub bandwidth: Option<String>,

    #[serde(rename = "@maxFrameRate", default)]
    pub max_frame_rate: Option<String>,

    #[serde(rename = "@frameRate", default)]
    pub frame_rate: Option<String>,

    #[serde(rename = "@startWithSAP", default)]
    pub start_with_sap: Option<String>,

    #[serde(rename = "@audioSamplingRate", default)]
    pub audio_sampling_rate: Option<String>,

    #[serde(rename = "SegmentTemplate", default)]
    pub segment_template: Option<SegmentTemplate>,

    #[serde(rename = "SegmentList", default)]
    pub segment_list: Option<SegmentList>,

    #[serde(rename = "ContentProtection", default)]
    pub content_protection: Vec<ContentProtection>,

    #[serde(rename = "Representation", default)]
    pub representations: Vec<Representation>,
}

// ---------------------------------------------------------------------------
// Representation
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Representation {
    #[serde(rename = "@id", default)]
    pub id: Option<String>,

    #[serde(rename = "@mimeType", default)]
    pub mime_type: Option<String>,

    #[serde(rename = "@codecs", default)]
    pub codecs: Option<String>,

    #[serde(rename = "@bandwidth", default)]
    pub bandwidth: Option<String>,

    #[serde(rename = "@width", default)]
    pub width: Option<String>,

    #[serde(rename = "@height", default)]
    pub height: Option<String>,

    #[serde(rename = "@frameRate", default)]
    pub frame_rate: Option<String>,

    #[serde(rename = "@sar", default)]
    pub sar: Option<String>,

    #[serde(rename = "@lang", default)]
    pub lang: Option<String>,

    #[serde(rename = "@audioSamplingRate", default)]
    pub audio_sampling_rate: Option<String>,

    #[serde(rename = "AudioChannelConfiguration", default)]
    pub audio_channel_config: Option<AudioChannelConfiguration>,

    #[serde(rename = "SegmentTemplate", default)]
    pub segment_template: Option<SegmentTemplate>,

    #[serde(rename = "SegmentList", default)]
    pub segment_list: Option<SegmentList>,

    #[serde(rename = "SegmentBase", default)]
    pub segment_base: Option<SegmentBase>,

    #[serde(rename = "ContentProtection", default)]
    pub content_protection: Vec<ContentProtection>,

    /// A BaseURL inside a Representation (often a path prefix or full URL).
    #[serde(rename = "BaseURL", default)]
    pub base_url: Option<BaseUrl>,
}

// ---------------------------------------------------------------------------
// AudioChannelConfiguration
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default, Clone)]
pub struct AudioChannelConfiguration {
    #[serde(rename = "@value", default)]
    pub value: Option<String>,
}

// ---------------------------------------------------------------------------
// BaseURL
// ---------------------------------------------------------------------------

/// `<BaseURL>` element — may be a simple text URL or have attributes.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct BaseUrl {
    #[serde(rename = "$text", default)]
    pub value: Option<String>,
}

// ---------------------------------------------------------------------------
// SegmentTemplate
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default, Clone)]
pub struct SegmentTemplate {
    /// Template for media segments (e.g. "seg$Number$.m4s").
    #[serde(rename = "@media", default)]
    pub media: Option<String>,

    /// Template for the initialization segment (e.g. "init$RepresentationID$.mp4").
    #[serde(rename = "@initialization", default)]
    pub initialization: Option<String>,

    #[serde(rename = "@timescale", default)]
    pub timescale: Option<String>,

    #[serde(rename = "@duration", default)]
    pub duration: Option<String>,

    #[serde(rename = "@startNumber", default)]
    pub start_number: Option<String>,

    #[serde(rename = "@presentationTimeOffset", default)]
    pub presentation_time_offset: Option<String>,

    #[serde(rename = "SegmentTimeline", default)]
    pub segment_timeline: Option<SegmentTimeline>,
}

// ---------------------------------------------------------------------------
// SegmentTimeline / S element
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default, Clone)]
pub struct SegmentTimeline {
    #[serde(rename = "S", default)]
    pub segments: Vec<SElement>,
}

/// A single `<S>` entry in `<SegmentTimeline>`.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct SElement {
    /// Start time in timescale units.
    #[serde(rename = "@t", default)]
    pub t: Option<String>,

    /// Duration in timescale units.
    #[serde(rename = "@d")]
    pub d: String,

    /// Repeat count (0 means 1 segment, N means N+1 segments).
    #[serde(rename = "@r", default)]
    pub r: Option<String>,
}

// ---------------------------------------------------------------------------
// SegmentList
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default, Clone)]
pub struct SegmentList {
    #[serde(rename = "@timescale", default)]
    pub timescale: Option<String>,

    #[serde(rename = "@duration", default)]
    pub duration: Option<String>,

    #[serde(rename = "Initialization", default)]
    pub initialization: Option<SegmentListInit>,

    #[serde(rename = "SegmentURL", default)]
    pub segment_urls: Vec<SegmentUrl>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct SegmentListInit {
    #[serde(rename = "@sourceURL", default)]
    pub source_url: Option<String>,

    #[serde(rename = "@range", default)]
    pub range: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct SegmentUrl {
    #[serde(rename = "@media", default)]
    pub media: Option<String>,

    #[serde(rename = "@mediaRange", default)]
    pub media_range: Option<String>,
}

// ---------------------------------------------------------------------------
// SegmentBase
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default, Clone)]
pub struct SegmentBase {
    #[serde(rename = "@indexRange", default)]
    pub index_range: Option<String>,

    #[serde(rename = "Initialization", default)]
    pub initialization: Option<SegmentBaseInit>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct SegmentBaseInit {
    #[serde(rename = "@range", default)]
    pub range: Option<String>,
}

// ---------------------------------------------------------------------------
// ContentProtection (DRM)
// ---------------------------------------------------------------------------

/// `<ContentProtection>` element — may appear in AdaptationSet or Representation.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct ContentProtection {
    #[serde(rename = "@schemeIdUri", default)]
    pub scheme_id_uri: Option<String>,

    /// ClearKey `cenc:default_KID` attribute (UUID with dashes).
    /// Using a flat field name; serde rename handles the colon-prefixed original.
    #[serde(rename = "@cenc:default_KID", default)]
    pub cenc_default_kid: Option<String>,

    /// Widevine PSSH box (base64).
    #[serde(rename = "cenc:pssh", default)]
    pub cenc_pssh: Option<CencPssh>,

    /// ClearKey LA URL from `<clearkey:Laurl>` child.
    #[serde(rename = "clearkey:Laurl", default)]
    pub clearkey_laurl: Option<ClearkeyLaurl>,

    /// PlayReady LA URL from `<ms:laurl>`.
    #[serde(rename = "ms:laurl", default)]
    pub ms_laurl: Option<MsLaurl>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct CencPssh {
    #[serde(rename = "$text", default)]
    pub value: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct ClearkeyLaurl {
    #[serde(rename = "$text", default)]
    pub value: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct MsLaurl {
    #[serde(rename = "@licenseUrl", default)]
    pub license_url: Option<String>,
}

// ---------------------------------------------------------------------------
// Parse entry point
// ---------------------------------------------------------------------------

/// Parse raw MPD bytes into an [`MpdDocument`].
///
/// Returns an error string if the XML cannot be deserialized.
pub fn parse_mpd(content: &[u8]) -> Result<MpdDocument, String> {
    quick_xml::de::from_reader(content).map_err(|e| format!("MPD parse error: {e}"))
}
