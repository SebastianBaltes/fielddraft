//! Closed-form wireframe fast paths (lone box / cylinder / edgeless
//! bodies) and the `weave_parametric` entry that dispatches them before the
//! seed route.

use std::collections::HashMap;

use crate::brep::{
    BRep, Edge, EdgeGeometry, NurbsCurve, Vertex, VertexId,
};
use crate::bvh::BoundedCsg;
use crate::csg::BranchId;
use fielddraft_protocol::{Primitive, Transform};

// ─── small vector helpers (f64): shared, see brep::vec3 ─────────────────────
use super::*;

/// Analytic ("parametric") B-Rep weave — no octree, no sampling.
///
/// Two layers, tried in order:
/// 1. **Exact grundform fast-paths** (`build_box`/`cylinder`/`edgeless_wireframe`):
///    closed-form wireframes for a lone Box / Cylinder / Sphere|Torus. These
///    carry precision the general path cannot — exact rational-quadratic circle
///    rims, corners exact to f64 — so they stay even though the seed path below
///    would also handle them (it would only fit, not solve, those circles).
/// 2. **Branch-seed general weave** ([`crate::brep::seed_weaver::seed_wireframe`],
///    Phase 2b): seeds edges from CSG branch adjacency, traces them with the
///    active-interval tracer, and dedups trim boundaries into corners. This
///    covers Booleans, analytic Extrude/Revolve sweeps, and the cone — every
///    body the fast-paths reject but that is still fully analytic.
///
/// All layers finish through the same [`finish_weave`], and every layer's branch
/// signatures match the octree decomposition, so the resulting `id_hash` element
/// handles are byte-identical to the SDF route (a UI selection survives toggling
/// the feature on/off — asserted across the `parametric_*`/`seed_*` tests).
///
/// Returns `None` only for bodies outside the analytic scope — domain warps
/// (Twist/Bend/Displace), non-Lipschitz features (Thread/Gyroid), and any seam
/// the seed path cannot resolve cleanly — and the caller falls back to the
/// octree [`weave`].
pub fn weave_parametric(body: &BoundedCsg) -> Option<BRep> {
    // The first layer that recognises `body` wins; the exact grundform paths take
    // precedence over the general seed path so a lone Box/Cylinder/Sphere keeps
    // its closed-form precision. Anything none of them lift falls through to the
    // octree via `None`.
    let wireframe = build_box_wireframe(body)
        .or_else(|| build_cylinder_wireframe(body))
        .or_else(|| build_edgeless_wireframe(body))
        .or_else(|| crate::brep::seed_weaver::seed_wireframe(body))?;
    Some(finish_weave(wireframe, body))
}

/// Exact 8-corner / 12-edge wireframe of a single axis-aligned Box leaf, in the
/// body-local frame. `None` unless `body` is exactly one Box primitive.
///
/// The corner positions are computed with the SAME face transform the analytic
/// faces use (`branch_halfspace_map`, which folds the primitive `center` in),
/// so every corner lies exactly on its three face planes — the body is
/// watertight by construction, not by sampling tolerance.
pub(super) fn build_box_wireframe(body: &BoundedCsg) -> Option<BRep> {
    let (id, half_extents) = match body {
        BoundedCsg::Leaf {
            id,
            primitive: Primitive::Box { half_extents, .. },
            ..
        } => (id, *half_extents),
        _ => return None,
    };

    // Reuse the analytic face transform (sub 0; all six box faces share it).
    let halfspaces = branch_halfspace_map(body);
    let face_xf = halfspaces.get(&BranchId::from_feature_sub(id, 0).as_u32())?.1;

    let [hx, hy, hz] = half_extents;
    let mut brep = BRep::new();

    // Sub-id layout for a Box (see `primitive_halfspaces`): X faces are subs
    // 0(+)/1(-), Y are 2(+)/3(-), Z are 4(+)/5(-). Subs are already ascending
    // per axis, so a corner's three branches come out canonically sorted.
    let axis_sub = |axis: usize, positive: bool| (axis as u32) * 2 + u32::from(!positive);
    let signs = [-1.0f32, 1.0f32]; // index 0 = negative, 1 = positive

    // 8 corners, keyed by (xi, yi, zi) sign-bit indices.
    let mut corner: HashMap<(usize, usize, usize), VertexId> = HashMap::new();
    for (xi, &sx) in signs.iter().enumerate() {
        for (yi, &sy) in signs.iter().enumerate() {
            for (zi, &sz) in signs.iter().enumerate() {
                let p = xf_point(&face_xf, [sx * hx, sy * hy, sz * hz]);
                let branches = vec![
                    BranchId::from_feature_sub(id, axis_sub(0, sx > 0.0)),
                    BranchId::from_feature_sub(id, axis_sub(1, sy > 0.0)),
                    BranchId::from_feature_sub(id, axis_sub(2, sz > 0.0)),
                ];
                let vid = brep.add_vertex(Vertex::new(p, branches));
                corner.insert((xi, yi, zi), vid);
            }
        }
    }

    // 12 edges: every pair of faces on DIFFERENT axes. The edge runs along the
    // third (free) axis between the two corners sharing the two fixed signs.
    for &(a, b) in &[(0usize, 1usize), (0, 2), (1, 2)] {
        let free = 3 - a - b;
        for (ia, &sa) in signs.iter().enumerate() {
            for (ib, &sb) in signs.iter().enumerate() {
                // Endpoints: hold the two fixed sign-bits, vary the free axis.
                let mut idx = [0usize; 3];
                idx[a] = ia;
                idx[b] = ib;
                idx[free] = 0;
                let v0 = corner[&(idx[0], idx[1], idx[2])];
                idx[free] = 1;
                let v1 = corner[&(idx[0], idx[1], idx[2])];

                let ba = BranchId::from_feature_sub(id, axis_sub(a, sa > 0.0));
                let bb = BranchId::from_feature_sub(id, axis_sub(b, sb > 0.0));
                // A box edge is a straight segment → exact 2-point polyline.
                let p0 = brep.vertex(v0).position;
                let p1 = brep.vertex(v1).position;
                brep.add_edge(Edge::new(
                    EdgeGeometry::Polyline(vec![p0, p1]),
                    ba,
                    bb,
                    Some(v0),
                    Some(v1),
                ));
            }
        }
    }

    Some(brep)
}

/// Exact analytic wireframe of a single axis-aligned Cylinder leaf, in the
/// body-local frame. `None` unless `body` is exactly one Cylinder primitive.
///
/// A cylinder weaves into three faces — one [`HalfSpace::InfiniteCylinder`] wall
/// (sub 0) plus two planar caps (sub 1 = +Y, sub 2 = −Y) — joined at exactly two
/// rim circles and carrying NO vertices (a closed-loop edge per rim). We emit the
/// two rims directly as *exact rational-quadratic NURBS circles* (the closed-form
/// circle, resolution-independent — the milestone-1 precision win over the
/// sampled octree fit), each tagged with its wall∩cap branch signature.
/// [`finish_weave`] groups them into the three analytic faces via
/// [`branch_halfspace_map`], so the resulting `id_hash` handles are byte-identical
/// to the octree route.
pub(super) fn build_cylinder_wireframe(body: &BoundedCsg) -> Option<BRep> {
    let (id, radius, half_height) = match body {
        BoundedCsg::Leaf {
            id,
            primitive: Primitive::Cylinder { radius, half_height, .. },
            ..
        } => (id, *radius, *half_height),
        _ => return None,
    };

    // The wall transform (sub 0) maps the centred-local frame to world; it
    // already folds the primitive centre (branch_halfspace_map / DL-20). The two
    // caps share the same frame, so both rims use it.
    let halfspaces = branch_halfspace_map(body);
    let wall = BranchId::from_feature_sub(id, 0);
    let top = BranchId::from_feature_sub(id, 1);
    let bottom = BranchId::from_feature_sub(id, 2);
    let wall_xf = halfspaces.get(&wall.as_u32())?.1;

    let mut brep = BRep::new();
    // Rims are the closed-loop edges wall∩cap: top at local y = +half_height,
    // bottom at y = −half_height (the cylinder axis is local Y, radius in x-z).
    brep.add_edge(Edge::new(
        EdgeGeometry::Nurbs(nurbs_circle(&wall_xf, radius, half_height)),
        wall,
        top,
        None,
        None,
    ));
    brep.add_edge(Edge::new(
        EdgeGeometry::Nurbs(nurbs_circle(&wall_xf, radius, -half_height)),
        wall,
        bottom,
        None,
        None,
    ));
    Some(brep)
}

// NOTE — the Cone (frustum) grundform is intentionally NOT handled in the
// *parametric* route yet; it weaves correctly via the octree route. A truncated
// cone's analytic wall is a [`HalfSpace::Cone`] whose SDF (`r·cosα − |y|·sinα`)
// puts the apex at the transform's LOCAL ORIGIN. The frustum's extrapolated apex
// sits up the axis at `y = half_height·(r_bottom+r_top)/(r_bottom−r_top)`, so
// [`primitive_halfspaces`] carries that shift as the wall face's `local_offset`
// and [`branch_halfspace_map`] folds it into the face transform (DL-76). With
// the analytic cone now passing exactly through both rims, the octree weaver's
// `chain_on_both_surfaces` accepts both rim seams (a frustum comes out 3 faces /
// 2 edges, watertight) and STEP export places the CONICAL_SURFACE apex at the
// correct world point. A *parametric* cone is still deferred: it would have to
// reproduce the octree handles byte-for-byte to keep the id_hash toggle-
// invariant, which is the only reason to add it — no correctness gap remains.

/// Empty wireframe of a single closed-surface leaf with NO edges and NO vertices
/// — a lone Sphere or Torus (SA-6). `None` unless `body` is exactly one such
/// primitive.
///
/// These grundforms are a single un-split analytic face bounded by nothing:
/// there is no closed-form wireframe to build, only the (empty) edge set.
/// [`finish_weave`] step (2b) then materialises the lone closed face directly
/// from [`branch_halfspace_map`] — the same path the octree route takes when an
/// edgeless body produces no edge cells — so the `id_hash` handle is byte-
/// identical to the SDF route.
pub(super) fn build_edgeless_wireframe(body: &BoundedCsg) -> Option<BRep> {
    match body {
        BoundedCsg::Leaf {
            primitive: Primitive::Sphere { .. } | Primitive::Torus { .. },
            ..
        } => Some(BRep::new()),
        _ => None,
    }
}

/// The exact closed circle of `radius` in the plane `y = y_local` of the
/// centred-local frame, mapped to world by `xf`, as a rational quadratic NURBS.
///
/// This is the canonical 9-control-point degree-2 representation of a full
/// circle: the four axis points carry weight 1, the four square corners weight
/// √2/2, with a clamped knot vector — geometrically *exact* (a true circle, not a
/// polygonal sample), and exactly periodic (first control point repeated to
/// close). A rigid + uniform-scale `xf` maps it to another exact circle.
pub(super) fn nurbs_circle(xf: &Transform, radius: f32, y_local: f32) -> NurbsCurve {
    let r = radius;
    let y = y_local;
    // Square corner ↔ axis points, CCW about +Y starting at +x, closed.
    let local: [[f32; 3]; 9] = [
        [r, y, 0.0],
        [r, y, r],
        [0.0, y, r],
        [-r, y, r],
        [-r, y, 0.0],
        [-r, y, -r],
        [0.0, y, -r],
        [r, y, -r],
        [r, y, 0.0],
    ];
    let control_points: Vec<[f64; 3]> = local.iter().map(|&p| xf_point(xf, p)).collect();
    let s = std::f64::consts::FRAC_1_SQRT_2; // √2/2 for the square-corner points
    let weights = vec![1.0, s, 1.0, s, 1.0, s, 1.0, s, 1.0];
    // Clamped quadratic knot vector for four quarter-arc Bézier spans.
    let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 4.0];
    NurbsCurve { degree: 2, control_points, weights, knots }
}

/// `R·(s·p) + t` for a rigid + uniform-scale transform (f32 in, f64 out).
pub(super) fn xf_point(t: &Transform, p: [f32; 3]) -> [f64; 3] {
    let s = t.scale[0];
    let scaled = [p[0] * s, p[1] * s, p[2] * s];
    let r = crate::quat_rotate(t.rotation, scaled);
    [
        (r[0] + t.translate[0]) as f64,
        (r[1] + t.translate[1]) as f64,
        (r[2] + t.translate[2]) as f64,
    ]
}
