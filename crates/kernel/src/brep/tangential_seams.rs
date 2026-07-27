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

use std::collections::HashMap;

use fielddraft_protocol::sketch2d::{
    tangential_group_breaks, Sketch2dPrimitive, PATH_G1_COS_THRESHOLD,
};
use fielddraft_protocol::{HalfSpace, Transform};

use crate::brep::{BranchSurfaceMap, HermiteSample};
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
        | BoundedCsg::Sweep { .. }
        // #385 P3: no-op stays correct. A tangential seam is where an ANALYTIC
        // curved WALL grazes a cap; the loft's caps are analytic planes but its
        // side wall is non-analytic (the morph mean has no analytic carrier), so
        // there is no analytic wall↔cap tangency to witness. The cap rims are
        // ordinary (transverse) rim edges, handled by the octree route.
        | BoundedCsg::Loft { .. } => {}
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

// ─── extrude wall × revolve wall: the SAME profile swept twice (#435) ────────

/// Branch pairs whose two carriers TOUCH tangentially instead of crossing,
/// because one feature EXTRUDED a profile and another REVOLVED it.
///
/// # Why this configuration exists at all
///
/// Sweep a profile curve `c` twice from the same plane `Π`:
///
/// * The **extrude** wall over `c` is `c × ℝe`, where `e ⊥ Π` is the extrusion
///   direction. Its tangent plane at `Q ∈ c` is `span(ċ, e)`.
/// * The **revolve** wall over `c` about an axis `a ⊂ Π` is the surface of
///   revolution. Its tangent plane at `Q ∈ c` is spanned by `ċ` and the
///   ROTATION tangent `â × (Q − foot)` — and at a point OF the profile plane
///   that rotation tangent is perpendicular to `Π`, i.e. it *is* `e`. So the
///   revolve's tangent plane at `Q` is `span(ċ, e)` too.
///
/// The two tangent planes are identical along the whole of `c`: **the extruded
/// wall IS the tangent plane of the revolved wall along their shared profile
/// curve.** This is an identity, not a coincidence — it holds for every
/// segment shape (a straight segment gives plane × cone, an arc gives cylinder
/// × torus) and it needs no more than `e ⊥ Π_revolve`, which is exactly what
/// "the axis lies in the sketch plane" amounts to. Note it does NOT require
/// the two sketch planes to be at the same offset: only their NORMALS must
/// agree, because the extrude wall is invariant along `e`.
///
/// A CAD document hits this whenever it extrudes and revolves one profile —
/// a common pattern, not a quirk of one file.
///
/// # Why the pair must leave the seed set
///
/// The intersection of a tangent plane with a cone is a DOUBLE line, not a
/// transversal curve. SSI still returns roots (under f32 noise the double root
/// splits into two mirror-image branches a few tens of nanometres apart), and
/// the tracer then chases arcs that are orders of magnitude shorter than the
/// route's own resolution floor (`1e-4 · diag`), stalls, reports
/// `EdgeEnd::Open` and takes the WHOLE body to the octree — which resolves
/// them no better. Handing the pair to the weaver as "not a transversal
/// candidate" is the same treatment, and for the same reason, that
/// [`crate::brep::coincidence`] already gives coincident carriers.
///
/// # Why NO edge is the right topology
///
/// The two carriers share a tangent plane along `c`, so their outward normals
/// there are parallel — and, coming from the same profile side, EQUAL. Hence
/// near `c` one operand's surface lies strictly inside the other's half-space
/// (a cone/cylinder is on one side of its tangent plane):
///
/// * `A ∪ B` and `A ∩ B`: exactly ONE of the two carriers bounds the result
///   near `c`; the other has no face there, so an edge between them would
///   bound nothing on one side.
/// * `A ∖ B`: the remainder is the lens between two tangent surfaces — either
///   empty, or a cusp sliver whose thickness goes to ZERO at `c`. An edge
///   there would bound a region of zero volume and make the shell
///   non-manifold.
///
/// Either way both faces keep closed loops made of their TRANSVERSAL edges;
/// the tangential contact contributes zero length to either loop. Measured on
/// the #419 rail body, where the extrude wall and the cone are the two faces
/// in question: the FACE count is unchanged at 37, the edge count falls
/// 110 → 74, and every one of the 36 lost edges was octree noise on those two
/// faces (35 sub-micron NURBS loops, 0.02–0.07 mm across, plus one stub). The
/// shell's closure is unchanged as well — that body was already open before,
/// for reasons that have nothing to do with this contact. The counts are
/// pinned in `onshape_regression_tests::onshape_419_revolve_remove_stays_bounded`.
///
/// This is the exact opposite of the G1 seams above, and deliberately so:
/// there two faces of the SAME sweep meet smoothly and both genuinely exist on
/// either side of the seam, so the seam is real and is handed to the weaver.
///
/// # Scope
///
/// Straight profile segments only — those are the groups
/// [`super::weaving::sketch_surfaces::collect_revolve_halfspaces`] lifts to a
/// `Cone` / `InfiniteCylinder` / `Plane` carrier, which is where a closed-form
/// tangency identity exists. A CURVED group carries a
/// [`HalfSpace::RevolvedProfile`], for which there is no such identity; such a
/// pair still reaches SSI and behaves exactly as before this function existed.
pub(crate) fn sweep_contact_pairs(
    body: &BoundedCsg,
    map: &BranchSurfaceMap,
) -> std::collections::HashSet<(u32, u32)> {
    let mut out = std::collections::HashSet::new();
    let mut frames: Vec<SweepFrame> = Vec::new();
    collect_sweep_frames(body, &mut frames);
    let (extrudes, revolves): (Vec<_>, Vec<_>) = frames.iter().partition(|f| !f.is_revolve);
    if extrudes.is_empty() || revolves.is_empty() {
        return out; // no extrude/revolve pair in this body — the common case.
    }

    // Branches by owning feature, so the pair loop below never rescans the map.
    let mut by_feature: HashMap<u32, Vec<u32>> = HashMap::new();
    for &b in map.keys() {
        by_feature.entry(BranchId(b).feature_id()).or_default().push(b);
    }

    for ext in &extrudes {
        for rev in &revolves {
            // THE structural gate: the extrusion direction must be the normal
            // of the revolve's profile plane. Everything above follows from
            // it; without it the two walls simply cross.
            if (dot(ext.normal, rev.normal).abs() - 1.0).abs() > AXIS_TOL {
                continue;
            }
            let (Some(walls), Some(rings)) =
                (by_feature.get(&ext.feature), by_feature.get(&rev.feature))
            else {
                continue;
            };
            for &a in walls {
                // Only the extrude's WALLS take part. A cap plane is normal to
                // `e`, so it is parallel to every revolve axis here and would
                // pass the axis test for free — but a cap touching a revolve
                // wall is an accident of the extrude DEPTH, not the identity
                // above, so it stays an ordinary candidate pair.
                if is_cap(a) {
                    continue;
                }
                let Some((HalfSpace::Plane { normal, offset }, xa)) = map.get(&a) else {
                    continue;
                };
                let Some(plane) = world_plane(*normal, *offset, xa) else {
                    continue;
                };
                for &b in rings {
                    let Some((hb, xb)) = map.get(&b) else { continue };
                    if plane_touches_revolved(&plane, hb, xb) {
                        out.insert((a.min(b), a.max(b)));
                    }
                }
            }
        }
    }
    out
}

/// A sweep leaf's identity: which feature, and where its profile plane points.
struct SweepFrame {
    feature: u32,
    is_revolve: bool,
    /// World direction of the leaf's local +z — the extrusion direction for an
    /// extrude, the normal of the profile half-plane for a revolve.
    normal: [f64; 3],
}

/// `|e·p|` may differ from 1 only by float noise: both directions come out of
/// unit quaternions, so the product is accurate to a few ulps of f32.
const AXIS_TOL: f64 = 1e-5;

/// Relative slack for the two tangency identities. The residuals measured on
/// the #419 rail are 8e-7 mm and 4e-8 (f32 evaluation noise on coordinates of
/// order 17 mm); 1e-5 keeps two decades of headroom over that while staying
/// far below any honest geometric separation. The two identities together are
/// a CODIMENSION-2 condition, so a merely grazing pair cannot slip through:
/// the 3.6° graze of `lichen_adv_graze_sphere_plane_watertight` misses the
/// angular identity by 0.06 — four orders of magnitude.
const TANGENCY_REL_TOL: f64 = 1e-5;

fn is_cap(branch: u32) -> bool {
    let sub = BranchId(branch).sub() & !BranchId::INNER_WALL_FLAG;
    sub == BranchId::EXTRUDE_CAP_TOP_SUB || sub == BranchId::EXTRUDE_CAP_BOTTOM_SUB
}

fn collect_sweep_frames(node: &BoundedCsg, out: &mut Vec<SweepFrame>) {
    let push = |id: &fielddraft_protocol::FeatureId,
                xf: &Transform,
                is_revolve: bool,
                out: &mut Vec<SweepFrame>| {
        // A non-uniformly scaled sweep is no longer a cone/cylinder in its own
        // frame, so the closed-form identities below do not apply.
        if !is_uniform(xf) {
            return;
        }
        out.push(SweepFrame {
            feature: BranchId::from_feature(id).feature_id(),
            is_revolve,
            normal: unit(rotate_f64(xf, [0.0, 0.0, 1.0])),
        });
    };
    match node {
        BoundedCsg::Extrude { id, world_transform, .. } => {
            push(id, world_transform, false, out)
        }
        BoundedCsg::Revolve { id, world_transform, .. } => {
            push(id, world_transform, true, out)
        }
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::SmoothUnion { left, right, .. }
        | BoundedCsg::SmoothDifference { left, right, .. }
        | BoundedCsg::MaskedBlend { left, right, .. }
        | BoundedCsg::Morph { left, right, .. } => {
            collect_sweep_frames(left, out);
            collect_sweep_frames(right, out);
        }
        BoundedCsg::Shell { inner, .. }
        | BoundedCsg::Twist { inner, .. }
        | BoundedCsg::Bend { inner, .. }
        | BoundedCsg::Displace { inner, .. }
        | BoundedCsg::Offset { inner, .. }
        | BoundedCsg::InnerWall { inner, .. }
        | BoundedCsg::ArcChannel { inner, .. } => collect_sweep_frames(inner, out),
        BoundedCsg::Empty
        | BoundedCsg::Leaf { .. }
        | BoundedCsg::Helix { .. }
        | BoundedCsg::Sweep { .. }
        | BoundedCsg::Loft { .. } => {}
    }
}

/// A world-space plane: unit normal plus signed offset (`p·n̂ − d = 0`).
struct WorldPlane {
    n: [f64; 3],
    d: f64,
}

/// Lift a local `HalfSpace::Plane` into world space. `None` for a degenerate
/// normal or a non-uniformly scaled placement.
fn world_plane(normal: [f32; 3], offset: f32, xf: &Transform) -> Option<WorldPlane> {
    if !is_uniform(xf) {
        return None;
    }
    let n = [normal[0] as f64, normal[1] as f64, normal[2] as f64];
    let len = dot(n, n).sqrt();
    if len < 1e-9 {
        return None;
    }
    let nl = [n[0] / len, n[1] / len, n[2] / len];
    // The local plane is `p·n − offset = 0`, so `n̂ · offset/|n|` is on it.
    // The world normal is rotation-only (a uniform scale leaves directions
    // alone); the world offset comes from that point.
    let t = offset as f64 / len;
    let p0 = local_to_world_f64(xf, [nl[0] * t, nl[1] * t, nl[2] * t]);
    let nw = unit(rotate_f64(xf, nl));
    Some(WorldPlane { n: nw, d: dot(nw, p0) })
}

/// Is `plane` the tangent plane of the revolved carrier `hs` @ `xf` along one
/// of its generators? Two identities, both stated in the carriers' own exact
/// parameters:
///
/// * **Cone** (apex `A`, unit axis `â`, half-angle `α`): the plane is tangent
///   iff it passes through the apex AND `|n̂·â| = sin α`. (Generators are
///   `d = cos α · â + sin α · u`, `u ⊥ â`; `d·n̂ = 0` has a DOUBLE root iff
///   `cos α |n̂·â| = sin α √(1 − (n̂·â)²)`, i.e. `|n̂·â| = sin α`.)
/// * **Cylinder** (axis point `C`, unit axis `â`, radius `ρ`): tangent iff the
///   plane is parallel to the axis AND `|dist(C, plane)| = ρ`.
/// * **Plane** (a revolved constant-`y` line, i.e. a disc): two planes are
///   never *tangent* — they either cross or coincide, and coincidence is
///   [`crate::brep::coincidence`]'s business. Not our case.
fn plane_touches_revolved(plane: &WorldPlane, hs: &HalfSpace, xf: &Transform) -> bool {
    if !is_uniform(xf) {
        return false;
    }
    let scale = xf.scale[0] as f64;
    // Both carriers put their axis of revolution on the local +y axis.
    let axis = unit(rotate_f64(xf, [0.0, 1.0, 0.0]));
    let origin = local_to_world_f64(xf, [0.0, 0.0, 0.0]);
    let dist = dot(plane.n, origin) - plane.d;
    let cos_na = dot(plane.n, axis).abs();
    match hs {
        HalfSpace::Cone { half_angle } => {
            // The apex sits at the carrier's local origin (the profile line's
            // r = 0 point is folded into `xf` as a pre-translation).
            let len_tol = TANGENCY_REL_TOL * norm(origin).max(1.0);
            dist.abs() <= len_tol
                && (cos_na - (*half_angle as f64).sin()).abs() <= TANGENCY_REL_TOL
        }
        HalfSpace::InfiniteCylinder { radius } => {
            let r = *radius as f64 * scale;
            let len_tol = TANGENCY_REL_TOL * norm(origin).max(r).max(1.0);
            cos_na <= TANGENCY_REL_TOL && (dist.abs() - r).abs() <= len_tol
        }
        _ => false,
    }
}

fn is_uniform(t: &Transform) -> bool {
    let [sx, sy, sz] = t.scale;
    (sx - sy).abs() <= 1e-6 * sx.abs().max(1.0) && (sy - sz).abs() <= 1e-6 * sy.abs().max(1.0)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn norm(v: [f64; 3]) -> f64 {
    dot(v, v).sqrt()
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
