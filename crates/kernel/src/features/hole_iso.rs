//! ISO metric thread / clearance-hole reference data for the Hole feature.
//!
//! Transcribed from **ISO 261** (ISO general-purpose metric screw threads —
//! coarse-pitch series) and **ISO 273 / ISO 20273** (clearance holes for
//! bolts and screws, fine / medium / coarse rows). The tap-drill diameter is
//! the coarse-pitch tapping drill (≈ `nominal − pitch`, the 75 %-thread rule),
//! rounded to the nearest standard drill where the norm does.
//!
//! P1 only consumes [`MetricThread::nominal_mm`] (drilled / simple bores use
//! the nominal diameter). The pitch / tap-drill / clearance columns are carried
//! now so P2 (clearance / tapped hole types + fit dropdown) can derive its
//! effective bore diameter from the same single source — and so the
//! transcription-guard test can assert `tap_drill ≈ nominal − pitch` per row
//! against the norm, rather than trusting a hand-typed table.

/// One row of the ISO metric coarse-pitch table.
#[derive(Debug, Clone, Copy)]
pub struct MetricThread {
    /// Dropdown key, e.g. `"M6"`.
    pub key: &'static str,
    /// Nominal (major) diameter in mm — the drilled/simple bore diameter.
    pub nominal_mm: f32,
    /// Coarse-series pitch in mm (ISO 261).
    pub coarse_pitch_mm: f32,
    /// Tapping-drill (core-hole) diameter in mm for the coarse pitch.
    pub tap_drill_mm: f32,
    /// ISO 273 clearance hole — fine / medium / coarse rows (mm).
    pub clear_close_mm: f32,
    pub clear_normal_mm: f32,
    pub clear_loose_mm: f32,
}

/// ISO metric coarse-pitch series, M2 … M12 (the P1 subset).
pub const ISO_METRIC: &[MetricThread] = &[
    MetricThread { key: "M2",   nominal_mm: 2.0,  coarse_pitch_mm: 0.40, tap_drill_mm: 1.60,  clear_close_mm: 2.2,  clear_normal_mm: 2.4,  clear_loose_mm: 2.6 },
    MetricThread { key: "M2.5", nominal_mm: 2.5,  coarse_pitch_mm: 0.45, tap_drill_mm: 2.05,  clear_close_mm: 2.7,  clear_normal_mm: 2.9,  clear_loose_mm: 3.1 },
    MetricThread { key: "M3",   nominal_mm: 3.0,  coarse_pitch_mm: 0.50, tap_drill_mm: 2.50,  clear_close_mm: 3.2,  clear_normal_mm: 3.4,  clear_loose_mm: 3.6 },
    MetricThread { key: "M4",   nominal_mm: 4.0,  coarse_pitch_mm: 0.70, tap_drill_mm: 3.30,  clear_close_mm: 4.3,  clear_normal_mm: 4.5,  clear_loose_mm: 4.8 },
    MetricThread { key: "M5",   nominal_mm: 5.0,  coarse_pitch_mm: 0.80, tap_drill_mm: 4.20,  clear_close_mm: 5.3,  clear_normal_mm: 5.5,  clear_loose_mm: 5.8 },
    MetricThread { key: "M6",   nominal_mm: 6.0,  coarse_pitch_mm: 1.00, tap_drill_mm: 5.00,  clear_close_mm: 6.4,  clear_normal_mm: 6.6,  clear_loose_mm: 7.0 },
    MetricThread { key: "M8",   nominal_mm: 8.0,  coarse_pitch_mm: 1.25, tap_drill_mm: 6.80,  clear_close_mm: 8.4,  clear_normal_mm: 9.0,  clear_loose_mm: 10.0 },
    MetricThread { key: "M10",  nominal_mm: 10.0, coarse_pitch_mm: 1.50, tap_drill_mm: 8.50,  clear_close_mm: 10.5, clear_normal_mm: 11.0, clear_loose_mm: 12.0 },
    MetricThread { key: "M12",  nominal_mm: 12.0, coarse_pitch_mm: 1.75, tap_drill_mm: 10.20, clear_close_mm: 13.0, clear_normal_mm: 13.5, clear_loose_mm: 14.5 },
];

/// Look up an ISO metric row by its dropdown key (`"M6"`). `None` for an
/// unknown key (e.g. the `"custom"` sentinel, which uses the free diameter).
pub fn metric_by_key(key: &str) -> Option<&'static MetricThread> {
    ISO_METRIC.iter().find(|t| t.key == key)
}

/// The `variants = "…"` spec string for the `standard_size` schema field:
/// every ISO row plus a `custom` sentinel that switches to the free diameter.
pub fn size_variants_spec() -> String {
    let mut s = String::from("custom=Custom (Ø)");
    for t in ISO_METRIC {
        s.push(',');
        s.push_str(t.key);
        s.push('=');
        s.push_str(t.key);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Transcription guard: every coarse-pitch tap drill must sit within
    /// 0.1 mm of `nominal − pitch` (the 75 %-thread rule the norm rounds to a
    /// standard drill). Catches a fat-fingered table cell.
    #[test]
    fn tap_drill_matches_nominal_minus_pitch() {
        for t in ISO_METRIC {
            let expected = t.nominal_mm - t.coarse_pitch_mm;
            assert!(
                (t.tap_drill_mm - expected).abs() <= 0.1,
                "{}: tap_drill {} vs nominal−pitch {}",
                t.key,
                t.tap_drill_mm,
                expected
            );
        }
    }

    /// Clearance rows must be strictly ordered close < normal < loose and all
    /// larger than the nominal diameter (ISO 273 invariant).
    #[test]
    fn clearances_are_ordered_and_above_nominal() {
        for t in ISO_METRIC {
            assert!(t.nominal_mm < t.clear_close_mm, "{}: close ≤ nominal", t.key);
            assert!(t.clear_close_mm < t.clear_normal_mm, "{}: close ≥ normal", t.key);
            assert!(t.clear_normal_mm < t.clear_loose_mm, "{}: normal ≥ loose", t.key);
        }
    }

    #[test]
    fn lookup_by_key_hits_and_misses() {
        assert_eq!(metric_by_key("M6").unwrap().nominal_mm, 6.0);
        assert!(metric_by_key("custom").is_none());
        assert!(metric_by_key("M999").is_none());
    }
}
