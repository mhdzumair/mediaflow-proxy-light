//! DASH segment URL template expansion and URL resolution utilities.
//!
//! Ports `create_segment_data()` and `resolve_url()` from `mpd_utils.py`.

use url::Url;

// ---------------------------------------------------------------------------
// URL resolution
// ---------------------------------------------------------------------------

/// Resolve a (potentially relative) URL against a base URL.
/// - Absolute URLs (start with http:// / https://) are returned unchanged.
/// - Absolute path URLs (start with /) are resolved against the origin.
/// - Relative paths are resolved against the directory portion of `base_url`.
pub fn resolve_url(base_url: &str, relative: &str) -> String {
    if relative.is_empty() {
        return base_url.to_string();
    }
    if relative.starts_with("http://") || relative.starts_with("https://") {
        return relative.to_string();
    }
    if let Ok(base) = Url::parse(base_url) {
        if let Ok(resolved) = base.join(relative) {
            return resolved.to_string();
        }
    }
    // Fallback: simple concatenation for relative paths
    if base_url.ends_with('/') {
        format!("{base_url}{relative}")
    } else if let Some(dir) = base_url.rfind('/') {
        format!("{}/{relative}", &base_url[..dir])
    } else {
        relative.to_string()
    }
}

// ---------------------------------------------------------------------------
// Template variable expansion
// ---------------------------------------------------------------------------

/// Expand a DASH SegmentTemplate URL pattern.
///
/// Replaces `$RepresentationID$`, `$Number$`, `$Number%04d$`, `$Bandwidth$`,
/// and `$Time$` with the provided values.
pub fn expand_template(
    template: &str,
    representation_id: &str,
    bandwidth: u64,
    number: u64,
    time: Option<u64>,
) -> String {
    let mut s = template.to_string();
    s = s.replace("$RepresentationID$", representation_id);
    s = s.replace("$Bandwidth$", &bandwidth.to_string());

    // $Number%04d$ style padding — search for closing $ after the prefix
    if s.contains("$Number%") {
        let prefix = "$Number%";
        if let Some(start) = s.find(prefix) {
            let after_prefix = start + prefix.len();
            // Find the closing $ starting after the prefix
            if let Some(rel_end) = s[after_prefix..].find('$') {
                let end = after_prefix + rel_end;
                let fmt = format!("%{}", &s[after_prefix..end]); // e.g. "%05d"
                let padded = format_number_with_spec(&fmt, number);
                s = format!("{}{}{}", &s[..start], padded, &s[end + 1..]);
            }
        }
    }
    s = s.replace("$Number$", &number.to_string());

    if let Some(t) = time {
        s = s.replace("$Time$", &t.to_string());
    }

    s
}

/// Simple C-style `%NNd` number formatter (only supports zero-padded decimal).
fn format_number_with_spec(spec: &str, n: u64) -> String {
    // strip leading '%' and trailing 'd'
    let spec = spec.trim_start_matches('%').trim_end_matches('d');
    if let Ok(width) = spec.parse::<usize>() {
        format!("{:0>width$}", n, width = width)
    } else {
        n.to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_template_number() {
        let tmpl = "seg$Number$.m4s";
        assert_eq!(expand_template(tmpl, "v1", 1000000, 5, None), "seg5.m4s");
    }

    #[test]
    fn test_expand_template_padded() {
        let tmpl = "seg$Number%05d$.m4s";
        assert_eq!(
            expand_template(tmpl, "v1", 1000000, 5, None),
            "seg00005.m4s"
        );
    }

    #[test]
    fn test_expand_template_time() {
        let tmpl = "chunk_ctvideo_cfm4s_ridv0_ts$Time$.m4s";
        assert_eq!(
            expand_template(tmpl, "v0", 1000000, 1, Some(180000)),
            "chunk_ctvideo_cfm4s_ridv0_ts180000.m4s"
        );
    }

    #[test]
    fn test_expand_template_init() {
        let tmpl = "init_$RepresentationID$.mp4";
        assert_eq!(
            expand_template(tmpl, "video_1", 2000000, 1, None),
            "init_video_1.mp4"
        );
    }

    #[test]
    fn test_resolve_url_absolute() {
        assert_eq!(
            resolve_url(
                "https://example.com/manifest.mpd",
                "https://cdn.com/seg1.mp4"
            ),
            "https://cdn.com/seg1.mp4"
        );
    }

    #[test]
    fn test_resolve_url_relative() {
        assert_eq!(
            resolve_url("https://example.com/path/manifest.mpd", "segs/seg1.mp4"),
            "https://example.com/path/segs/seg1.mp4"
        );
    }

    #[test]
    fn test_resolve_url_absolute_path() {
        assert_eq!(
            resolve_url("https://example.com/path/manifest.mpd", "/segs/seg1.mp4"),
            "https://example.com/segs/seg1.mp4"
        );
    }
}
