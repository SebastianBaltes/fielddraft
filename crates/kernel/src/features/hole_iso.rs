//! ISO metric thread / clearance-hole reference data for the Hole feature.
//!
//! Transcribed from **ISO 261** (ISO general-purpose metric screw threads —
//! coarse-pitch series) and **ISO 273 / ISO 20273** (clearance holes for
//! bolts and screws, fine / medium / coarse rows — identical to the metric
//! table of ASME B18.2.8, whose close / normal / loose naming this file uses).
//! The tap-drill diameter is the coarse-pitch tapping drill
//! (`nominal − pitch`, the 75 %-thread rule), rounded to the nearest standard
//! drill where the norm does.
//!
//! The three columns feed the three hole *types* (#404 P2):
//! `drilled` → [`MetricThread::nominal_mm`], `clearance` →
//! [`MetricThread::clearance_mm`] for the chosen fit, `tapped` →
//! [`MetricThread::tap_drill_mm`]. Tapped is **cosmetic** (Onshape semantics):
//! the real volume removed is the core hole, the thread itself is annotation,
//! never a swept helix.
//!
//! Transcription guards live in the test module below — `tap ≈ nominal −
//! pitch` per row plus the ISO 273 ordering invariant `nominal < close <
//! normal < loose`. Values are transcribed from the norm, never interpolated.

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

impl MetricThread {
    /// Clearance-hole diameter (mm) for the chosen fit key
    /// (`"close" | "normal" | "loose"`). An unknown key falls back to
    /// `normal`, the ISO 273 medium row — the same default the schema ships.
    pub fn clearance_mm(&self, fit: &str) -> f32 {
        match fit {
            "close" => self.clear_close_mm,
            "loose" => self.clear_loose_mm,
            _ => self.clear_normal_mm,
        }
    }
}

/// ISO metric coarse-pitch series, M1.6 … M24.
///
/// Columns: nominal Ø · ISO 261 coarse pitch · tapping drill (`nominal −
/// pitch`) · ISO 273 clearance close / normal / loose.
#[rustfmt::skip]
pub const ISO_METRIC: &[MetricThread] = &[
    MetricThread { key: "M1.6", nominal_mm: 1.6,  coarse_pitch_mm: 0.35, tap_drill_mm: 1.25,  clear_close_mm: 1.7,  clear_normal_mm: 1.8,  clear_loose_mm: 2.0 },
    MetricThread { key: "M2",   nominal_mm: 2.0,  coarse_pitch_mm: 0.40, tap_drill_mm: 1.60,  clear_close_mm: 2.2,  clear_normal_mm: 2.4,  clear_loose_mm: 2.6 },
    MetricThread { key: "M2.5", nominal_mm: 2.5,  coarse_pitch_mm: 0.45, tap_drill_mm: 2.05,  clear_close_mm: 2.7,  clear_normal_mm: 2.9,  clear_loose_mm: 3.1 },
    MetricThread { key: "M3",   nominal_mm: 3.0,  coarse_pitch_mm: 0.50, tap_drill_mm: 2.50,  clear_close_mm: 3.2,  clear_normal_mm: 3.4,  clear_loose_mm: 3.6 },
    MetricThread { key: "M3.5", nominal_mm: 3.5,  coarse_pitch_mm: 0.60, tap_drill_mm: 2.90,  clear_close_mm: 3.7,  clear_normal_mm: 3.9,  clear_loose_mm: 4.2 },
    MetricThread { key: "M4",   nominal_mm: 4.0,  coarse_pitch_mm: 0.70, tap_drill_mm: 3.30,  clear_close_mm: 4.3,  clear_normal_mm: 4.5,  clear_loose_mm: 4.8 },
    MetricThread { key: "M5",   nominal_mm: 5.0,  coarse_pitch_mm: 0.80, tap_drill_mm: 4.20,  clear_close_mm: 5.3,  clear_normal_mm: 5.5,  clear_loose_mm: 5.8 },
    MetricThread { key: "M6",   nominal_mm: 6.0,  coarse_pitch_mm: 1.00, tap_drill_mm: 5.00,  clear_close_mm: 6.4,  clear_normal_mm: 6.6,  clear_loose_mm: 7.0 },
    MetricThread { key: "M8",   nominal_mm: 8.0,  coarse_pitch_mm: 1.25, tap_drill_mm: 6.80,  clear_close_mm: 8.4,  clear_normal_mm: 9.0,  clear_loose_mm: 10.0 },
    MetricThread { key: "M10",  nominal_mm: 10.0, coarse_pitch_mm: 1.50, tap_drill_mm: 8.50,  clear_close_mm: 10.5, clear_normal_mm: 11.0, clear_loose_mm: 12.0 },
    MetricThread { key: "M12",  nominal_mm: 12.0, coarse_pitch_mm: 1.75, tap_drill_mm: 10.20, clear_close_mm: 13.0, clear_normal_mm: 13.5, clear_loose_mm: 14.5 },
    MetricThread { key: "M14",  nominal_mm: 14.0, coarse_pitch_mm: 2.00, tap_drill_mm: 12.00, clear_close_mm: 15.0, clear_normal_mm: 15.5, clear_loose_mm: 16.5 },
    MetricThread { key: "M16",  nominal_mm: 16.0, coarse_pitch_mm: 2.00, tap_drill_mm: 14.00, clear_close_mm: 17.0, clear_normal_mm: 17.5, clear_loose_mm: 18.5 },
    MetricThread { key: "M18",  nominal_mm: 18.0, coarse_pitch_mm: 2.50, tap_drill_mm: 15.50, clear_close_mm: 19.0, clear_normal_mm: 20.0, clear_loose_mm: 21.0 },
    MetricThread { key: "M20",  nominal_mm: 20.0, coarse_pitch_mm: 2.50, tap_drill_mm: 17.50, clear_close_mm: 21.0, clear_normal_mm: 22.0, clear_loose_mm: 24.0 },
    MetricThread { key: "M22",  nominal_mm: 22.0, coarse_pitch_mm: 2.50, tap_drill_mm: 19.50, clear_close_mm: 23.0, clear_normal_mm: 24.0, clear_loose_mm: 26.0 },
    MetricThread { key: "M24",  nominal_mm: 24.0, coarse_pitch_mm: 3.00, tap_drill_mm: 21.00, clear_close_mm: 25.0, clear_normal_mm: 26.0, clear_loose_mm: 28.0 },
];

/// Look up an ISO metric row by its dropdown key (`"M6"`). `None` for an
/// unknown key (e.g. the `"custom"` sentinel, which uses the free diameter).
pub fn metric_by_key(key: &str) -> Option<&'static MetricThread> {
    ISO_METRIC.iter().find(|t| t.key == key)
}

/// The `variants = "…"` spec string for the `standard_size` schema field:
/// every ISO row plus a `custom` sentinel that switches to the free diameter.
///
/// The schema macro needs a string *literal*, so `HoleDef` spells the list
/// out; `hole_size_variants_match_iso_table` asserts the two never drift.
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

    /// The table must be strictly ascending in nominal diameter (a duplicated
    /// or out-of-order row would make the dropdown lie) and the key must be
    /// the `M`-prefixed nominal — a mismatch means a copy-paste slip.
    #[test]
    fn rows_ascend_and_keys_match_nominal() {
        for w in ISO_METRIC.windows(2) {
            assert!(
                w[0].nominal_mm < w[1].nominal_mm,
                "table not ascending at {} → {}",
                w[0].key,
                w[1].key
            );
        }
        for t in ISO_METRIC {
            let from_key: f32 = t.key.trim_start_matches('M').parse().expect("M<number> key");
            assert!(
                (from_key - t.nominal_mm).abs() < 1e-6,
                "{}: key says {from_key}, nominal says {}",
                t.key,
                t.nominal_mm
            );
        }
    }

    /// Tapped bores are always *smaller* than the nominal (core hole) and
    /// clearance bores always *larger* — the invariant that makes the three
    /// hole types visibly different (#404 P2).
    #[test]
    fn tap_below_nominal_below_clearance() {
        for t in ISO_METRIC {
            assert!(t.tap_drill_mm < t.nominal_mm, "{}: tap ≥ nominal", t.key);
            assert!(t.nominal_mm < t.clearance_mm("close"), "{}: close ≤ nominal", t.key);
        }
    }

    /// Fit lookup: the three keys hit their columns, anything else (including
    /// an empty string from a half-built dialog) falls back to `normal`.
    #[test]
    fn fit_lookup_selects_the_right_column() {
        let m6 = metric_by_key("M6").unwrap();
        assert_eq!(m6.clearance_mm("close"), 6.4);
        assert_eq!(m6.clearance_mm("normal"), 6.6);
        assert_eq!(m6.clearance_mm("loose"), 7.0);
        assert_eq!(m6.clearance_mm(""), 6.6, "unknown fit → normal");
    }

    #[test]
    fn lookup_by_key_hits_and_misses() {
        assert_eq!(metric_by_key("M6").unwrap().nominal_mm, 6.0);
        assert_eq!(metric_by_key("M24").unwrap().tap_drill_mm, 21.0);
        assert!(metric_by_key("custom").is_none());
        assert!(metric_by_key("M999").is_none());
    }
}
