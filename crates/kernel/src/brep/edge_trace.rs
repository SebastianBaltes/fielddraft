//! General numeric SDF edge tracer (DL-68).
//!
//! An edge is the joint zero-set of two per-face scalar fields,
//! `E = { p : f_a(p) = 0 ∧ f_b(p) = 0 }`. Two evaluable fields with usable
//! gradients are all this needs — it never asks whether a face is a plane, a
//! sphere, a swept Bézier or a domain-warp. That is the whole point: it
//! replaces DL-67's per-pair closed-form circle (and its slide toward "1000
//! analytic special cases") with **one** method that generalises to any face
//! the SDF can evaluate.
//!
//! Two building blocks:
//!   1. **Project** ([`project_to_edge`]) — Newton on `(f_a, f_b) = (0, 0)`
//!      pulls a near-edge sample *exactly* onto the edge (quadratic
//!      convergence, independent of where the sample sat). Cures jaggedness.
//!   2. **Trace** ([`trace_edge`]) — predictor-corrector march along the edge
//!      tangent `t = ∇f_a × ∇f_b`: step `step·t̂`, reproject onto the edge, from
//!      an interior seed in *both* directions until each end hits a bounding
//!      vertex or the march closes a loop. Cures fragmentation / missing pieces:
//!      the edge geometry comes from the surfaces, not from how many octree
//!      cells happened to tag the seam.
//!
//! Everything runs in **f64**: the dual SDF layer is f64 internally, and the
//! rank guard near tangential intersections lives well below f32-ε, so the f32
//! render/mesh floor would cap the achievable exactness. See
//! [`crate::sdf_dual::halfspace_value_grad`] / [`crate::sdf_dual::bounded_value_grad`].
//!
//! This is the edge (1D) analogue of DL-66's [`super::edge_walker`]
//! `refine_triple_vertex` (a 0D vertex = joint zero-set of *three* fields).

use crate::sketch2d::BoundedSketch2d;
use fielddraft_protocol::{HalfSpace, Transform};

use super::vec3::{add, cross, dist, dot, norm, normalize, scale, sub};
use crate::bvh::BoundedCsg;
use crate::csg::EvalMode;
use crate::sdf_dual::{bounded_value_grad, extrude_wall_value_grad, halfspace_value_grad};

/// One per-face scalar field whose zero set *is* that face, together with a
/// usable gradient. The edge tracer is generic over which kind backs a face:
/// analytic primitives get the exact, globally smooth half-space; everything
/// else (Extrude/Revolve/sweep/warp) gets its leaf SDF via dual numbers.
pub(crate) enum Field<'a> {
    /// Analytic per-face half-space (box face, sphere, infinite cylinder, cone,
    /// torus) in its centred local frame, plus the local→world transform.
    /// Globally smooth — the preferred field wherever it exists, because the
    /// leaf SDF of a multi-face primitive (e.g. a box) is only C0 at the
    /// primitive's own edges.
    Analytic { hs: HalfSpace, xf: Transform },
    /// General fallback: a single leaf subtree evaluated by the dual SDF layer.
    /// Its zero set is the whole primitive boundary; near a seam, away from the
    /// primitive's own creases, it is smooth and its gradient is the surface
    /// normal — enough for project + trace on swept faces.
    ///
    /// Wired in `build_wireframe` (DL-69) for branches with no analytic
    /// half-space — a curved sketch-sweep wall (sector arc, Bézier/arc path).
    /// Restricted there to **cross-feature** seams: a curved wall meeting another
    /// body's face or wall, where the leaf SDF is smooth at the seam. A sweep's
    /// *own* wall∩wall seam is exactly the leaf SDF's C0 crease (ambiguous
    /// gradient), so it is excluded and falls back to the sample fits.
    Leaf { node: &'a BoundedCsg },
    /// An extrude lateral wall as the **cap-clamp-free** swept profile (DL-77):
    /// the *signed* profile SDF of `sketch`, swept infinitely along the local
    /// extrude axis, rescaled to world by `min_scale`. Unlike [`Leaf`](Self::Leaf)
    /// it drops the `|z|−h` cap term, so the field is smooth *across the rim*
    /// (`z = ±h`) — letting a rim seam (this wall ∩ the analytic cap plane) trace
    /// cleanly instead of shattering into the fragments the leaf's C0 crease
    /// forces onto the sample fits. The whole profile is one field; the per-group
    /// bounding corners decide which segment-arc each seam recovers.
    ExtrudeWall {
        sketch: &'a BoundedSketch2d,
        xf: Transform,
    },
    /// An analytic half-space under a **domain-warp chain** (Twist / Bend):
    /// `f(p) = hs(world_to_local(xf, T(p)))` with `T` the chain's closed-form
    /// rück-warp — the per-face field of a warped body's face, evaluated at
    /// *warped* world points. A warp is a diffeomorphism, so the field is
    /// smooth wherever the carrier is and its zero set is exactly the warped
    /// face. Lets the tracer march the helical edges of a twisted box exactly
    /// instead of abandoning warped bodies wholesale to the octree sample fits.
    WarpedAnalytic {
        hs: HalfSpace,
        xf: Transform,
        warps: Vec<crate::sdf_dual::DomainWarp>,
    },
}

impl Field<'_> {
    /// Field value and gradient at a world-space point, in f64.
    #[inline]
    pub(crate) fn value_grad(&self, p: [f64; 3]) -> (f64, [f64; 3]) {
        match self {
            Field::Analytic { hs, xf } => halfspace_value_grad(hs, xf, p),
            // A single leaf has no smooth blend, so Logical == Visual; Logical
            // is the well-defined hard branch the B-Rep pipeline extracts in.
            Field::Leaf { node } => bounded_value_grad(node, p, EvalMode::Logical),
            Field::ExtrudeWall { sketch, xf } => extrude_wall_value_grad(sketch, xf, p),
            Field::WarpedAnalytic { hs, xf, warps } => {
                crate::sdf_dual::warped_halfspace_value_grad(hs, xf, warps, p)
            }
        }
    }

    /// The field's intrinsic VALUE noise floor at `p`. The classic analytic
    /// half-spaces evaluate from exact f32 parameters in f64 and reach the
    /// frozen `PROJECT_TOL = 1e-9`; a [`HalfSpace::RevolvedProfile`] carrier
    /// runs its closest-point solve in f32, so its value carries
    /// ~ε_f32 · |coordinate| of quantisation noise that NO Newton iteration
    /// can push below. Projection acceptance uses `max(PROJECT_TOL, floor)`.
    #[inline]
    fn noise_floor(&self, p: [f64; 3]) -> f64 {
        match self {
            Field::Analytic { hs: HalfSpace::RevolvedProfile { .. }, .. }
            | Field::WarpedAnalytic { hs: HalfSpace::RevolvedProfile { .. }, .. } => {
                let linf = p[0].abs().max(p[1].abs()).max(p[2].abs());
                64.0 * (f32::EPSILON as f64) * (1.0 + linf)
            }
            _ => 0.0,
        }
    }
}

/// Newton iteration cap. With quadratic convergence 8 is generous; a sample
/// that has not converged by then sits on a degenerate/spurious pair.
const PROJECT_MAX_ITER: usize = 8;
// Convergence tolerance on `max(|f_a|, |f_b|)` — frozen-absolute, see
// `brep::tol`.
use super::tol::PROJECT_TOL;
/// Rank guard: reject the step when `sin²θ` between the two surface normals
/// drops below this (θ ≲ 0.06°). There the cross product collapses, the
/// Jacobian is rank-deficient, and the "edge" is a tangential degeneracy with
/// no isolated curve — mirrors `refine_triple_vertex`'s eigenvalue guard one
/// dimension up.
const RANK_SIN2_MIN: f64 = 1e-6;

/// Project a near-edge point exactly onto `{f_a = 0, f_b = 0}` by minimum-norm
/// Newton.
///
/// The residual is `F = [f_a; f_b]` with Jacobian `J = [∇f_a; ∇f_b]` (2×3).
/// `J` is wide, so the minimum-norm Newton step is
/// `Δ = Jᵀ (J Jᵀ)⁻¹ (−F)`: it moves `p` only within the plane spanned by the
/// two normals (driving both distances linearly to zero) and leaves the edge
/// tangent untouched. `J Jᵀ` is the 2×2 Gram matrix of the gradients, whose
/// determinant is exactly `|∇f_a × ∇f_b|²` — the squared edge-tangent length —
/// so the rank guard and the tangent share one quantity.
///
/// Returns `None` when the pair is tangential/rank-deficient at `p` or the
/// iteration fails to converge (a spurious sample that is not actually near a
/// shared zero).
pub(crate) fn project_to_edge(fa: &Field, fb: &Field, p0: [f64; 3]) -> Option<[f64; 3]> {
    let mut p = p0;
    for _ in 0..PROJECT_MAX_ITER {
        let (va, ga) = fa.value_grad(p);
        let (vb, gb) = fb.value_grad(p);
        let tol = PROJECT_TOL.max(fa.noise_floor(p)).max(fb.noise_floor(p));
        if va.abs().max(vb.abs()) < tol {
            return Some(p);
        }
        if !va.is_finite() || !vb.is_finite() {
            return None;
        }

        // 2×2 Gram matrix M = J Jᵀ.
        let m00 = dot(ga, ga);
        let m01 = dot(ga, gb);
        let m11 = dot(gb, gb);
        let det = m00 * m11 - m01 * m01; // = |ga × gb|²

        // Scale-invariant rank guard: sin²θ = det / (|ga|²|gb|²).
        let scale_sq = m00 * m11;
        if scale_sq <= 0.0 || det <= RANK_SIN2_MIN * scale_sq {
            return None;
        }

        // y = M⁻¹ (−F); Δ = Jᵀ y = ga·y0 + gb·y1.
        let f0 = -va;
        let f1 = -vb;
        let inv = 1.0 / det;
        let y0 = inv * (m11 * f0 - m01 * f1);
        let y1 = inv * (-m01 * f0 + m00 * f1);
        let delta = add(scale(ga, y0), scale(gb, y1));
        p = add(p, delta);

        if !p[0].is_finite() || !p[1].is_finite() || !p[2].is_finite() {
            return None;
        }
    }
    // Final acceptance check after the last step.
    let (va, _) = fa.value_grad(p);
    let (vb, _) = fb.value_grad(p);
    let tol = PROJECT_TOL.max(fa.noise_floor(p)).max(fb.noise_floor(p));
    (va.abs().max(vb.abs()) < tol).then_some(p)
}

/// Unit edge tangent at a point assumed to lie on the edge: `∇f_a × ∇f_b`,
/// normalised. Returns `None` at a tangential degeneracy (collapsing cross
/// product) — the same condition the projection's rank guard rejects.
pub(crate) fn edge_tangent(fa: &Field, fb: &Field, p: [f64; 3]) -> Option<[f64; 3]> {
    let (_, ga) = fa.value_grad(p);
    let (_, gb) = fb.value_grad(p);
    let t = cross(ga, gb);
    let len = norm(t);
    let scale_len = norm(ga) * norm(gb);
    if scale_len <= 0.0 || len * len <= RANK_SIN2_MIN * scale_len * scale_len {
        return None;
    }
    Some(scale(t, 1.0 / len))
}

/// Tuning for [`trace_edge`]. All lengths are in world units; pick `step`
/// relative to the body's scale and the desired wireframe density.
#[derive(Clone, Copy)]
pub(crate) struct TraceParams {
    /// Nominal arc step between successive polyline points (predictor length).
    pub step: f64,
    /// Hard cap on points per march direction (loop/escape backstop). A full
    /// loop needs ≈ circumference / step points; size accordingly.
    pub max_steps: usize,
    /// A march end snaps to a bounding vertex once one lies ahead within this
    /// distance. Must exceed `step` so a vertex is never stepped over.
    pub vertex_reach: f64,
    /// A forward march closes a loop once it returns within this distance of the
    /// seed (after leaving its neighbourhood). Keep ≈ `step`.
    pub close_tol: f64,
    /// Arc-length budget for one march direction. Exceeding it stalls the march
    /// (returning a non-clean end) — the guard against an *unbounded* edge (e.g.
    /// a plane parallel to an infinite cylinder) marching off to infinity. The
    /// caller sets it from the seam's spatial extent; [`f64::INFINITY`] disables.
    pub max_arc: f64,
    /// Optional **curvature-adaptive** stepping (DL-81, experimental). When
    /// `None` the march uses the fixed nominal `step` for every predictor —
    /// even spacing, byte-identical to the historical tracer. When `Some` the
    /// step size is driven by a *geometric chord-deviation tolerance* instead:
    /// long strides on straight runs, short ones through tight bends, so the
    /// point count tracks the edge's *curvature* — not the seed/octree spacing
    /// and not the object's size. A 1 m straight edge collapses to a single
    /// span; a tight fillet gets just enough points for the tolerance.
    pub adapt: Option<Adapt>,
}

/// Curvature-adaptive step control for [`trace_edge`] (DL-81). Keeps the arc's
/// deviation from its chord under `tol` by sizing each predictor from the
/// locally measured curvature (sagitta relation `s ≈ ½·L·tan(θ/4)`, target
/// `L = √(8·tol/κ)`), clamped to `[min_step, max_step]`. All lengths in world
/// units, independent of the body's overall scale.
#[derive(Clone, Copy)]
pub(crate) struct Adapt {
    /// Max allowed arc-to-chord deviation (sagitta) of any emitted segment.
    pub tol: f64,
    /// Shortest predictor the march will shrink to (sharp-corner floor — a
    /// tighter turn is left to vertex snapping rather than crawled across).
    pub min_step: f64,
    /// Longest predictor the march will grow to on a straight run.
    pub max_step: f64,
}

impl TraceParams {
    /// Defaults derived from a single arc step: vertices are caught within 1.5
    /// steps ahead, loops close within one step, arc length unbounded.
    pub(crate) fn from_step(step: f64) -> Self {
        Self {
            step,
            max_steps: 100_000,
            vertex_reach: 1.5 * step,
            close_tol: step,
            max_arc: f64::INFINITY,
            adapt: None,
        }
    }

    /// Enable curvature-adaptive stepping (DL-81). `tol` is the chord-deviation
    /// budget; the nominal `step` stays the *initial* predictor and `max_step`
    /// becomes the straight-run cap. Neither `vertex_reach` nor `close_tol` is
    /// widened here: the march derives both per-step from the *local* stride
    /// (`1.5 × step`). A `max_step`-sized reach/closure radius would shortcut a
    /// curved arc to a chord between its endpoints, or fire a false loop closure
    /// a few small steps from the seed.
    pub(crate) fn with_adaptive(mut self, tol: f64, min_step: f64, max_step: f64) -> Self {
        self.adapt = Some(Adapt { tol, min_step, max_step });
        self
    }
}

/// A traced edge polyline. `closed` rings carry no meaningful `ends` (both set
/// to [`EdgeEnd::Open`]); open arcs carry how each end terminated — a caller
/// bounding vertex, a trimming boundary, or an unresolved stall.
pub(crate) struct TracedEdge {
    pub points: Vec<[f64; 3]>,
    pub closed: bool,
    pub ends: (EdgeEnd, EdgeEnd),
}

/// How one end of a traced open arc terminated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum EdgeEnd {
    /// Reached one of the caller's bounding vertices (index into its slice).
    Vertex(usize),
    /// Hit a trimming boundary (a third branch became active) at this world
    /// point — a newly discovered vertex, not in the caller's slice. This is the
    /// "trimming multiplicity": the intersection curve continues geometrically
    /// but leaves the real model boundary here, so the edge must stop exactly at
    /// the active→inactive transition rather than at any pre-known vertex.
    Trim([f64; 3]),
    /// Marched to a stall / max_steps without a clean end.
    Open,
}

/// How a one-directional march terminated.
enum MarchEnd {
    /// Reached `vertices[idx]` — the natural end of an open arc.
    Vertex(usize),
    /// Hit the active→inactive boundary at this world point — the edge leaves
    /// the real surface here (a third branch became active). See [`EdgeEnd::Trim`].
    Trimmed([f64; 3]),
    /// Returned to the seed neighbourhood — the edge is a closed ring.
    Loop,
    /// Projection failed, the tangent collapsed, or `max_steps` was hit. The
    /// collected points are still usable; the arc just isn't pinned to a vertex.
    Stalled,
}

/// A forward march must leave the seed's neighbourhood before it may close a
/// loop, else it would "close" on step one. Counted in ACCEPTED points, not
/// loop iterations: the adaptive sagitta retries consume iterations without
/// emitting a point, so an iteration count lets a march "close" a couple of
/// real chords from the seed (a seed at the very end of its active interval
/// strides off at near `max_step`, and `close_tol` follows the stride).
const MIN_LOOP_STEPS: usize = 4;

/// Adaptive-step floor: when a nominal predictor step stalls, the march halves
/// it down to `step / 2^k` before giving up. Six halvings (≈ step/64) resolves a
/// sharp turn or a near-degenerate numeric gradient without an unbounded crawl.
const MARCH_REFINE_HALVINGS: u32 = 6;
/// A refined (sub-nominal) step is accepted only if its tangent continues the
/// curve smoothly — `cosθ ≥ ½` (≤ 60° turn per sub-step). This lets the march
/// push through a sharp but *smooth* turn while refusing to crawl across a true
/// seam node, where the re-acquired tangent forks (near-perpendicular). Applies
/// only on the refinement path, so a nominal step is never gated by it.
const MARCH_CONTINUITY_COS: f64 = 0.5;

/// One predictor + corrector step of length `step` from `cur` along `t_prev`.
/// Returns the reprojected edge point and its forward-oriented unit tangent, or
/// `None` if the predictor lands where projection diverges or the tangent
/// collapses. This is the historical single-step body, factored out so both the
/// nominal step and the refinement reuse identical acceptance.
fn step_once(
    fa: &Field,
    fb: &Field,
    cur: [f64; 3],
    t_prev: [f64; 3],
    step: f64,
) -> Option<([f64; 3], [f64; 3])> {
    let q = add(cur, scale(t_prev, step));
    let np = project_to_edge(fa, fb, q)?;
    // Forward-progress guard: a predictor that overshoots a high-curvature
    // stretch (e.g. a 2 mm stadium-cap arc rounding a revolved wall's chain
    // end) can be pulled by the Newton corrector to a curve point BEHIND
    // `cur`. Accepting it reverses the march direction and the polyline
    // zigzags around the turn instead of walking through it. Rejecting here
    // sends `advance` down its refinement halvings, whose shorter predictors
    // stay on the near branch of the curve.
    if dot(sub(np, cur), t_prev) <= 0.0 {
        return None;
    }
    let mut t = edge_tangent(fa, fb, np)?;
    if dot(t, t_prev) < 0.0 {
        t = scale(t, -1.0);
    }
    Some((np, t))
}

/// Advance the march one step, refining the step length on a stall (DL-69b).
///
/// The nominal `step` is tried first with the exact historical acceptance (no
/// continuity gate), so a step that already succeeds is returned unchanged — the
/// refinement is strictly additive. Only when the nominal predictor stalls
/// (projection diverged or the tangent collapsed, typically a sharp turn
/// overshooting or two numeric leaf gradients going briefly near-parallel) does
/// it halve the step toward the edge, down to `step / 2^MARCH_REFINE_HALVINGS`.
/// A refined step is accepted only when its tangent continues the curve smoothly
/// (`MARCH_CONTINUITY_COS`); at a genuine seam node the re-acquired tangent forks
/// near-perpendicular and fails the guard, so the arc still terminates there
/// (→ falls back to the sample fits) rather than crawling onto the wrong branch.
fn advance(
    fa: &Field,
    fb: &Field,
    cur: [f64; 3],
    t_prev: [f64; 3],
    step: f64,
) -> Option<([f64; 3], [f64; 3])> {
    // Nominal step: historical path, byte-identical when it succeeds.
    if let Some(hit) = step_once(fa, fb, cur, t_prev, step) {
        return Some(hit);
    }
    // Refinement: only reached on a stalled nominal step.
    let mut h = step * 0.5;
    for _ in 0..MARCH_REFINE_HALVINGS {
        if let Some((np, t)) = step_once(fa, fb, cur, t_prev, h) {
            if dot(t, t_prev) >= MARCH_CONTINUITY_COS {
                return Some((np, t));
            }
        }
        h *= 0.5;
    }
    None
}

/// Bisection passes for [`bisect_active`]. 30 midpoint halvings shrink a step-
/// sized segment by 2^-30 (≈ 1e-9 of `step`), far below the on-surface scale.
const BISECT_ACTIVE_STEPS: usize = 30;
// Segment-length floor at which [`bisect_active`] stops early — frozen-
// absolute, see `brep::tol`.
use super::tol::BISECT_ACTIVE_TOL;

/// Locate the active→inactive transition on the curve span `[a_active,
/// b_inactive]` by midpoint bisection, returning the last *active* point.
/// Each chord midpoint is reprojected onto the edge `{f_a = 0, f_b = 0}`
/// before the activity test: the raw chord deviates from the curve by up to
/// the sagitta of a full march step, which would dwarf a tight activity band
/// and make the test evaluate activity *off-surface* (an off-surface point is
/// "inactive" under a band smaller than the sagitta, collapsing the bisection
/// toward `a` and leaving the trim a whole step short). A midpoint whose
/// projection diverges (tangential degeneracy) falls back to the raw chord
/// midpoint — the historical behaviour. `a` is invariant-active and `b`
/// invariant-inactive; each step replaces whichever endpoint the midpoint
/// matches, so `a` always stays the active side.
fn bisect_active(
    fa: &Field,
    fb: &Field,
    f: &dyn Fn([f64; 3]) -> bool,
    a_active: [f64; 3],
    b_inactive: [f64; 3],
) -> [f64; 3] {
    let mut a = a_active;
    let mut b = b_inactive;
    for _ in 0..BISECT_ACTIVE_STEPS {
        if dist(a, b) < BISECT_ACTIVE_TOL {
            break;
        }
        let raw = scale(add(a, b), 0.5);
        let mid = project_to_edge(fa, fb, raw).unwrap_or(raw);
        // A projection that jumps outside the bracket would break the
        // invariant; keep the raw chord midpoint in that case.
        let mid = if dist(mid, raw) > dist(a, b) { raw } else { mid };
        if f(mid) {
            a = mid;
        } else {
            b = mid;
        }
    }
    a
}

/// March one direction from `start` (already on the edge) with initial unit
/// tangent `t0`, collecting reprojected points until a bounding vertex, a loop
/// back to `seed`, a trimming boundary, or a stall. Does not include `start` in
/// the returned points.
fn march(
    fa: &Field,
    fb: &Field,
    start: [f64; 3],
    t0: [f64; 3],
    seed: [f64; 3],
    vertices: &[[f64; 3]],
    p: &TraceParams,
    active: Option<&dyn Fn([f64; 3]) -> bool>,
) -> (Vec<[f64; 3]>, MarchEnd) {
    let mut pts = Vec::new();
    let mut cur = start;
    let mut t_prev = t0;
    let mut arc = 0.0;
    // Adaptive marches carry a *mutable* predictor length; fixed marches leave
    // it at `p.step` for every step, so the historical path is byte-identical.
    let mut step = p.step;
    // Loop-closure radius. Fixed: the constant `p.close_tol`. Adaptive: tracks the
    // *current* stride (set from the segment just taken) — a fixed `close_tol`
    // tied to `max_step` would dwarf the small strides on a finely-sampled curved
    // loop and fire a false closure a few steps in, shattering the ring (DL-81).
    let mut close_tol = p.close_tol;
    for _ in 0..p.max_steps {
        // A bounding vertex ahead within reach ends the arc exactly: append the
        // vertex itself (not a projected approximation) and stop. The reach
        // follows the *current* stride in adaptive mode (`1.5 × step`): on a
        // curved arc the stride is small, so a distant bounding vertex is NOT
        // grabbed early — a `max_step`-sized reach would shortcut a whole arc to
        // a near-straight chord between its endpoints (DL-81). The fixed path
        // keeps its constant `vertex_reach`, so it is byte-identical.
        let reach = if p.adapt.is_some() {
            1.5 * step
        } else {
            p.vertex_reach
        };
        if let Some(vi) = vertex_ahead(cur, t_prev, vertices, reach) {
            pts.push(vertices[vi]);
            return (pts, MarchEnd::Vertex(vi));
        }

        // Predictor + corrector, with adaptive step refinement (DL-69b). The
        // nominal step is tried first and accepted exactly as before, so any
        // seam that already traced is byte-for-byte unchanged; only a stalled
        // nominal step refines toward the edge (decoupling the march from the
        // seed spacing) instead of abandoning the whole group to the sample fits.
        let Some((np, t)) = advance(fa, fb, cur, t_prev, step) else {
            return (pts, MarchEnd::Stalled);
        };

        // Curvature-adaptive accept / resize (DL-81). On the fixed path
        // `p.adapt` is `None`, `step` never changes, and this block is skipped.
        if let Some(a) = &p.adapt {
            let chord = dist(np, cur);
            // Closure radius follows the local stride: ~1.5× the segment just
            // taken catches the seed on the genuine return without matching it
            // mid-march, where `dist(np, seed)` is many strides large.
            close_tol = 1.5 * chord;
            // Turn angle across the step just taken (both tangents are unit).
            let theta = dot(t_prev, t).clamp(-1.0, 1.0).acos();
            // Arc-to-chord deviation of a circular step: s = ½·L·tan(θ/4).
            let sagitta = 0.5 * chord * (0.25 * theta).tan();
            // Over budget here and still room to shrink → drop this step and
            // retry shorter from the same point (never below `min_step`).
            if sagitta > a.tol && step > a.min_step * (1.0 + 1e-9) {
                step = (step * 0.5).max(a.min_step);
                continue;
            }
            // Accept; size the *next* predictor from the local curvature
            // κ ≈ θ/L so the following segment lands right at the tolerance.
            let kappa = if chord > 1e-12 { theta / chord } else { 0.0 };
            step = if kappa > 1e-12 {
                (8.0 * a.tol / kappa).sqrt()
            } else {
                a.max_step
            }
            .clamp(a.min_step, a.max_step);
        }

        // Active-interval termination (Phase 2a). The invariant is that `cur` is
        // always active (the seed is guaranteed active by `trace_edge`, and only
        // active `np` ever becomes the next `cur`). So a freshly accepted `np`
        // that is *inactive* means the edge crossed the real model boundary
        // somewhere inside this final segment — a third branch became active and
        // trims the edge here. Bisect `[cur(active), np(inactive)]` to the exact
        // on-surface boundary point, emit it as the arc's terminus, and stop.
        // Checked AFTER the adaptive accept (so `np` is final) but BEFORE the
        // loop-closure / arc-budget checks, so trimming takes priority over a
        // spurious closure or a budget stall straddling the boundary. `np` itself
        // is never pushed — it lies in the trimmed-away region.
        if let Some(f) = active {
            if !f(np) {
                let boundary = bisect_active(fa, fb, f, cur, np);
                pts.push(boundary);
                return (pts, MarchEnd::Trimmed(boundary));
            }
            // Gap guard (ticket 7sp1): a long adaptive stride can leap clean
            // OVER a trimmed-away stretch — both endpoints active, the covered
            // gap between them never sampled — and the march then bridges two
            // arcs that a third surface separates (the g4rr lens gap). Test
            // the stride midpoint too, REPROJECTED onto the edge first: the
            // raw chord midpoint sits up to the sagitta off the curve, which
            // exceeds the activity band and would fake inactivity on every
            // curved edge. Halves the smallest leapable gap; the seed route
            // additionally caps its `max_step` for the same reason.
            if p.adapt.is_some() {
                let chord_mid = scale(add(cur, np), 0.5);
                if let Some(mid) = project_to_edge(fa, fb, chord_mid) {
                    if !f(mid) {
                        let boundary = bisect_active(fa, fb, f, cur, mid);
                        pts.push(boundary);
                        return (pts, MarchEnd::Trimmed(boundary));
                    }
                }
            }
        }

        // Loop closure: back near the seed after genuinely leaving it.
        if pts.len() + 1 >= MIN_LOOP_STEPS && dist(np, seed) < close_tol {
            return (pts, MarchEnd::Loop);
        }

        // Arc-length budget: an unbounded edge that neither loops nor hits a
        // vertex stalls here instead of running to infinity.
        arc += dist(np, cur);
        if arc > p.max_arc {
            return (pts, MarchEnd::Stalled);
        }

        pts.push(np);
        cur = np;
        t_prev = t;
    }
    (pts, MarchEnd::Stalled)
}

/// Index of the nearest vertex that lies *ahead* of `cur` along `t` (forward
/// half-space) and within `reach`, if any. Forward-only so the march doesn't
/// snap back to the vertex it just departed.
fn vertex_ahead(
    cur: [f64; 3],
    t: [f64; 3],
    vertices: &[[f64; 3]],
    reach: f64,
) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (i, &v) in vertices.iter().enumerate() {
        let d = dist(v, cur);
        if d > reach {
            continue;
        }
        // Ahead = the step toward v has a positive component along the tangent.
        if dot(normalize(sub(v, cur)), t) <= 0.0 {
            continue;
        }
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// Trace the full edge through an interior `seed_pt` on the active arc.
///
/// Projects the seed onto the edge, then marches both tangent directions. If
/// the forward march loops back, the edge is a closed ring (vertices ignored);
/// otherwise the two marches are stitched — `reverse(backward) ++ seed ++
/// forward` — into one open polyline whose ends record which bounding vertices
/// were reached. Returns `None` only if the seed itself is not near a
/// well-conditioned edge point.
pub(crate) fn trace_edge(
    fa: &Field,
    fb: &Field,
    seed_pt: [f64; 3],
    vertices: &[[f64; 3]],
    p: &TraceParams,
    active: Option<&dyn Fn([f64; 3]) -> bool>,
) -> Option<TracedEdge> {
    let seed = project_to_edge(fa, fb, seed_pt)?;
    let t0 = edge_tangent(fa, fb, seed)?;

    // The seed anchors the march invariant "`cur` is always active". A seed in a
    // trimmed-away region is not a real edge point — both marches would start
    // inactive and the invariant would be violated — so reject it outright.
    if let Some(f) = active {
        if !f(seed) {
            return None;
        }
    }

    let (fwd, fend) = march(fa, fb, seed, t0, seed, vertices, p, active);
    if matches!(fend, MarchEnd::Loop) {
        let mut points = Vec::with_capacity(fwd.len() + 1);
        points.push(seed);
        points.extend(fwd);
        return Some(TracedEdge {
            points,
            closed: true,
            ends: (EdgeEnd::Open, EdgeEnd::Open),
        });
    }

    let (bwd, bend) = march(fa, fb, seed, scale(t0, -1.0), seed, vertices, p, active);
    let mut points = Vec::with_capacity(bwd.len() + 1 + fwd.len());
    points.extend(bwd.iter().rev().copied());
    points.push(seed);
    points.extend(fwd.iter().copied());

    let to_edge_end = |e: &MarchEnd| match e {
        MarchEnd::Vertex(i) => EdgeEnd::Vertex(*i),
        MarchEnd::Trimmed(pt) => EdgeEnd::Trim(*pt),
        MarchEnd::Loop | MarchEnd::Stalled => EdgeEnd::Open,
    };
    Some(TracedEdge {
        points,
        closed: false,
        ends: (to_edge_end(&bend), to_edge_end(&fend)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plane half-space `n·p = offset`, with `n` normalised, in world coords.
    fn plane(normal: [f64; 3], offset: f64) -> Field<'static> {
        let n = norm(normal);
        Field::Analytic {
            hs: HalfSpace::Plane {
                normal: [
                    (normal[0] / n) as f32,
                    (normal[1] / n) as f32,
                    (normal[2] / n) as f32,
                ],
                offset: (offset / n) as f32,
            },
            xf: Transform::IDENTITY,
        }
    }

    /// Sphere of `radius` centred at `center` (centre folded into the
    /// transform translate, mirroring `branch_halfspace_map`).
    fn sphere(center: [f64; 3], radius: f64) -> Field<'static> {
        Field::Analytic {
            hs: HalfSpace::Sphere { radius: radius as f32 },
            xf: Transform {
                translate: [center[0] as f32, center[1] as f32, center[2] as f32],
                ..Transform::IDENTITY
            },
        }
    }

    /// Infinite cylinder of `radius` about the Y axis through `center`.
    fn cylinder_y(center: [f64; 3], radius: f64) -> Field<'static> {
        Field::Analytic {
            hs: HalfSpace::InfiniteCylinder { radius: radius as f32 },
            xf: Transform {
                translate: [center[0] as f32, center[1] as f32, center[2] as f32],
                ..Transform::IDENTITY
            },
        }
    }

    /// A handful of deterministic near-edge seeds (no RNG in the kernel).
    /// Each is a true edge point nudged off in a fixed direction.
    fn nudges() -> [[f64; 3]; 6] {
        [
            [0.30, -0.20, 0.10],
            [-0.25, 0.15, -0.30],
            [0.05, 0.40, 0.05],
            [-0.35, -0.10, 0.20],
            [0.18, 0.18, -0.18],
            [-0.12, -0.28, -0.06],
        ]
    }

    #[test]
    fn sphere_plane_projects_onto_exact_circle() {
        // Unit sphere at origin ∩ plane z = 0.3 → circle radius √(1-0.09).
        let s = sphere([0.0, 0.0, 0.0], 1.0);
        let pl = plane([0.0, 0.0, 1.0], 0.3);
        let expected_r = (1.0 - 0.3 * 0.3_f64).sqrt();
        // Seeds near the +x point of the circle.
        let base = [expected_r, 0.0, 0.3];
        for d in nudges() {
            let p0 = add(base, d);
            let p = project_to_edge(&s, &pl, p0).expect("must converge");
            // On the sphere: |p| == 1 (radius 1.0 is f32-exact → tight).
            assert!((norm(p) - 1.0).abs() < 1e-9, "|p|={} for seed {p0:?}", norm(p));
            // On the plane: z == 0.3 (offset stored as f32 → f32 floor).
            assert!((p[2] - 0.3).abs() < 1e-6, "z={} for seed {p0:?}", p[2]);
            // On the circle: radius in z=0.3 plane.
            let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
            assert!((r - expected_r).abs() < 1e-6, "r={r} expected {expected_r}");
        }
    }

    #[test]
    fn cylinder_plane_projects_onto_cap_circle() {
        // Cylinder radius 2 about Y at origin ∩ plane y = 1.5 → circle r=2 @ y=1.5.
        let c = cylinder_y([0.0, 0.0, 0.0], 2.0);
        let pl = plane([0.0, 1.0, 0.0], 1.5);
        let base = [2.0, 1.5, 0.0];
        for d in nudges() {
            let p0 = add(base, d);
            let p = project_to_edge(&c, &pl, p0).expect("must converge");
            assert!((p[1] - 1.5).abs() < 1e-9, "y={}", p[1]);
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            assert!((r - 2.0).abs() < 1e-9, "r={r} expected 2");
        }
    }

    #[test]
    fn sphere_sphere_projects_onto_radical_circle() {
        // Two unit spheres, centres at ±(0.6,0,0). Seam: plane x=0, circle
        // radius √(1-0.36) in that plane.
        let a = sphere([-0.6, 0.0, 0.0], 1.0);
        let b = sphere([0.6, 0.0, 0.0], 1.0);
        let expected_r = (1.0 - 0.6 * 0.6_f64).sqrt();
        let base = [0.0, expected_r, 0.0];
        for d in nudges() {
            let p0 = add(base, d);
            let p = project_to_edge(&a, &b, p0).expect("must converge");
            // Radical plane x = 0 (symmetric ±0.6 f32 centres → exact).
            assert!(p[0].abs() < 1e-9, "x={}", p[0]);
            // Circle radius vs ideal (centre 0.6 stored as f32 → f32 floor).
            let r = (p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((r - expected_r).abs() < 1e-6, "r={r} expected {expected_r}");
        }
    }

    #[test]
    fn sphere_cylinder_projects_onto_both_surfaces() {
        // Unit sphere at origin ∩ cylinder radius 0.5 about Y. The seam is a
        // pair of circles at y = ±√(1-0.25); seeds near the upper one. No
        // closed form is asserted — only that the projected point lies on BOTH
        // surfaces (the general guarantee).
        let s = sphere([0.0, 0.0, 0.0], 1.0);
        let c = cylinder_y([0.0, 0.0, 0.0], 0.5);
        let y = (1.0 - 0.25_f64).sqrt();
        let base = [0.5, y, 0.0];
        for d in nudges() {
            let p0 = add(base, [d[0] * 0.3, d[1] * 0.3, d[2] * 0.3]);
            let p = project_to_edge(&s, &c, p0).expect("must converge");
            // On the sphere.
            assert!((norm(p) - 1.0).abs() < 1e-8, "|p|={}", norm(p));
            // On the cylinder.
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            assert!((r - 0.5).abs() < 1e-8, "r={r} expected 0.5");
        }
    }

    #[test]
    fn parallel_planes_rejected_by_rank_guard() {
        // Two parallel planes never meet → tangential, rank-deficient.
        let p1 = plane([0.0, 0.0, 1.0], 0.3);
        let p2 = plane([0.0, 0.0, 1.0], 0.7);
        assert!(project_to_edge(&p1, &p2, [0.1, 0.2, 0.5]).is_none());
    }

    #[test]
    fn edge_tangent_is_perpendicular_to_both_normals() {
        // On the sphere∩plane circle, the tangent must be ⟂ to z (plane normal)
        // and ⟂ to the sphere radius.
        let s = sphere([0.0, 0.0, 0.0], 1.0);
        let pl = plane([0.0, 0.0, 1.0], 0.3);
        let r = (1.0 - 0.09_f64).sqrt();
        let p = project_to_edge(&s, &pl, [r + 0.1, 0.05, 0.32]).unwrap();
        let t = edge_tangent(&s, &pl, p).unwrap();
        assert!(t[2].abs() < 1e-9, "tangent must be in-plane, tz={}", t[2]);
        // ⟂ to sphere normal (= p, since centred at origin).
        assert!(dot(t, p).abs() < 1e-9, "tangent must be ⟂ radius");
        assert!((norm(t) - 1.0).abs() < 1e-12, "unit tangent");
    }

    /// Assert every polyline point lies on both the unit origin-sphere and the
    /// plane `z = z0`.
    fn assert_on_sphere_plane(points: &[[f64; 3]], z0: f64, tol: f64) {
        for &q in points {
            assert!((norm(q) - 1.0).abs() < tol, "|p|={} off sphere", norm(q));
            assert!((q[2] - z0).abs() < tol, "z={} off plane", q[2]);
        }
    }

    /// Consecutive points sit ≈ one step apart (no gaps, no doublings).
    fn assert_even_spacing(points: &[[f64; 3]], step: f64) {
        for w in points.windows(2) {
            let d = dist(w[0], w[1]);
            assert!(d < 1.5 * step, "gap {d} exceeds 1.5·step");
            assert!(d > 0.25 * step, "doubled point, spacing {d}");
        }
    }

    #[test]
    fn sphere_plane_traces_closed_ring() {
        // Unit sphere ∩ plane z = 0.3, no bounding vertices → full closed ring.
        let s = sphere([0.0, 0.0, 0.0], 1.0);
        let pl = plane([0.0, 0.0, 1.0], 0.3);
        let r = (1.0 - 0.3 * 0.3_f64).sqrt();
        let step = 0.1;
        let params = TraceParams::from_step(step);
        // Seed nudged off the +x point of the circle.
        let traced = trace_edge(&s, &pl, [r + 0.07, -0.05, 0.34], &[], &params, None)
            .expect("must trace");
        assert!(traced.closed, "no vertices → closed ring");
        assert_eq!(traced.ends, (EdgeEnd::Open, EdgeEnd::Open));
        // ≈ circumference / step points.
        let circ = std::f64::consts::TAU * r;
        let expected = (circ / step) as usize;
        assert!(
            (expected.saturating_sub(8)..expected + 8).contains(&traced.points.len()),
            "len {} not ≈ {expected}",
            traced.points.len()
        );
        assert_on_sphere_plane(&traced.points, 0.3, 1e-6);
        assert_even_spacing(&traced.points, step);
        // Implicit closing segment (last → first) is also ≈ one step.
        let closing = dist(*traced.points.first().unwrap(), *traced.points.last().unwrap());
        assert!(closing < 2.0 * step, "ring not closed, gap {closing}");
    }

    #[test]
    fn sphere_plane_open_arc_terminates_at_vertices() {
        // Same circle, but two bounding vertices at θ = ±60° clip it to the
        // short arc through θ = 0 (the seed). Each march end must snap to a
        // vertex.
        let s = sphere([0.0, 0.0, 0.0], 1.0);
        let pl = plane([0.0, 0.0, 1.0], 0.3);
        let r = (1.0 - 0.3 * 0.3_f64).sqrt();
        let a = std::f64::consts::FRAC_PI_3; // 60°
        let v_pos = [r * a.cos(), r * a.sin(), 0.3];
        let v_neg = [r * a.cos(), -r * a.sin(), 0.3];
        let verts = [v_pos, v_neg];
        let step = 0.1;
        let params = TraceParams::from_step(step);
        let traced = trace_edge(&s, &pl, [r + 0.05, 0.0, 0.31], &verts, &params, None)
            .expect("must trace");

        assert!(!traced.closed, "clipped arc is open");
        let (b, f) = traced.ends;
        let EdgeEnd::Vertex(b) = b else { panic!("back end at vertex, got {b:?}") };
        let EdgeEnd::Vertex(f) = f else { panic!("fwd end at vertex, got {f:?}") };
        assert_ne!(b, f, "the two ends are distinct vertices");
        // Endpoints are the exact vertices, in end order.
        assert_eq!(*traced.points.first().unwrap(), verts[b]);
        assert_eq!(*traced.points.last().unwrap(), verts[f]);
        // The active arc is the short one through θ = 0: every point has x > 0.
        for &q in &traced.points {
            assert!(q[0] > 0.0, "point {q:?} left the short arc");
        }
        assert_on_sphere_plane(&traced.points, 0.3, 1e-6);
        assert_even_spacing(&traced.points, step);
    }

    #[test]
    fn cylinder_plane_traces_closed_ring() {
        // Cylinder r=2 about Y ∩ plane y = 1.5 → closed circle r=2 at y=1.5.
        let c = cylinder_y([0.0, 0.0, 0.0], 2.0);
        let pl = plane([0.0, 1.0, 0.0], 1.5);
        let step = 0.15;
        let params = TraceParams::from_step(step);
        let traced = trace_edge(&c, &pl, [2.1, 1.55, 0.05], &[], &params, None).expect("trace");
        assert!(traced.closed);
        for &q in &traced.points {
            assert!((q[1] - 1.5).abs() < 1e-6, "y={} off plane", q[1]);
            let rad = (q[0] * q[0] + q[2] * q[2]).sqrt();
            assert!((rad - 2.0).abs() < 1e-6, "r={rad} off cylinder");
        }
        assert_even_spacing(&traced.points, step);
    }

    #[test]
    fn leaf_field_traces_closed_ring() {
        // The general dual-SDF Leaf path traces the same ring as the analytic
        // half-space — proving the tracer is field-agnostic end to end.
        use crate::bvh::BoundedCsg;
        use crate::csg::{CsgNode, DecodedFeature, PrimitiveModifiers};
        use fielddraft_protocol::{BoolOp, FeatureId, Primitive};

        let sphere_feat = DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "S".into(),
            BoolOp::New,
            Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 1.0 },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        );
        let csg = CsgNode::from_decoded(&[sphere_feat]);
        let leaf = BoundedCsg::from_csg(&csg);
        let s = Field::Leaf { node: &leaf };
        let pl = plane([0.0, 0.0, 1.0], 0.3);
        let r = (1.0 - 0.09_f64).sqrt();
        let step = 0.12;
        let params = TraceParams::from_step(step);
        let traced = trace_edge(&s, &pl, [r + 0.06, 0.04, 0.33], &[], &params, None).expect("trace");
        assert!(traced.closed, "leaf ring should close");
        // Dual leaf SDF carries the f32 mesh floor → looser but still tight.
        assert_on_sphere_plane(&traced.points, 0.3, 1e-6);
        assert_even_spacing(&traced.points, step);
    }

    #[test]
    fn two_leaf_fields_trace_transversal_curved_seam() {
        // DL-69 leaf↔leaf: the tracer pairs TWO dual-SDF Leaf fields — the
        // deferred case (two curved sweep walls, neither with an analytic
        // half-space). Leaf A: full-sweep `Sector` r=1 extruded → a cylinder
        // about Z. Leaf B: full-sweep `Sector` r=0.3 extruded then rotated 90°
        // about Y → a thin drill about X. The drill crosses A's curved wall
        // **transversally** (∇f_a ∝ (x,y,0), ∇f_b ∝ (0,y,z) are never parallel on
        // the seam), so the seam is a small closed loop x²+y²=1 ∧ y²+z²=0.09 with
        // NO tangent turning point. The march projects and steps using ONLY the
        // two leaf gradients; a clean, surface-exact closed ring proves leaf↔leaf
        // tracing end to end. (A coaxial Steinmetz, by contrast, has tangent poles
        // where ∇f_a×∇f_b→0 — two numeric fields stall there and the build_wireframe
        // integration falls back to the sample fits; one analytic field would
        // stabilise that pole. See the leaf↔analytic weave test.)
        use crate::bvh::BoundedCsg;
        use crate::csg::CsgNode;
        use fielddraft_protocol::{FeatureId, Sketch2d, Sketch2dPrimitive};

        let sector = |r: f32| {
            Sketch2d::Prim(Sketch2dPrimitive::Sector {
                center: [0.0, 0.0],
                radius: r,
                start_angle: 0.0,
                sweep_angle: std::f32::consts::TAU,
            })
        };
        let leaf_a = BoundedCsg::from_csg(&CsgNode::Extrude { wall_subs: None,
            id: FeatureId::from_u32(1),
            sketch: Box::new(sector(1.0)),
            half_height: 1.5,
        });
        // Extrude about Z, then rotate +90° about Y (quaternion [0, √½, 0, √½])
        // so the drill lies about X. The folded transform rides into the leaf's
        // placement and the dual SDF honours it.
        let leaf_b = BoundedCsg::from_csg(&CsgNode::Transform(
            Transform {
                translate: [0.0, 0.0, 0.0],
                rotation: [
                    0.0,
                    std::f32::consts::FRAC_1_SQRT_2,
                    0.0,
                    std::f32::consts::FRAC_1_SQRT_2,
                ],
                scale: [1.0, 1.0, 1.0],
            },
            Box::new(CsgNode::Extrude { wall_subs: None,
                id: FeatureId::from_u32(2),
                sketch: Box::new(sector(0.3)),
                half_height: 1.5,
            }),
        ));
        let fa = Field::Leaf { node: &leaf_a };
        let fb = Field::Leaf { node: &leaf_b };

        // Seam point near (1, 0, 0.3): on_a = √(x²+y²) = 1, on_b = √(y²+z²) = 0.3.
        let step = 0.04;
        let params = TraceParams::from_step(step);
        let traced = trace_edge(&fa, &fb, [1.02, 0.03, 0.31], &[], &params, None)
            .expect("two leaf fields must trace");
        assert!(traced.closed, "transversal drill seam is a closed ring");
        for &q in &traced.points {
            let on_a = (q[0] * q[0] + q[1] * q[1]).sqrt();
            let on_b = (q[1] * q[1] + q[2] * q[2]).sqrt();
            assert!((on_a - 1.0).abs() < 1e-3, "off leaf A wall: r={on_a} at {q:?}");
            assert!((on_b - 0.3).abs() < 1e-3, "off leaf B wall: r={on_b} at {q:?}");
        }
        assert_even_spacing(&traced.points, step);
    }

    #[test]
    fn leaf_field_projects_like_analytic() {
        // The general Leaf path (dual SDF) must land on the same circle as the
        // analytic half-space for an analytic primitive — proves the projection
        // is field-agnostic, not secretly relying on the closed form.
        use crate::bvh::BoundedCsg;
        use crate::csg::{CsgNode, DecodedFeature, PrimitiveModifiers};
        use fielddraft_protocol::{BoolOp, FeatureId, Primitive};

        let sphere_feat = DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "S".into(),
            BoolOp::New,
            Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 1.0 },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        );
        let csg = CsgNode::from_decoded(&[sphere_feat]);
        let leaf = BoundedCsg::from_csg(&csg);
        let s = Field::Leaf { node: &leaf };
        let pl = plane([0.0, 0.0, 1.0], 0.3);
        let expected_r = (1.0 - 0.09_f64).sqrt();
        let base = [expected_r, 0.0, 0.3];
        for d in nudges() {
            let p0 = add(base, [d[0] * 0.3, d[1] * 0.3, d[2] * 0.3]);
            let p = project_to_edge(&s, &pl, p0).expect("leaf path must converge");
            assert!((norm(p) - 1.0).abs() < 1e-7, "|p|={}", norm(p));
            assert!((p[2] - 0.3).abs() < 1e-7, "z={}", p[2]);
        }
    }

    // ─── DL-81: curvature-adaptive stepping ──────────────────────────────────

    /// Worst arc-to-chord deviation of a polyline known to lie on a circle of
    /// radius `r`: per segment `s = r − √(r² − (d/2)²)` for chord length `d`.
    fn max_circle_sagitta(points: &[[f64; 3]], r: f64) -> f64 {
        let mut worst = 0.0_f64;
        for w in points.windows(2) {
            let half = 0.5 * dist(w[0], w[1]);
            let s = r - (r * r - half * half).max(0.0).sqrt();
            worst = worst.max(s);
        }
        worst
    }

    #[test]
    fn adaptive_straight_edge_collapses_independent_of_length() {
        // Two planes meeting along the x-axis (y=0 ∩ z=0), clipped to a HUGE
        // length L=1000 by two vertices. A straight edge has zero curvature, so
        // the adaptive march grows to `max_step` and spans the whole edge in a
        // handful of points — regardless of L. The fixed even-step path needs
        // ≈ L/step points for the same edge: the size-dependence the octree
        // would otherwise impose.
        let p1 = plane([0.0, 1.0, 0.0], 0.0);
        let p2 = plane([0.0, 0.0, 1.0], 0.0);
        let big = 1000.0;
        let verts = [[-big, 0.0, 0.0], [big, 0.0, 0.0]];

        let even = trace_edge(&p1, &p2, [0.0, 0.02, -0.03], &verts, &TraceParams::from_step(1.0), None)
            .expect("even trace");
        let adaptive = trace_edge(
            &p1,
            &p2,
            [0.0, 0.02, -0.03],
            &verts,
            &TraceParams::from_step(1.0).with_adaptive(1e-3, 1e-2, 2.0 * big),
            None,
        )
        .expect("adaptive trace");

        assert!(even.points.len() > 1000, "even path dense: {}", even.points.len());
        assert!(
            adaptive.points.len() <= 5,
            "straight edge should collapse, got {}",
            adaptive.points.len()
        );
        // Every adaptive point lies exactly on the x-axis (zero deviation).
        for &q in &adaptive.points {
            assert!(q[1].abs() < 1e-6 && q[2].abs() < 1e-6, "off-axis: {q:?}");
        }
        // Ends pinned to the two clipping vertices.
        assert!(!matches!(adaptive.ends.0, EdgeEnd::Open) && !matches!(adaptive.ends.1, EdgeEnd::Open));
    }

    #[test]
    fn adaptive_decouples_point_count_from_seed_spacing() {
        // Sphere ∩ plane ring. A *fine* nominal step (proxy for a finely
        // refined octree) forces ≈ circumference/step points on the even path.
        // The adaptive path is driven by a geometric tolerance instead, so the
        // same fine initial step grows to the curvature need: far fewer points,
        // while the arc-to-chord deviation stays under tolerance.
        let s = sphere([0.0, 0.0, 0.0], 1.0);
        let pl = plane([0.0, 0.0, 1.0], 0.3);
        let r = (1.0 - 0.09_f64).sqrt();
        let fine = 0.02; // a deliberately dense seed spacing
        let tol = 1e-3;

        let even = trace_edge(&s, &pl, [r + 0.05, -0.03, 0.32], &[], &TraceParams::from_step(fine), None)
            .expect("even trace");
        let adaptive = trace_edge(
            &s,
            &pl,
            [r + 0.05, -0.03, 0.32],
            &[],
            &TraceParams::from_step(fine).with_adaptive(tol, 1e-3, 1.0),
            None,
        )
        .expect("adaptive trace");

        assert!(even.closed && adaptive.closed, "both are closed rings");
        // The fine even path is dense; the adaptive path is far sparser.
        assert!(even.points.len() > 250, "even should be dense: {}", even.points.len());
        assert!(
            adaptive.points.len() * 3 < even.points.len(),
            "adaptive {} should be ≪ even {}",
            adaptive.points.len(),
            even.points.len()
        );
        // …yet still honours the deviation budget (small slack for the f32
        // half-space floor and the discrete curvature estimate).
        let sag = max_circle_sagitta(&adaptive.points, r);
        assert!(sag <= tol * 1.5, "adaptive sagitta {sag} exceeds tol {tol}");
        // All points exactly on both surfaces.
        assert_on_sphere_plane(&adaptive.points, 0.3, 1e-6);
    }

    #[test]
    fn adaptive_point_count_tracks_curvature_not_scale() {
        // The SAME ring geometry at two absolute scales (r and 100·r), traced
        // with a tolerance scaled the same way, yields the same point count:
        // the cost follows the *shape's* curvature, not the object's size.
        let tol_rel = 1e-3;
        let count_at = |scale: f64| {
            let s = sphere([0.0, 0.0, 0.0], scale);
            let pl = plane([0.0, 0.0, 1.0], 0.3 * scale);
            let r = scale * (1.0 - 0.09_f64).sqrt();
            let seed = [r + 0.05 * scale, -0.03 * scale, 0.32 * scale];
            let params = TraceParams::from_step(0.1 * scale).with_adaptive(
                tol_rel * scale,
                1e-4 * scale,
                scale,
            );
            trace_edge(&s, &pl, seed, &[], &params, None).expect("trace").points.len()
        };
        let small = count_at(1.0);
        let large = count_at(100.0);
        assert!(
            (small as i64 - large as i64).abs() <= 4,
            "scale-invariance broken: small={small} large={large}"
        );
    }

    // ─── Phase 2a: active-interval termination (trimming) ────────────────────

    #[test]
    fn march_trims_at_active_boundary() {
        // Two orthogonal planes (y=0 ∩ x=0) intersect along the world Z axis — an
        // infinite, otherwise-open edge. An active predicate restricts the edge to
        // |z| ≤ 1, modelling a third branch trimming the seam. Both march
        // directions must stop at the active→inactive boundary (z = ±1), on BOTH
        // planes, with NO closure.
        let py = plane([0.0, 1.0, 0.0], 0.0); // y = 0
        let px = plane([1.0, 0.0, 0.0], 0.0); // x = 0
        let active = |p: [f64; 3]| p[2].abs() <= 1.0;
        let mut params = TraceParams::from_step(0.1);
        params.max_arc = 100.0; // finite, so an untrimmed run would stall not loop
        let traced = trace_edge(&px, &py, [0.001, -0.001, 0.0], &[], &params, Some(&active))
            .expect("must trace the trimmed Z-axis edge");

        assert!(!traced.closed, "trimmed edge is an open arc, not a ring");
        // Both ends are trim boundaries near z = ±1, lying on both planes.
        let ends = [traced.ends.0, traced.ends.1];
        let mut z_signs = (false, false);
        for e in ends {
            let EdgeEnd::Trim(p) = e else { panic!("end must be Trim, got {e:?}") };
            assert!(p[0].abs() < 1e-3, "trim x off plane x=0: {}", p[0]);
            assert!(p[1].abs() < 1e-3, "trim y off plane y=0: {}", p[1]);
            assert!((p[2].abs() - 1.0).abs() < 0.01, "trim z not near ±1: {}", p[2]);
            if p[2] > 0.0 { z_signs.0 = true } else { z_signs.1 = true }
        }
        assert!(z_signs.0 && z_signs.1, "ends should bracket z=+1 and z=-1");
        // The polyline endpoints are exactly the trim boundaries.
        for &q in &[*traced.points.first().unwrap(), *traced.points.last().unwrap()] {
            assert!((q[2].abs() - 1.0).abs() < 0.01, "endpoint z not near ±1: {}", q[2]);
        }
    }

    #[test]
    fn trace_edge_rejects_inactive_seed() {
        // A never-active predicate means the seed lies in a trimmed-away region —
        // not a real edge point — so the whole trace is rejected.
        let py = plane([0.0, 1.0, 0.0], 0.0);
        let px = plane([1.0, 0.0, 0.0], 0.0);
        let active = |_: [f64; 3]| false;
        let params = TraceParams::from_step(0.1);
        assert!(
            trace_edge(&px, &py, [0.001, -0.001, 0.0], &[], &params, Some(&active)).is_none(),
            "inactive seed must yield None"
        );
    }

    // ── gyroid rim tracing (spike phase 2: SSI seeds → 1D trace) ────────────

    fn gyroid(cell_size: f64, thickness: f64) -> Field<'static> {
        Field::Analytic {
            hs: HalfSpace::Gyroid {
                cell_size: cell_size as f32,
                thickness: thickness as f32,
            },
            xf: Transform::IDENTITY,
        }
    }

    /// Trace every SSI-seeded component of one carrier ∩ gyroid pair and
    /// return (closed loops, open arcs, total points), asserting each trace
    /// resolves, stays on BOTH surfaces (f64-tight) and terminates cleanly
    /// (loop closure or an active-window trim — never a stall).
    fn trace_gyroid_pair(
        carrier: &Field,
        carrier_hs: &HalfSpace,
        gy_field: &Field,
        gy_hs: &HalfSpace,
        overlap: crate::csg::Aabb,
        active: &dyn Fn([f64; 3]) -> bool,
    ) -> (usize, usize, usize) {
        use crate::brep::ssi::{ssi_seeds, SsiResult};
        let id = Transform::IDENTITY;
        let SsiResult::Components(comps) =
            ssi_seeds(carrier_hs, &id, gy_hs, &id, &overlap)
        else {
            panic!("carrier∩gyroid must be seedable");
        };
        assert!(!comps.is_empty(), "no components seeded");
        let params = TraceParams::from_step(0.3);
        let (mut loops, mut arcs, mut pts) = (0usize, 0usize, 0usize);
        for comp in &comps {
            let s = comp[0];
            let seed = [s[0] as f64, s[1] as f64, s[2] as f64];
            let edge = trace_edge(carrier, gy_field, seed, &[], &params, Some(active))
                .expect("seeded component must trace");
            assert!(edge.points.len() >= 3, "degenerate trace: {} pts", edge.points.len());
            for &q in &edge.points {
                let (dc, _) = carrier.value_grad(q);
                let (dg, _) = gy_field.value_grad(q);
                assert!(dc.abs() < 1e-6, "point off carrier: {dc}");
                assert!(dg.abs() < 1e-6, "point off gyroid: {dg}");
            }
            if edge.closed {
                loops += 1;
            } else {
                arcs += 1;
                for end in [&edge.ends.0, &edge.ends.1] {
                    assert!(
                        matches!(end, EdgeEnd::Trim(_) | EdgeEnd::Vertex(_)),
                        "march stalled instead of trimming: {end:?}"
                    );
                }
            }
            pts += edge.points.len();
        }
        (loops, arcs, pts)
    }

    /// A box-face plane ∩ gyroid shell: every SSI component must trace to a
    /// closed rim loop or a window-trimmed arc, exactly on both surfaces —
    /// the full seed→trace pipeline for TPMS Boolean rims.
    #[test]
    fn gyroid_plane_rims_trace_closed_or_trimmed() {
        let gy_hs = HalfSpace::Gyroid { cell_size: 4.0, thickness: 0.3 };
        let pl_hs = HalfSpace::Plane { normal: [0.0, 1.0, 0.0], offset: 0.0 };
        let carrier = plane([0.0, 1.0, 0.0], 0.0);
        let gy = gyroid(4.0, 0.3);
        let overlap = crate::csg::Aabb {
            min: [-20.0, -0.5, -20.0],
            max: [20.0, 0.5, 20.0],
        };
        // Active window = the face patch; rims wandering off the face get
        // trimmed like a third surface would trim them in a real body. Sized
        // to the PADDED seed box: SSI pads the overlap by 5% (seeds land out
        // to ±22), so a ±20 window would reject boundary seeds outright.
        let active = |q: [f64; 3]| q[0].abs() <= 22.5 && q[2].abs() <= 22.5;
        let (loops, arcs, pts) =
            trace_gyroid_pair(&carrier, &pl_hs, &gy, &gy_hs, overlap, &active);
        println!("plane∩gyroid traces: {loops} closed loops, {arcs} trimmed arcs, {pts} points");
        // The y=0 gyroid section is a set of OPEN channels (the labyrinth
        // property), so no closed loops are expected here — every curve runs
        // to the window and trims. Loop closure is proven on the compact
        // carrier below (sphere∩gyroid).
        assert!(loops + arcs >= 2, "expected several rim curves");
    }

    /// Compact carrier: sphere ∩ gyroid. Every rim curve on a closed surface
    /// with no trim window MUST close as a loop — the loop-closure proof.
    #[test]
    fn gyroid_sphere_rims_all_close() {
        let gy_hs = HalfSpace::Gyroid { cell_size: 4.0, thickness: 0.3 };
        let sph_hs = HalfSpace::Sphere { radius: 5.0 };
        let carrier = sphere([0.0, 0.0, 0.0], 5.0);
        let gy = gyroid(4.0, 0.3);
        let overlap = crate::csg::Aabb {
            min: [-5.5, -5.5, -5.5],
            max: [5.5, 5.5, 5.5],
        };
        let active = |_: [f64; 3]| true;
        let (loops, arcs, pts) =
            trace_gyroid_pair(&carrier, &sph_hs, &gy, &gy_hs, overlap, &active);
        println!("sphere∩gyroid traces: {loops} closed loops, {arcs} arcs, {pts} points");
        assert!(loops >= 1, "a sphere through a gyroid lattice must carry closed rims");
        assert_eq!(arcs, 0, "no trim window → every curve on the sphere must close");
    }

    /// Curved carrier: cylinder wall ∩ gyroid shell (a drilled gyroid lattice).
    #[test]
    fn gyroid_cylinder_rims_trace() {
        let gy_hs = HalfSpace::Gyroid { cell_size: 4.0, thickness: 0.3 };
        let cyl_hs = HalfSpace::InfiniteCylinder { radius: 6.0 };
        let carrier = cylinder_y([0.0, 0.0, 0.0], 6.0);
        let gy = gyroid(4.0, 0.3);
        let overlap = crate::csg::Aabb {
            min: [-8.0, -8.0, -8.0],
            max: [8.0, 8.0, 8.0],
        };
        // Padded like the plane test: seeds may sit up to 5% past ±8.
        let active = |q: [f64; 3]| q[1].abs() <= 9.0;
        let (loops, arcs, pts) =
            trace_gyroid_pair(&carrier, &cyl_hs, &gy, &gy_hs, overlap, &active);
        println!("cylinder∩gyroid traces: {loops} closed loops, {arcs} trimmed arcs, {pts} points");
        assert!(loops + arcs >= 1, "no rim curves traced on the cylinder wall");
    }

}
