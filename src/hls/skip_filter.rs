//! Segment skip-filter — direct port of Python `SkipSegmentFilter`.
//!
//! Tracks cumulative playback time and identifies segments that overlap
//! with any of the configured skip ranges.

/// One time range that should be skipped (start/end in seconds).
#[derive(Debug, Clone)]
pub struct SkipRange {
    pub start: f64,
    pub end: f64,
}

impl SkipRange {
    pub fn new(start: f64, end: f64) -> Self {
        Self { start, end }
    }
}

/// Stateful filter that consumes one segment at a time and reports whether
/// that segment overlaps with any configured skip range.
#[derive(Debug, Default)]
pub struct SkipSegmentFilter {
    ranges: Vec<SkipRange>,
    current_time: f64,
}

impl SkipSegmentFilter {
    /// Build a new filter from a slice of `(start, end)` pairs.
    pub fn new(ranges: Vec<SkipRange>) -> Self {
        Self {
            ranges,
            current_time: 0.0,
        }
    }

    /// Returns `true` if the next segment (with the given duration) overlaps
    /// any skip range.  Always advances the internal clock regardless.
    pub fn check_and_advance(&mut self, duration: f64) -> bool {
        let segment_start = self.current_time;
        let segment_end = segment_start + duration;
        self.current_time = segment_end;

        self.ranges.iter().any(|r| {
            // Overlap check: start < range_end AND end > range_start
            segment_start < r.end && segment_end > r.start
        })
    }

    /// Returns `true` if any skip ranges are configured.
    pub fn is_active(&self) -> bool {
        !self.ranges.is_empty()
    }

    /// Current cumulative playback time in seconds.
    pub fn current_time(&self) -> f64 {
        self.current_time
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_ranges() {
        let mut f = SkipSegmentFilter::default();
        assert!(!f.is_active());
        assert!(!f.check_and_advance(10.0));
    }

    #[test]
    fn test_skip_overlapping_segment() {
        // Skip range: 5s - 15s
        let mut f = SkipSegmentFilter::new(vec![SkipRange::new(5.0, 15.0)]);
        // Segment 0-10s: overlaps with 5-15 → should skip
        assert!(f.check_and_advance(10.0));
        // Segment 10-20s: overlaps with 5-15 → should skip
        assert!(f.check_and_advance(10.0));
        // Segment 20-30s: no overlap → should keep
        assert!(!f.check_and_advance(10.0));
    }

    #[test]
    fn test_segment_before_skip_range() {
        let mut f = SkipSegmentFilter::new(vec![SkipRange::new(30.0, 60.0)]);
        // Segment 0-10: no overlap
        assert!(!f.check_and_advance(10.0));
        // Segment 10-20: no overlap
        assert!(!f.check_and_advance(10.0));
        // Segment 20-30: edge case (ends exactly at start of skip) → no overlap
        assert!(!f.check_and_advance(10.0));
        // Segment 30-40: overlaps → skip
        assert!(f.check_and_advance(10.0));
    }

    #[test]
    fn test_time_advances_on_skip() {
        let mut f = SkipSegmentFilter::new(vec![SkipRange::new(0.0, 5.0)]);
        f.check_and_advance(3.0); // skipped, time = 3.0
        assert!((f.current_time() - 3.0).abs() < 1e-9);
    }
}
