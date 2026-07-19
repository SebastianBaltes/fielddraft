//! Constructive seams for TANGENTIAL wall junctions.
//!
//! A sketch boundary group becomes one B-Rep face, and a face carries one
//! surface. Where a straight segment meets its fillet arc, a PLANE meets a
//! CYLINDER — two faces — but they meet *smoothly*: the seam between them is
//! a tangential contact, not a transversal crossing.
//!
//! The seed weaver cannot find such a seam, and no amount of sampling would
//! help:
//!
//! * **SSI misses it.** Both closed-form paths ([`ssi::coaxial_revolution`]
//!   and the parameter scan) locate the intersection as a SIGN CHANGE of one
//!   surface's field on the other. A tangential contact is a *double root* —
//!   the field touches zero and comes back — so there is no sign change to
//!   find.
//! * **Tracing could not follow it either.** The edge tracer walks along
//!   `∇f_a × ∇f_b`. At a tangency the two gradients are PARALLEL, so the
//!   cross product vanishes: the direction is undefined and the Newton
//!   projection is singular.
//! * **The witness gate rejects its corners on purpose.** A triple point is
//!   only a point when the three normals span 3-space; at a tangential corner
//!   two of them are parallel, so the rank gate (correctly) refuses it as a
//!   phantom.
//!
//! The weaver is built for transversal intersections, and that is not a
//! defect — it is what makes it robust. So we do not teach it to *search* for
//! tangential seams; we HAND it the ones we already know exactly.
//!
//! And we do know them exactly: a tangential seam is the swept image of the
//! 2D breakpoint where the profile changes carrier. For an extrude that is a
//! straight line through the slab, for a revolve a circle about the axis. The
//! corners (where the seam meets a cap) come out of the same construction.
//!
//! [`ssi::coaxial_revolution`]: crate::brep::ssi

use fielddraft_protocol::sketch2d::{
    tangential_group_breaks, Sketch2dPrimitive, PATH_G1_COS_THRESHOLD,
};
use fielddraft_protocol::Transform;

use crate::brep::HermiteSample;
use crate::bvh::BoundedCsg;
use crate::csg::{map_wall_sub, BranchId};
use crate::sketch2d::BoundedSketch2d;

/// How densely a constructive seam is sampled before the caller trims it to
/// the body. The samples only have to bracket every active↔inactive
/// transition; the caller bisects each bracket to the exact boundary, so this
/// is a topology budget, not an accuracy one.
const SEAM_SAMPLES: usize = 96;

/// One exactly-known seam between two tangential wall carriers.
pub(crate) struct TangentialSeam {
    pub a: BranchId,
    pub b: BranchId,
    /// The seam curve, densely sampled with exact tangents.
    pub samples: Vec<HermiteSample>,
    /// A revolve's seam is a closed circle; an extrude's is an open segment.
    pub closed: bool,
    /// The seam's two ends (open seams only): the corners where it runs into
    /// a cap. Handing these to the weaver as witnesses is what lets the CAP
    /// RIM marches stop exactly here — their own exit is grazing (the plane
    /// leaves the body tangentially), so activity bisection alone would put
    /// the corner off by `√(2·r·eps)`, which is millimetres, not microns.
    pub corners: [Option<SeamCorner>; 2],
}

/// A seam end: the exact point plus the three branches that concur there.
#[derive(Clone)]
pub(crate) struct SeamCorner {
    pub pos: [f64; 3],
    pub branches: [BranchId; 3],
}

/// Every tangential seam of `body`, walked over its sweep leaves (including
/// the inner walls a Shell adds). Bodies with no tangent carrier change
/// return an empty vec — the overwhelmingly common case, and free.
pub(crate) fn collect(body: &BoundedCsg) -> Vec<TangentialSeam> {
    let mut out = Vec::new();
    walk(body, None, &mut out);
    out
}

/// `shell` carries the wall thickness of an enclosing Shell, if any: the same
/// breakpoints then produce a SECOND seam on the inward-offset carriers.
fn walk(node: &BoundedCsg, shell: Option<f32>, out: &mut Vec<TangentialSeam>) {
    match node {
        BoundedCsg::Extrude { id, sketch, half_height, world_transform, wall_subs, .. } => {
            let ws = wall_subs.as_deref().map(|v| v.as_slice());
            let mut breaks = Vec::new();
            collect_breaks(sketch, 0, &mut breaks);
            for br in &breaks {
                extrude_seam(id, br, ws, *half_height, world_transform, None, out);
                if let Some(t) = shell {
                    extrude_seam(id, br, ws, *half_height, world_transform, Some(t), out);
                }
            }
        }
        BoundedCsg::Revolve { id, sketch, world_transform, wall_subs, .. } => {
            let ws = wall_subs.as_deref().map(|v| v.as_slice());
            let mut breaks = Vec::new();
            collect_breaks(sketch, 0, &mut breaks);
            for br in &breaks {
                revolve_seam(id, br, ws, world_transform, None, out);
                if let Some(t) = shell {
                    revolve_seam(id, br, ws, world_transform, Some(t), out);
                }
            }
        }
        BoundedCsg::Shell { inner, thickness, .. } => walk(inner, Some(*thickness), out),
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::SmoothUnion { left, right, .. }
        | BoundedCsg::SmoothDifference { left, right, .. }
        | BoundedCsg::MaskedBlend { left, right, .. }
        | BoundedCsg::Morph { left, right, .. } => {
            walk(left, shell, out);
            walk(right, shell, out);
        }
        BoundedCsg::Twist { inner, .. }
        | BoundedCsg::Bend { inner, .. }
        | BoundedCsg::Displace { inner, .. }
        | BoundedCsg::Offset { inner, .. }
        // #384 D2: the eroded copy is already at its offset position, so pass the
        // ambient `shell` through unchanged (do NOT re-offset by a thickness).
        | BoundedCsg::InnerWall { inner, .. }
        | BoundedCsg::ArcChannel { inner, .. } => walk(inner, shell, out),
        BoundedCsg::Empty
        | BoundedCsg::Leaf { .. }
        | BoundedCsg::Helix { .. }
        | BoundedCsg::Sweep { .. } => {}
    }
}

/// One tangential breakpoint of a profile: the two boundary groups it
/// separates, the 2D point, and the OUTWARD normal there (which is the shared
/// normal of both carriers — a shell's inner seam sits `thickness` along it).
struct Break {
    group_a: u32,
    group_b: u32,
    p: [f32; 2],
    normal_out: [f32; 2],
}

/// Walk the sketch tree with the same `edge_count` base offsets the boundary
/// numbering uses, collecting every tangential group break.
fn collect_breaks(sketch: &BoundedSketch2d, base: u32, out: &mut Vec<Break>) {
    match sketch {
        BoundedSketch2d::Prim { primitive, .. } => {
            let Sketch2dPrimitive::Path { segments, closed: true } = primitive else {
                // Only a Path can change carrier mid-boundary. A Rectangle's
                // or Polygon's corners are creases; a Circle/Sector is one
                // carrier — all seed transversally.
                return;
            };
            let ccw = crate::brep::weaving::sketch_surfaces::path_chord_signed_area(segments) > 0.0;
            for (i, ga, gb, p) in tangential_group_breaks(segments, PATH_G1_COS_THRESHOLD) {
                // The shared tangent at the joint; both carriers touch it, so
                // its perpendicular is the common normal.
                let t = segments[i].tangent_at_start();
                let len = (t[0] * t[0] + t[1] * t[1]).sqrt();
                if len < 1e-9 {
                    continue;
                }
                let (tx, ty) = (t[0] / len, t[1] / len);
                // Same convention as `collect_prim_walls`: CCW ⇒ the outward
                // normal is the tangent turned −90°.
                let normal_out = if ccw { [ty, -tx] } else { [-ty, tx] };
                out.push(Break {
                    group_a: base + ga,
                    group_b: base + gb,
                    p,
                    normal_out,
                });
            }
        }
        BoundedSketch2d::Union { left, right, .. }
        | BoundedSketch2d::Difference { left, right, .. }
        | BoundedSketch2d::Intersection { left, right, .. } => {
            collect_breaks(left, base, out);
            collect_breaks(right, base + left.edge_count(), out);
        }
        // An Offset sketch has no analytic walls at all (DL-50) — it never
        // reaches the seed route.
        BoundedSketch2d::Empty | BoundedSketch2d::Offset { .. } => {}
    }
}

/// The seam of one extrude breakpoint: a straight line through the slab at
/// the (possibly shell-offset) 2D point.
fn extrude_seam(
    id: &fielddraft_protocol::FeatureId,
    br: &Break,
    ws: Option<&[u32]>,
    half_height: f32,
    xf: &Transform,
    shell: Option<f32>,
    out: &mut Vec<TangentialSeam>,
) {
    let (a, b) = seam_branches(id, br, ws, shell.is_some());
    let p = offset_point(br, shell);
    // Over-extend by nothing: the caps ARE the ends, and their corners are the
    // witnesses below. Everything beyond gets trimmed away anyway.
    let lo = local_to_world(xf, [p[0], p[1], -half_height]);
    let hi = local_to_world(xf, [p[0], p[1], half_height]);
    let dir = unit(sub(hi, lo));
    let samples: Vec<HermiteSample> = (0..SEAM_SAMPLES)
        .map(|k| {
            let t = k as f64 / (SEAM_SAMPLES - 1) as f64;
            HermiteSample {
                point: [
                    lo[0] + t * (hi[0] - lo[0]),
                    lo[1] + t * (hi[1] - lo[1]),
                    lo[2] + t * (hi[2] - lo[2]),
                ],
                tangent: dir,
            }
        })
        .collect();

    // The corners: where the seam runs into the two caps. A shelled body's
    // inner seam runs into the INNER caps (same reserved sub, inner flag).
    let cap = |top: bool| {
        let sub = if top {
            BranchId::EXTRUDE_CAP_TOP_SUB
        } else {
            BranchId::EXTRUDE_CAP_BOTTOM_SUB
        };
        let c = BranchId::from_feature_sub(id, sub);
        if shell.is_some() { c.inner_wall() } else { c }
    };
    out.push(TangentialSeam {
        a,
        b,
        samples,
        closed: false,
        corners: [
            Some(SeamCorner { pos: lo, branches: [a, b, cap(false)] }),
            Some(SeamCorner { pos: hi, branches: [a, b, cap(true)] }),
        ],
    });
}

/// The seam of one revolve breakpoint: a circle about the local Y axis, at the
/// breakpoint's radius and height. A breakpoint ON the axis sweeps to a POINT,
/// not a circle — there is no seam to emit.
fn revolve_seam(
    id: &fielddraft_protocol::FeatureId,
    br: &Break,
    ws: Option<&[u32]>,
    xf: &Transform,
    shell: Option<f32>,
    out: &mut Vec<TangentialSeam>,
) {
    let (a, b) = seam_branches(id, br, ws, shell.is_some());
    let p = offset_point(br, shell);
    let (r, y) = (p[0] as f64, p[1] as f64);
    if r.abs() < 1e-6 {
        return;
    }
    let samples: Vec<HermiteSample> = (0..SEAM_SAMPLES)
        .map(|k| {
            let th = std::f64::consts::TAU * k as f64 / SEAM_SAMPLES as f64;
            let (s, c) = th.sin_cos();
            HermiteSample {
                point: local_to_world_f64(xf, [r * c, y, r * s]),
                // d/dθ of (r·cosθ, y, r·sinθ), rotated into world.
                tangent: unit(rotate_f64(xf, [-s, 0.0, c])),
            }
        })
        .collect();
    out.push(TangentialSeam { a, b, samples, closed: true, corners: [None, None] });
}

/// The two branches a seam separates — the stable wall subs of its groups,
/// lifted to the inner wall when the seam belongs to a shell's hollow.
fn seam_branches(
    id: &fielddraft_protocol::FeatureId,
    br: &Break,
    ws: Option<&[u32]>,
    inner: bool,
) -> (BranchId, BranchId) {
    let mk = |g: u32| {
        let b = BranchId::from_feature_sub(id, map_wall_sub(ws, g));
        if inner { b.inner_wall() } else { b }
    };
    (mk(br.group_a), mk(br.group_b))
}

/// The breakpoint, moved inward by the wall thickness for a shell's hollow.
/// Both carriers offset by the same amount along their SHARED normal, so they
/// stay tangent and their contact point simply travels along that normal.
fn offset_point(br: &Break, shell: Option<f32>) -> [f32; 2] {
    match shell {
        None => br.p,
        Some(t) => [
            br.p[0] - t * br.normal_out[0],
            br.p[1] - t * br.normal_out[1],
        ],
    }
}

// ─── small vector / transform helpers (f64 world space) ──────────────────────

fn local_to_world(t: &Transform, p: [f32; 3]) -> [f64; 3] {
    local_to_world_f64(t, [p[0] as f64, p[1] as f64, p[2] as f64])
}

fn local_to_world_f64(t: &Transform, p: [f64; 3]) -> [f64; 3] {
    let s = t.scale;
    let scaled = [p[0] * s[0] as f64, p[1] * s[1] as f64, p[2] * s[2] as f64];
    let r = rotate_f64(t, scaled);
    [
        r[0] + t.translate[0] as f64,
        r[1] + t.translate[1] as f64,
        r[2] + t.translate[2] as f64,
    ]
}

fn rotate_f64(t: &Transform, v: [f64; 3]) -> [f64; 3] {
    let r = crate::quat_rotate(t.rotation, [v[0] as f32, v[1] as f32, v[2] as f32]);
    [r[0] as f64, r[1] as f64, r[2] as f64]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < 1e-12 {
        [0.0, 0.0, 1.0]
    } else {
        [v[0] / l, v[1] / l, v[2] / l]
    }
}
