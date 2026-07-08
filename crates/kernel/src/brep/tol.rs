//! Central tolerance policy of the B-Rep pipeline (plan Phase 5a,
//! `docs/BREP-CLASSIFICATION-PLAN.md`).
//!
//! Every tolerance that decides an *identity or negligibility question* —
//! "are these the same surface / the same point / close enough to touch?" —
//! lives here, with a name, a rationale and one of three classes:
//!
//! - **Scale-relative** (the default): a fraction of the body's AABB
//!   diagonal. Geometry questions scale with the model; absolute epsilons
//!   are the root of the oa9h/vyee class of bugs (behaviour flips with
//!   model size).
//! - **Dimensionless**: angles and ratios (sin θ between unit vectors,
//!   fractions of a local spacing). Scale-free by construction.
//! - **Frozen absolute**: world-unit constants kept at their historical
//!   value because converting them is a behaviour change that the current
//!   test net does not yet cover. Each carries a conversion mandate; do not
//!   add new members to this class.
//!
//! Two kinds of numbers deliberately do NOT live here:
//!
//! - **Degenerate guards** (e.g. `len < 1e-9` before normalising a vector):
//!   pure div-by-zero protection, not a geometric decision — they stay at
//!   their use site.
//! - **Algorithm parameters** (seed counts, iteration caps, gap factors
//!   like `CHAIN_GAP_FACTOR`): they tune convergence and clustering
//!   behaviour, not identity — they stay with their algorithm.
//!
//! Adding a tolerance? Add it here, classify it, justify the value, and
//! derive it from an existing anchor if the question is related (see
//! [`ON_BOUNDARY_REL_TOL`] for the pattern).
//!
//! ## Kernel-wide audit (2026-07-07, ARCHITECTURE-ROADMAP Punkt 3)
//!
//! The constants OUTSIDE `brep/` were surveyed against this policy. The
//! remaining absolute literals fall into categories that are exempt by the
//! rules above or by domain:
//!
//! - **Dimensionless** — quaternion axis tests, feature-angle cosines,
//!   Lipschitz guards, fit weights: scale-free, stay at their site.
//! - **Sketch/mm-domain** — sketch coordinates are millimetres by unit
//!   convention (`LOOP_JOIN_TOL`, `SPLINE_LOWER_TOL`, snap radii): these are
//!   *user-scale* thresholds, not geometry-identity epsilons; converting
//!   them to model-relative would change UX, not robustness.
//! - **Meshing/export** (`decimate`, `export_mesh`, `dc`/`mt` samplers):
//!   resolution-domain parameters tied to cell size, audited separately
//!   with the export-mesher work line.
//! - Genuinely scale-fragile stragglers are migrated as found (e.g. the
//!   body-docking AABB slack in `body.rs::aabbs_overlap`, now
//!   span-relative with a 1 µm floor). New geometric length tolerances
//!   anywhere in the kernel should anchor here.

// ─── Anchor: coincidence (plan Phase 1) ─────────────────────────────────────

/// THE coincidence tolerance: relative to the body's AABB diagonal for
/// lengths ([`coincidence_len`]), raw for angles ([`COINCIDENCE_ANG_TOL`]).
/// The single allowed tolerance of the coincidence topic — carriers are
/// compared symbolically on construction parameters, and this constant only
/// covers honest numeric drift through transform chains.
///
/// Sizing: construction-identical parameters compare bit-equal and pass any
/// tolerance. The binding case is **mesh-derived face-extrude geometry**
/// (ticket o2j9/px2o): a face picked off the woven B-Rep anchors its plane
/// on sampled positions that sit up to ~0.5 µm off the exact bound at
/// 100 mm scale (~5e-6 relative). 1e-5 covers that with 2× headroom while
/// still being far below feature size (2.8 µm at a 280 mm diagonal). It must
/// stay well above f32 round-off (≈ 6e-8 relative, times transform-chain
/// headroom) and well below feature size.
pub const COINCIDENCE_REL_TOL: f64 = 1e-5;

/// Length form of the coincidence tolerance. The `max(1.0)` clamp keeps the
/// registry usable on degenerate (near-zero-extent) bodies.
pub fn coincidence_len(diag: f64) -> f64 {
    diag.max(1.0) * COINCIDENCE_REL_TOL
}

/// Angular form of the coincidence tolerance (dimensionless: compared
/// against `‖n₁ × n₂‖ = sin θ` of unit normals, ≈ θ in radians).
pub const COINCIDENCE_ANG_TOL: f64 = COINCIDENCE_REL_TOL;

// ─── Derived identity tolerances (scale-relative) ──────────────────────────

/// Identity tolerance for *traced/refined* positions on the body boundary:
/// vertex welding across a flush seam, point-contact detection, membrane
/// and fin on-boundary tests ([`on_boundary`]).
///
/// Derived: 10 × [`COINCIDENCE_REL_TOL`]. Carrier surfaces are exact to the
/// coincidence tolerance; positions that *live* on them went through
/// sampling, marching and Newton refinement on f32 fields, which adds about
/// an order of magnitude of honest noise on top.
pub const ON_BOUNDARY_REL_TOL: f64 = 10.0 * COINCIDENCE_REL_TOL;

/// Length form of [`ON_BOUNDARY_REL_TOL`] (no clamp: callers operate on
/// woven bodies whose diagonal is the genuine scale).
pub fn on_boundary(diag: f64) -> f64 {
    diag * ON_BOUNDARY_REL_TOL
}

/// Identity tolerance for *chord-sampled* loop centroids (closed-rim
/// dedup in the coincidence edge merge). Much looser than [`on_boundary`]:
/// a centroid of a sagitta-limited polyline of a curved rim carries the
/// full arc-to-chord error, not just refinement noise. Value matches the
/// `ADAPT_*` sagitta budget with ~20× gathering headroom.
pub const LOOP_CENTROID_REL_TOL: f64 = 2e-3;

/// Length form of [`LOOP_CENTROID_REL_TOL`].
pub fn loop_centroid(diag: f64) -> f64 {
    diag * LOOP_CENTROID_REL_TOL
}

// ─── Trace fidelity (scale-relative fractions, both weave routes) ──────────
//
// One set of constants for the octree route (`edge_walker`) and the seed
// route (`seed_weaver`): a seed-woven edge and an octree-woven one of the
// same geometry must sample to the same chord fidelity, or downstream
// dedup/identity checks see route-dependent geometry.

/// Arc-to-chord (sagitta) budget per traced segment: 1e-4 of the model
/// diagonal → ~0.1 mm on a 1 m object, independent of octree resolution.
pub const ADAPT_TOL_FRACTION: f64 = 1.0e-4;

/// High-curvature stride floor: bounds the work at a near-crease (a true
/// crease is a vertex, handled by topology) while still allowing sub-mm
/// sampling.
pub const ADAPT_MIN_FRACTION: f64 = 1.0e-4;

/// Straight-run stride cap: on a near-flat seam the step grows to this
/// fraction of the diagonal. Kept modest so the widened `vertex_reach`/
/// `close_tol` stay well below an edge's length and never short-circuit its
/// topology.
pub const ADAPT_MAX_FRACTION: f64 = 0.1;

// ─── Dimensionless guards ───────────────────────────────────────────────────

/// "Two unit directions are (anti)parallel": compared against the norm of
/// their cross product (= sin θ) or a single unit-vector component.
/// Dimensionless; 1e-6 ≈ 0.2 arc-seconds, far below any constructed angle
/// yet far above f32 round-off on a normalised vector.
pub const PARALLEL_EPS: f32 = 1e-6;

// ─── Frozen absolutes (world units — conversion mandate, do not extend) ────
//
// Each constant below answers a geometric question and should be
// scale-relative, but its value predates the policy and converting it
// changes behaviour outside the current gate coverage (the gate family
// tests up to ~1 m; these matter at the extremes). Mandate: convert to a
// diagonal-relative form once a test pins the behaviour at ≥ 1 m and
// ≤ 1 mm model scale. Until then they are frozen here so the full
// tolerance inventory has one home.

/// Inclusive slack on AABB membership / overlap so a seed or a branch pair
/// landing exactly on a box wall is not rejected by float rounding
/// (`ssi` line-box clipping, `branch_pairs` touch test). 1e-4 (0.1 µm at
/// mm units) is well below modelling-relevant feature size yet comfortably
/// above float noise from transform-composed leaf AABBs.
pub const AABB_TOUCH_SLACK: f32 = 1e-4;

/// Convergence threshold on the largest per-surface signed-distance
/// residual when re-solving an analytic triple vertex (Gauss-Newton in
/// f32). A near-tangential corner whose surfaces do not truly meet leaves
/// the residual above this and is rejected (keep centroid).
pub const VERTEX_REFINE_TOL: f32 = 1e-3;

/// Convergence tolerance on `max(|f_a|, |f_b|)` when projecting a point
/// onto an SSI curve (f64 Newton). The fields are true distances near
/// their surfaces, so this is a metric tolerance.
pub const PROJECT_TOL: f64 = 1e-9;

/// Segment-length floor at which the activity bisection stops early — once
/// the active and inactive endpoints are this close, the boundary is
/// pinned to world-ε.
pub const BISECT_ACTIVE_TOL: f64 = 1e-7;
