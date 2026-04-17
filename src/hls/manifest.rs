/// HLS M3U8 manifest processor.
///
/// Fetches an upstream M3U8 playlist, rewrites all segment/playlist/key URLs to
/// go through the local proxy, and returns the modified content.
///
/// Port of Python `M3U8Processor` in `mediaflow_proxy/utils/m3u8_processor.py`.
use std::collections::HashMap;

use m3u8_rs::{MasterPlaylist, MediaPlaylist, MediaSegment, Playlist};

use crate::hls::skip_filter::{SkipRange, SkipSegmentFilter};
use crate::utils::url::{resolve_url, segment_extension};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a proxied URL for a **manifest / sub-playlist** endpoint.
///
/// Format: `{proxy_base}/proxy/hls/manifest?d={encoded_url}&{passthrough_params}`
pub fn proxy_manifest_url(proxy_base: &str, destination: &str, params: &ProxyParams) -> String {
    let encoded = urlencoding::encode(destination);
    let mut qs = format!("d={}", encoded);
    if !params.api_password.is_empty() {
        qs.push_str(&format!(
            "&api_password={}",
            urlencoding::encode(&params.api_password)
        ));
    }
    for (k, v) in &params.pass_headers {
        qs.push_str(&format!(
            "&h_{}={}",
            urlencoding::encode(k),
            urlencoding::encode(v)
        ));
    }
    format!(
        "{}/proxy/hls/manifest?{}",
        proxy_base.trim_end_matches('/'),
        qs
    )
}

/// Build a proxied URL for a **segment** endpoint.
///
/// Format: `{proxy_base}/proxy/hls/segment.{ext}?d={encoded_url}&{passthrough_params}`
pub fn proxy_segment_url(proxy_base: &str, destination: &str, params: &ProxyParams) -> String {
    let ext = segment_extension(destination);
    let encoded = urlencoding::encode(destination);
    let mut qs = format!("d={}", encoded);
    if !params.api_password.is_empty() {
        qs.push_str(&format!(
            "&api_password={}",
            urlencoding::encode(&params.api_password)
        ));
    }
    for (k, v) in &params.pass_headers {
        // Skip `range` for segments — each segment manages its own range
        if k.eq_ignore_ascii_case("range") {
            continue;
        }
        qs.push_str(&format!(
            "&h_{}={}",
            urlencoding::encode(k),
            urlencoding::encode(v)
        ));
    }
    format!(
        "{}/proxy/hls/segment.{}?{}",
        proxy_base.trim_end_matches('/'),
        ext,
        qs
    )
}

/// Build a proxied URL for a **key** endpoint.
/// Uses the segment endpoint path (no extension override).
pub fn proxy_key_url(proxy_base: &str, destination: &str, params: &ProxyParams) -> String {
    let encoded = urlencoding::encode(destination);
    let mut qs = format!("d={}", encoded);
    if !params.api_password.is_empty() {
        qs.push_str(&format!(
            "&api_password={}",
            urlencoding::encode(&params.api_password)
        ));
    }
    for (k, v) in &params.pass_headers {
        qs.push_str(&format!(
            "&h_{}={}",
            urlencoding::encode(k),
            urlencoding::encode(v)
        ));
    }
    format!(
        "{}/proxy/hls/segment?{}",
        proxy_base.trim_end_matches('/'),
        qs
    )
}

// ---------------------------------------------------------------------------
// ProxyParams — context passed to the manifest processor
// ---------------------------------------------------------------------------

/// Parameters forwarded from the incoming request to all generated proxy URLs.
#[derive(Debug, Clone, Default)]
pub struct ProxyParams {
    /// Value of `api_password` query parameter, if any.
    pub api_password: String,
    /// `h_*` request headers to re-attach to proxied URLs.
    pub pass_headers: HashMap<String, String>,
}

impl ProxyParams {
    pub fn new(api_password: &str, pass_headers: HashMap<String, String>) -> Self {
        Self {
            api_password: api_password.to_string(),
            pass_headers,
        }
    }
}

// ---------------------------------------------------------------------------
// ManifestProcessor
// ---------------------------------------------------------------------------

/// Options that control manifest rewriting behaviour.
#[derive(Debug, Default)]
pub struct ManifestOptions {
    /// If set, only the key URL is proxied; segment URLs are returned direct.
    pub key_only_proxy: bool,
    /// If set, return all absolute URLs without any proxying.
    pub no_proxy: bool,
    /// Force all playlist/variant URLs through the proxy.
    pub force_playlist_proxy: bool,
    /// Skip ranges to filter out.
    pub skip_ranges: Vec<SkipRange>,
    /// Optional `EXT-X-START:TIME-OFFSET` value to inject.
    pub start_offset: Option<f64>,
    /// Inject `start_offset` even for VOD streams.
    pub force_start_offset: bool,
}

/// Processes an upstream M3U8 playlist, rewriting URLs through the local proxy.
pub struct ManifestProcessor {
    proxy_base: String,
    params: ProxyParams,
    opts: ManifestOptions,
}

impl ManifestProcessor {
    pub fn new(proxy_base: &str, params: ProxyParams, opts: ManifestOptions) -> Self {
        Self {
            proxy_base: proxy_base.to_string(),
            params,
            opts,
        }
    }

    /// Process the raw `content` of an M3U8 playlist fetched from `source_url`.
    ///
    /// Returns the modified playlist as a `String`.
    pub fn process(&self, content: &[u8], source_url: &str) -> String {
        match m3u8_rs::parse_playlist_res(content) {
            Ok(Playlist::MasterPlaylist(pl)) => self.process_master(pl, source_url),
            Ok(Playlist::MediaPlaylist(pl)) => self.process_media(pl, source_url),
            Err(_) => {
                // Try line-by-line fallback for non-standard playlists
                tracing::warn!(
                    "m3u8-rs failed to parse playlist from {}, using line fallback",
                    source_url
                );
                self.process_lines(std::str::from_utf8(content).unwrap_or_default(), source_url)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Master playlist rewriting
    // -----------------------------------------------------------------------

    fn process_master(&self, mut pl: MasterPlaylist, base_url: &str) -> String {
        if self.opts.no_proxy {
            // Just resolve relative URLs to absolute
            for v in &mut pl.variants {
                v.uri = resolve_url(base_url, &v.uri);
            }
            for alt in &mut pl.alternatives {
                if let Some(ref uri) = alt.uri.clone() {
                    alt.uri = Some(resolve_url(base_url, uri));
                }
            }
        } else {
            for v in &mut pl.variants {
                v.uri = self.rewrite_playlist_uri(&v.uri, base_url);
            }
            for alt in &mut pl.alternatives {
                if let Some(ref uri) = alt.uri.clone() {
                    alt.uri = Some(self.rewrite_playlist_uri(uri, base_url));
                }
            }
            // m3u8-rs rejects certain valid-in-practice EXT-X-MEDIA attributes
            // (e.g. FORCED=NO on TYPE=AUDIO) and stores the whole tag in
            // unknown_tags, where write_to emits it verbatim.  Rewrite any URI=
            // found there so those rendition sub-playlists go through the proxy.
            for unknown in &mut pl.unknown_tags {
                if unknown.tag == "X-MEDIA" {
                    if let Some(ref rest) = unknown.rest.clone() {
                        if rest.contains("URI=\"") {
                            unknown.rest = Some(self.rewrite_tag_rest_uri(rest, base_url, true));
                        }
                    }
                }
            }
        }

        let mut out = Vec::new();
        pl.write_to(&mut out).unwrap_or_default();
        String::from_utf8_lossy(&out).into_owned()
    }

    // -----------------------------------------------------------------------
    // Media playlist rewriting
    // -----------------------------------------------------------------------

    fn process_media(&self, mut pl: MediaPlaylist, base_url: &str) -> String {
        let is_vod = pl.end_list
            || pl
                .playlist_type
                .as_ref()
                .map(|t| format!("{:?}", t).to_lowercase().contains("vod"))
                .unwrap_or(false);

        // Inject EXT-X-START if requested for live streams
        if let Some(offset) = self.opts.start_offset {
            if self.opts.force_start_offset || !is_vod {
                // EXT-X-START is a field on MediaPlaylist in m3u8-rs >= 6
                // We inject it via the unknown_tags mechanism if needed.
                // For now, inject as a raw tag — handled in line fallback.
                let _ = offset; // used below via write_to
            }
        }

        // Rewrite keys and map (init segment) URIs
        for seg in &mut pl.segments {
            // Key URI
            if let Some(ref mut key) = seg.key {
                if let Some(ref uri) = key.uri.clone() {
                    let resolved = resolve_url(base_url, uri);
                    key.uri = Some(if self.opts.no_proxy {
                        resolved
                    } else {
                        proxy_key_url(&self.proxy_base, &resolved, &self.params)
                    });
                }
            }

            // Init segment (EXT-X-MAP)
            if let Some(ref mut map) = seg.map {
                let resolved = resolve_url(base_url, &map.uri);
                map.uri = if self.opts.no_proxy {
                    resolved
                } else {
                    // Init segments are MP4 boxes — use segment endpoint with .mp4 ext
                    let encoded = urlencoding::encode(&resolved);
                    let mut qs = format!("d={}", encoded);
                    if !self.params.api_password.is_empty() {
                        qs.push_str(&format!(
                            "&api_password={}",
                            urlencoding::encode(&self.params.api_password)
                        ));
                    }
                    for (k, v) in &self.params.pass_headers {
                        qs.push_str(&format!(
                            "&h_{}={}",
                            urlencoding::encode(k),
                            urlencoding::encode(v)
                        ));
                    }
                    format!(
                        "{}/proxy/hls/segment.mp4?{}",
                        self.proxy_base.trim_end_matches('/'),
                        qs
                    )
                };
            }

            // Segment URI
            seg.uri = self.rewrite_segment_or_playlist_uri(&seg.uri, base_url);

            // Rewrite URI= values in unknown segment tags.
            //
            // m3u8-rs stores unrecognised tags (e.g. #EXT-X-MEDIA embedded in
            // VixCloud video sub-playlists) as ExtTag in MediaSegment.unknown_tags
            // and writes them back verbatim — leaving audio sub-playlist URIs
            // unproxied.  We fix them up here.
            for unknown in &mut seg.unknown_tags {
                if let Some(ref rest) = unknown.rest.clone() {
                    if rest.contains("URI=\"") {
                        // #EXT-X-MEDIA / #EXT-X-I-FRAME-STREAM-INF carry sub-playlist URIs;
                        // everything else (e.g. custom DRM tags) is treated as a key.
                        let is_playlist =
                            unknown.tag == "X-MEDIA" || unknown.tag == "X-I-FRAME-STREAM-INF";
                        unknown.rest = Some(self.rewrite_tag_rest_uri(rest, base_url, is_playlist));
                    }
                }
            }
        }

        // Rewrite URI= values in playlist-level unknown tags.
        //
        // If #EXT-X-MEDIA (or similar) appears before the first #EXTINF,
        // m3u8-rs places it in MediaPlaylist.unknown_tags rather than in any
        // MediaSegment.unknown_tags.  We handle both locations so that audio
        // and subtitle rendition sub-playlists are always proxied through the
        // manifest endpoint regardless of where the tag sits in the file.
        for unknown in &mut pl.unknown_tags {
            if let Some(ref rest) = unknown.rest.clone() {
                if rest.contains("URI=\"") {
                    let is_playlist =
                        unknown.tag == "X-MEDIA" || unknown.tag == "X-I-FRAME-STREAM-INF";
                    unknown.rest = Some(self.rewrite_tag_rest_uri(rest, base_url, is_playlist));
                }
            }
        }

        // Apply skip-segment filtering if configured
        let pl = if !self.opts.skip_ranges.is_empty() {
            self.apply_skip_filter(pl)
        } else {
            pl
        };

        let mut out = Vec::new();
        pl.write_to(&mut out).unwrap_or_default();
        let result = String::from_utf8_lossy(&out).into_owned();

        // Inject EXT-X-START after #EXTM3U if needed (m3u8-rs doesn't expose this field directly)
        if let Some(offset) = self.opts.start_offset {
            if self.opts.force_start_offset || !is_vod {
                return inject_ext_x_start(&result, offset);
            }
        }

        result
    }

    // -----------------------------------------------------------------------
    // Skip segment filtering
    // -----------------------------------------------------------------------

    fn apply_skip_filter(&self, mut pl: MediaPlaylist) -> MediaPlaylist {
        let ranges = self.opts.skip_ranges.clone();
        let mut filter = SkipSegmentFilter::new(ranges);
        let mut kept: Vec<MediaSegment> = Vec::with_capacity(pl.segments.len());
        let mut need_discontinuity = false;

        for seg in pl.segments.drain(..) {
            let duration = seg.duration as f64;
            if filter.check_and_advance(duration) {
                need_discontinuity = true;
            } else {
                let mut s = seg;
                if need_discontinuity {
                    s.discontinuity = true;
                    need_discontinuity = false;
                }
                kept.push(s);
            }
        }

        pl.segments = kept;
        pl
    }

    // -----------------------------------------------------------------------
    // URL rewriting helpers
    // -----------------------------------------------------------------------

    fn rewrite_playlist_uri(&self, uri: &str, base_url: &str) -> String {
        let abs = resolve_url(base_url, uri);
        proxy_manifest_url(&self.proxy_base, &abs, &self.params)
    }

    /// Rewrite the `URI="..."` value inside the **attribute string** of a parsed `ExtTag.rest`.
    ///
    /// Used to fix up `#EXT-X-MEDIA` (and similar) tags that land in
    /// `MediaSegment.unknown_tags` when VixCloud-style playlists embed audio
    /// rendition references inside video sub-playlists.
    ///
    /// `is_playlist` — if `true` the URI points to a sub-playlist and is
    /// routed through the manifest endpoint; otherwise the key endpoint is used.
    fn rewrite_tag_rest_uri(&self, rest: &str, base_url: &str, is_playlist: bool) -> String {
        let Some(start) = rest.find("URI=\"") else {
            return rest.to_string();
        };
        let after_quote = start + 5; // skip 'URI="'
        let Some(end) = rest[after_quote..].find('"') else {
            return rest.to_string();
        };
        let original_uri = &rest[after_quote..after_quote + end];
        let resolved = resolve_url(base_url, original_uri);

        let new_uri = if self.opts.no_proxy {
            resolved
        } else if is_playlist {
            proxy_manifest_url(&self.proxy_base, &resolved, &self.params)
        } else {
            proxy_key_url(&self.proxy_base, &resolved, &self.params)
        };

        rest.replacen(
            &format!("URI=\"{}\"", original_uri),
            &format!("URI=\"{}\"", new_uri),
            1,
        )
    }

    fn rewrite_segment_or_playlist_uri(&self, uri: &str, base_url: &str) -> String {
        if self.opts.no_proxy {
            return resolve_url(base_url, uri);
        }
        if self.opts.key_only_proxy {
            // Only key is proxied; return segment directly
            return resolve_url(base_url, uri);
        }

        let abs = resolve_url(base_url, uri);

        // Sub-playlists (e.g. variant streams embedded in a media playlist)
        if abs.contains(".m3u8") || abs.contains(".m3u") {
            return proxy_manifest_url(&self.proxy_base, &abs, &self.params);
        }

        // If forced-playlist-proxy is on, route everything as a manifest
        if self.opts.force_playlist_proxy {
            return proxy_manifest_url(&self.proxy_base, &abs, &self.params);
        }

        proxy_segment_url(&self.proxy_base, &abs, &self.params)
    }

    // -----------------------------------------------------------------------
    // Line-by-line fallback for non-standard playlists
    // -----------------------------------------------------------------------

    fn process_lines(&self, content: &str, base_url: &str) -> String {
        let mut out = String::with_capacity(content.len() + 256);
        let mut skip_filter = if !self.opts.skip_ranges.is_empty() {
            Some(SkipSegmentFilter::new(self.opts.skip_ranges.clone()))
        } else {
            None
        };
        let mut pending_extinf: Option<String> = None;
        let mut need_discontinuity = false;
        let mut start_offset_injected = false;

        for line in content.lines() {
            // EXT-X-START injection point
            if line.trim() == "#EXTM3U" {
                out.push_str(line);
                out.push('\n');
                if let Some(offset) = self.opts.start_offset {
                    if !start_offset_injected {
                        out.push_str(&format!(
                            "#EXT-X-START:TIME-OFFSET={:.1},PRECISE=YES\n",
                            offset
                        ));
                        start_offset_injected = true;
                    }
                }
                continue;
            }

            // EXTINF line
            if line.starts_with("#EXTINF:") {
                let duration = parse_extinf_duration(line);
                if let Some(ref mut f) = skip_filter {
                    if f.check_and_advance(duration) {
                        // Will skip this segment
                        need_discontinuity = true;
                        pending_extinf = None;
                        continue;
                    }
                }
                pending_extinf = Some(line.to_string());
                continue;
            }

            // Segment URL line
            if !line.starts_with('#') && !line.trim().is_empty() {
                if skip_filter.is_some() && pending_extinf.is_none() {
                    // Segment was skipped
                    continue;
                }
                if need_discontinuity {
                    out.push_str("#EXT-X-DISCONTINUITY\n");
                    need_discontinuity = false;
                }
                if let Some(extinf) = pending_extinf.take() {
                    out.push_str(&extinf);
                    out.push('\n');
                }
                out.push_str(&self.rewrite_segment_or_playlist_uri(line, base_url));
                out.push('\n');
                continue;
            }

            // Key / map URI line
            if line.contains("URI=") {
                out.push_str(&self.process_tag_with_uri(line, base_url));
                out.push('\n');
                continue;
            }

            // EXT-X-DISCONTINUITY — pass through, reset pending
            if line.starts_with("#EXT-X-DISCONTINUITY") {
                out.push_str(line);
                out.push('\n');
                need_discontinuity = false;
                continue;
            }

            // All other lines
            out.push_str(line);
            out.push('\n');
        }

        out
    }

    /// Rewrite the `URI="..."` value within a tag line (`#EXT-X-KEY`, `#EXT-X-MAP`, etc.).
    fn process_tag_with_uri(&self, line: &str, base_url: &str) -> String {
        // Extract URI value
        let Some(start) = line.find("URI=\"") else {
            return line.to_string();
        };
        let after_quote = start + 5; // skip 'URI="'
        let Some(end) = line[after_quote..].find('"') else {
            return line.to_string();
        };
        let original_uri = &line[after_quote..after_quote + end];
        let resolved = resolve_url(base_url, original_uri);

        let new_uri = if self.opts.no_proxy {
            resolved
        } else if line.starts_with("#EXT-X-MAP") {
            // Init segment
            let encoded = urlencoding::encode(&resolved);
            let mut qs = format!("d={}", encoded);
            if !self.params.api_password.is_empty() {
                qs.push_str(&format!(
                    "&api_password={}",
                    urlencoding::encode(&self.params.api_password)
                ));
            }
            format!(
                "{}/proxy/hls/segment.mp4?{}",
                self.proxy_base.trim_end_matches('/'),
                qs
            )
        } else if line.starts_with("#EXT-X-MEDIA") {
            // EXT-X-MEDIA URI is a rendition sub-playlist — route through manifest endpoint
            proxy_manifest_url(&self.proxy_base, &resolved, &self.params)
        } else {
            // Key or other URI tag → use key endpoint
            proxy_key_url(&self.proxy_base, &resolved, &self.params)
        };

        line.replacen(
            &format!("URI=\"{}\"", original_uri),
            &format!("URI=\"{}\"", new_uri),
            1,
        )
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Parse the duration value from an `#EXTINF:<duration>[,<title>]` line.
fn parse_extinf_duration(line: &str) -> f64 {
    line.strip_prefix("#EXTINF:")
        .and_then(|rest| rest.split(',').next())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// Inject `#EXT-X-START:TIME-OFFSET=<offset>,PRECISE=YES` right after `#EXTM3U`.
fn inject_ext_x_start(content: &str, offset: f64) -> String {
    if let Some(pos) = content.find("#EXTM3U") {
        let after = pos + "#EXTM3U".len();
        // Find end of the #EXTM3U line
        let nl = content[after..]
            .find('\n')
            .map(|i| after + i + 1)
            .unwrap_or(after);
        let tag = format!("#EXT-X-START:TIME-OFFSET={:.1},PRECISE=YES\n", offset);
        format!("{}{}{}", &content[..nl], tag, &content[nl..])
    } else {
        content.to_string()
    }
}

/// Generate a minimal valid M3U8 that signals a normal stream end.
pub fn graceful_end_playlist(message: &str) -> String {
    format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-PLAYLIST-TYPE:VOD\n# {}\n#EXT-X-ENDLIST\n",
        message
    )
}

/// Generate a minimal valid M3U8 for error scenarios.
pub fn error_playlist(error_message: &str) -> String {
    format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-PLAYLIST-TYPE:VOD\n# Error: {}\n#EXT-X-ENDLIST\n",
        error_message
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_processor(proxy_base: &str) -> ManifestProcessor {
        ManifestProcessor::new(
            proxy_base,
            ProxyParams::new("secret", HashMap::new()),
            ManifestOptions::default(),
        )
    }

    #[test]
    fn test_proxy_segment_url_ts() {
        let params = ProxyParams::new("pass", HashMap::new());
        let url = proxy_segment_url(
            "http://proxy:8888",
            "https://cdn.example.com/seg001.ts",
            &params,
        );
        assert!(url.starts_with("http://proxy:8888/proxy/hls/segment.ts?"));
        assert!(url.contains("d=https"));
        assert!(url.contains("api_password=pass"));
    }

    #[test]
    fn test_proxy_manifest_url() {
        let params = ProxyParams::new("pass", HashMap::new());
        let url = proxy_manifest_url(
            "http://proxy:8888",
            "https://cdn.example.com/playlist.m3u8",
            &params,
        );
        assert!(url.starts_with("http://proxy:8888/proxy/hls/manifest?"));
    }

    #[test]
    fn test_process_media_playlist() {
        let processor = default_processor("http://proxy:8888");
        let m3u8 = b"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:10\n\
            #EXTINF:10.0,\nseg001.ts\n#EXTINF:10.0,\nseg002.ts\n#EXT-X-ENDLIST\n";

        let result = processor.process(m3u8, "https://cdn.example.com/playlist.m3u8");

        // Segment URLs should be rewritten
        assert!(result.contains("/proxy/hls/segment.ts?"));
        assert!(result.contains("cdn.example.com"));
        // Should NOT contain original relative URLs
        assert!(!result.contains("\nseg001.ts\n"));
    }

    #[test]
    fn test_process_master_playlist() {
        let processor = default_processor("http://proxy:8888");
        let m3u8 = b"#EXTM3U\n#EXT-X-VERSION:3\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1400000\nhigh/playlist.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=400000\nlow/playlist.m3u8\n";

        let result = processor.process(m3u8, "https://cdn.example.com/master.m3u8");

        // Variant stream URLs should be rewritten as manifest proxy URLs
        assert!(result.contains("/proxy/hls/manifest?"));
    }

    #[test]
    fn test_no_proxy_mode() {
        let processor = ManifestProcessor::new(
            "http://proxy:8888",
            ProxyParams::new("pass", HashMap::new()),
            ManifestOptions {
                no_proxy: true,
                ..Default::default()
            },
        );
        let m3u8 = b"#EXTM3U\n#EXT-X-TARGETDURATION:10\n\
            #EXTINF:10.0,\nseg001.ts\n#EXT-X-ENDLIST\n";

        let result = processor.process(m3u8, "https://cdn.example.com/playlist.m3u8");

        // Should contain the absolute URL but NOT the proxy path
        assert!(result.contains("cdn.example.com"));
        assert!(!result.contains("/proxy/hls/segment"));
    }

    #[test]
    fn test_inject_ext_x_start() {
        let content = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:10\n";
        let result = inject_ext_x_start(content, -30.0);
        assert!(result.contains("#EXT-X-START:TIME-OFFSET=-30.0,PRECISE=YES"));
        // Must appear after #EXTM3U line
        let extm3u_pos = result.find("#EXTM3U").unwrap();
        let start_pos = result.find("#EXT-X-START").unwrap();
        assert!(start_pos > extm3u_pos);
    }

    /// When a video sub-playlist embeds #EXT-X-MEDIA audio references inline,
    /// m3u8-rs stores them in MediaSegment.unknown_tags and writes them back
    /// verbatim — we must rewrite the URI through the manifest proxy.
    #[test]
    fn test_process_media_with_embedded_ext_x_media() {
        let processor = default_processor("http://proxy:8888");
        let m3u8 = concat!(
            "#EXTM3U\n",
            "#EXT-X-VERSION:3\n",
            "#EXT-X-TARGETDURATION:6\n",
            "#EXT-X-MEDIA-SEQUENCE:0\n",
            // EXT-X-MEDIA embedded inside a video sub-playlist
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio-0\",NAME=\"lang\",DEFAULT=YES,",
            "URI=\"https://upstream.example.com/playlist?type=audio&rendition=lang&token=TOK\"\n",
            "#EXT-X-KEY:METHOD=AES-128,URI=\"https://upstream.example.com/enc.key\",IV=0x0000\n",
            "#EXTINF:6.0,\n",
            "https://cdn.example.com/seg001.ts\n",
            "#EXT-X-ENDLIST\n",
        );
        let result = processor.process(
            m3u8.as_bytes(),
            "https://upstream.example.com/playlist?type=video&rendition=hd",
        );

        // Audio sub-playlist URI must be proxied through manifest endpoint
        assert!(
            result.contains("/proxy/hls/manifest?"),
            "EXT-X-MEDIA URI in media playlist not proxied. Got:\n{}",
            result
        );
        // Must NOT contain the bare audio URL
        assert!(
            !result.contains("URI=\"https://upstream.example.com/playlist?type=audio"),
            "Audio URI is still bare. Got:\n{}",
            result
        );
    }

    /// m3u8-rs rejects FORCED=NO on TYPE=AUDIO (only valid on SUBTITLES) and
    /// puts the whole tag into MasterPlaylist.unknown_tags, where write_to emits
    /// it verbatim with an unproxied URI.  We must catch and rewrite it.
    #[test]
    fn test_process_master_audio_forced_no_proxied() {
        let processor = default_processor("http://proxy:8888");
        let m3u8 = concat!(
            "#EXTM3U\n",
            "#EXT-X-VERSION:3\n",
            // FORCED=NO on AUDIO — m3u8-rs rejects this → goes into unknown_tags
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"lang1\",",
            "DEFAULT=YES,AUTOSELECT=YES,FORCED=NO,LANGUAGE=\"ita\",",
            "URI=\"https://upstream.example.com/playlist?type=audio&rendition=ita&token=TOK\"\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=3000000,AUDIO=\"audio\"\n",
            "https://upstream.example.com/playlist?type=video&rendition=hd&token=VID\n",
        );
        let result = processor.process(m3u8.as_bytes(), "https://upstream.example.com/master.m3u8");

        assert!(
            result.contains("/proxy/hls/manifest?"),
            "Audio URI with FORCED=NO not proxied. Got:\n{}",
            result
        );
        assert!(
            !result.contains("URI=\"https://upstream.example.com/playlist?type=audio"),
            "Audio URI is still bare. Got:\n{}",
            result
        );
    }

    #[test]
    fn test_process_master_ext_x_media_proxied() {
        let processor = default_processor("http://proxy:8888");
        // Master playlist with EXT-X-MEDIA whose URI contains & in the query string
        let m3u8 = concat!(
            "#EXTM3U\n",
            "#EXT-X-VERSION:3\n",
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio-0\",NAME=\"lang1\",DEFAULT=YES,AUTOSELECT=YES,",
            "URI=\"https://upstream.example.com/playlist?type=audio&rendition=lang1&token=TOK\"\n",
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio-0\",NAME=\"lang2\",DEFAULT=NO,AUTOSELECT=YES,",
            "URI=\"https://upstream.example.com/playlist?type=audio&rendition=lang2&token=TOK\"\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=3000000,AUDIO=\"audio-0\"\n",
            "https://upstream.example.com/playlist?type=video&rendition=hd&token=VID\n",
        );
        let result = processor.process(m3u8.as_bytes(), "https://upstream.example.com/master.m3u8");

        // EXT-X-MEDIA URIs must be proxied through manifest endpoint
        assert!(
            result.contains("/proxy/hls/manifest?"),
            "EXT-X-MEDIA URI not proxied. Got:\n{}",
            result
        );
        // The audio URI must NOT appear bare
        assert!(
            !result.contains("URI=\"https://upstream.example.com/playlist?type=audio"),
            "Audio sub-playlist URI is still bare (unproxied). Got:\n{}",
            result
        );
    }
}
