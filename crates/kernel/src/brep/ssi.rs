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
//!   cylinder∩cylinder, cone∩anything, sphere∩cylinder/cone, non-coaxial
//!   revolved-profile pairs, …): parametrise
//!   one surface `A` over a `(u,v)` rectangle derived from the overlap box,
//!   sample `g(u,v) = implicit_B(A(u,v))` on a grid, mask to in-box cells,
//!   march the zero-contour, union-find the contour cells into connected
//!   components (wrapping the periodic θ seam), and emit one bisection-refined
//!   seed per component.
//! - **Unsupported** (torus as either operand; gyroid∩gyroid): caller falls
//!   back to the octree route. A **gyroid** works as the IMPLICIT operand `B`
//!   under any parametrisable `A` (its shell field `(|f|−t)·s·0.2887` is a
//!   signed implicit like every other `sdf_halfspace`); the scan resolution
//!   scales with the gyroid's lattice period and shell width so the zero
//!   ribbons are resolved. It can never be the parametrised side (a TPMS has
//!   no (u,v) chart), which is what keeps gyroid∩gyroid unsupported.

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
    /// Torus operand, gyroid∩gyroid, or a configuration the scan cannot
    /// parametrise — the caller should fall back to the octree route.
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
    // Torus has no support at all yet — bail to the octree. A gyroid is fine
    // as the IMPLICIT side of the scan; it only fails as the parametrised
    // side, which `choose_param` reports (gyroid∩gyroid → Unsupported below).
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

    // Coaxial revolution pair: both surfaces revolve about the same world
    // axis → every intersection component is a CIRCLE, found as a root of
    // the two profile curves in the shared (r, y) half-plane. Cheap and
    // exact, so it short-circuits before the scan for `RevolvedProfile`
    // carriers (spline revolve walls and their shell inner walls).
    if let Some(res) = coaxial_revolution(a, ta, b, tb, overlap) {
        return res;
    }

    // General 2D-parameter-domain scan. Non-coaxial pairs involving a
    // `RevolvedProfile` land here too: the profile carrier is a signed
    // implicit (`sdf_halfspace`), so it works as operand B under any
    // parametrised A, and as A it parametrises over (θ, profile pieces).
    scan_components(a, ta, b, tb, overlap)
}

// ─── coaxial revolution pairs ────────────────────────────────────────────────

/// A world-space revolution axis (point + unit direction).
struct RevAxis {
    origin: [f32; 3],
    dir: [f32; 3],
}

/// Length tolerance for the coaxiality test (axis anchors / sphere centres off
/// the shared axis). Pairs from one revolve feature share a transform exactly;
/// this absolute slack only decides whether *cross-feature* pairs may take the
/// coaxial route — drifted ones fall back to the scan, which is correct either
/// way, so the tolerance is not load-bearing for correctness.
const COAX_TOL: f32 = 1e-3;

/// The intrinsic revolution axis of `h`, if it has one (local +Y mapped to
/// world). Planes and spheres adapt to any axis and return `None`.
fn axis_of(h: &HalfSpace, t: &Transform) -> Option<RevAxis> {
    match h {
        HalfSpace::InfiniteCylinder { .. }
        | HalfSpace::Cone { .. }
        | HalfSpace::Torus { .. }
        | HalfSpace::RevolvedProfile { .. } => Some(RevAxis {
            origin: local_to_world(t, [0.0, 0.0, 0.0]),
            dir: vnorm(quat_rotate(t.rotation, [0.0, 1.0, 0.0])),
        }),
        _ => None,
    }
}

/// Distance of world point `p` from the axis line.
fn dist_to_axis(p: [f32; 3], axis: &RevAxis) -> f32 {
    let d = vsub(p, axis.origin);
    let along = vdot(d, axis.dir);
    vlen(vsub(d, vscale(axis.dir, along)))
}

/// Whether `h` is a revolution surface about `axis` (within tolerance).
fn coaxial_with(h: &HalfSpace, t: &Transform, axis: &RevAxis) -> bool {
    match h {
        HalfSpace::Plane { normal, .. } => {
            vlen(vcross(plane_world_normal(*normal, t), axis.dir)) < PARALLEL_EPS
        }
        HalfSpace::Sphere { .. } => dist_to_axis(local_to_world(t, [0.0, 0.0, 0.0]), axis) < COAX_TOL,
        HalfSpace::InfiniteCylinder { .. }
        | HalfSpace::Cone { .. }
        | HalfSpace::RevolvedProfile { .. } => match axis_of(h, t) {
            Some(own) => {
                vlen(vcross(own.dir, axis.dir)) < PARALLEL_EPS
                    && dist_to_axis(own.origin, axis) < COAX_TOL
            }
            None => false,
        },
        HalfSpace::Torus { .. } | HalfSpace::Gyroid { .. } => false,
    }
}

/// Curve-A preference: the most structured profile is sampled, the other
/// operand is evaluated implicitly along it.
fn profile_rank(h: &HalfSpace) -> u8 {
    match h {
        HalfSpace::RevolvedProfile { .. } => 0,
        HalfSpace::Cone { .. } => 1,
        HalfSpace::InfiniteCylinder { .. } => 2,
        HalfSpace::Sphere { .. } => 3,
        _ => 9,
    }
}

/// One contiguous, monotone-parameter piece of operand A's profile curve:
/// `eval(t ∈ [0,1])` returns a LOCAL `(r, y)` profile point in A's centred
/// frame (the φ=0 meridian is `[r, y, 0]`), `samples` is the sign-scan
/// density along the piece. Pieces are emitted in CONTINUOUS chain order
/// (start end-piece reversed, segments, end end-piece), so concatenating
/// them yields a continuous curve parametrisation — the property the
/// revolved-surface arm of [`ParamA::point`] relies on.
struct ProfilePiece<'a> {
    samples: usize,
    eval: Box<dyn Fn(f32) -> [f32; 2] + 'a>,
}

/// Decompose operand A's profile into sampleable pieces, bounded by the
/// overlap box (transformed into A's local frame, generously padded). The
/// RevolvedProfile pieces include the two tangent-extension rays past the
/// chain ends: that is where shared-corner roots with adjacent groups live
/// (the carriers cross transversally there), and an inner-wall corner's foot
/// can sit far beyond the group end at shallow knick angles (`offset/tanθ`).
fn profile_pieces<'a>(h: &'a HalfSpace, t: &'a Transform, overlap: &Aabb) -> Option<Vec<ProfilePiece<'a>>> {
    // Overlap corners in A's local frame → y-range + a local length scale.
    let corners = box_corners(overlap);
    let local: Vec<[f32; 3]> = corners.iter().map(|&c| world_to_local(t, c)).collect();
    let (ymin, ymax) = {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for c in &local {
            lo = lo.min(c[1]);
            hi = hi.max(c[1]);
        }
        let padd = 0.05 * (hi - lo).max(1e-3);
        (lo - padd, hi + padd)
    };
    let local_diag = {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for c in &local {
            for k in 0..3 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
            }
        }
        let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-3)
    };

    let mut pieces: Vec<ProfilePiece<'a>> = Vec::new();
    match h {
        HalfSpace::InfiniteCylinder { radius } => {
            let r = *radius;
            pieces.push(ProfilePiece {
                samples: 256,
                eval: Box::new(move |s| [r, ymin + s * (ymax - ymin)]),
            });
        }
        HalfSpace::Cone { half_angle } => {
            let tan_a = half_angle.tan();
            pieces.push(ProfilePiece {
                samples: 256,
                eval: Box::new(move |s| {
                    let y = ymin + s * (ymax - ymin);
                    [y.abs() * tan_a, y]
                }),
            });
        }
        HalfSpace::Sphere { radius } => {
            let r = *radius;
            pieces.push(ProfilePiece {
                samples: 256,
                eval: Box::new(move |s| {
                    let phi = std::f32::consts::PI * s;
                    [r * phi.sin(), r * phi.cos()]
                }),
            });
        }
        HalfSpace::RevolvedProfile { segments, ccw, offset, extend_start, extend_end } => {
            let (off, ccw) = (*offset, *ccw);
            let (ext_start, ext_end) = (*extend_start, *extend_end);
            // Offset-corrected point on one segment: p(t) − offset·n_out(t),
            // with the EXACT closed-form tangent. (A central difference is
            // noise here: adaptive spline lowering emits millimetre-short
            // segments whose f32 point differences corrupt the normal by
            // ~1e-3 — enough to push bisected roots off the overlap box.)
            let offset_pt = move |seg: &'a fielddraft_protocol::sketch2d::SketchSegment,
                                  tt: f32|
                  -> [f32; 2] {
                let p = seg.point_at(tt);
                let tan = seg.tangent_at(tt);
                let n = if ccw { [tan[1], -tan[0]] } else { [-tan[1], tan[0]] };
                [p[0] - off * n[0], p[1] - off * n[1]]
            };
            // Chain-end pieces flank the segment run so the concatenated
            // piece list is one CONTINUOUS curve (start piece runs param-
            // reversed: s=1 lands exactly on the chain start). An EXTENDED
            // end contributes its tangent ray (path-backward from the first
            // point, path-forward from the last), offset applied with the
            // ray's constant normal; length = the local overlap diagonal. A
            // STADIUM end (adjacent group opened) contributes the endpoint
            // ARC instead: the offset level set rounds the chain end at
            // radius |off| — that arc is where the opened face's rim root
            // lives.
            let ends = match (segments.first(), segments.last()) {
                (Some(first), Some(last)) => Some((first, last)),
                _ => None,
            };
            let pad = local_diag + 4.0 * off.abs();
            if let Some((first, _)) = ends {
                let a0 = first.start_point();
                let ta0 = first.tangent_at_start();
                let n0 = if ccw { [ta0[1], -ta0[0]] } else { [-ta0[1], ta0[0]] };
                if ext_start {
                    pieces.push(ProfilePiece {
                        samples: 96,
                        eval: Box::new(move |s| {
                            let u = (1.0 - s) * pad;
                            [
                                a0[0] - u * ta0[0] - off * n0[0],
                                a0[1] - u * ta0[1] - off * n0[1],
                            ]
                        }),
                    });
                } else if off.abs() > 1e-6 {
                    // Stadium cap arc, quarter sweep: φ=π/2 → the backward
                    // cap apex (−ta0), φ=0 → the offset curve's own start
                    // (−n0). Beyond π/2 the closest-point sign flips and the
                    // carrier has no surface, so sampling further would
                    // manufacture phantom roots.
                    pieces.push(ProfilePiece {
                        samples: 96,
                        eval: Box::new(move |s| {
                            let phi = (1.0 - s) * std::f32::consts::FRAC_PI_2;
                            let (sin_p, cos_p) = phi.sin_cos();
                            [
                                a0[0] + off * (-n0[0] * cos_p - ta0[0] * sin_p),
                                a0[1] + off * (-n0[1] * cos_p - ta0[1] * sin_p),
                            ]
                        }),
                    });
                }
            }
            for seg in segments.iter() {
                pieces.push(ProfilePiece {
                    samples: 32,
                    eval: Box::new(move |s| offset_pt(seg, s)),
                });
            }
            if let Some((_, last)) = ends {
                let b0 = last.end_point();
                let tb0 = last.tangent_at_end();
                let n1 = if ccw { [tb0[1], -tb0[0]] } else { [-tb0[1], tb0[0]] };
                if ext_end {
                    pieces.push(ProfilePiece {
                        samples: 96,
                        eval: Box::new(move |s| {
                            let u = s * pad;
                            [
                                b0[0] + u * tb0[0] - off * n1[0],
                                b0[1] + u * tb0[1] - off * n1[1],
                            ]
                        }),
                    });
                } else if off.abs() > 1e-6 {
                    pieces.push(ProfilePiece {
                        samples: 96,
                        eval: Box::new(move |s| {
                            let phi = s * std::f32::consts::FRAC_PI_2;
                            let (sin_p, cos_p) = phi.sin_cos();
                            [
                                b0[0] + off * (-n1[0] * cos_p + tb0[0] * sin_p),
                                b0[1] + off * (-n1[1] * cos_p + tb0[1] * sin_p),
                            ]
                        }),
                    });
                }
            }
        }
        _ => return None,
    }
    Some(pieces)
}

/// Try the coaxial-revolution short-circuit. `None` = the pair is not coaxial
/// (caller continues with the general machinery); `Some` = handled, every
/// intersection component is a circle about the shared axis.
fn coaxial_revolution(
    a: &HalfSpace,
    ta: &Transform,
    b: &HalfSpace,
    tb: &Transform,
    overlap: &Aabb,
) -> Option<SsiResult> {
    let axis = axis_of(a, ta).or_else(|| axis_of(b, tb))?;
    if !(coaxial_with(a, ta, &axis) && coaxial_with(b, tb, &axis)) {
        return None;
    }
    // Torus profiles are circles in (r,y) — representable, but the torus
    // operand is bailed before this point (`is_unsupported`); keep the rank
    // gate honest anyway.
    let (ca, cta, cb, ctb) = if profile_rank(a) <= profile_rank(b) {
        (a, ta, b, tb)
    } else {
        (b, tb, a, ta)
    };
    let pieces = profile_pieces(ca, cta, overlap)?;
    // Pieces evaluate in A's local (r, y) profile plane; lift the φ=0
    // meridian point to world for the implicit probe and the root itself.
    let to_world = |q: [f32; 2]| local_to_world(cta, [q[0], q[1], 0.0]);

    // Walk every piece of A's profile, find sign changes of B's implicit and
    // bisect them down. Each root is one circle about the shared axis.
    let mut roots: Vec<[f32; 3]> = Vec::new();
    for piece in &pieces {
        let f = |s: f32| implicit_world(cb, ctb, to_world((piece.eval)(s)));
        let n = piece.samples;
        let mut prev_s = 0.0f32;
        let mut prev_v = f(0.0);
        for i in 1..=n {
            let s = i as f32 / n as f32;
            let v = f(s);
            if prev_v == 0.0 {
                roots.push(to_world((piece.eval)(prev_s)));
            } else if prev_v * v < 0.0 {
                let (mut lo, mut hi, mut flo) = (prev_s, s, prev_v);
                for _ in 0..BISECT_STEPS {
                    let mid = 0.5 * (lo + hi);
                    let fm = f(mid);
                    if flo * fm <= 0.0 {
                        hi = mid;
                    } else {
                        lo = mid;
                        flo = fm;
                    }
                }
                roots.push(to_world((piece.eval)(0.5 * (lo + hi))));
            }
            prev_s = s;
            prev_v = v;
        }
    }

    // Dedup roots: same circle found from two adjacent pieces (e.g. a corner
    // shared by a segment end and the extension ray) collapses to one.
    let mut kept: Vec<[f32; 3]> = Vec::new();
    'outer: for r in roots {
        for k in &kept {
            // Compare in (radius, height) — azimuth is irrelevant for circles.
            let (ra, ha) = (dist_to_axis(r, &axis), vdot(vsub(r, axis.origin), axis.dir));
            let (rb, hb) = (dist_to_axis(*k, &axis), vdot(vsub(*k, axis.origin), axis.dir));
            if (ra - rb).abs() < 1e-3 && (ha - hb).abs() < 1e-3 {
                continue 'outer;
            }
        }
        kept.push(r);
    }

    let mut comps: Vec<Vec<[f32; 3]>> = Vec::new();
    for w in kept {
        let along = vdot(vsub(w, axis.origin), axis.dir);
        let center = vadd(axis.origin, vscale(axis.dir, along));
        let radius = vlen(vsub(w, center));
        if radius < 1e-4 {
            continue; // pole touch — a point, not an edge circle
        }
        if let SsiResult::Components(mut cs) = seed_from_circle(center, axis.dir, radius, overlap)
        {
            comps.append(&mut cs);
        }
    }
    Some(SsiResult::Components(comps))
}

// ─── world-space helpers ─────────────────────────────────────────────────────

fn is_unsupported(h: &HalfSpace) -> bool {
    matches!(h, HalfSpace::Torus { .. })
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
    // Closest point to the origin on the intersection line of planes
    // `n_i·p = c_i`: `p0 = [c1(n2×dir) + c2(dir×n1)] / det`, with
    // `det = n1·(n2×dir) = |n1×n2|` for a UNIT `dir`. The division was
    // previously omitted — harmless when the normals are perpendicular
    // (`det = 1`: box edges, wall↔cap rims), but for OBLIQUE planes (e.g.
    // adjacent polygon-extrude walls meeting at a corner) it scaled the
    // point toward the origin, landing it off the line and outside the
    // clip box, so the corner seam edge was silently dropped (ticket ifog).
    let n2xd = vcross(n2, dir);
    let dxn1 = vcross(dir, n1);
    let num = vadd(vscale(n2xd, c1), vscale(dxn1, c2));
    let det = vdot(n1, n2xd);
    if det.abs() < 1e-9 {
        return num;
    }
    vscale(num, 1.0 / det)
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
    /// `RevolvedProfile` only: the profile pieces in continuous chain order;
    /// `v` walks them concatenated (piece `⌊v⌋` at local parameter `v−⌊v⌋`),
    /// `u` is the revolution angle θ.
    pieces: Option<Vec<ProfilePiece<'a>>>,
}

impl ParamA<'_> {
    /// Grid resolution for the scan over this parametrisation. The flat
    /// `SCAN_RES` is enough for the closed-form primitives; a revolved
    /// profile chain needs the v-axis to resolve EVERY piece (a ~40-piece
    /// adaptive spline at 64 rows is < 2 samples per segment), so it scales
    /// with the piece count, capped to keep the grid `O(100k)` samples.
    fn grid_res(&self) -> usize {
        match &self.pieces {
            Some(ps) => (8 * ps.len()).clamp(SCAN_RES, 384),
            None => SCAN_RES,
        }
    }

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
            HalfSpace::RevolvedProfile { .. } => {
                // u = θ around local Y, v = concatenated piece parameter.
                let pieces = self.pieces.as_ref().expect("revolved param carries pieces");
                let k = (v.floor().max(0.0) as usize).min(pieces.len() - 1);
                let s = (v - k as f32).clamp(0.0, 1.0);
                let q = (pieces[k].eval)(s);
                [q[0] * u.cos(), q[1], q[0] * u.sin()]
            }
            HalfSpace::Torus { .. } | HalfSpace::Gyroid { .. } => {
                // Torus is bailed in `ssi_seeds`; a gyroid can be the IMPLICIT
                // operand but never the parametrised side (`build_param` → None).
                unreachable!("never parametrised")
            }
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
            // Worst SUPPORTED rank: a revolved profile is parametrised only
            // when the other operand cannot be (revolved × revolved) — its
            // implicit (`sdf_halfspace`) is cheap, its parametrisation isn't.
            HalfSpace::RevolvedProfile { .. } => 4,
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
                pieces: None,
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
                pieces: None,
            })
        }
        HalfSpace::Sphere { .. } => Some(ParamA {
            h,
            t,
            u_range: (0.0, std::f32::consts::TAU),
            // Full polar sweep [0,π]; θ periodic.
            v_range: (0.0, std::f32::consts::PI),
            u_periodic: true,
            pieces: None,
        }),
        HalfSpace::RevolvedProfile { .. } => {
            // v concatenates the profile pieces (continuous chain order, one
            // unit each — adaptive lowering makes segments roughly equal-
            // curvature, so uniform-per-piece is well-matched); θ periodic.
            let pieces = profile_pieces(h, t, overlap)?;
            if pieces.is_empty() {
                return None;
            }
            let v_max = pieces.len() as f32;
            Some(ParamA {
                h,
                t,
                u_range: (0.0, std::f32::consts::TAU),
                v_range: (0.0, v_max),
                u_periodic: true,
                pieces: Some(pieces),
            })
        }
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
                pieces: None,
            })
        }
        HalfSpace::Torus { .. } | HalfSpace::Gyroid { .. } => None,
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
/// Expand an AABB by an absolute margin on every axis.
fn expand_aabb(b: &Aabb, m: f32) -> Aabb {
    Aabb {
        min: [b.min[0] - m, b.min[1] - m, b.min[2] - m],
        max: [b.max[0] + m, b.max[1] + m, b.max[2] + m],
    }
}

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

/// Largest scale component of a transform (world stretch upper bound).
fn tb_max_scale(t: &Transform) -> f32 {
    t.scale[0].abs().max(t.scale[1].abs()).max(t.scale[2].abs()).max(1e-6)
}

/// Approximate world-space (mm) extents of A's `(u,v)` domain, measured by
/// metric probes at the domain centre (u/v can be angles or piece parameters,
/// so the param-space spans alone say nothing about arc length).
fn domain_extents_mm(param: &ParamA) -> (f32, f32) {
    let (u0, u1) = param.u_range;
    let (v0, v1) = param.v_range;
    let (uc, vc) = (0.5 * (u0 + u1), 0.5 * (v0 + v1));
    let (du, dv) = ((u1 - u0).max(1e-9), (v1 - v0).max(1e-9));
    let (eu, ev) = (du * 1e-3, dv * 1e-3);
    let c = param.point(uc, vc);
    let pu = param.point(uc + eu, vc);
    let pv = param.point(uc, vc + ev);
    let ext_u = vlen(vsub(pu, c)) / eu * du;
    let ext_v = vlen(vsub(pv, c)) / ev * dv;
    (ext_u, ext_v)
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

    // Base resolution from A's parametrisation; when the IMPLICIT operand is
    // a revolved profile chain its zero set can wiggle at the profile's
    // segment scale across A's domain, so the grid scales with that segment
    // count too (capped — the implicit prunes, so this stays cheap).
    let mut n = param.grid_res();
    if let HalfSpace::RevolvedProfile { segments, .. } = other {
        n = n.max((4 * segments.len()).clamp(SCAN_RES, 256));
    }
    if let HalfSpace::Gyroid { cell_size, thickness } = other {
        // A gyroid zero set wiggles at the LATTICE scale and its shell forms
        // ribbons on A whose narrowest transversal width is bounded below by
        // 2·t·s·0.2887 (the shell implicit has |∇| ≤ 1 after normalisation, so
        // the ± dip can never be thinner). Sample finer than both scales or
        // whole ribbons slip between grid rows: spacing ≤ min(period/8,
        // half the minimum ribbon width). Domain extents are measured in mm
        // via a metric probe (u/v may be angles, e.g. cylinder θ). Capped —
        // beyond the cap thin shells on huge overlaps may miss ribbons; the
        // cap keeps the grid ≤ ~260k samples (sub-ms native, few ms wasm).
        let scale = tb_max_scale(ot);
        let period = std::f32::consts::TAU * cell_size * scale;
        let ribbon_half = (thickness * cell_size * 0.2887 * scale).max(1e-4);
        let h = (period / 8.0).min(ribbon_half);
        let (ext_u, ext_v) = domain_extents_mm(&param);
        let needed = (ext_u.max(ext_v) / h).ceil() as usize;
        n = n.max(needed.clamp(SCAN_RES, 512));
    }
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
    // The CORNER mask is additionally expanded by one grid-cell world
    // diagonal (ticket 7sp1): when the parametrised surface is much larger
    // than the overlap (a giant cylinder wall × a small box side face), a
    // crossing lying ON the overlap wall — exactly where the other surface
    // IS that wall — sits in a cell whose outer corner falls beyond the 5%
    // pad, and the both-endpoints-in-box rule then kills the only cells
    // carrying the sign change: the pair silently seeds nothing. Widening
    // ONLY the mask is safe ("never miss a real edge"): every refined seed
    // still passes the tight `mbox` gate below, so nothing outside the
    // overlap is ever emitted.
    let (ext_u, ext_v) = domain_extents_mm(&param);
    let cell_diag =
        ((ext_u / n as f32).powi(2) + (ext_v / n as f32).powi(2)).sqrt();
    let mask_box = expand_aabb(&mbox, cell_diag);

    // Sample grid: world point, in-box mask, and signed value g.
    let mut g = vec![0.0f32; n * n];
    let mut inbox = vec![false; n * n];
    for j in 0..n {
        for i in 0..n {
            let u = u0 + du * i as f32;
            let v = v0 + dv * j as f32;
            let p = param.point(u, v);
            inbox[j * n + i] = in_box(&mask_box, p);
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

    // ── gyroid as the implicit operand (spike: SSI feasibility for TPMS) ──

    /// Looser on-surface tolerance for gyroid seeds: the shell implicit is a
    /// normalised (non-Euclidean) field, so a bisected root carries a slightly
    /// larger residual than the exact closed-form surfaces.
    const ON_GYROID: f32 = 1e-3;

    fn assert_gyroid_seeds(
        comps: &[Vec<[f32; 3]>],
        a: &HalfSpace,
        ta: &Transform,
        gy: &HalfSpace,
        tg: &Transform,
        overlap: &Aabb,
    ) -> (usize, usize) {
        let mut total = 0usize;
        for comp in comps {
            assert!(!comp.is_empty(), "a component must carry at least one seed");
            for &s in comp {
                let da = implicit_world(a, ta, s).abs();
                let dg = implicit_world(gy, tg, s).abs();
                assert!(da < ON_SURF, "seed {s:?} not on A: |implicit_A|={da}");
                assert!(dg < ON_GYROID, "seed {s:?} not on gyroid: |implicit_G|={dg}");
                assert!(in_box(&pad_aabb(overlap), s), "seed {s:?} escaped the box");
                total += 1;
            }
        }
        (comps.len(), total)
    }

    /// A box face plane cutting a gyroid shell — the user-visible case (every
    /// edge of a gyroid-CSG body is a Boolean rim like this one). The zero set
    /// on the face is a family of closed ribbon-boundary curves; the scan must
    /// find a plausible number of components and every seed must lie on BOTH
    /// surfaces.
    #[test]
    fn plane_gyroid_box_face_components() {
        let gy = HalfSpace::Gyroid { cell_size: 4.0, thickness: 0.3 };
        let pl = HalfSpace::Plane { normal: [0.0, 1.0, 0.0], offset: 0.0 };
        let id = Transform::IDENTITY;
        // A 40×40 face patch: ~1.6 lattice periods (2π·4 ≈ 25.1) per axis.
        let box_ = aabb([-20.0, -0.5, -20.0], [20.0, 0.5, 20.0]);
        let r = ssi_seeds(&pl, &id, &gy, &id, &box_);
        let SsiResult::Components(v) = &r else {
            panic!("plane∩gyroid must be supported now");
        };
        let (comps, seeds) = assert_gyroid_seeds(v, &pl, &id, &gy, &id, &box_);
        println!("plane∩gyroid 40×40 (s=4, t=0.3): {comps} components, {seeds} seeds");
        assert!(
            (2..=80).contains(&comps),
            "component count out of plausible range: {comps}"
        );
    }

    /// Same face, THIN shell (t=0.1 → min ribbon width ≈ 0.23mm): the
    /// resolution scaling must keep catching the ± dips of the shell field.
    #[test]
    fn plane_gyroid_thin_shell_still_found() {
        let gy = HalfSpace::Gyroid { cell_size: 4.0, thickness: 0.1 };
        let pl = HalfSpace::Plane { normal: [0.0, 1.0, 0.0], offset: 0.0 };
        let id = Transform::IDENTITY;
        let box_ = aabb([-20.0, -0.5, -20.0], [20.0, 0.5, 20.0]);
        let r = ssi_seeds(&pl, &id, &gy, &id, &box_);
        let SsiResult::Components(v) = &r else {
            panic!("plane∩gyroid must be supported");
        };
        let (comps, seeds) = assert_gyroid_seeds(v, &pl, &id, &gy, &id, &box_);
        println!("plane∩gyroid thin (s=4, t=0.1): {comps} components, {seeds} seeds");
        assert!(comps >= 2, "thin shell ribbons were missed entirely");
    }

    /// Gyroid under a parametrised CYLINDER — the curved-carrier case (a hole
    /// drilled through a gyroid lattice).
    #[test]
    fn cylinder_gyroid_components() {
        let gy = HalfSpace::Gyroid { cell_size: 4.0, thickness: 0.3 };
        let cyl = HalfSpace::InfiniteCylinder { radius: 6.0 };
        let id = Transform::IDENTITY;
        let box_ = aabb([-8.0, -8.0, -8.0], [8.0, 8.0, 8.0]);
        let r = ssi_seeds(&cyl, &id, &gy, &id, &box_);
        let SsiResult::Components(v) = &r else {
            panic!("cylinder∩gyroid must be supported");
        };
        let (comps, seeds) = assert_gyroid_seeds(v, &cyl, &id, &gy, &id, &box_);
        println!("cylinder∩gyroid (r=6, s=4, t=0.3): {comps} components, {seeds} seeds");
        assert!(comps >= 1, "no rim curves found on the cylinder wall");
    }

    /// The user's scenario end-to-end: all 6 face planes of a 40mm box cutting
    /// one gyroid lattice — the complete Boolean rim set of Box∩Gyroid. Prints
    /// the per-face survey (the spike's feasibility data) and asserts every
    /// face finds its rims.
    #[test]
    fn box_gyroid_full_survey() {
        let gy = HalfSpace::Gyroid { cell_size: 4.0, thickness: 0.3 };
        let id = Transform::IDENTITY;
        let hx = 20.0f32;
        let faces: [([f32; 3], f32); 6] = [
            ([1.0, 0.0, 0.0], hx),
            ([-1.0, 0.0, 0.0], hx),
            ([0.0, 1.0, 0.0], hx),
            ([0.0, -1.0, 0.0], hx),
            ([0.0, 0.0, 1.0], hx),
            ([0.0, 0.0, -1.0], hx),
        ];
        let mut grand_comps = 0usize;
        let mut grand_seeds = 0usize;
        for (i, (normal, offset)) in faces.iter().enumerate() {
            let pl = HalfSpace::Plane { normal: *normal, offset: *offset };
            // Overlap: the face slab, ±0.5 across the face.
            let mut min = [-hx, -hx, -hx];
            let mut max = [hx, hx, hx];
            for k in 0..3 {
                if normal[k] > 0.5 {
                    min[k] = hx - 0.5;
                } else if normal[k] < -0.5 {
                    max[k] = -hx + 0.5;
                }
            }
            let box_ = aabb(min, max);
            let r = ssi_seeds(&pl, &id, &gy, &id, &box_);
            let SsiResult::Components(v) = &r else {
                panic!("face {i}: plane∩gyroid must be supported");
            };
            let (comps, seeds) = assert_gyroid_seeds(v, &pl, &id, &gy, &id, &box_);
            println!("box face {i}: {comps} components, {seeds} seeds");
            assert!(comps >= 1, "face {i} found no rim curves");
            grand_comps += comps;
            grand_seeds += seeds;
        }
        println!("box∩gyroid TOTAL: {grand_comps} components, {grand_seeds} seeds");
    }

    /// Two gyroids cannot be parametrised against each other — still bails.
    #[test]
    fn gyroid_gyroid_stays_unsupported() {
        let gy = HalfSpace::Gyroid { cell_size: 4.0, thickness: 0.3 };
        let id = Transform::IDENTITY;
        let box_ = aabb([-5.0, -5.0, -5.0], [5.0, 5.0, 5.0]);
        assert_eq!(
            ssi_seeds(&gy, &id, &gy, &id, &box_),
            SsiResult::Unsupported
        );
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

    /// A cylinder-of-revolution as a `RevolvedProfile` carrier: one vertical
    /// line segment at r = `radius`, y ∈ [−h, h], outward normal +r (ccw).
    fn revolved_cylinder(radius: f32, h: f32) -> HalfSpace {
        HalfSpace::RevolvedProfile {
            segments: vec![fielddraft_protocol::sketch2d::SketchSegment::Line {
                start: [radius, -h],
                end: [radius, h],
            }],
            ccw: true,
            offset: 0.0,
            extend_start: false,
            extend_end: false,
        }
    }

    #[test]
    fn revolved_profile_meets_non_coaxial_plane_two_lines() {
        // The cutaway case: a vertical plane (normal ⊥ revolve axis) cutting
        // through a revolved wall. Mirror of
        // `plane_parallel_cylinder_axis_offset_two_lines` with the wall
        // expressed as a RevolvedProfile — previously bailed `Unsupported`.
        let wall = revolved_cylinder(1.0, 1.8);
        let pl = HalfSpace::Plane { normal: [1.0, 0.0, 0.0], offset: 0.5 };
        let id = Transform::IDENTITY;
        let box_ = aabb([-1.5, -1.5, -1.5], [1.5, 1.5, 1.5]);
        let r = ssi_seeds(&wall, &id, &pl, &id, &box_);
        assert_eq!(count(&r), 2, "plane through revolved wall = 2 lines");
        if let SsiResult::Components(v) = &r {
            assert_seeds_on_both(v, &wall, &id, &pl, &id);
        }
    }

    #[test]
    fn revolved_profile_meets_tilted_plane_one_ellipse() {
        // 45°-tilted plane (normal (1,1,0)/√2) through the revolved wall:
        // NOT coaxial, NOT axis-parallel → one ellipse. Exercises the scan
        // with the profile as the IMPLICIT operand under a plane param.
        let wall = revolved_cylinder(1.0, 1.8);
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let pl = HalfSpace::Plane { normal: [s, s, 0.0], offset: 0.0 };
        let id = Transform::IDENTITY;
        let box_ = aabb([-1.6, -1.6, -1.6], [1.6, 1.6, 1.6]);
        let r = ssi_seeds(&wall, &id, &pl, &id, &box_);
        assert_eq!(count(&r), 1, "tilted plane through revolved wall = 1 ellipse");
        if let SsiResult::Components(v) = &r {
            assert_seeds_on_both(v, &wall, &id, &pl, &id);
        }
    }

    #[test]
    fn two_revolved_profiles_perpendicular_axes_two_components() {
        // Revolved×revolved, non-coaxial: a thin revolved wall (r=0.5) punches
        // through a fat one (r=1) at perpendicular axes → 2 closed seam
        // curves. Exercises the RevolvedProfile PARAMETRISATION arm (both
        // operands are profiles, so one must become surface A).
        let fat = revolved_cylinder(1.0, 1.8);
        let thin = revolved_cylinder(0.5, 1.8);
        let id = Transform::IDENTITY;
        // Rotate +90° about Z maps local Y (revolve axis) to world X.
        let rot_z90 = Transform {
            translate: [0.0, 0.0, 0.0],
            rotation: [
                0.0,
                0.0,
                (std::f32::consts::FRAC_PI_4).sin(),
                (std::f32::consts::FRAC_PI_4).cos(),
            ],
            scale: [1.0, 1.0, 1.0],
        };
        let box_ = aabb([-1.5, -1.5, -1.5], [1.5, 1.5, 1.5]);
        let r = ssi_seeds(&fat, &id, &thin, &rot_z90, &box_);
        assert_eq!(count(&r), 2, "thin revolved wall through fat one = 2 seams");
        if let SsiResult::Components(v) = &r {
            assert_seeds_on_both(v, &fat, &id, &thin, &rot_z90);
        }
    }

    /// Ticket 7sp1: a GIANT cylinder wall meeting a small box's side face —
    /// the intersection line lies exactly ON the overlap wall (the box face IS
    /// the other operand), and the whole in-box arc spans only ~2 cells of the
    /// θ-grid. Without the cell-diagonal mask expansion the crossing cells had
    /// an out-of-box corner and the pair silently seeded NOTHING (two missing
    /// vertical edges in the app).
    #[test]
    fn giant_cylinder_small_box_side_face_seeds() {
        let cyl = HalfSpace::InfiniteCylinder { radius: 30.0 };
        let txp = Transform {
            translate: [7.970110677489131, 22.314295, 29.869688],
            ..Transform::IDENTITY
        };
        let id = Transform::IDENTITY;
        let overlap = aabb([4.970111, 18.314295, 26.870415], [10.970111, 26.314295, 30.0]);
        for (normal, expect_z) in [([-1.0f32, 0.0, 0.0], 29.585), ([1.0, 0.0, 0.0], 27.93)] {
            let pl = HalfSpace::Plane { normal, offset: 3.0 };
            let SsiResult::Components(v) = ssi_seeds(&cyl, &id, &pl, &txp, &overlap) else {
                panic!("cylinder∩plane must be supported");
            };
            assert_eq!(v.len(), 1, "one seam line for face normal {normal:?}");
            for s in &v[0] {
                let dc = implicit_world(&cyl, &id, *s).abs();
                let dp = implicit_world(&pl, &txp, *s).abs();
                assert!(dc < 1e-3 && dp < 1e-3, "seed {s:?} off-surface: {dc}/{dp}");
                assert!((s[2] - expect_z).abs() < 0.01, "seed {s:?} not on the seam line");
            }
        }
    }

}
