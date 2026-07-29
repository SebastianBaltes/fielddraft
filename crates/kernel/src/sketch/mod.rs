//! Sketch sub-feature-tree: per-sketch `Vec<FeatureCall>` spine.
//!
//! K6b.1, the first slice of the sketch editor. This module stands
//! up the kernel-side infrastructure that the memory note in
//! `project_sketch_subtree.md` requires: every sketch owns its own
//! fully-typed feature spine, every sketch tool is a feature with
//! a schema, and regeneration walks the sub-spine the same way
//! top-level regeneration walks the document spine.
//!
//! ## Shape of the design
//!
//! * A new top-level `FeatureKind` called `sketch` lives in the
//!   main [`FeatureRegistry`]. Its definition JSON carries two
//!   fields:
//!     - `plane` — a [`Query`] identifying the planar target the
//!       sketch is locked against. K6b.1 stores the query
//!       verbatim but does not resolve it; the face-lock UX is
//!       K6b.2's job.
//!     - `sub_calls` — an ordered array of nested
//!       [`FeatureCall`]s, the sketch's own feature spine.
//!
//! * Each entry of `sub_calls` dispatches through a **separate**
//!   [`SketchToolRegistry`]. That registry's types implement
//!   [`SketchToolKind`], not [`FeatureKind`] — a sketch tool's
//!   job is to push entities into a [`SketchState`], not to
//!   mutate the document spine. Keeping the two registries
//!   disjoint means a top-level call like `box` can't accidentally
//!   appear inside a sketch and vice versa; the type namespaces
//!   stay clean and regeneration errors point at the right layer.
//!
//! * The sketch feature's `execute` builds a fresh [`SketchState`],
//!   walks its sub_calls through the sketch-tool registry, and
//!   stashes the resulting state on
//!   [`Context::sketches`] keyed by the sketch's feature id. Later
//!   slices (K6b.2+) will route that state through the PlaneGCS
//!   solver and resolve downstream `qSketchRegion` queries against
//!   it. K6b.1 stops short of solving because the solver lives in
//!   JavaScript — the state is ready for the JS-side wrapper to
//!   pick up.
//!
//! ## Built-in sketch tools in K6b.1
//!
//! * `sketch_point { at: [x, y] }` — one free point.
//! * `sketch_line  { start: [x, y], end: [x, y] }` — two points
//!   plus a line entity linking them.
//!
//! That's the minimum set needed to smoke-test the sub-spine
//! pipeline end-to-end. Rectangle / Circle / Arc / Trim / Mirror /
//! Offset land in K6b.3 and K6b.4, each as their own
//! `SketchToolKind` impl.

use fielddraft_protocol::FeatureCall;
use fielddraft_solver::SketchState;

// ─── Modul-Layout ────────────────────────────────────────────────
//
// sketch_features.rs wurde mechanisch in ein Modul-Verzeichnis
// gesplittet. Die flachen `crate::sketch::*`-Pfade bleiben über die
// Re-Exports unten stabil.

pub mod constraints;
pub mod feature;
pub mod frame;
pub mod loops;
pub mod lower;
pub mod spline_math;
pub mod tools;

pub use constraints::*;
pub use feature::*;
pub use frame::*;
pub use lower::*;
pub use spline_math::*;
pub use tools::*;

// ─── #452: fence curves ─────────────────────────────────────────

/// Param key that marks a sketch sub-call as a **fence** curve: geometry the
/// sketch did not draw but is bounded by — today the projected boundary of the
/// FACE the sketch stands on (see [`feature::carrier_fence_sub_calls`]).
///
/// A fence curve is deliberately NOT a third kind of sub-call but the ordinary
/// `sketch_line` / `sketch_arc` / `sketch_circle` with one extra flag, exactly
/// as `construction` is. The two flags are the two halves of a table:
///
/// |               | region arrangement | extrudable profile |
/// |---------------|--------------------|--------------------|
/// | plain         | yes                | yes                |
/// | `construction`| no                 | no                 |
/// | `fence`       | yes                | **no**             |
///
/// So a fence can CLOSE a region the user's own curves leave open, while
/// `qCreatedBy(<sketch>)` — the whole-sketch profile — comes out bit-identical
/// to a sketch without one.
pub const FENCE_PARAM: &str = "fence";

/// Is this sub-call a [fence](FENCE_PARAM) curve?
pub fn is_fence_call(call: &FeatureCall) -> bool {
    call.params
        .get(FENCE_PARAM)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

// ─── K7.0: Context-side sketch handle ───────────────────────────

/// Bundle of everything a downstream feature (extrude, revolve,
/// etc.) needs about a resolved sketch: the solver state the
/// kernel walked during regeneration, plus the original sub-spine
/// calls so profile extractors can tell a `sketch_rectangle`
/// call apart from four free `sketch_line` segments. Stored in
/// [`Context::sketches`] keyed by the sketch feature's id.
#[derive(Debug, Clone)]
pub struct SketchData {
    pub state: SketchState,
    pub sub_calls: Vec<FeatureCall>,
    /// C1: the 3D placement frame the 2D sketch lives on. Sketch
    /// entities stay 2D (`(u, v)` in the frame's `x_axis`/`y_axis`
    /// plane); lowering (extrude/revolve) composes
    /// [`SketchFrame::to_transform`] into the emitted feature transform
    /// so the profile sits on the real plane. `origin_plane_xy` yields
    /// the identity frame so legacy XY sketches are unchanged.
    pub frame: SketchFrame,
}


// ─── tests ──────────────────────────────────────────────────────

#[cfg(test)]
#[path = "../sketch_features_tests.rs"]
mod tests;
