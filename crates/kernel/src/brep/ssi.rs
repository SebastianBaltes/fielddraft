//! Surface–surface intersection (SSI) core — Phase 1b-i of the
//! Parametric-Weaving rebuild (`docs/PARAMETRIC-WEAVING-PLAN.md`).
//!
//! Pure geometry, no CSG-tree involvement: given two analytic
//! [`HalfSpace`] surfaces (each with a local→world [`Transform`], the exact
//! convention [`crate::brep::weaving::branch_halfspace_map`] hands out) and an
//! overlap AABB **in world space**, count the connected components of the two
//! surfaces' intersection curve *within that box* and return one representative
//! seed point per component. The next unit (1b-ii) wires this into the
//! branch-pair edge seeding; here we only prove the geometry and test it hard.
//!
//! ## Space convention
//! A [`HalfSpace`] lives in its primitive's centred *local* frame; its
//! [`Transform`] maps local→world (`s·R·p + t`). To evaluate surface `X`'s
//! signed implicit at a world point `p` we therefore do
//! `sdf_halfspace(X, world_to_local(tx, p))` — the same path
//! [`crate::brep::face_export`] and the octree extractor use. All sampling and
//! all AABB tests in this module happen in **world space**.
//!
//! ## Strategy ([approach B])
//! - **Closed-form short-circuits** for the cheap, exactly-knowable cases:
//!   plane∩plane, plane∩sphere, sphere∩sphere.
//! - **General 2D-parameter scan** for everything else (plane∩cylinder/cone,
//!   cylinder∩cylinder, cone∩anything, sphere∩cylinder/cone, …): parametrise
//!   one surface `A` over a `(u,v)` rectangle derived from the overlap box,
//!   sample `g(u,v) = implicit_B(A(u,v))` on a grid, mask to in-box cells,
//!   march the zero-contour, union-find the contour cells into connected
//!   components (wrapping the periodic θ seam), and emit one bisection-refined
//!   seed per component.
//! - **Unsupported** (torus / gyroid as either operand): caller falls back to
//!   the octree route.

use std::collections::HashMap;

use fielddraft_protocol::{HalfSpace, Transform};

use crate::csg::{sdf_halfspace, Aabb};
use crate::{quat_rotate, world_to_local};

/// Result of an SSI query within an overlap box.
#[derive(Debug, Clone, PartialEq)]
pub enum SsiResult {
    /// One entry per connected component of the intersection curve inside the
    /// box; each entry holds **several** world-space seeds spread along that
    /// component. The outer length is the component count (the topology signal);
    /// the inner seeds exist so a component later trimmed by a *third* surface
    /// into multiple disjoint active arcs still has a seed on each arc — the
    /// downstream active-interval march and the weaver's co-component dedup
    /// collapse the redundant ones (the plan's "never miss a real edge"
    /// contract). Empty outer ⇒ the surfaces do not meet inside the box.
    Components(Vec<Vec<[f32; 3]>>),
    /// Torus / Gyroid operand, or a configuration the scan cannot parametrise
    /// — the caller should fall back to the octree route.
    Unsupported,
}

// ─── tunable constants ───────────────────────────────────────────────────────

/// 2D-scan grid resolution per axis. 64×64 ≈ 4k samples: dense enough to
/// resolve the two close branches of a hyperbola / two-line case across a
/// typical overlap box, while staying a sub-millisecond per branch-pair cost.
/// Both axes use the same count so the seam-wrap bookkeeping stays simple.
const SCAN_RES: usize = 64;

// Identity/slack tolerances live in `brep::tol`; `BOX_SLACK` is the AABB
// membership slack on line-box clipping, `PARALLEL_EPS` the unit-direction
// degeneracy guard.
use super::tol::{AABB_TOUCH_SLACK as BOX_SLACK, PARALLEL_EPS};

/// Bisection-refinement steps applied to each raw contour crossing. The grid
/// crossing is already linearly interpolated (≈ one cell of error); 16 halvings
/// of a one-cell bracket drive the residual to ≈ cell/2¹⁶ ≪ 1e-5 of the box,
/// comfortably inside the 1e-4 on-surface tolerance the tests assert.
const BISECT_STEPS: usize = 16;

/// How many seeds the closed-form paths spread along a single component (a line
/// segment or a circle). A component can be trimmed by a third surface into
/// several active arcs, so one seed is not enough — we sample uniformly so every
/// arc down to ≈ `curve_length / SEEDS_PER_CURVE` carries a seed. Redundant
/// same-arc seeds are collapsed downstream, so over-sampling is free correctness.
const SEEDS_PER_CURVE: usize = 64;

// ─── public entry point ──────────────────────────────────────────────────────

/// Count SSI components of two analytic surfaces within `overlap` (world space)
/// and return one seed per component. See the module docs for the space
/// convention and strategy.
pub fn ssi_seeds(
    a: &HalfSpace,
    ta: &Transform,
    b: &HalfSpace,
    tb: &Transform,
    overlap: &Aabb,
) -> SsiResult {
    // Torus / Gyroid have no clean parametrisation here — bail to the octree.
    if is_unsupported(a) || is_unsupported(b) {
        return SsiResult::Unsupported;
    }
    if overlap.is_empty() {
        return SsiResult::Components(Vec::new());
    }

    // Closed-form short-circuits (exact; no scan).
    match (a, b) {
        (HalfSpace::Plane { .. }, HalfSpace::Plane { .. }) => {
            return plane_plane(a, ta, b, tb, overlap);
        }
        (HalfSpace::Plane { .. }, HalfSpace::Sphere { .. })
        | (HalfSpace::Sphere { .. }, HalfSpace::Plane { .. })
        | (HalfSpace::Sphere { .. }, HalfSpace::Sphere { .. }) => {
            return sphere_like(a, ta, b, tb, overlap);
        }
        _ => {}
    }

    // General 2D-parameter-domain scan.
    scan_components(a, ta, b, tb, overlap)
}

// ─── world-space helpers ─────────────────────────────────────────────────────

fn is_unsupported(h: &HalfSpace) -> bool {
    matches!(h, HalfSpace::Torus { .. } | HalfSpace::Gyroid { .. })
}

/// Local → world point transform `s·R·p + t` (mirror of [`world_to_local`]).
fn local_to_world(t: &Transform, p: [f32; 3]) -> [f32; 3] {
    let scaled = [p[0] * t.scale[0], p[1] * t.scale[1], p[2] * t.scale[2]];
    let r = quat_rotate(t.rotation, scaled);
    [
        r[0] + t.translate[0],
        r[1] + t.translate[1],
        r[2] + t.translate[2],
    ]
}

/// Surface `h`'s signed implicit at world point `p` (0 on the surface).
fn implicit_world(h: &HalfSpace, t: &Transform, p: [f32; 3]) -> f32 {
    sdf_halfspace(h, world_to_local(t, p))
}

fn vsub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn vadd(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn vscale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn vdot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn vcross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn vlen(a: [f32; 3]) -> f32 {
    vdot(a, a).sqrt()
}
fn vnorm(a: [f32; 3]) -> [f32; 3] {
    let l = vlen(a);
    if l > 1e-20 {
        vscale(a, 1.0 / l)
    } else {
        a
    }
}

/// `true` if world point `p` lies within the AABB (with a small slack so a seed
/// landing exactly on the box wall is not rejected by rounding).
fn in_box(b: &Aabb, p: [f32; 3]) -> bool {
    (0..3).all(|i| p[i] >= b.min[i] - BOX_SLACK && p[i] <= b.max[i] + BOX_SLACK)
}

/// World-space normal of a plane half-space (rotates the local normal out).
fn plane_world_normal(normal: [f32; 3], t: &Transform) -> [f32; 3] {
    vnorm(quat_rotate(t.rotation, normal))
}

/// A world-space point on a plane half-space (its local foot point mapped out).
fn plane_world_point(normal: [f32; 3], offset: f32, t: &Transform) -> [f32; 3] {
    // Local foot of the plane: offset·n (since n·p = offset, n unit).
    let n = vnorm(normal);
    local_to_world(t, vscale(n, offset))
}

// ─── closed-form: plane ∩ plane ──────────────────────────────────────────────

fn plane_plane(
    a: &HalfSpace,
    ta: &Transform,
    b: &HalfSpace,
    tb: &Transform,
    overlap: &Aabb,
) -> SsiResult {
    let (HalfSpace::Plane { normal: na, offset: oa }, HalfSpace::Plane { normal: nb, offset: ob }) =
        (a, b)
    else {
        unreachable!("plane_plane called with non-plane operands");
    };
    let n1 = plane_world_normal(*na, ta);
    let n2 = plane_world_normal(*nb, tb);
    let p1 = plane_world_point(*na, *oa, ta);
    let p2 = plane_world_point(*nb, *ob, tb);

    let dir = vcross(n1, n2);
    let dl = vlen(dir);
    if dl < PARALLEL_EPS {
        // Parallel (or coincident) planes: no isolated intersection curve.
        return SsiResult::Components(Vec::new());
    }
    let dir = vscale(dir, 1.0 / dl);

    // `c_i = n_i · (point on plane i)` are the two plane constants; with the unit
    // direction `dir`, `line_point` gives one point on the intersection line.
    let c1 = vdot(n1, p1);
    let c2 = vdot(n2, p2);
    let base = line_point(n1, c1, n2, c2, dir);

    // Clip the infinite intersection line `base + t·dir` against the overlap box
    // (slab method) and seed at the midpoint of the in-box segment. A single
    // clamped point would be pulled OFF the line whenever the line misses the
    // box, emitting an off-surface seed for perpendicular planes whose true line
    // lies outside the box; proper clipping returns 0 components in that case.
    let mut t_lo = f32::NEG_INFINITY;
    let mut t_hi = f32::INFINITY;
    for i in 0..3 {
        if dir[i].abs() < PARALLEL_EPS {
            // Line is constant on this axis: if `base` sits outside the slab the
            // line never enters the box.
            if base[i] < overlap.min[i] - BOX_SLACK || base[i] > overlap.max[i] + BOX_SLACK {
                return SsiResult::Components(Vec::new());
            }
        } else {
            let inv = 1.0 / dir[i];
            let mut ta = (overlap.min[i] - base[i]) * inv;
            let mut tb = (overlap.max[i] - base[i]) * inv;
            if ta > tb {
                std::mem::swap(&mut ta, &mut tb);
            }
            t_lo = t_lo.max(ta);
            t_hi = t_hi.min(tb);
        }
    }
    if t_lo > t_hi {
        // The line misses the box.
        return SsiResult::Components(Vec::new());
    }
    // The intersection of two planes is a single connected line; seed it densely
    // along the in-box segment so a line later trimmed into several active
    // intervals (e.g. a slot cut across an edge) keeps a seed on each.
    let mut comp = Vec::with_capacity(SEEDS_PER_CURVE);
    for k in 0..SEEDS_PER_CURVE {
        let f = if SEEDS_PER_CURVE == 1 {
            0.5
        } else {
            k as f32 / (SEEDS_PER_CURVE as f32 - 1.0)
        };
        let t = t_lo + (t_hi - t_lo) * f;
        comp.push(vadd(base, vscale(dir, t)));
    }
    SsiResult::Components(vec![comp])
}

/// One point on the line `{x : n1·x = c1, n2·x = c2}` with unit direction `dir`.
fn line_point(n1: [f32; 3], c1: f32, n2: [f32; 3], c2: f32, dir: [f32; 3]) -> [f32; 3] {
    let n2xd = vcross(n2, dir);
    let dxn1 = vcross(dir, n1);
    vadd(vscale(n2xd, c1), vscale(dxn1, c2))
}

// ─── closed-form: plane∩sphere, sphere∩sphere ────────────────────────────────

/// Handles plane∩sphere and sphere∩sphere: each reduces to "is there an
/// intersection circle, and if so, is a point of it inside the box?".
fn sphere_like(
    a: &HalfSpace,
    ta: &Transform,
    b: &HalfSpace,
    tb: &Transform,
    overlap: &Aabb,
) -> SsiResult {
    // Normalise to (sphere, other) where convenient.
    let (sphere, ts, other, to) = match (a, b) {
        (HalfSpace::Sphere { .. }, _) => (a, ta, b, tb),
        (_, HalfSpace::Sphere { .. }) => (b, tb, a, ta),
        _ => unreachable!("sphere_like called without a sphere operand"),
    };
    let HalfSpace::Sphere { radius: r } = sphere else {
        unreachable!()
    };
    let r = *r;
    let centre = local_to_world(ts, [0.0, 0.0, 0.0]);

    match other {
        HalfSpace::Plane { normal, offset } => {
            let n = plane_world_normal(*normal, to);
            let pp = plane_world_point(*normal, *offset, to);
            let dist = vdot(vsub(centre, pp), n); // signed distance centre→plane
            if dist.abs() > r {
                return SsiResult::Components(Vec::new()); // no intersection
            }
            // Circle: centre projected onto the plane, radius √(r²−dist²).
            let circ_c = vsub(centre, vscale(n, dist));
            let circ_r = (r * r - dist * dist).max(0.0).sqrt();
            seed_from_circle(circ_c, n, circ_r, overlap)
        }
        HalfSpace::Sphere { radius: r2 } => {
            let r2 = *r2;
            let c2 = local_to_world(to, [0.0, 0.0, 0.0]);
            let d = vlen(vsub(c2, centre));
            if d < PARALLEL_EPS {
                // Concentric — no curve (coincident or nested).
                return SsiResult::Components(Vec::new());
            }
            if d > r + r2 || d < (r - r2).abs() {
                return SsiResult::Components(Vec::new()); // separate or contained
            }
            // Intersection circle plane: perpendicular to centre-line at
            // distance `h` from the first sphere centre.
            let n = vnorm(vsub(c2, centre));
            let h = (d * d + r * r - r2 * r2) / (2.0 * d);
            let circ_c = vadd(centre, vscale(n, h));
            let circ_r = (r * r - h * h).max(0.0).sqrt();
            seed_from_circle(circ_c, n, circ_r, overlap)
        }
        _ => unreachable!("sphere_like other operand must be plane or sphere"),
    }
}

/// A circle of radius `cr` centred at `cc` lying in the plane with normal `n`.
/// The circle is one connected component; return **all** uniformly-sampled
/// points of it that fall inside the box (so a circle later trimmed into several
/// active arcs keeps a seed on each), or no component if none fall inside.
fn seed_from_circle(cc: [f32; 3], n: [f32; 3], cr: f32, overlap: &Aabb) -> SsiResult {
    // Two in-plane basis vectors.
    let (u, v) = ortho_basis(n);
    let mut comp = Vec::new();
    for i in 0..SEEDS_PER_CURVE {
        let ang = std::f32::consts::TAU * (i as f32) / (SEEDS_PER_CURVE as f32);
        let p = vadd(cc, vadd(vscale(u, cr * ang.cos()), vscale(v, cr * ang.sin())));
        if in_box(overlap, p) {
            comp.push(p);
        }
    }
    if comp.is_empty() {
        SsiResult::Components(Vec::new())
    } else {
        SsiResult::Components(vec![comp])
    }
}

/// Two orthonormal vectors spanning the plane with unit normal `n`.
fn ortho_basis(n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let pick = if n[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = vnorm(vcross(n, pick));
    let v = vnorm(vcross(n, u));
    (u, v)
}

// ─── general 2D-parameter-domain scan ────────────────────────────────────────

/// Parametrisation of surface `A` over a `(u,v)` rectangle: `point(u,v)` maps a
/// domain coordinate to a world point on `A`, `u_periodic` marks the θ axis
/// (column 0 adjacent to column N for seam union-find).
struct ParamA<'a> {
    h: &'a HalfSpace,
    t: &'a Transform,
    u_range: (f32, f32),
    v_range: (f32, f32),
    u_periodic: bool,
}

impl ParamA<'_> {
    fn point(&self, u: f32, v: f32) -> [f32; 3] {
        let local = match self.h {
            HalfSpace::InfiniteCylinder { radius } => {
                // u = θ around Y, v = height along Y.
                [radius * u.cos(), v, radius * u.sin()]
            }
            HalfSpace::Cone { half_angle } => {
                // u = θ around Y, v = signed axial coord; radius = |v|·tanα.
                let r = v.abs() * half_angle.tan();
                [r * u.cos(), v, r * u.sin()]
            }
            HalfSpace::Sphere { radius } => {
                // u = azimuth around Y, v = polar angle from +Y.
                let sv = v.sin();
                [radius * sv * u.cos(), radius * v.cos(), radius * sv * u.sin()]
            }
            HalfSpace::Plane { normal, offset } => {
                // u,v are coordinates in the plane's in-plane basis.
                let n = vnorm(*normal);
                let (e1, e2) = ortho_basis(n);
                let foot = vscale(n, *offset);
                [
                    foot[0] + e1[0] * u + e2[0] * v,
                    foot[1] + e1[1] * u + e2[1] * v,
                    foot[2] + e1[2] * u + e2[2] * v,
                ]
            }
            HalfSpace::Torus { .. }
            | HalfSpace::Gyroid { .. }
            | HalfSpace::RevolvedProfile { .. } => unreachable!("filtered earlier"),
        };
        local_to_world(self.t, local)
    }
}

/// Pick which operand becomes the parametrised surface `A` and build its domain
/// rectangle from the overlap box. Prefers cylinder/cone (bounded θ-period) over
/// sphere over plane, per the plan.
fn choose_param<'a>(
    a: &'a HalfSpace,
    ta: &'a Transform,
    b: &'a HalfSpace,
    tb: &'a Transform,
    overlap: &Aabb,
) -> Option<(ParamA<'a>, &'a HalfSpace, &'a Transform)> {
    fn rank(h: &HalfSpace) -> u8 {
        match h {
            HalfSpace::Cone { .. } => 0,
            HalfSpace::InfiniteCylinder { .. } => 1,
            HalfSpace::Sphere { .. } => 2,
            HalfSpace::Plane { .. } => 3,
            _ => 9,
        }
    }
    // Lower rank wins as the parametrised surface.
    let (pa, pta, ob, otb) = if rank(a) <= rank(b) {
        (a, ta, b, tb)
    } else {
        (b, tb, a, ta)
    };
    let param = build_param(pa, pta, overlap)?;
    Some((param, ob, otb))
}

/// Project the eight world corners of `overlap` into `A`'s local frame to derive
/// linear param ranges; θ always spans the full period.
fn build_param<'a>(h: &'a HalfSpace, t: &'a Transform, overlap: &Aabb) -> Option<ParamA<'a>> {
    let corners = box_corners(overlap);
    // Corners in A's local frame.
    let local: Vec<[f32; 3]> = corners.iter().map(|&c| world_to_local(t, c)).collect();

    match h {
        HalfSpace::InfiniteCylinder { .. } => {
            let (vmin, vmax) = axis_range(&local, 1); // Y
            Some(ParamA {
                h,
                t,
                u_range: (0.0, std::f32::consts::TAU),
                v_range: pad(vmin, vmax),
                u_periodic: true,
            })
        }
        HalfSpace::Cone { .. } => {
            let (vmin, vmax) = axis_range(&local, 1); // Y (signed axial)
            Some(ParamA {
                h,
                t,
                u_range: (0.0, std::f32::consts::TAU),
                v_range: pad(vmin, vmax),
                u_periodic: true,
            })
        }
        HalfSpace::Sphere { .. } => Some(ParamA {
            h,
            t,
            u_range: (0.0, std::f32::consts::TAU),
            // Full polar sweep [0,π]; θ periodic.
            v_range: (0.0, std::f32::consts::PI),
            u_periodic: true,
        }),
        HalfSpace::Plane { normal, offset } => {
            let n = vnorm(*normal);
            let (e1, e2) = ortho_basis(n);
            let foot = vscale(n, *offset);
            // u,v are in-plane coordinates of each local corner relative to foot.
            let mut umin = f32::INFINITY;
            let mut umax = f32::NEG_INFINITY;
            let mut vmin = f32::INFINITY;
            let mut vmax = f32::NEG_INFINITY;
            for c in &local {
                let rel = vsub(*c, foot);
                let cu = vdot(rel, e1);
                let cv = vdot(rel, e2);
                umin = umin.min(cu);
                umax = umax.max(cu);
                vmin = vmin.min(cv);
                vmax = vmax.max(cv);
            }
            Some(ParamA {
                h,
                t,
                u_range: pad(umin, umax),
                v_range: pad(vmin, vmax),
                u_periodic: false,
            })
        }
        HalfSpace::Torus { .. }
        | HalfSpace::Gyroid { .. }
        | HalfSpace::RevolvedProfile { .. } => None,
    }
}

/// Min/max of one local-frame axis over the corner set.
fn axis_range(local: &[[f32; 3]], axis: usize) -> (f32, f32) {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for c in local {
        lo = lo.min(c[axis]);
        hi = hi.max(c[axis]);
    }
    (lo, hi)
}

/// Pad a linear range slightly so a curve grazing the box wall is still sampled.
fn pad(lo: f32, hi: f32) -> (f32, f32) {
    let span = (hi - lo).abs().max(1e-3);
    let p = span * 0.05;
    (lo - p, hi + p)
}

/// Expand an AABB by 5 % of each axis extent (with a small absolute floor) for
/// the scan in-box mask, so a curve lying exactly on the overlap-box boundary is
/// still captured. See the call site in [`scan_components`] for why.
fn pad_aabb(b: &Aabb) -> Aabb {
    let mut min = b.min;
    let mut max = b.max;
    for i in 0..3 {
        let p = (b.max[i] - b.min[i]).abs().max(1e-3) * 0.05;
        min[i] -= p;
        max[i] += p;
    }
    Aabb { min, max }
}

fn box_corners(b: &Aabb) -> [[f32; 3]; 8] {
    [
        [b.min[0], b.min[1], b.min[2]],
        [b.max[0], b.min[1], b.min[2]],
        [b.min[0], b.max[1], b.min[2]],
        [b.max[0], b.max[1], b.min[2]],
        [b.min[0], b.min[1], b.max[2]],
        [b.max[0], b.min[1], b.max[2]],
        [b.min[0], b.max[1], b.max[2]],
        [b.max[0], b.max[1], b.max[2]],
    ]
}

/// The core scan: sample `g = implicit_B(A(u,v))`, mask to in-box cells, march
/// the zero-contour, union-find into components, emit one refined seed each.
fn scan_components(
    a: &HalfSpace,
    ta: &Transform,
    b: &HalfSpace,
    tb: &Transform,
    overlap: &Aabb,
) -> SsiResult {
    let Some((param, other, ot)) = choose_param(a, ta, b, tb, overlap) else {
        return SsiResult::Unsupported;
    };

    let n = SCAN_RES;
    let (u0, u1) = param.u_range;
    let (v0, v1) = param.v_range;
    let du = (u1 - u0) / (n as f32 - 1.0);
    let dv = (v1 - v0) / (n as f32 - 1.0);

    // The in-box mask uses a slightly PADDED overlap box. A real edge can lie
    // exactly on the overlap-box boundary (e.g. a cylinder wall meeting a box
    // top face where that face plane is the box's own AABB wall): the curve sits
    // on the box face, and the cell straddling it has its outer row out-of-box,
    // so a strict mask drops the only cell that carries the sign change and the
    // edge is missed. Padding the mask captures boundary-coincident curves. This
    // can only ADD candidate seeds (never miss one) — Phase 2's active-interval
    // march rejects any that aren't real body edges — which is the safe
    // direction per the plan's "never miss a real edge" contract.
    let mbox = pad_aabb(overlap);

    // Sample grid: world point, in-box mask, and signed value g.
    let mut g = vec![0.0f32; n * n];
    let mut inbox = vec![false; n * n];
    for j in 0..n {
        for i in 0..n {
            let u = u0 + du * i as f32;
            let v = v0 + dv * j as f32;
            let p = param.point(u, v);
            inbox[j * n + i] = in_box(&mbox, p);
            g[j * n + i] = implicit_world(other, ot, p);
        }
    }

    // ── Marching-squares with proper arc topology ───────────────────────────
    //
    // Union-find runs over *grid-edge crossings* (one node per grid edge whose
    // two endpoints straddle g = 0), NOT over cells. Inside each cell we connect
    // crossings the way the contour ARCS run: the 0/1/3-crossing cases link
    // their (≤2) crossings into one arc, but the **saddle** case (the 2-2
    // diagonal "checkerboard" sign pattern, 4 crossings) carries TWO disjoint
    // arcs — we pair its crossings as two separate arcs and do NOT merge them.
    // That is exactly the intrinsic-multiplicity-(B) split: a self-touching seam
    // (two equal perpendicular cylinders) meets at a saddle and must count as 2
    // components, not 1. Edges shared between adjacent cells are the SAME node,
    // so arcs continue across cell walls automatically; θ-wrap is handled by the
    // periodic edge index. The seam never desyncs because both sides reference
    // one edge id.

    // Edge node ids. Horizontal edge h(i,j) joins corners (i,j)-(i+1,j);
    // vertical edge w(i,j) joins (i,j)-(i,j+1). i in 0..n-1 (h) / 0..n (w, with
    // wrap), j as appropriate.
    let h_id = |i: usize, j: usize| j * (n - 1) + i; // 0 .. (n-1)*n
    let h_count = (n - 1) * n;
    let w_id = |i: usize, j: usize| h_count + j * n + i; // 0 .. n*(n-1)
    let total_nodes = h_count + n * (n - 1);
    let mut uf = UnionFind::new(total_nodes);

    let sample_in = |i: usize, j: usize| inbox[j * n + i];
    let sample_g = |i: usize, j: usize| g[j * n + i];

    // Does a grid edge cross zero, with both endpoints in-box?
    let h_cross = |i: usize, j: usize| -> bool {
        i + 1 < n
            && sample_in(i, j)
            && sample_in(i + 1, j)
            && (sample_g(i, j) > 0.0) != (sample_g(i + 1, j) > 0.0)
    };
    let w_cross = |i: usize, j: usize| -> bool {
        j + 1 < n
            && sample_in(i, j)
            && sample_in(i, j + 1)
            && (sample_g(i, j) > 0.0) != (sample_g(i, j + 1) > 0.0)
    };

    let cols = if param.u_periodic { n } else { n - 1 };
    for cj in 0..(n - 1) {
        for ci in 0..cols {
            let i1 = if param.u_periodic { (ci + 1) % n } else { ci + 1 };
            // Four corners must be in-box for this cell to host contour arcs.
            if !(sample_in(ci, cj)
                && sample_in(i1, cj)
                && sample_in(ci, cj + 1)
                && sample_in(i1, cj + 1))
            {
                continue;
            }
            // The cell's four edges (bottom, right, top, left) and whether each
            // crosses. For the periodic wrap column the right/top edges live at
            // index 0 (i1==0), which `h_id`/`w_id` address correctly.
            let bottom = (h_cross(ci, cj), h_id(ci, cj));
            let top = (h_cross(ci, cj + 1), h_id(ci, cj + 1));
            let left = (w_cross(ci, cj), w_id(ci, cj));
            let right = (w_cross(i1, cj), w_id(i1, cj));
            let crossings: Vec<usize> = [bottom, right, top, left]
                .iter()
                .filter(|(c, _)| *c)
                .map(|(_, id)| *id)
                .collect();
            match crossings.len() {
                0 | 1 => {}
                2 => {
                    uf.union(crossings[0], crossings[1]);
                }
                4 => {
                    // Saddle: two arcs. Resolve with the asymptotic decider —
                    // pair the four edges so each arc separates one same-sign
                    // diagonal. Using the cell-centre value picks the pairing.
                    let cu = u0 + du * (ci as f32 + 0.5);
                    let cv = v0 + dv * (cj as f32 + 0.5);
                    let centre_g = implicit_world(other, ot, param.point(cu, cv));
                    let c00 = sample_g(ci, cj) > 0.0; // bottom-left corner sign
                    // bottom edge id, right, top, left already in [b,r,t,l].
                    let (b, r, t, l) = (bottom.1, right.1, top.1, left.1);
                    // Two pairings: {b-r, t-l} or {b-l, t-r}. The asymptotic
                    // decider connects so the contour does not pass through the
                    // saddle: pick the pairing matching the centre sign vs the
                    // corner-(0,0) sign.
                    if (centre_g > 0.0) == c00 {
                        uf.union(b, r);
                        uf.union(t, l);
                    } else {
                        uf.union(b, l);
                        uf.union(t, r);
                    }
                }
                _ => {
                    // 3 crossings can only happen with a zero exactly on a
                    // corner; treat conservatively as one arc.
                    for k in 1..crossings.len() {
                        uf.union(crossings[0], crossings[k]);
                    }
                }
            }
        }
    }

    // Every grid-edge crossing yields a seed (refined onto the curve), bucketed
    // by its union-find root so each connected component carries ALL its seeds —
    // not just one. A component trimmed by a third surface into several active
    // arcs then keeps a seed on each arc; the weaver's co-component dedup
    // collapses the redundant same-arc seeds. Insertion order is deterministic
    // (the fixed grid traversal), so the seed stream is stable across runs.
    let mut root_idx: HashMap<usize, usize> = HashMap::new();
    let mut comps: Vec<Vec<[f32; 3]>> = Vec::new();
    let mut bucket = |uf: &mut UnionFind, node: usize, seed: [f32; 3]| {
        let r = uf.find(node);
        let next = comps.len();
        let idx = *root_idx.entry(r).or_insert(next);
        if idx == next {
            comps.push(Vec::new());
        }
        comps[idx].push(seed);
    };

    // Horizontal-edge crossings.
    for j in 0..n {
        for i in 0..(n - 1) {
            if !h_cross(i, j) {
                continue;
            }
            let ua = u0 + du * i as f32;
            let ub = u0 + du * (i + 1) as f32;
            let vv = v0 + dv * j as f32;
            if let Some(seed) = refine_edge(&param, other, ot, &mbox, ua, vv, ub, vv) {
                bucket(&mut uf, h_id(i, j), seed);
            }
        }
    }
    // Vertical-edge crossings.
    for j in 0..(n - 1) {
        for i in 0..n {
            if !w_cross(i, j) {
                continue;
            }
            let uu = u0 + du * i as f32;
            let va = v0 + dv * j as f32;
            let vb = v0 + dv * (j + 1) as f32;
            if let Some(seed) = refine_edge(&param, other, ot, &mbox, uu, va, uu, vb) {
                bucket(&mut uf, w_id(i, j), seed);
            }
        }
    }

    SsiResult::Components(comps)
}

/// Bisect `g = implicit_other(A(u,v))` along the grid edge from `(ua,va)` to
/// `(ub,vb)` (which straddle the contour) and return the refined world seed if
/// it lands inside the box.
#[allow(clippy::too_many_arguments)]
fn refine_edge(
    param: &ParamA,
    other: &HalfSpace,
    ot: &Transform,
    overlap: &Aabb,
    ua: f32,
    va: f32,
    ub: f32,
    vb: f32,
) -> Option<[f32; 3]> {
    let ga = implicit_world(other, ot, param.point(ua, va));
    let (mut lo_u, mut lo_v, mut lo_g) = (ua, va, ga);
    let (mut hi_u, mut hi_v) = (ub, vb);
    for _ in 0..BISECT_STEPS {
        let mu = 0.5 * (lo_u + hi_u);
        let mv = 0.5 * (lo_v + hi_v);
        let gm = implicit_world(other, ot, param.point(mu, mv));
        if (gm > 0.0) == (lo_g > 0.0) {
            lo_u = mu;
            lo_v = mv;
            lo_g = gm;
        } else {
            hi_u = mu;
            hi_v = mv;
        }
    }
    let su = 0.5 * (lo_u + hi_u);
    let sv = 0.5 * (lo_v + hi_v);
    let seed = param.point(su, sv);
    if in_box(overlap, seed) {
        Some(seed)
    } else {
        None
    }
}

// ─── union-find ──────────────────────────────────────────────────────────────

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const ON_SURF: f32 = 1e-4;

    fn aabb(min: [f32; 3], max: [f32; 3]) -> Aabb {
        Aabb { min, max }
    }

    /// Assert every seed of every component lies on BOTH surfaces to tolerance.
    fn assert_seeds_on_both(
        comps: &[Vec<[f32; 3]>],
        a: &HalfSpace,
        ta: &Transform,
        b: &HalfSpace,
        tb: &Transform,
    ) {
        for comp in comps {
            assert!(!comp.is_empty(), "a component must carry at least one seed");
            for &s in comp {
                let da = implicit_world(a, ta, s).abs();
                let db = implicit_world(b, tb, s).abs();
                assert!(da < ON_SURF, "seed {s:?} not on A: |implicit_A|={da}");
                assert!(db < ON_SURF, "seed {s:?} not on B: |implicit_B|={db}");
            }
        }
    }

    /// Number of connected components (the outer length).
    fn count(r: &SsiResult) -> usize {
        match r {
            SsiResult::Components(v) => v.len(),
            SsiResult::Unsupported => panic!("unexpected Unsupported"),
        }
    }

    #[test]
    fn two_perpendicular_planes_one_component() {
        // x=0 plane and z=0 plane → intersection is the Y axis.
        let a = HalfSpace::Plane { normal: [1.0, 0.0, 0.0], offset: 0.0 };
        let b = HalfSpace::Plane { normal: [0.0, 0.0, 1.0], offset: 0.0 };
        let id = Transform::IDENTITY;
        let box_ = aabb([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]);
        let r = ssi_seeds(&a, &id, &b, &id, &box_);
        assert_eq!(count(&r), 1);
        if let SsiResult::Components(v) = &r {
            assert_seeds_on_both(v, &a, &id, &b, &id);
        }
    }

    #[test]
    fn two_parallel_planes_zero_components() {
        let a = HalfSpace::Plane { normal: [0.0, 1.0, 0.0], offset: 0.0 };
        let b = HalfSpace::Plane { normal: [0.0, 1.0, 0.0], offset: 0.5 };
        let id = Transform::IDENTITY;
        let box_ = aabb([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]);
        let r = ssi_seeds(&a, &id, &b, &id, &box_);
        assert_eq!(count(&r), 0);
    }

    #[test]
    fn plane_cuts_cylinder_perpendicular_one_circle() {
        // Cylinder axis = Y, radius 1. Plane y=0 cuts it → one circle.
        let cyl = HalfSpace::InfiniteCylinder { radius: 1.0 };
        let pl = HalfSpace::Plane { normal: [0.0, 1.0, 0.0], offset: 0.0 };
        let id = Transform::IDENTITY;
        let box_ = aabb([-2.0, -2.0, -2.0], [2.0, 2.0, 2.0]);
        let r = ssi_seeds(&cyl, &id, &pl, &id, &box_);
        assert_eq!(count(&r), 1, "perpendicular plane through cylinder = 1 circle");
        if let SsiResult::Components(v) = &r {
            assert_seeds_on_both(v, &cyl, &id, &pl, &id);
        }
    }

    #[test]
    fn plane_parallel_cylinder_axis_offset_two_lines() {
        // Plane x = 0.5 (normal +X), parallel to cylinder Y-axis, offset < radius
        // → two parallel line components on the cylinder. THE key (A) case.
        let cyl = HalfSpace::InfiniteCylinder { radius: 1.0 };
        let pl = HalfSpace::Plane { normal: [1.0, 0.0, 0.0], offset: 0.5 };
        let id = Transform::IDENTITY;
        let box_ = aabb([-2.0, -2.0, -2.0], [2.0, 2.0, 2.0]);
        let r = ssi_seeds(&cyl, &id, &pl, &id, &box_);
        assert_eq!(count(&r), 2, "plane through cylinder body = 2 lines");
        if let SsiResult::Components(v) = &r {
            assert_seeds_on_both(v, &cyl, &id, &pl, &id);
        }
    }

    #[test]
    fn plane_parallel_cone_axis_hyperbola_two_branches() {
        // Cone (axis Y, half-angle 45°). Plane x = 0.5 parallel to the axis,
        // offset → a hyperbola with two branches (one per nappe / two arms).
        let cone = HalfSpace::Cone { half_angle: std::f32::consts::FRAC_PI_4 };
        let pl = HalfSpace::Plane { normal: [1.0, 0.0, 0.0], offset: 0.5 };
        let id = Transform::IDENTITY;
        let box_ = aabb([-3.0, -3.0, -3.0], [3.0, 3.0, 3.0]);
        let r = ssi_seeds(&cone, &id, &pl, &id, &box_);
        assert_eq!(count(&r), 2, "plane offset from cone axis = 2 hyperbola branches");
        if let SsiResult::Components(v) = &r {
            assert_seeds_on_both(v, &cone, &id, &pl, &id);
        }
    }

    #[test]
    fn two_equal_cylinders_perpendicular_axes_two_components() {
        // Cylinder A: axis Y (identity). Cylinder B: axis X (rotate Y-axis onto
        // X by 90° about Z). Equal radii, intersecting axes → classic 2-curve
        // bicylinder seam.
        let cyl = HalfSpace::InfiniteCylinder { radius: 1.0 };
        let id = Transform::IDENTITY;
        // Rotate +90° about Z maps local Y to world X.
        let rot_z90 = Transform {
            translate: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, (std::f32::consts::FRAC_PI_4).sin(), (std::f32::consts::FRAC_PI_4).cos()],
            scale: [1.0, 1.0, 1.0],
        };
        let box_ = aabb([-2.0, -2.0, -2.0], [2.0, 2.0, 2.0]);
        let r = ssi_seeds(&cyl, &id, &cyl, &rot_z90, &box_);
        assert_eq!(count(&r), 2, "perpendicular equal cylinders = 2 seam curves");
        if let SsiResult::Components(v) = &r {
            assert_seeds_on_both(v, &cyl, &id, &cyl, &rot_z90);
        }
    }

    #[test]
    fn plane_sphere_cutting_one_component() {
        let sph = HalfSpace::Sphere { radius: 1.0 };
        let pl = HalfSpace::Plane { normal: [0.0, 1.0, 0.0], offset: 0.0 };
        let id = Transform::IDENTITY;
        let box_ = aabb([-2.0, -2.0, -2.0], [2.0, 2.0, 2.0]);
        let r = ssi_seeds(&sph, &id, &pl, &id, &box_);
        assert_eq!(count(&r), 1);
        if let SsiResult::Components(v) = &r {
            assert_seeds_on_both(v, &sph, &id, &pl, &id);
        }
    }

    #[test]
    fn plane_sphere_non_cutting_zero_components() {
        let sph = HalfSpace::Sphere { radius: 1.0 };
        let pl = HalfSpace::Plane { normal: [0.0, 1.0, 0.0], offset: 2.0 };
        let id = Transform::IDENTITY;
        let box_ = aabb([-3.0, -3.0, -3.0], [3.0, 3.0, 3.0]);
        let r = ssi_seeds(&sph, &id, &pl, &id, &box_);
        assert_eq!(count(&r), 0);
    }

    #[test]
    fn two_spheres_cutting_one_circle() {
        let sph = HalfSpace::Sphere { radius: 1.0 };
        let id = Transform::IDENTITY;
        let shifted = Transform {
            translate: [1.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        };
        let box_ = aabb([-2.0, -2.0, -2.0], [2.0, 2.0, 2.0]);
        let r = ssi_seeds(&sph, &id, &sph, &shifted, &box_);
        assert_eq!(count(&r), 1);
        if let SsiResult::Components(v) = &r {
            assert_seeds_on_both(v, &sph, &id, &sph, &shifted);
        }
    }

    #[test]
    fn torus_operand_unsupported() {
        let tor = HalfSpace::Torus { major_radius: 2.0, minor_radius: 0.5 };
        let pl = HalfSpace::Plane { normal: [0.0, 1.0, 0.0], offset: 0.0 };
        let id = Transform::IDENTITY;
        let box_ = aabb([-3.0, -3.0, -3.0], [3.0, 3.0, 3.0]);
        let r = ssi_seeds(&tor, &id, &pl, &id, &box_);
        assert_eq!(r, SsiResult::Unsupported);
    }
}
