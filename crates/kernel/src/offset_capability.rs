//! offset_capability (#384): a machine-readable **capability contract** for
//! Offset/Shell features.
//!
//! The offset_tree pipeline (docs/OFFSET-TREE-PLAN.md) compiles Offset/Shell
//! structurally away at regen time (`expand_shells` → `push_down_offsets` →
//! `fold_offset_leaves` → channel synthesis → pruning). Wherever a pass
//! **refuses**, it silently leaves `f − δ` standing — the user never learned
//! that the result is only approximately dimension-true there. This module
//! records, per feature, WHICH of the four capability tiers the expansion
//! actually reached, plus human-readable reasons for every downgrade.
//!
//! The four tiers (as implemented):
//!
//! * [`CapabilityStatus::Exact`] — every affected leaf was folded analytically
//!   (or `f − δ` on an exact carrier IS the true offset SDF, e.g. dilation on
//!   an exact leaf, or a closed box shell dualised to a shrunk-box
//!   Difference). No correction body was needed, no `f − δ` was left standing
//!   on an inexact carrier.
//! * [`CapabilityStatus::ToleranceBounded`] — the result is correct up to a
//!   **known tolerance**: at least one dual Boolean seam was corrected by an
//!   arc-spline channel / vertex sphere (an arc-spline approximation of the
//!   seam curve, measured to the build path's `channel_tol`), and nothing was
//!   left conservative.
//! * [`CapabilityStatus::Conservative`] — somewhere `f − δ` was left standing
//!   on an inexact carrier (fold refused, or a smooth-blend / morph / warp /
//!   scaled-transform stop, or a dual seam whose channel synthesis refused).
//!   The zero-set may deviate locally from the true offset surface and the
//!   error is not bounded. Carries the list of reasons.
//! * [`CapabilityStatus::Unsupported`] — already rejected today as a
//!   `FeatureError` (Twist/Bend in scope). Kept in the enum for completeness;
//!   realised through the existing feature-error channel, not through a report.
//!
//! **Side-effect-free w.r.t. the field:** a [`CapabilityReport`] only
//! accumulates strings. Recording a reason never touches a distance value, so
//! the compiled/evaluated field is bit-identical whether or not a report is
//! collected (see `offset_capability_tests::field_bit_identical_with_report`).

use serde::Serialize;

/// The four-tier capability of an Offset/Shell feature's expansion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    /// Fully analytic — no correction needed, no inexact `f − δ` left standing.
    Exact,
    /// Approximated to a known tolerance (arc-spline channel correction).
    ToleranceBounded,
    /// `f − δ` left standing somewhere on an inexact carrier — unbounded
    /// local deviation possible.
    Conservative,
    /// Rejected as a `FeatureError` (Twist/Bend in scope).
    Unsupported,
}

impl CapabilityStatus {
    fn rank(self) -> u8 {
        match self {
            Self::Exact => 0,
            Self::ToleranceBounded => 1,
            Self::Conservative => 2,
            Self::Unsupported => 3,
        }
    }
}

/// Diagnostic collector threaded through the offset/shell tree passes
/// (`CsgNode::collect_offset_capability` for the non-dual residuals and
/// `offset_channel::synthesize_offset_channels_reported` for the dual seams).
/// Purely additive — see the module doc on field bit-identity.
#[derive(Debug, Clone, Default)]
pub struct CapabilityReport {
    tolerance: Vec<String>,
    conservative: Vec<String>,
}

impl CapabilityReport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a tolerance-bounded approximation (deduplicated).
    pub fn note_tolerance(&mut self, msg: impl Into<String>) {
        push_unique(&mut self.tolerance, msg.into());
    }

    /// Record a conservative `f − δ` left standing (deduplicated).
    pub fn note_conservative(&mut self, msg: impl Into<String>) {
        push_unique(&mut self.conservative, msg.into());
    }

    pub fn is_empty(&self) -> bool {
        self.tolerance.is_empty() && self.conservative.is_empty()
    }

    /// Aggregate status: the worst downgrade recorded (conservative beats
    /// tolerance-bounded beats exact).
    pub fn status(&self) -> CapabilityStatus {
        if !self.conservative.is_empty() {
            CapabilityStatus::Conservative
        } else if !self.tolerance.is_empty() {
            CapabilityStatus::ToleranceBounded
        } else {
            CapabilityStatus::Exact
        }
    }

    /// Human-readable reasons, conservative first (the ones that actually
    /// degrade correctness), then tolerance-bounded notes.
    pub fn reasons(&self) -> Vec<String> {
        self.conservative
            .iter()
            .chain(self.tolerance.iter())
            .cloned()
            .collect()
    }
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !v.contains(&s) {
        v.push(s);
    }
}

/// Per-feature capability surfaced across the WASM boundary to the UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FeatureCapability {
    pub status: CapabilityStatus,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

impl FeatureCapability {
    pub fn exact() -> Self {
        Self { status: CapabilityStatus::Exact, reasons: Vec::new() }
    }

    pub fn unsupported() -> Self {
        Self { status: CapabilityStatus::Unsupported, reasons: Vec::new() }
    }

    /// Fold the worst of two statuses (used when aggregating a body report
    /// onto a feature that already has a default status).
    pub fn merge_status(&mut self, s: CapabilityStatus) {
        if s.rank() > self.status.rank() {
            self.status = s;
        }
    }
}

#[cfg(test)]
#[path = "offset_capability_tests.rs"]
mod tests;
