//! B-Rep extraction, **Phase 4: B-Rep weaving**.
//!
//! Implements the final stage of the B-Rep pipeline described in
//! `docs/GEOMETRY-KERNEL-CONCEPT.md` ("Phase 4: B-Rep Weaving"). It takes the
//! wireframe [`BRep`] produced by the Phase-3.5 edge walker
//! ([`crate::brep::edge_walker::build_wireframe`]) — deduplicated vertices plus
//! Hermite edge chains bounded by `start`/`end` vertices — and finishes it into
//! a watertight solid:
//!
//! 1. **Hermite → NURBS fitting.** Each edge's `(point, tangent)` chain is
//!    converted into a piecewise-cubic-Bézier [`NurbsCurve`] (degree 3). Every
//!    consecutive Hermite segment becomes one cubic Bézier whose interior
//!    control points sit at `± m/3` of the segment's endpoint tangents; the
//!    unit edge-walk tangents are rescaled to chord-length magnitude (DL-21).
//!    The Béziers are stitched into a single clamped NURBS with `C⁰` interior
//!    joins (DL-22).
//! 2. **Face / trimmed-surface weaving.** Every [`BranchId`] that appears in an
//!    edge signature becomes one [`Face`]. Its boundary edges (all edges whose
//!    signature contains the branch) are ordered into an outer [`Loop`] plus
//!    zero or more inner (hole) loops by a **reflex-aware turning-angle walk**
//!    (DL-58): at each vertex the next edge is chosen by signed turning angle in
//!    the face's tangent plane (oriented by the surface normal), so concave
//!    (reflex) vertices and vertices shared with holes are traversed correctly;
//!    disjoint leftover rings become inner loops, classified outer-vs-inner by
//!    projected signed area (planar) or ring extent (curved). The face's
//!    analytic surface is looked up from the body's branch→half-space table
//!    (DL-19, DL-20).
//! 3. **Shell + solid assembly.** All faces form one [`Shell`]; it is
//!    `is_closed` iff every edge is referenced by exactly two face loops — the
//!    manifold / watertight criterion (DL-24). One [`Solid`] wraps that shell.

use std::collections::HashMap;

use crate::brep::edge_walker::build_wireframe;
use crate::brep::octree::Octree;
use crate::brep::{
    BRep, EdgeGeometry, EdgeId, Face, HermiteSample, Loop, NurbsCurve, Shell, Solid,
    SurfaceGeometry, VertexId,
};
use crate::bvh::BoundedCsg;
use crate::compose_transforms;
use crate::csg::{primitive_halfspaces, BranchId};
use crate::sketch2d::{BoundedSketch2d, Sketch2dPrimitive};
use fielddraft_protocol::{HalfSpace, Transform};

// ─── small vector helpers (f64): shared, see brep::vec3 ─────────────────────
use super::vec3::{add, normalize as unit, scale, sub};
use super::vec3::norm;

// ─── (1) Hermite → NURBS fitting ────────────────────────────────────────────

/// Convert an ordered Hermite chain into a single clamped, piecewise-cubic
/// [`NurbsCurve`] (degree 3, all weights 1).
///
/// Each consecutive pair `(p_i, t_i) → (p_{i+1}, t_{i+1})` is one cubic Bézier
/// with control points `b0 = p_i`, `b1 = p_i + m_i/3`, `b2 = p_{i+1} − m_{i+1}/3`,
/// `b3 = p_{i+1}`. The edge-walk tangents are *unit* directions, so each segment
/// scales the unit tangents by the segment's chord length (DL-21) to recover a
/// magnitude. The per-segment Béziers are concatenated with `C⁰` interior joins:
/// shared endpoints are stored once, giving `3·s + 1` control points for `s`
/// segments and a clamped knot vector with end multiplicity 4 and interior
/// multiplicity 3 (DL-22).
///
/// Panics if `samples.len() < 2` (an edge must have at least two samples).
pub fn hermite_to_nurbs(samples: &[HermiteSample]) -> NurbsCurve {
    assert!(
        samples.len() >= 2,
        "hermite_to_nurbs needs at least 2 samples"
    );

    let mut control_points: Vec<[f64; 3]> = Vec::new();
    let n_seg = samples.len() - 1;

    for i in 0..n_seg {
        let p0 = samples[i].point;
        let p1 = samples[i + 1].point;
        // Chord-length magnitude: unit tangents × this segment's chord (DL-21).
        let chord = norm(sub(p1, p0));
        let t0 = unit(samples[i].tangent);
        let t1 = unit(samples[i + 1].tangent);
        let m0 = scale(t0, chord);
        let m1 = scale(t1, chord);
        let b1 = add(p0, scale(m0, 1.0 / 3.0));
        let b2 = sub(p1, scale(m1, 1.0 / 3.0));

        if i == 0 {
            control_points.push(p0);
        }
        control_points.push(b1);
        control_points.push(b2);
        control_points.push(p1);
    }

    let degree = 3;
    let n = control_points.len();
    let weights = vec![1.0; n];
    // Clamped knot vector: end multiplicity 4 (= degree + 1), each interior
    // segment join at multiplicity 3 (= degree) for a C⁰ Bézier joint (DL-22).
    // Total length = n + degree + 1 = 3·n_seg + 1 + 4 = 3·n_seg + 5.
    let mut knots: Vec<f64> = Vec::with_capacity(n + degree + 1);
    knots.extend(std::iter::repeat_n(0.0, degree + 1));
    for s in 1..n_seg {
        knots.extend(std::iter::repeat_n(s as f64, degree));
    }
    knots.extend(std::iter::repeat_n(n_seg as f64, degree + 1));
    debug_assert_eq!(knots.len(), n + degree + 1);

    NurbsCurve {
        degree,
        control_points,
        weights,
        knots,
    }
}

// ─── branch → (HalfSpace, Transform) lookup ─────────────────────────────────

/// Walk a [`BoundedCsg`] and build the map `branch (as u32) → (HalfSpace,
/// Transform)` for every analytic sub-face of every leaf. The half-space
/// enumeration is delegated to [`primitive_halfspaces`] (the single source of
/// truth, co-located with `expand_evaluate_leaf` in `csg.rs`, DL-19).
///
/// The stored [`Transform`] maps the half-space's centred local frame to world
/// space: the primitive `center` offset (applied as `lp = p − center` inside
/// the expansion, *not* part of `world_transform`) is folded in as a local
/// pre-translation, `world_transform ∘ translate(center)` (DL-20).
pub fn branch_halfspace_map(body: &BoundedCsg) -> HashMap<u32, (HalfSpace, Transform)> {
    let mut out = HashMap::new();
    collect_halfspaces(body, &mut out);
    out
}

fn collect_halfspaces(node: &BoundedCsg, out: &mut HashMap<u32, (HalfSpace, Transform)>) {
    match node {
        BoundedCsg::Leaf {
            id,
            primitive,
            world_transform,
            ..
        } => {
            if let Some(hs) = primitive_halfspaces(primitive) {
                // Fold the primitive `center` into the face transform as a
                // local pre-translation: world ∘ translate(center) (DL-20).
                let center_xf = Transform {
                    translate: hs.center,
                    ..Transform::IDENTITY
                };
                let face_xf = compose_transforms(world_transform, &center_xf);
                for f in hs.faces() {
                    let branch = BranchId::from_feature_sub(id, f.sub);
                    out.insert(branch.as_u32(), (f.half_space.clone(), face_xf));
                }
            }
            // Primitives without a half-space split (Sphere/Torus/Gyroid/...)
            // and sketch sweeps (Extrude/Revolve) get no analytic entry; their
            // faces fall back to Tessellated in the weaver (DL-25).
        }
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::SmoothUnion { left, right, .. }
        | BoundedCsg::SmoothDifference { left, right, .. }
        | BoundedCsg::MaskedBlend { left, right, .. }
        | BoundedCsg::Morph { left, right, .. } => {
            collect_halfspaces(left, out);
            collect_halfspaces(right, out);
        }
        BoundedCsg::Twist { inner, .. }
        | BoundedCsg::Bend { inner, .. }
        | BoundedCsg::Displace { inner, .. }
        | BoundedCsg::Shell { inner, .. } => collect_halfspaces(inner, out),
        BoundedCsg::Extrude {
            id,
            sketch,
            half_height,
            world_transform,
            ..
        } => collect_extrude_halfspaces(id, sketch, *half_height, world_transform, out),
        BoundedCsg::Revolve {
            id,
            sketch,
            world_transform,
            ..
        } => collect_revolve_halfspaces(id, sketch, world_transform, out),
        BoundedCsg::Empty => {}
    }
}

// ─── Extrude / Revolve analytic faces (DL-50) ────────────────────────────────

/// A 2D sketch boundary group's analytic geometry, used to lift each
/// wall of an [`BoundedCsg::Extrude`] / [`BoundedCsg::Revolve`] into a
/// 3D [`HalfSpace`]. Only straight lines and full circles are
/// analytically representable here; curved (quad-bézier / arc / sector)
/// groups yield `None` and fall back to a tessellated face (DL-50).
enum SketchWall {
    /// Straight boundary segment with an outward 2D unit normal and the
    /// plane offset `n·p` for any point `p` on the line.
    Line { normal: [f32; 2], offset: f32 },
    /// Full circle (single boundary group): centre + radius.
    Circle { center: [f32; 2], radius: f32 },
}

/// Walk a [`BoundedSketch2d`] tree and emit `(group_sub, SketchWall)`
/// for every analytically-representable boundary group, using the
/// **same** global group-index convention as
/// [`BoundedSketch2d::sdf_with_edge`] (right-child ids offset past the
/// left child's `edge_count`). Curved groups are simply skipped, so the
/// weaver leaves them as tessellated faces.
fn collect_sketch_walls(sketch: &BoundedSketch2d, base: u32, out: &mut Vec<(u32, SketchWall)>) {
    match sketch {
        BoundedSketch2d::Empty => {}
        BoundedSketch2d::Prim { primitive, .. } => {
            collect_prim_walls(primitive, base, out)
        }
        BoundedSketch2d::Union { left, right, .. }
        | BoundedSketch2d::Difference { left, right, .. }
        | BoundedSketch2d::Intersection { left, right, .. } => {
            collect_sketch_walls(left, base, out);
            collect_sketch_walls(right, base + left.edge_count(), out);
        }
        // Offset shifts the boundary; a straight line stays straight (it
        // moves along its normal) but recomputing the offset for every
        // primitive type is brittle, so offset sketches fall back to
        // tessellated faces (DL-50).
        BoundedSketch2d::Offset { .. } => {}
    }
}

/// Per-primitive wall enumeration, matching the group order of
/// [`Sketch2dPrimitive::sdf_with_edge`]: Rectangle +x,−x,+y,−y;
/// Circle single group 0; Polygon per-edge `e` = `v[e]→v[(e+1)%n]`.
fn collect_prim_walls(prim: &Sketch2dPrimitive, base: u32, out: &mut Vec<(u32, SketchWall)>) {
    match prim {
        Sketch2dPrimitive::Rectangle { center, half_extents } => {
            let [cx, cy] = *center;
            let [hx, hy] = *half_extents;
            // Branch order +x, −x, +y, −y (DL-32) with outward normals.
            out.push((base, SketchWall::Line { normal: [1.0, 0.0], offset: cx + hx }));
            out.push((base + 1, SketchWall::Line { normal: [-1.0, 0.0], offset: -(cx - hx) }));
            out.push((base + 2, SketchWall::Line { normal: [0.0, 1.0], offset: cy + hy }));
            out.push((base + 3, SketchWall::Line { normal: [0.0, -1.0], offset: -(cy - hy) }));
        }
        Sketch2dPrimitive::Circle { center, radius } => {
            out.push((base, SketchWall::Circle { center: *center, radius: *radius }));
        }
        Sketch2dPrimitive::Polygon { vertices } => {
            let n = vertices.len();
            if n < 3 {
                return;
            }
            // Winding sign: CCW (area > 0) ⇒ outward normal is the edge
            // direction rotated −90°, CW ⇒ +90°. (sdf_polygon recovers
            // the interior sign via even-odd, so either winding works.)
            let area = polygon_signed_area(vertices);
            let ccw = area > 0.0;
            for e in 0..n {
                let a = vertices[e];
                let b = vertices[(e + 1) % n];
                let ex = b[0] - a[0];
                let ey = b[1] - a[1];
                let len = (ex * ex + ey * ey).sqrt();
                if len < 1e-9 {
                    continue; // degenerate edge → no analytic plane
                }
                // Outward unit normal.
                let (nx, ny) = if ccw {
                    (ey / len, -ex / len)
                } else {
                    (-ey / len, ex / len)
                };
                let offset = nx * a[0] + ny * a[1];
                out.push((base + e as u32, SketchWall::Line { normal: [nx, ny], offset }));
            }
        }
        // Sector and Path (lines, quad-béziers, arcs) are not enumerated
        // here: Sector mixes radial lines + an arc into a single group 0,
        // and Path groups can be curved. They fall back to tessellated
        // faces (DL-50). A Path made purely of straight, per-segment lines
        // could in principle be lifted, but its G1 grouping can merge
        // collinear segments, so we conservatively skip it.
        Sketch2dPrimitive::Sector { .. } | Sketch2dPrimitive::Path { .. } => {}
    }
}

fn polygon_signed_area(v: &[[f32; 2]]) -> f32 {
    let n = v.len();
    let mut area = 0.0f32;
    for i in 0..n {
        let j = (i + 1) % n;
        area += v[i][0] * v[j][1] - v[j][0] * v[i][1];
    }
    area * 0.5
}

/// Analytic faces of an [`BoundedCsg::Extrude`] (DL-50): two caps as
/// planes plus one wall per straight/circular sketch boundary group.
/// The extrude slab is centred in the local frame (`z ∈ [−hh, +hh]`,
/// sketch in the x-y plane); `world_transform` maps that frame to world.
fn collect_extrude_halfspaces(
    id: &fielddraft_protocol::FeatureId,
    sketch: &BoundedSketch2d,
    half_height: f32,
    world_transform: &Transform,
    out: &mut HashMap<u32, (HalfSpace, Transform)>,
) {
    let xf = *world_transform;
    // Caps: top z = +hh, bottom z = −hh. Outward normals ±z, offsets
    // = +hh (matching the Box top/bottom plane convention, p·n − offset).
    let top = BranchId::from_feature_sub(id, BranchId::EXTRUDE_CAP_TOP_SUB);
    let bottom = BranchId::from_feature_sub(id, BranchId::EXTRUDE_CAP_BOTTOM_SUB);
    out.insert(
        top.as_u32(),
        (HalfSpace::Plane { normal: [0.0, 0.0, 1.0], offset: half_height }, xf),
    );
    out.insert(
        bottom.as_u32(),
        (HalfSpace::Plane { normal: [0.0, 0.0, -1.0], offset: half_height }, xf),
    );

    // Walls: lift each 2D boundary group to a 3D half-space.
    let mut walls = Vec::new();
    collect_sketch_walls(sketch, 0, &mut walls);
    for (group, wall) in walls {
        let branch = BranchId::from_feature_sub(id, group);
        match wall {
            SketchWall::Line { normal, offset } => {
                // 2D outward normal lifted to [nx, ny, 0]; offset unchanged.
                out.insert(
                    branch.as_u32(),
                    (HalfSpace::Plane { normal: [normal[0], normal[1], 0.0], offset }, xf),
                );
            }
            SketchWall::Circle { center, radius } => {
                // Wall is an infinite cylinder about the local z-axis. The
                // 2D circle centre folds into the face transform as a
                // local pre-translation (x,y), exactly like DL-20.
                let center_xf = compose_transforms(
                    &xf,
                    &Transform {
                        translate: [center[0], center[1], 0.0],
                        ..Transform::IDENTITY
                    },
                );
                out.insert(
                    branch.as_u32(),
                    (HalfSpace::InfiniteCylinder { radius }, center_xf),
                );
            }
        }
    }
}

/// Analytic faces of a [`BoundedCsg::Revolve`] (DL-50). The profile is
/// authored in `(r, y)` and revolved about the local y-axis. A straight
/// profile segment becomes: a constant-`r` line → cylinder; a
/// constant-`y` line → plane; an oblique line → cone. Revolves have no
/// caps. Curved groups fall back to tessellated faces.
fn collect_revolve_halfspaces(
    id: &fielddraft_protocol::FeatureId,
    sketch: &BoundedSketch2d,
    world_transform: &Transform,
    out: &mut HashMap<u32, (HalfSpace, Transform)>,
) {
    let xf = *world_transform;
    // A `SketchWall::Line` here carries the (r, y)-plane normal/offset,
    // not a world plane. We re-derive the revolved surface from the
    // segment endpoints instead, so collect raw line groups and map them.
    let mut walls = Vec::new();
    collect_sketch_walls(sketch, 0, &mut walls);
    for (group, wall) in walls {
        let branch = BranchId::from_feature_sub(id, group);
        let SketchWall::Line { normal, offset } = wall else {
            // Circle profile revolves into a torus; not enumerated here
            // (would need HalfSpace::Torus placement) → tessellated.
            continue;
        };
        // The line in (r, y) is { p : normal·p = offset }. Recover its
        // orientation: a vertical line (normal ∥ r-axis) is constant r
        // ⇒ cylinder; a horizontal line (normal ∥ y-axis) is constant y
        // ⇒ plane; otherwise oblique ⇒ cone.
        let [nr, ny] = normal;
        if ny.abs() < 1e-6 {
            // Constant r = offset / nr (nr = ±1). Radius is |r|.
            let radius = (offset / nr).abs();
            out.insert(branch.as_u32(), (HalfSpace::InfiniteCylinder { radius }, xf));
        } else if nr.abs() < 1e-6 {
            // Constant y = offset / ny. Revolving a horizontal line gives
            // a flat disc/annulus in the plane y = const. Outward normal
            // along ±y matches the 2D normal's sign.
            let y_plane = offset / ny;
            let sign = ny.signum();
            let center_xf = compose_transforms(
                &xf,
                &Transform {
                    translate: [0.0, y_plane, 0.0],
                    ..Transform::IDENTITY
                },
            );
            out.insert(
                branch.as_u32(),
                (HalfSpace::Plane { normal: [0.0, sign, 0.0], offset: 0.0 }, center_xf),
            );
        } else {
            // Oblique line → cone. The cone half-angle is measured from
            // the y-axis; the line direction (dr, dy) is perpendicular to
            // the normal (nr, ny): (dr, dy) = (−ny, nr). half_angle =
            // atan(|dr| / |dy|). The apex sits on the y-axis where r = 0.
            let dr = -ny;
            let dy = nr;
            let half_angle = (dr.abs()).atan2(dy.abs());
            // Apex y: on the line normal·(r,y)=offset at r=0 ⇒ y = offset/ny.
            let apex_y = offset / ny;
            let center_xf = compose_transforms(
                &xf,
                &Transform {
                    translate: [0.0, apex_y, 0.0],
                    ..Transform::IDENTITY
                },
            );
            out.insert(
                branch.as_u32(),
                (HalfSpace::Cone { half_angle }, center_xf),
            );
        }
    }
}

// ─── (2) face / loop weaving ────────────────────────────────────────────────

/// The ordered boundary loops of one face: an outer ring plus zero or more
/// inner (hole) rings, all guaranteed connected and consistently wound.
struct FaceLoops {
    outer: Vec<(EdgeId, bool)>,
    inner: Vec<Vec<(EdgeId, bool)>>,
}

/// The world-space outward surface normal of `surface` at world point `p`, used
/// to build the tangent plane for turning-angle ring ordering. Returns `None`
/// for surfaces without a usable point normal (tessellated / gyroid) — the
/// caller then falls back to the topology-only greedy walk.
///
/// - `Plane`: the (rotated) plane normal — constant over the face.
/// - `InfiniteCylinder` / `Cone`: the radial direction from the local axis
///   (local +Y) to `p`, rotated to world. (For a cone this is the radial, not
///   the true slant normal, but it is a stable, point-dependent tangent-plane
///   reference, which is all the ring ordering needs.)
/// - `Sphere`: the unit vector from the centre to `p`.
/// - `Torus`: the direction from the nearest point on the tube centre-circle
///   (radius = major) to `p`.
fn surface_normal_at(surface: &SurfaceGeometry, p: [f64; 3]) -> Option<[f64; 3]> {
    let SurfaceGeometry::Analytic { surface, transform } = surface else {
        return None;
    };
    let rot = transform.rotation;
    let to_local = |p: [f64; 3]| -> [f64; 3] {
        let w = [p[0] as f32, p[1] as f32, p[2] as f32];
        let l = crate::world_to_local(transform, w);
        [l[0] as f64, l[1] as f64, l[2] as f64]
    };
    let rot_to_world = |d: [f64; 3]| -> [f64; 3] {
        let r = crate::quat_rotate(rot, [d[0] as f32, d[1] as f32, d[2] as f32]);
        unit([r[0] as f64, r[1] as f64, r[2] as f64])
    };
    match surface {
        HalfSpace::Plane { normal, .. } => {
            Some(rot_to_world([normal[0] as f64, normal[1] as f64, normal[2] as f64]))
        }
        HalfSpace::InfiniteCylinder { .. } | HalfSpace::Cone { .. } => {
            let l = to_local(p);
            let radial = [l[0], 0.0, l[2]]; // strip the local +Y axis component
            if norm(radial) < 1e-9 {
                return None;
            }
            Some(rot_to_world(radial))
        }
        HalfSpace::Sphere { .. } => {
            let l = to_local(p);
            if norm(l) < 1e-9 {
                return None;
            }
            Some(rot_to_world(l))
        }
        HalfSpace::Torus { major_radius, .. } => {
            let l = to_local(p);
            let r = norm([l[0], 0.0, l[2]]);
            if r < 1e-9 {
                return None;
            }
            // Nearest point on the centre-circle (radius = major, in the local
            // x-z plane), then the outward tube direction toward `p`.
            let scale = *major_radius as f64 / r;
            let center = [l[0] * scale, 0.0, l[2] * scale];
            let dir = sub(l, center);
            if norm(dir) < 1e-9 {
                return None;
            }
            Some(rot_to_world(dir))
        }
        HalfSpace::Gyroid { .. } => None,
    }
}

/// Inverse parameterization of an analytic surface: map a world point lying on
/// the surface to its `(u, v)` parameters. Used to classify outer vs. inner
/// loops on curved faces by parametric signed area — a containment test that,
/// unlike world-space chord length, is invariant to a tapered surface's varying
/// radius (a small-radius outer boundary can have shorter chords than a
/// large-radius hole, which the chord proxy would misrank). Returns `None` for
/// surfaces without a usable parameterization (plane / gyroid / tessellated) or
/// at a parametric pole where `(u, v)` is undefined.
///
/// Local-frame conventions match [`surface_normal_at`] (axis = local +Y):
/// - `InfiniteCylinder` / `Cone`: `u = atan2(z, x)` around the axis, `v = y`
///   along it.
/// - `Sphere`: `u = atan2(z, x)` longitude, `v = atan2(y, r_xz)` latitude.
/// - `Torus`: `u = atan2(z, x)` major angle, `v = atan2(y, r_xz − major)` minor.
fn surface_uv(surface: &SurfaceGeometry, p: [f64; 3]) -> Option<(f64, f64)> {
    let SurfaceGeometry::Analytic { surface, transform } = surface else {
        return None;
    };
    let l = crate::world_to_local(transform, [p[0] as f32, p[1] as f32, p[2] as f32]);
    let l = [l[0] as f64, l[1] as f64, l[2] as f64];
    let r_xz = norm([l[0], 0.0, l[2]]);
    match surface {
        HalfSpace::InfiniteCylinder { .. } | HalfSpace::Cone { .. } => {
            if r_xz < 1e-9 {
                return None;
            }
            Some((l[2].atan2(l[0]), l[1]))
        }
        HalfSpace::Sphere { .. } => {
            if r_xz < 1e-9 && l[1].abs() < 1e-9 {
                return None;
            }
            Some((l[2].atan2(l[0]), l[1].atan2(r_xz)))
        }
        HalfSpace::Torus { major_radius, .. } => {
            if r_xz < 1e-9 {
                return None;
            }
            Some((l[2].atan2(l[0]), l[1].atan2(r_xz - *major_radius as f64)))
        }
        HalfSpace::Plane { .. } | HalfSpace::Gyroid { .. } => None,
    }
}

/// Project a 3D direction into the tangent plane defined by unit `normal`,
/// dropping its normal component. (Not re-normalised; callers compare angles.)
fn project_tangent(d: [f64; 3], normal: [f64; 3]) -> [f64; 3] {
    let dn = d[0] * normal[0] + d[1] * normal[1] + d[2] * normal[2];
    [
        d[0] - dn * normal[0],
        d[1] - dn * normal[1],
        d[2] - dn * normal[2],
    ]
}

/// Signed CCW angle (about `normal`) needed to rotate tangent vector `from`
/// onto `to`, in `(-π, π]`. Positive = counter-clockwise seen from the side
/// `normal` points toward.
fn signed_angle(from: [f64; 3], to: [f64; 3], normal: [f64; 3]) -> f64 {
    let cross = [
        from[1] * to[2] - from[2] * to[1],
        from[2] * to[0] - from[0] * to[2],
        from[0] * to[1] - from[1] * to[0],
    ];
    let s = cross[0] * normal[0] + cross[1] * normal[1] + cross[2] * normal[2];
    let c = from[0] * to[0] + from[1] * to[1] + from[2] * to[2];
    s.atan2(c)
}

/// Outgoing world-space tangent of edge `eid` *leaving* vertex `at` (i.e. when
/// the edge is oriented to start at `at`). Uses the adjacent curve sample so a
/// curved edge contributes its true departure direction; falls back to the
/// straight chord between endpoints.
fn outgoing_tangent(brep: &BRep, eid: EdgeId, at: VertexId, reversed: bool) -> Option<[f64; 3]> {
    let edge = brep.edge(eid);
    let pts: Vec<[f64; 3]> = match &edge.geometry {
        EdgeGeometry::Nurbs(c) => c.control_points.clone(),
        EdgeGeometry::Polyline(p) => p.clone(),
        EdgeGeometry::Hermite(s) => s.iter().map(|s| s.point).collect(),
    };
    let at_pos = brep.vertex(at).position;
    // Direction from the vertex toward the first distinct interior point along
    // the traversal direction.
    let ordered: Vec<[f64; 3]> = if reversed {
        pts.iter().rev().cloned().collect()
    } else {
        pts
    };
    for p in &ordered {
        let d = sub(*p, at_pos);
        if norm(d) > 1e-7 {
            return Some(unit(d));
        }
    }
    // No usable curve samples (e.g. an empty polyline in hand-built test
    // geometry): fall back to the straight chord toward the far endpoint. With
    // `reversed` the edge is walked end → start, so the far vertex is `start`;
    // otherwise it is `end`.
    let far = if reversed { edge.start } else { edge.end }?;
    let d = sub(brep.vertex(far).position, at_pos);
    if norm(d) > 1e-7 {
        Some(unit(d))
    } else {
        None
    }
}

/// Order one connected ring out of `pool` (indices into `edge_ids`), starting at
/// `seed`, using **turning-angle disambiguation** so reflex vertices (where ≥3
/// of the face's edges meet) are traversed correctly. At each step we sit at the
/// current walk vertex `cur`, having arrived along reversed-incoming tangent
/// `back` (the direction pointing back the way we came, in the face tangent
/// plane); among all unused candidate edges incident to `cur` we pick the one
/// whose outgoing tangent makes the **largest CCW turn** from `back` about the
/// face normal — i.e. the next edge encountered sweeping counter-clockwise,
/// which keeps the face interior consistently on one side and hugs the local
/// boundary even at a reflex vertex or a vertex shared with a hole.
///
/// Marks consumed indices in `used`. Returns the ordered ring, or `None` if the
/// chain cannot close (degenerate topology).
#[allow(clippy::too_many_arguments)]
fn order_one_ring(
    brep: &BRep,
    edge_ids: &[EdgeId],
    used: &mut [bool],
    seed: usize,
    normal_at: &dyn Fn([f64; 3]) -> Option<[f64; 3]>,
) -> Option<Vec<(EdgeId, bool)>> {
    let endpoints = |e: EdgeId| -> Option<(VertexId, VertexId)> {
        let edge = brep.edge(e);
        Some((edge.start?, edge.end?))
    };

    let (first_start, mut cur) = endpoints(edge_ids[seed])?;
    used[seed] = true;
    let mut ring: Vec<(EdgeId, bool)> = vec![(edge_ids[seed], false)];
    // Reversed-incoming tangent: pointing from the new `cur` back toward the
    // seed's start (i.e. the negated outgoing tangent of the seed edge).
    let mut back = outgoing_tangent(brep, edge_ids[seed], cur, true)?;

    loop {
        // Gather all unused candidate edges incident to `cur`.
        let mut best: Option<(usize, bool, f64)> = None; // (index, reversed, angle)
        let normal = normal_at(brep.vertex(cur).position);
        for (i, &eid) in edge_ids.iter().enumerate() {
            if used[i] {
                continue;
            }
            let (s, e) = endpoints(eid)?;
            let reversed = if s == cur {
                false
            } else if e == cur {
                true
            } else {
                continue;
            };
            let out = match outgoing_tangent(brep, eid, cur, reversed) {
                Some(t) => t,
                None => continue,
            };
            let angle = match normal {
                Some(n) => {
                    let bt = project_tangent(back, n);
                    let ot = project_tangent(out, n);
                    if norm(bt) < 1e-9 || norm(ot) < 1e-9 {
                        // Degenerate projection: fall back to "any" (angle 0).
                        0.0
                    } else {
                        // Largest CCW turn from `back` to the outgoing tangent.
                        // Map into [0, 2π) so the maximum is the sharpest hug.
                        let a = signed_angle(bt, ot, n);
                        if a <= 0.0 {
                            a + std::f64::consts::TAU
                        } else {
                            a
                        }
                    }
                }
                None => 0.0, // no usable normal → first-match (greedy) behaviour
            };
            let better = match best {
                None => true,
                Some((_, _, ba)) => angle > ba,
            };
            if better {
                best = Some((i, reversed, angle));
            }
            if normal.is_none() {
                break; // greedy fallback: take the first incident edge
            }
        }

        let (i, reversed, _) = best?;
        used[i] = true;
        let (s, e) = endpoints(edge_ids[i])?;
        let next = if reversed { s } else { e };
        ring.push((edge_ids[i], reversed));
        // New reversed-incoming tangent at `next`: negated outgoing tangent of
        // the edge we just traversed, i.e. its outgoing tangent leaving `next`
        // when oriented the *opposite* way.
        back = outgoing_tangent(brep, edge_ids[i], next, !reversed)?;
        cur = next;

        if cur == first_start {
            return Some(ring);
        }
    }
}

/// Signed area of a ring projected into the tangent plane defined by `normal`,
/// using vertex positions. Positive = the ring winds counter-clockwise seen
/// from the side `normal` points toward. Edges without bounding vertices (full
/// circles) are skipped; a single closed-circle edge yields area 0 and is
/// classified by length elsewhere.
fn ring_signed_area(brep: &BRep, ring: &[(EdgeId, bool)], normal: [f64; 3]) -> f64 {
    // Build an in-plane orthonormal basis (u, v) with u×v = normal.
    let u = {
        let a = if normal[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        unit(project_tangent(a, normal))
    };
    let v = [
        normal[1] * u[2] - normal[2] * u[1],
        normal[2] * u[0] - normal[0] * u[2],
        normal[0] * u[1] - normal[1] * u[0],
    ];
    let proj = |p: [f64; 3]| -> (f64, f64) {
        (
            p[0] * u[0] + p[1] * u[1] + p[2] * u[2],
            p[0] * v[0] + p[1] * v[1] + p[2] * v[2],
        )
    };
    let mut area = 0.0;
    for &(eid, reversed) in ring {
        let edge = brep.edge(eid);
        let (Some(s), Some(e)) = (edge.start, edge.end) else {
            continue;
        };
        let (a, b) = if reversed { (e, s) } else { (s, e) };
        let (x0, y0) = proj(brep.vertex(a).position);
        let (x1, y1) = proj(brep.vertex(b).position);
        area += x0 * y1 - x1 * y0;
    }
    area * 0.5
}

/// Total chord length of a ring. Used to rank the *full-revolution* trim
/// circles of a curved wall (cylinder/cone tube) against each other — among
/// those periodic boundaries the longest is taken as the outer bound, the rest
/// as inner (DL-58). Bounded (vertex-delimited) loops are classified instead by
/// parametric signed area ([`ring_uv_area`]), which world-space chord length
/// would misrank on a tapered surface.
fn ring_extent(brep: &BRep, ring: &[(EdgeId, bool)]) -> f64 {
    let mut len = 0.0;
    for &(eid, _) in ring {
        let edge = brep.edge(eid);
        if let (Some(s), Some(e)) = (edge.start, edge.end) {
            len += norm(sub(brep.vertex(s).position, brep.vertex(e).position));
        } else {
            // Closed-circle edge: use its control polygon span as a proxy.
            if let EdgeGeometry::Nurbs(c) = &edge.geometry {
                for w in c.control_points.windows(2) {
                    len += norm(sub(w[0], w[1]));
                }
            }
        }
    }
    len
}

/// Parametric signed area of a bounded ring, computed in the surface's `(u, v)`
/// domain ([`surface_uv`]). This is the curved-surface analogue of
/// [`ring_signed_area`]: it measures how much *parameter* area the loop
/// encloses, which — unlike world-space chord length — is invariant to a
/// tapered surface's varying radius, so the true outer boundary outranks holes
/// regardless of their physical size. `u` is unwrapped across the periodic seam
/// so the shoelace stays continuous. Returns `None` if any vertex has no
/// `(u, v)` (e.g. a full-revolution circle edge, or an unparameterized surface).
fn ring_uv_area(brep: &BRep, ring: &[(EdgeId, bool)], surface: &SurfaceGeometry) -> Option<f64> {
    // Collect the ring's ordered tail vertices (b of edge i == a of edge i+1,
    // so the tails are the polygon vertices), mapped to (u, v) and unwrapped.
    let mut uv: Vec<(f64, f64)> = Vec::with_capacity(ring.len());
    for &(eid, reversed) in ring {
        let edge = brep.edge(eid);
        let (Some(s), Some(e)) = (edge.start, edge.end) else {
            return None;
        };
        let tail = if reversed { e } else { s };
        let (mut u, v) = surface_uv(surface, brep.vertex(tail).position)?;
        if let Some(&(prev_u, _)) = uv.last() {
            while u - prev_u > std::f64::consts::PI {
                u -= std::f64::consts::TAU;
            }
            while u - prev_u < -std::f64::consts::PI {
                u += std::f64::consts::TAU;
            }
        }
        uv.push((u, v));
    }
    if uv.len() < 3 {
        return None;
    }
    let mut area = 0.0;
    for i in 0..uv.len() {
        let (x0, y0) = uv[i];
        let (x1, y1) = uv[(i + 1) % uv.len()];
        area += x0 * y1 - x1 * y0;
    }
    Some(area * 0.5)
}

/// Whether a ring is a single full-revolution circle edge (no bounding
/// vertices) — the natural periodic boundary of a cylinder/cone/sphere/torus
/// wall, peeled off whole at the start of [`order_loops`].
fn is_full_revolution(brep: &BRep, ring: &[(EdgeId, bool)]) -> bool {
    ring.len() == 1 && {
        let e = brep.edge(ring[0].0);
        e.start.is_none() && e.end.is_none()
    }
}

/// Order a face's edges into one outer ring plus inner (hole) rings, with
/// reflex-correct turning-angle ordering and outer/inner classification.
///
/// The face surface provides the outward normal that orients the tangent plane;
/// `surface_normal_at` yields it per-vertex (constant for a plane). The seed
/// ring is oriented so the OUTER loop winds counter-clockwise seen from outside
/// the face (positive projected signed area), matching the STEP
/// `ADVANCED_FACE(... .T.)` convention; holes are reversed to wind the opposite
/// way. For planar faces classification is by signed-area magnitude (largest =
/// outer); for curved faces a full-revolution trim circle wins by chord extent
/// (DL-58), otherwise bounded loops are ranked by parametric (u, v) signed area.
///
/// Returns `None` only if the edges cannot be assembled into closed rings at all
/// (degenerate topology) — the caller then keeps the unordered fallback.
fn order_loops(brep: &BRep, edge_ids: &[EdgeId], surface: &SurfaceGeometry) -> Option<FaceLoops> {
    if edge_ids.is_empty() {
        return None;
    }

    // A self-closing circle edge (no bounding vertices) is its own ring; a face
    // can be bounded by one or more such circles (a cylinder wall between two
    // trim circles). Peel those off first.
    let mut rings: Vec<Vec<(EdgeId, bool)>> = Vec::new();
    let mut used = vec![false; edge_ids.len()];
    for (i, &eid) in edge_ids.iter().enumerate() {
        let edge = brep.edge(eid);
        if edge.start.is_none() && edge.end.is_none() {
            used[i] = true;
            rings.push(vec![(eid, false)]);
        }
    }

    let normal_at = |p: [f64; 3]| surface_normal_at(surface, p);

    // Order the remaining vertex-bounded edges into one or more rings.
    while let Some(seed) = (0..edge_ids.len()).find(|&i| !used[i]) {
        let ring = order_one_ring(brep, edge_ids, &mut used, seed, &normal_at)?;
        rings.push(ring);
    }

    if rings.is_empty() {
        return None;
    }

    // Determine a representative face normal (for planar faces it is constant;
    // for curved faces we just need a consistent reference for area signs).
    let rep_pos = {
        // Use the first ring's first vertex position if available.
        let mut p = None;
        'outer: for ring in &rings {
            for &(eid, _) in ring {
                let edge = brep.edge(eid);
                if let Some(s) = edge.start {
                    p = Some(brep.vertex(s).position);
                    break 'outer;
                }
            }
        }
        p
    };
    let is_planar = matches!(
        surface,
        SurfaceGeometry::Analytic { surface: HalfSpace::Plane { .. }, .. }
    );

    // Classify outer vs. inner and fix winding.
    if is_planar {
        if let Some(p) = rep_pos {
            if let Some(n) = surface_normal_at(surface, p) {
                // Pick the largest-magnitude-area ring as outer.
                let mut areas: Vec<f64> =
                    rings.iter().map(|r| ring_signed_area(brep, r, n)).collect();
                let outer_idx = (0..rings.len())
                    .max_by(|&a, &b| areas[a].abs().total_cmp(&areas[b].abs()))
                    .unwrap();
                // Outer must wind CCW (area > 0); holes must wind CW (area < 0).
                if areas[outer_idx] < 0.0 {
                    rings[outer_idx].reverse();
                    flip_orientations(&mut rings[outer_idx]);
                    areas[outer_idx] = -areas[outer_idx];
                }
                for (i, ring) in rings.iter_mut().enumerate() {
                    if i == outer_idx {
                        continue;
                    }
                    if areas[i] > 0.0 {
                        ring.reverse();
                        flip_orientations(ring);
                        areas[i] = -areas[i];
                    }
                }
                let outer = rings.remove(outer_idx);
                return Some(FaceLoops { outer, inner: rings });
            }
        }
    }

    // Curved face (or planar fallback without a usable normal). Winding is left
    // as ordered (STEP only needs connected rings here); we only pick which ring
    // is the outer bound:
    //
    // - If any ring is a full-revolution trim circle, it is the surface's
    //   natural periodic boundary and encloses every bounded sub-loop, so the
    //   outer bound is the longest such circle (DL-58 extent rule — the
    //   cylinder/cone tube + drilled-box convention). All other rings are inner.
    // - Otherwise every ring is a bounded loop: classify by parametric (u, v)
    //   signed area, the largest by magnitude being outer. This is invariant to
    //   a tapered surface's varying radius, so a physically large hole no longer
    //   out-ranks a radially small true boundary (which world-space chord length
    //   would get wrong).
    // - With no usable parameterization (tessellated / gyroid) fall back to the
    //   extent rule.
    let extent_max = |rings: &[Vec<(EdgeId, bool)>]| {
        (0..rings.len())
            .max_by(|&a, &b| ring_extent(brep, &rings[a]).total_cmp(&ring_extent(brep, &rings[b])))
            .unwrap()
    };
    let full_rev: Vec<usize> = (0..rings.len())
        .filter(|&i| is_full_revolution(brep, &rings[i]))
        .collect();
    let outer_idx = if !full_rev.is_empty() {
        *full_rev
            .iter()
            .max_by(|&&a, &&b| ring_extent(brep, &rings[a]).total_cmp(&ring_extent(brep, &rings[b])))
            .unwrap()
    } else if let Some(areas) = rings
        .iter()
        .map(|r| ring_uv_area(brep, r, surface))
        .collect::<Option<Vec<f64>>>()
    {
        (0..rings.len())
            .max_by(|&a, &b| areas[a].abs().total_cmp(&areas[b].abs()))
            .unwrap()
    } else {
        extent_max(&rings)
    };
    let outer = rings.remove(outer_idx);
    Some(FaceLoops { outer, inner: rings })
}

/// Reverse the per-entry orientation flags of a ring after reversing its order,
/// so head-to-tail connectivity is preserved.
fn flip_orientations(ring: &mut [(EdgeId, bool)]) {
    for entry in ring.iter_mut() {
        entry.1 = !entry.1;
    }
}

// ─── public entry point ─────────────────────────────────────────────────────

/// Run the full B-Rep extraction and weaving pipeline: build the wireframe
/// (C.4), fit every Hermite edge to a NURBS curve, weave one trimmed face per
/// branch, and assemble a single watertight shell + solid.
pub fn weave(body: &BoundedCsg, octree: &Octree) -> BRep {
    let mut brep = build_wireframe(body, octree);

    // (1) Upgrade every Hermite edge to NURBS. The edge-walk samples stop
    // roughly one octree cell short of each bounding corner (the corner is a
    // Vertex cell, not an Edge cell), so we anchor the chain's first/last
    // sample to the resolved start/end vertex position before fitting — the
    // NURBS then interpolates its topological vertices exactly (DL-26).
    let vertex_pos: Vec<[f64; 3]> = brep.vertices.iter().map(|v| v.position).collect();
    for edge in &mut brep.edges {
        if let EdgeGeometry::Hermite(samples) = &edge.geometry {
            // Panic-safety (DL-29): `hermite_to_nurbs` asserts >= 2
            // samples. A degenerate edge-point group can in principle
            // survive `build_wireframe` with a single sample; leave
            // such an edge as Hermite rather than feeding the
            // assertion an undersized chain and crashing on arbitrary
            // user geometry.
            if samples.len() < 2 {
                continue;
            }
            let mut samples = samples.clone();
            if let Some(VertexId(i)) = edge.start {
                samples.first_mut().unwrap().point = vertex_pos[i as usize];
            }
            if let Some(VertexId(i)) = edge.end {
                samples.last_mut().unwrap().point = vertex_pos[i as usize];
            }
            edge.geometry = EdgeGeometry::Nurbs(hermite_to_nurbs(&samples));
        }
    }

    // (2) Group edges by branch: every BranchId appearing in any signature is
    // one face. Preserve first-seen order for determinism.
    let halfspaces = branch_halfspace_map(body);
    let mut branch_order: Vec<u32> = Vec::new();
    let mut edges_by_branch: HashMap<u32, Vec<EdgeId>> = HashMap::new();
    for (i, edge) in brep.edges.iter().enumerate() {
        let eid = EdgeId(i as u32);
        for b in [edge.signature.0, edge.signature.1] {
            let key = b.as_u32();
            edges_by_branch
                .entry(key)
                .or_insert_with(|| {
                    branch_order.push(key);
                    Vec::new()
                })
                .push(eid);
        }
    }

    // Build one face per branch.
    let mut face_ids = Vec::with_capacity(branch_order.len());
    for &branch_u32 in &branch_order {
        let edge_ids = &edges_by_branch[&branch_u32];
        let branch_id = BranchId(branch_u32);

        // Analytic surface from the lookup, else a tessellated fallback (DL-25).
        let surface = match halfspaces.get(&branch_u32) {
            Some((hs, xf)) => SurfaceGeometry::Analytic {
                surface: hs.clone(),
                transform: *xf,
            },
            None => SurfaceGeometry::Tessellated {
                positions: Vec::new(),
                indices: Vec::new(),
            },
        };

        // Order the boundary into an outer ring + inner (hole) rings, reflex-
        // correct, with outer/inner classification (DL-58). If ordering fails
        // for a degenerate face, fall back to the unordered forward list ONLY
        // as a last resort (kept so the face is still recorded) — this never
        // happens on a watertight body, which is asserted via the connectivity
        // check below.
        let (outer, inner) = match order_loops(&brep, edge_ids, &surface) {
            Some(loops) => {
                let outer = Loop::new(loops.outer);
                let inner: Vec<Loop> = loops.inner.into_iter().map(Loop::new).collect();
                // Connectivity guarantee: a successfully-ordered face must be
                // made of connected rings, otherwise we'd emit invalid STEP.
                debug_assert!(
                    outer.is_connected_ring(&brep.edges)
                        && inner.iter().all(|lp| lp.is_connected_ring(&brep.edges)),
                    "ordered face loops for branch {branch_u32} must be connected rings"
                );
                (outer, inner)
            }
            None => {
                // Last-resort fallback for degenerate topology (e.g. a face
                // whose boundary is incomplete because Phase-3 extraction
                // dropped a vertex/edge at a reflex region). Recorded loudly so
                // it is never a silent invalid-STEP path. Watertight bodies
                // built from complete topology never reach here.
                eprintln!(
                    "weave: WARNING could not order face loops for branch \
                     {branch_u32}; emitting unordered fallback loop (degenerate \
                     topology)"
                );
                (
                    Loop::new(edge_ids.iter().map(|&e| (e, false)).collect()),
                    Vec::new(),
                )
            }
        };

        let fid = brep.add_face(Face {
            branch_id,
            surface,
            outer,
            inner,
        });
        face_ids.push(fid);
    }

    // (2b) Edgeless closed analytic faces (SA-6). A *lone* closed analytic
    // surface — a full sphere — has ONE branch, no branch collisions, hence no
    // edge cells: it never appears in any edge signature, so the loop above
    // emits zero faces. Such a surface is a single face bounded by NOTHING (an
    // empty outer loop) on a closed shell. We materialise it only when the body
    // produced no edges at all: with edges present, an analytic branch missing
    // from `edges_by_branch` would be a *degenerate* (unbounded / clipped-away)
    // face, not a genuinely closed one — those stay out (DL-54).
    if brep.edges.is_empty() {
        for (&branch_u32, (hs, xf)) in &halfspaces {
            if edges_by_branch.contains_key(&branch_u32) {
                continue;
            }
            let fid = brep.add_face(Face {
                branch_id: BranchId(branch_u32),
                surface: SurfaceGeometry::Analytic {
                    surface: hs.clone(),
                    transform: *xf,
                },
                outer: Loop::new(Vec::new()),
                inner: Vec::new(),
            });
            face_ids.push(fid);
        }
    }

    // (3) Shell + solid. Watertight iff every edge is used by exactly two face
    // loops (DL-24) — or, for an edgeless body, iff it is a single closed
    // analytic face (a lone sphere: one face, no edges, closed shell — SA-6).
    let is_closed = if brep.edges.is_empty() {
        face_ids.len() == 1
    } else {
        is_watertight(&brep, &face_ids)
    };
    let shell = brep.add_shell(Shell::new(face_ids, is_closed));
    brep.add_solid(Solid::new(shell, Vec::new()));

    brep
}

/// Manifold / watertight test: every edge referenced by a face's outer loop
/// (faces are single-loop here) must be used by **exactly two** loops. See
/// DL-24.
fn is_watertight(brep: &BRep, face_ids: &[crate::brep::FaceId]) -> bool {
    if brep.edges.is_empty() {
        return false;
    }
    let mut uses = vec![0u32; brep.edges.len()];
    for &fid in face_ids {
        let face = brep.face(fid);
        for &(EdgeId(idx), _) in &face.outer.edges {
            uses[idx as usize] += 1;
        }
        for lp in &face.inner {
            for &(EdgeId(idx), _) in &lp.edges {
                uses[idx as usize] += 1;
            }
        }
    }
    uses.iter().all(|&u| u == 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::CsgNode;
    use fielddraft_protocol::{FeatureId, Primitive};
    use std::collections::HashSet;

    fn box_body(half: [f32; 3]) -> BoundedCsg {
        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: half,
            },
        };
        BoundedCsg::from_csg(&csg)
    }

    fn close(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
        norm(sub(a, b)) <= tol
    }

    // ── hermite_to_nurbs unit tests ──────────────────────────────────────

    #[test]
    fn nurbs_straight_segment_is_collinear() {
        // Two samples on a straight line along +x → one cubic Bézier that is
        // geometrically a line: 4 control points, all collinear, endpoints at
        // the samples.
        let samples = vec![
            HermiteSample {
                point: [0.0, 0.0, 0.0],
                tangent: [1.0, 0.0, 0.0],
            },
            HermiteSample {
                point: [3.0, 0.0, 0.0],
                tangent: [1.0, 0.0, 0.0],
            },
        ];
        let curve = hermite_to_nurbs(&samples);
        assert_eq!(curve.degree, 3);
        assert_eq!(curve.control_points.len(), 4);
        assert_eq!(curve.weights.len(), curve.control_points.len());
        assert_eq!(
            curve.knots.len(),
            curve.control_points.len() + curve.degree + 1
        );
        // Knot vector clamped: [0,0,0,0,1,1,1,1].
        assert_eq!(curve.knots, vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
        // Endpoints coincide with the samples.
        assert!(close(curve.control_points[0], [0.0, 0.0, 0.0], 1e-12));
        assert!(close(curve.control_points[3], [3.0, 0.0, 0.0], 1e-12));
        // All control points collinear (y == z == 0, x monotone).
        for cp in &curve.control_points {
            assert!(
                cp[1].abs() < 1e-12 && cp[2].abs() < 1e-12,
                "off the line: {cp:?}"
            );
        }
        // Interior control points are the 1/3, 2/3 thirds for a straight chord.
        assert!(close(curve.control_points[1], [1.0, 0.0, 0.0], 1e-9));
        assert!(close(curve.control_points[2], [2.0, 0.0, 0.0], 1e-9));
    }

    #[test]
    fn nurbs_two_segment_chain_has_c0_join() {
        // An L-shaped 3-sample chain → 2 cubic Béziers, 7 control points,
        // C0 join (the shared endpoint stored once), endpoints preserved.
        let samples = vec![
            HermiteSample {
                point: [0.0, 0.0, 0.0],
                tangent: [1.0, 0.0, 0.0],
            },
            HermiteSample {
                point: [1.0, 0.0, 0.0],
                tangent: [0.0, 1.0, 0.0],
            },
            HermiteSample {
                point: [1.0, 1.0, 0.0],
                tangent: [0.0, 1.0, 0.0],
            },
        ];
        let curve = hermite_to_nurbs(&samples);
        assert_eq!(curve.degree, 3);
        // 3·n_seg + 1 = 3·2 + 1 = 7 control points.
        assert_eq!(curve.control_points.len(), 7);
        assert_eq!(curve.weights.len(), 7);
        // Knots: end mult 4 + interior mult 3 = [0,0,0,0,1,1,1,2,2,2,2].
        assert_eq!(
            curve.knots,
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]
        );
        assert_eq!(
            curve.knots.len(),
            curve.control_points.len() + curve.degree + 1
        );
        // Endpoints preserved.
        assert!(close(curve.control_points[0], [0.0, 0.0, 0.0], 1e-12));
        assert!(close(
            *curve.control_points.last().unwrap(),
            [1.0, 1.0, 0.0],
            1e-12
        ));
        // C0 join: the shared point (index 3) sits exactly on the corner.
        assert!(close(curve.control_points[3], [1.0, 0.0, 0.0], 1e-12));
        // Tangent direction preserved at the start: b1 − b0 ∝ +x.
        let d0 = sub(curve.control_points[1], curve.control_points[0]);
        assert!(d0[0] > 0.0 && d0[1].abs() < 1e-12 && d0[2].abs() < 1e-12);
        // Tangent direction preserved at the end: b3 − b2 ∝ +y.
        let d1 = sub(curve.control_points[6], curve.control_points[5]);
        assert!(d1[1] > 0.0 && d1[0].abs() < 1e-12 && d1[2].abs() < 1e-12);
    }

    // ── loop ordering unit test ──────────────────────────────────────────

    #[test]
    fn order_loop_walks_four_cube_face_edges_into_a_cycle() {
        // Build a tiny BRep with one square face's 4 edges (a unit square in
        // the z=0 plane), deliberately added in a scrambled order and with
        // mixed start/end directions, then check order_loop returns a single
        // continuous 4-edge cycle.
        use crate::brep::{Edge, EdgeGeometry, Vertex};
        let mut brep = BRep::new();
        let a = BranchId(1 << 4);
        let b = BranchId(2 << 4);
        let v0 = brep.add_vertex(Vertex::new([0.0, 0.0, 0.0], vec![a, b]));
        let v1 = brep.add_vertex(Vertex::new([1.0, 0.0, 0.0], vec![a, b]));
        let v2 = brep.add_vertex(Vertex::new([1.0, 1.0, 0.0], vec![a, b]));
        let v3 = brep.add_vertex(Vertex::new([0.0, 1.0, 0.0], vec![a, b]));
        let mk = |brep: &mut BRep, s: VertexId, e: VertexId| {
            brep.add_edge(Edge::new(
                EdgeGeometry::Polyline(vec![]),
                a,
                b,
                Some(s),
                Some(e),
            ))
        };
        // Scrambled order; e2 reversed relative to the walk.
        let e0 = mk(&mut brep, v0, v1);
        let e2 = mk(&mut brep, v2, v3); // reversed vs CCW v3→v2
        let e3 = mk(&mut brep, v0, v3); // reversed vs CCW v3→v0... handled by flag
        let e1 = mk(&mut brep, v1, v2);

        // Planar face in the z=0 plane, outward normal +z.
        let surface = SurfaceGeometry::Analytic {
            surface: HalfSpace::Plane { normal: [0.0, 0.0, 1.0], offset: 0.0 },
            transform: Transform::IDENTITY,
        };
        let loops = order_loops(&brep, &[e0, e2, e3, e1], &surface)
            .expect("should close into a cycle");
        assert!(loops.inner.is_empty(), "a single square has no holes");
        let ring = loops.outer;
        assert_eq!(ring.len(), 4);

        // The ordered ring is a connected, closed loop.
        assert!(
            Loop::new(ring.clone()).is_connected_ring(&brep.edges),
            "loop must be a connected ring"
        );

        // The outer loop winds CCW seen from +z (positive signed area).
        let area = ring_signed_area(&brep, &ring, [0.0, 0.0, 1.0]);
        assert!(area > 0.0, "outer loop must wind CCW (area {area} > 0)");
    }

    // ── primary acceptance: 2×2×2 box → watertight 6/12/8 B-Rep ──────────

    #[test]
    fn cube_weaves_watertight_brep() {
        let body = box_body([1.0, 1.0, 1.0]);
        let octree = Octree::build(&body, 0.25);
        let brep = weave(&body, &octree);

        assert_eq!(brep.vertex_count(), 8, "cube has 8 corners");
        assert_eq!(brep.edge_count(), 12, "cube has 12 edges");
        assert_eq!(brep.face_count(), 6, "cube has 6 faces");
        assert_eq!(brep.shell_count(), 1);
        assert_eq!(brep.solid_count(), 1);
        assert_eq!(brep.validate(), Ok(()), "woven B-Rep must be well-formed");

        // Single shell is closed / watertight.
        let solid = brep.solid(crate::brep::SolidId(0));
        let shell = brep.shell(solid.outer);
        assert!(shell.is_closed, "cube shell must be watertight");
        assert_eq!(shell.faces.len(), 6);

        // Every edge referenced by exactly two face loops.
        let mut uses = vec![0u32; brep.edge_count()];
        for f in &brep.faces {
            for &(EdgeId(idx), _) in &f.outer.edges {
                uses[idx as usize] += 1;
            }
        }
        assert!(
            uses.iter().all(|&u| u == 2),
            "every edge used exactly twice"
        );

        // Each face: analytic plane, 4-edge continuous outer cycle.
        for f in &brep.faces {
            assert!(
                matches!(
                    f.surface,
                    SurfaceGeometry::Analytic {
                        surface: HalfSpace::Plane { .. },
                        ..
                    }
                ),
                "cube face must be an analytic plane, got {:?}",
                f.surface
            );
            assert!(f.inner.is_empty());
            assert_eq!(f.outer.edges.len(), 4, "cube face has a 4-edge loop");

            // Continuous vertex cycle.
            let walk_end = |(eid, rev): (EdgeId, bool)| -> (VertexId, VertexId) {
                let e = brep.edge(eid);
                if rev {
                    (e.end.unwrap(), e.start.unwrap())
                } else {
                    (e.start.unwrap(), e.end.unwrap())
                }
            };
            for w in f.outer.edges.windows(2) {
                let (_, end0) = walk_end(w[0]);
                let (start1, _) = walk_end(w[1]);
                assert_eq!(end0, start1, "face loop not continuous");
            }
            let (first_start, _) = walk_end(f.outer.edges[0]);
            let (_, last_end) = walk_end(*f.outer.edges.last().unwrap());
            assert_eq!(last_end, first_start, "face loop does not close");
        }

        // Every edge is a degree-3 NURBS with valid lengths and endpoints
        // coincident with its start/end vertices.
        for e in &brep.edges {
            let curve = match &e.geometry {
                EdgeGeometry::Nurbs(c) => c,
                other => panic!("expected NURBS edge, got {other:?}"),
            };
            assert_eq!(curve.degree, 3);
            assert_eq!(curve.weights.len(), curve.control_points.len());
            assert_eq!(
                curve.knots.len(),
                curve.control_points.len() + curve.degree + 1
            );
            let vs = brep.vertex(e.start.unwrap()).position;
            let ve = brep.vertex(e.end.unwrap()).position;
            assert!(
                close(curve.control_points[0], vs, 1e-9),
                "NURBS start {:?} != vertex {vs:?}",
                curve.control_points[0]
            );
            assert!(
                close(*curve.control_points.last().unwrap(), ve, 1e-9),
                "NURBS end {:?} != vertex {ve:?}",
                curve.control_points.last().unwrap()
            );
        }

        // Six distinct branch faces.
        let branches: HashSet<u32> = brep.faces.iter().map(|f| f.branch_id.as_u32()).collect();
        assert_eq!(branches.len(), 6, "six distinct cube-face branches");
    }

    fn sphere_body(center: [f32; 3], radius: f32) -> BoundedCsg {
        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Sphere { center, radius },
        };
        BoundedCsg::from_csg(&csg)
    }

    // ── SA-6: lone sphere → ONE analytic HalfSpace::Sphere face ───────────

    #[test]
    fn sphere_weaves_single_analytic_spherical_face() {
        // A lone sphere is one closed analytic surface: 1 face, 0 edges,
        // 0 vertices, a closed shell. The weaver must emit exactly that —
        // an analytic HalfSpace::Sphere face with an empty outer loop —
        // NOT a tessellated fallback and NOT an empty B-Rep.
        let body = sphere_body([0.0, 0.0, 0.0], 1.5);
        let octree = Octree::build(&body, 0.2);
        let brep = weave(&body, &octree);

        assert_eq!(brep.validate(), Ok(()), "sphere B-Rep must be well-formed");
        assert_eq!(brep.edge_count(), 0, "a lone sphere has no edges");
        assert_eq!(brep.vertex_count(), 0, "a lone sphere has no vertices");
        assert_eq!(brep.face_count(), 1, "a lone sphere is a single face");
        assert_eq!(brep.shell_count(), 1);
        assert_eq!(brep.solid_count(), 1);

        let solid = brep.solid(crate::brep::SolidId(0));
        let shell = brep.shell(solid.outer);
        assert!(shell.is_closed, "the lone-sphere shell is closed");
        assert_eq!(shell.faces.len(), 1);

        let f = &brep.faces[0];
        assert!(
            f.outer.edges.is_empty(),
            "a fully closed sphere face has no bounding edges"
        );
        assert!(f.inner.is_empty());
        match &f.surface {
            SurfaceGeometry::Analytic {
                surface: HalfSpace::Sphere { radius },
                ..
            } => assert!((*radius - 1.5).abs() < 1e-6, "sphere radius preserved"),
            other => panic!("sphere face must be analytic Sphere, got {other:?}"),
        }
    }

    #[test]
    fn offset_sphere_folds_center_into_face_transform() {
        // An off-origin sphere keeps a single analytic face; its centre folds
        // into the face transform (DL-20), so the HalfSpace stays centred.
        let body = sphere_body([2.0, -1.0, 0.5], 1.0);
        let map = branch_halfspace_map(&body);
        assert_eq!(map.len(), 1, "a sphere has exactly one analytic face");
        let (hs, xf) = map.values().next().unwrap();
        assert!(matches!(hs, HalfSpace::Sphere { radius } if (*radius - 1.0).abs() < 1e-6));
        assert_eq!(xf.translate, [2.0, -1.0, 0.5], "centre folded into transform");
    }

    #[test]
    fn torus_weaves_single_analytic_toroidal_face() {
        // A lone torus has the same edgeless-closed-surface topology as a
        // sphere: 1 analytic HalfSpace::Torus face, 0 edges, 0 vertices,
        // closed shell (SA-6).
        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Torus {
                center: [0.0, 0.0, 0.0],
                major_radius: 2.0,
                minor_radius: 0.5,
            },
        };
        let body = BoundedCsg::from_csg(&csg);
        let octree = Octree::build(&body, 0.2);
        let brep = weave(&body, &octree);

        assert_eq!(brep.validate(), Ok(()), "torus B-Rep must be well-formed");
        assert_eq!(brep.edge_count(), 0, "a lone torus has no edges");
        assert_eq!(brep.vertex_count(), 0, "a lone torus has no vertices");
        assert_eq!(brep.face_count(), 1, "a lone torus is a single face");
        let solid = brep.solid(crate::brep::SolidId(0));
        assert!(brep.shell(solid.outer).is_closed, "torus shell is closed");
        match &brep.faces[0].surface {
            SurfaceGeometry::Analytic {
                surface: HalfSpace::Torus { major_radius, minor_radius },
                ..
            } => {
                assert!((*major_radius - 2.0).abs() < 1e-6);
                assert!((*minor_radius - 0.5).abs() < 1e-6);
            }
            other => panic!("torus face must be analytic Torus, got {other:?}"),
        }
    }

    #[test]
    fn branch_halfspace_map_has_six_cube_planes() {
        let body = box_body([1.0, 1.0, 1.0]);
        let map = branch_halfspace_map(&body);
        assert_eq!(map.len(), 6, "a box has 6 analytic plane sub-faces");
        for (hs, xf) in map.values() {
            assert!(matches!(hs, HalfSpace::Plane { .. }));
            // Centered box → identity transform (center [0,0,0]).
            assert_eq!(xf.translate, [0.0, 0.0, 0.0]);
        }
    }

    // ── DL-50: analytic Extrude / Revolve faces ──────────────────────────

    use crate::sketch2d::SketchSegment;
    use fielddraft_protocol::Sketch2d as ProtoSketch2d;
    use fielddraft_protocol::Sketch2dPrimitive as ProtoSketch2dPrim;

    /// Build a `BoundedCsg::Extrude` body directly (mirrors `box_body`).
    fn extrude_body(sketch: ProtoSketch2d, half_height: f32) -> BoundedCsg {
        let csg = CsgNode::Extrude {
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
            half_height,
        };
        BoundedCsg::from_csg(&csg)
    }

    fn revolve_body(sketch: ProtoSketch2d) -> BoundedCsg {
        let csg = CsgNode::Revolve {
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
        };
        BoundedCsg::from_csg(&csg)
    }

    fn assert_watertight_solid(brep: &BRep) {
        assert_eq!(brep.validate(), Ok(()), "woven B-Rep must be well-formed");
        assert_eq!(brep.shell_count(), 1);
        assert_eq!(brep.solid_count(), 1);
        let solid = brep.solid(crate::brep::SolidId(0));
        let shell = brep.shell(solid.outer);
        assert!(shell.is_closed, "shell must be watertight");
        let mut uses = vec![0u32; brep.edge_count()];
        for f in &brep.faces {
            for &(EdgeId(idx), _) in &f.outer.edges {
                uses[idx as usize] += 1;
            }
            for lp in &f.inner {
                for &(EdgeId(idx), _) in &lp.edges {
                    uses[idx as usize] += 1;
                }
            }
        }
        assert!(uses.iter().all(|&u| u == 2), "every edge used exactly twice");
    }

    fn is_analytic_plane(f: &Face) -> bool {
        matches!(
            f.surface,
            SurfaceGeometry::Analytic { surface: HalfSpace::Plane { .. }, .. }
        )
    }

    #[test]
    fn rectangle_extrude_weaves_watertight_box_six_planes() {
        // A 2×2 rectangle profile extruded ±1 → a 2×2×2 box: 8 V, 12 E,
        // 6 F, all analytic planes (4 walls + 2 caps).
        let sketch = ProtoSketch2d::Prim(ProtoSketch2dPrim::Rectangle {
            center: [0.0, 0.0],
            half_extents: [1.0, 1.0],
        });
        let body = extrude_body(sketch, 1.0);
        let octree = Octree::build(&body, 0.25);
        let brep = weave(&body, &octree);

        println!(
            "rectangle_extrude: V={} E={} F={}",
            brep.vertex_count(),
            brep.edge_count(),
            brep.face_count()
        );
        assert_eq!(brep.vertex_count(), 8, "rectangle extrude has 8 corners");
        assert_eq!(brep.edge_count(), 12, "rectangle extrude has 12 edges");
        assert_eq!(brep.face_count(), 6, "4 walls + 2 caps");
        assert_watertight_solid(&brep);
        for f in &brep.faces {
            assert!(
                is_analytic_plane(f),
                "every rectangle-extrude face is an analytic plane, got {:?}",
                f.surface
            );
        }
    }

    #[test]
    fn l_polygon_extrude_weaves_watertight_eight_planes() {
        // 6-vertex L profile (CCW) extruded ±1 → an L-prism: 6 walls + 2
        // caps = 8 analytic planes, watertight.
        let l = ProtoSketch2d::Prim(ProtoSketch2dPrim::Polygon {
            vertices: vec![
                [0.0, 0.0],
                [2.0, 0.0],
                [2.0, 1.0],
                [1.0, 1.0],
                [1.0, 2.0],
                [0.0, 2.0],
            ],
        });
        let body = extrude_body(l, 1.0);
        let octree = Octree::build(&body, 0.1);
        let brep = weave(&body, &octree);

        println!(
            "l_polygon_extrude: V={} E={} F={}",
            brep.vertex_count(),
            brep.edge_count(),
            brep.face_count()
        );
        // DL-51: the per-signature global line fit collapses the (resolution-
        // dependent) fragmented edge runs into exactly one edge per signature:
        // 6 vertical wall–wall edges + 6 top wall–cap + 6 bottom wall–cap = 18.
        assert_eq!(brep.edge_count(), 18, "6 vertical + 6 top + 6 bottom edges");
        assert_eq!(brep.face_count(), 8, "6 walls + 2 caps");
        assert_watertight_solid(&brep);

        // Vertex count: an L-prism has 12 ideal corners (6 polygon corners ×
        // top/bottom). The octree never classifies a Vertex cell at the single
        // *reflex* corner (1,1) — the three-branch collision opens past 180° and
        // is mis-read as a 2-branch Edge cell at this resolution. The reflex-
        // vertex reconstruction post-pass (DL-60) recovers the two missing
        // corners (top + bottom of the reflex edge) analytically from the three
        // incident edges, so the full 12 corners are present and `order_loops`
        // no longer needs its unordered fallback for the touching faces.
        assert_eq!(
            brep.vertex_count(),
            12,
            "all 12 corners, incl. the reflex corner (1,1) recovered by DL-60"
        );

        // The two recovered corners sit exactly at the reflex edge's ends
        // (1,1,±1) — proof the reconstruction solves the analytic plane
        // intersection, not merely the (cell-short) dangling-endpoint centroid.
        for z in [-1.0, 1.0] {
            let found = brep.vertices.iter().any(|v| {
                (v.position[0] - 1.0).abs() < 1e-3
                    && (v.position[1] - 1.0).abs() < 1e-3
                    && (v.position[2] - z).abs() < 1e-3
            });
            assert!(found, "reflex corner (1,1,{z}) must be recovered analytically");
        }

        // Every edge referenced by exactly two faces, all 8 faces analytic
        // planes (4+2 from the 6 walls split into the L's straight runs + 2
        // caps). Each edge appears in exactly two face loops. With all 12
        // corners present every face now orders into a proper ring — no
        // `order_loops` unordered fallback is emitted (DL-60).
        for f in &brep.faces {
            assert!(
                is_analytic_plane(f),
                "every L-prism face is an analytic plane, got {:?}",
                f.surface
            );
        }
    }

    #[test]
    fn revolve_rectangle_profile_is_tube_with_analytic_sides() {
        // A rectangle profile offset from the axis (r ∈ [3,5], y ∈ [-1,1])
        // revolved about y → a tube/washer: inner + outer cylinder walls,
        // top + bottom annular planes. All four side faces analytic.
        let profile = ProtoSketch2d::Prim(ProtoSketch2dPrim::Rectangle {
            center: [4.0, 0.0],
            half_extents: [1.0, 1.0],
        });
        let body = revolve_body(profile);
        let octree = Octree::build(&body, 0.15);
        let brep = weave(&body, &octree);

        println!(
            "revolve_tube: V={} E={} F={}",
            brep.vertex_count(),
            brep.edge_count(),
            brep.face_count()
        );
        // DL-51: each of the 4 rectangle-profile groups revolves into ONE
        // closed circular face-dividing edge (the per-group circle fit
        // collapses the thousands of fragments into a single loop). The 4
        // edges are the two cylinder rims meeting the two annular planes.
        assert_eq!(brep.edge_count(), 4, "4 circular face-dividing edges");
        assert_eq!(brep.face_count(), 4, "2 cylinders + 2 planes");
        // The circular edges are *closed loops* (start = end = None): a full
        // circle has no bounding vertex, and Phase-3 extracts no Vertex cell on
        // a smooth tube (no ≥3-branch corner). So V = 0 by construction.
        assert_eq!(
            brep.vertex_count(),
            0,
            "closed circular edges have no bounding vertices"
        );
        for e in &brep.edges {
            assert!(
                e.start.is_none() && e.end.is_none(),
                "circular face-divider must be a closed loop (no vertices)"
            );
        }
        assert_watertight_solid(&brep);
        // The 4 rectangle groups revolve into: 2 cylinders (constant r)
        // + 2 planes (constant y). None may be tessellated.
        let mut cylinders = 0;
        let mut planes = 0;
        for f in &brep.faces {
            match &f.surface {
                SurfaceGeometry::Analytic {
                    surface: HalfSpace::InfiniteCylinder { .. }, ..
                } => cylinders += 1,
                SurfaceGeometry::Analytic {
                    surface: HalfSpace::Plane { .. }, ..
                } => planes += 1,
                other => panic!("revolve side face must be analytic, got {other:?}"),
            }
        }
        assert_eq!(cylinders, 2, "inner + outer wall cylinders");
        assert_eq!(planes, 2, "top + bottom annular planes");
    }

    #[test]
    fn extrude_caps_use_reserved_sub_ids() {
        // The cap branch sub-ids must be the reserved 0xFFF / 0xFFE.
        let sketch = ProtoSketch2d::Prim(ProtoSketch2dPrim::Rectangle {
            center: [0.0, 0.0],
            half_extents: [1.0, 1.0],
        });
        let body = extrude_body(sketch, 1.0);
        let map = branch_halfspace_map(&body);
        let top = BranchId::from_feature_sub(&FeatureId::from_u32(1), BranchId::EXTRUDE_CAP_TOP_SUB);
        let bottom =
            BranchId::from_feature_sub(&FeatureId::from_u32(1), BranchId::EXTRUDE_CAP_BOTTOM_SUB);
        assert!(map.contains_key(&top.as_u32()), "top cap present");
        assert!(map.contains_key(&bottom.as_u32()), "bottom cap present");
        // 4 walls + 2 caps.
        assert_eq!(map.len(), 6);
    }

    #[test]
    fn curved_path_extrude_falls_back_to_tessellated() {
        // A closed path with a quad-bézier group stays tessellated for
        // its curved wall (DL-50 documented limit). Caps remain analytic.
        let path = ProtoSketch2d::Prim(ProtoSketch2dPrim::Path {
            segments: vec![
                SketchSegment::Line { start: [-1.0, -1.0], end: [1.0, -1.0] },
                SketchSegment::QuadBezier { p0: [1.0, -1.0], p1: [2.0, 0.0], p2: [1.0, 1.0] },
                SketchSegment::Line { start: [1.0, 1.0], end: [-1.0, 1.0] },
                SketchSegment::Line { start: [-1.0, 1.0], end: [-1.0, -1.0] },
            ],
            closed: true,
        });
        let body = extrude_body(path, 1.0);
        let map = branch_halfspace_map(&body);
        // Only the 2 caps are analytic; the path walls are not enumerated.
        assert_eq!(map.len(), 2, "only caps analytic for a curved path");
        let top = BranchId::from_feature_sub(&FeatureId::from_u32(1), BranchId::EXTRUDE_CAP_TOP_SUB);
        assert!(map.contains_key(&top.as_u32()));
    }

    // ── DL-52: within-body booleans (Difference / Intersect) ─────────────

    /// Count edges by closed-loop status (start == end == None) and the number
    /// of edges whose signature contains `branch`.
    fn count_loops_for_branch(brep: &BRep, branch: u32) -> (usize, usize) {
        let mut loops = 0;
        let mut total = 0;
        for e in &brep.edges {
            if e.signature.0.as_u32() == branch || e.signature.1.as_u32() == branch {
                total += 1;
                if e.start.is_none() && e.end.is_none() {
                    loops += 1;
                }
            }
        }
        (loops, total)
    }

    #[test]
    fn difference_box_minus_notch_weaves_watertight() {
        // A 2×2×2 box with a rectangular corner notch cut out: a purely
        // planar Difference. Must stay watertight; no regression from DL-52.
        let outer = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [1.0, 1.0, 1.0] },
        };
        let notch = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Box { center: [0.6, 0.6, 0.0], half_extents: [0.6, 0.6, 2.0] },
        };
        let body = BoundedCsg::from_csg(&CsgNode::Difference(Box::new(outer), Box::new(notch)));
        let octree = Octree::build(&body, 0.25);
        let brep = weave(&body, &octree);

        println!(
            "diff_notch: V={} E={} F={}",
            brep.vertex_count(),
            brep.edge_count(),
            brep.face_count()
        );
        assert_watertight_solid(&brep);
        // The notched box is an L-prism cross-section extruded in z: 8 side
        // walls + 2 caps = 10 faces; 16 straight edges. All edges consolidate
        // to one per signature (DL-51), no over-segmentation.
        assert_eq!(brep.face_count(), 8, "notched box faces");
        assert_eq!(brep.edge_count(), 16, "notched box edges (one per signature)");
        // A planar Difference has no *closed-loop* edges: every edge is a
        // straight segment between two corners (some endpoints may stay
        // unsnapped where Phase-3 misses a vertex cell, but no edge is a full
        // start = end = None loop). This guards against a circle fit wrongly
        // firing on planar fragments.
        for e in &brep.edges {
            assert!(
                e.start.is_some() || e.end.is_some(),
                "planar Difference edge must touch at least one corner, not be a loop"
            );
        }
    }

    #[test]
    fn difference_box_minus_cylindrical_drill_consolidates_trim_circles() {
        // A 2×2×2 box drilled through by a cylinder (axis = Y, radius 0.5)
        // that pierces the +y and −y faces. The two trim circles where the
        // bore wall meets those faces must each consolidate to ONE clean
        // circular (closed-loop) edge — not the 100+ fragments the raw
        // octree edge cloud produces (DL-52). Watertight throughout.
        let outer = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [1.0, 1.0, 1.0] },
        };
        let drill = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Cylinder {
                center: [0.0, 0.0, 0.0],
                radius: 0.5,
                half_height: 2.0,
            },
        };
        let body = BoundedCsg::from_csg(&CsgNode::Difference(Box::new(outer), Box::new(drill)));
        let octree = Octree::build(&body, 0.25);
        let brep = weave(&body, &octree);

        println!(
            "diff_drill: V={} E={} F={}",
            brep.vertex_count(),
            brep.edge_count(),
            brep.face_count()
        );
        assert_watertight_solid(&brep);
        // 6 box faces + 1 cylinder-wall face.
        assert_eq!(brep.face_count(), 7, "6 box faces + bore wall");
        // 12 box edges + 2 trim circles.
        assert_eq!(brep.vertex_count(), 8, "the 8 box corners");
        assert_eq!(
            brep.edge_count(),
            14,
            "12 box edges + 2 consolidated trim circles"
        );
        // The bore-wall branch is the cylinder sub-0 (InfiniteCylinder). Its
        // boundary is exactly TWO closed-loop trim circles, no fragments.
        let wall = BranchId::from_feature_sub(&FeatureId::from_u32(2), 0).as_u32();
        let (loops, total) = count_loops_for_branch(&brep, wall);
        assert_eq!(total, 2, "bore wall bounded by exactly 2 edges");
        assert_eq!(loops, 2, "both trim edges are closed circular loops");
    }

    #[test]
    fn intersect_two_boxes_weaves_watertight_overlap_box() {
        // Two unit-half boxes offset by (0.5, 0.5, 0.5): their Intersection is
        // the overlap box [−0.5..1]³-ish region — a watertight 6-face box.
        let a = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [1.0, 1.0, 1.0] },
        };
        let b = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Box { center: [0.5, 0.5, 0.5], half_extents: [1.0, 1.0, 1.0] },
        };
        let body = BoundedCsg::from_csg(&CsgNode::Intersection(Box::new(a), Box::new(b)));
        let octree = Octree::build(&body, 0.25);
        let brep = weave(&body, &octree);

        println!(
            "intersect: V={} E={} F={}",
            brep.vertex_count(),
            brep.edge_count(),
            brep.face_count()
        );
        assert_watertight_solid(&brep);
        assert_eq!(brep.vertex_count(), 8, "overlap box has 8 corners");
        assert_eq!(brep.edge_count(), 12, "overlap box has 12 edges");
        assert_eq!(brep.face_count(), 6, "overlap box has 6 faces");
        for e in &brep.edges {
            assert!(
                e.start.is_some() && e.end.is_some(),
                "overlap box has no closed-loop edges"
            );
        }
    }

    // ── #22: reflex-aware ring ordering + multi-loop (outer + holes) ─────

    /// Hand-build a planar face's edge set with explicit vertex positions and
    /// scrambled edge directions, then return the `BRep` and edge ids. Each
    /// edge gets a degree-3 NURBS curve interpolating its endpoints so
    /// `surface_normal_at` / `outgoing_tangent` see real geometry.
    fn planar_face_edges(
        verts: &[[f64; 3]],
        edges: &[(usize, usize)],
    ) -> (BRep, Vec<EdgeId>) {
        use crate::brep::{Edge, Vertex};
        let mut brep = BRep::new();
        let a = BranchId(1 << 4);
        let b = BranchId(2 << 4);
        let vids: Vec<VertexId> = verts
            .iter()
            .map(|p| brep.add_vertex(Vertex::new(*p, vec![a, b])))
            .collect();
        let eids = edges
            .iter()
            .map(|&(s, e)| {
                let ps = verts[s];
                let pe = verts[e];
                // Straight NURBS (degree 3) between the two endpoints.
                let curve = hermite_to_nurbs(&[
                    HermiteSample { point: ps, tangent: unit(sub(pe, ps)) },
                    HermiteSample { point: pe, tangent: unit(sub(pe, ps)) },
                ]);
                brep.add_edge(Edge::new(
                    EdgeGeometry::Nurbs(curve),
                    a,
                    b,
                    Some(vids[s]),
                    Some(vids[e]),
                ))
            })
            .collect();
        (brep, eids)
    }

    fn plane_xy() -> SurfaceGeometry {
        SurfaceGeometry::Analytic {
            surface: HalfSpace::Plane { normal: [0.0, 0.0, 1.0], offset: 0.0 },
            transform: Transform::IDENTITY,
        }
    }

    /// A cone opening along the local +Y axis (apex at the origin), used to test
    /// outer/inner classification on a tapered curved surface.
    fn cone_surface() -> SurfaceGeometry {
        SurfaceGeometry::Analytic {
            surface: HalfSpace::Cone { half_angle: 0.05_f32.atan() },
            transform: Transform::IDENTITY,
        }
    }

    /// Build a BRep whose edges form the given closed rings (each an ordered
    /// vertex-position list), one straight NURBS edge between consecutive
    /// vertices. The rings are disjoint (share no vertices), so `order_loops`
    /// recovers them independently — what we then exercise is its outer/inner
    /// classification. Returns the BRep and the flat edge-id list.
    fn curved_face_rings(rings: &[Vec<[f64; 3]>]) -> (BRep, Vec<EdgeId>) {
        use crate::brep::{Edge, Vertex};
        let mut brep = BRep::new();
        let a = BranchId(1 << 4);
        let b = BranchId(2 << 4);
        let mut eids = Vec::new();
        for ring in rings {
            let vids: Vec<VertexId> = ring
                .iter()
                .map(|p| brep.add_vertex(Vertex::new(*p, vec![a, b])))
                .collect();
            let n = ring.len();
            for i in 0..n {
                let (ps, pe) = (ring[i], ring[(i + 1) % n]);
                let curve = hermite_to_nurbs(&[
                    HermiteSample { point: ps, tangent: unit(sub(pe, ps)) },
                    HermiteSample { point: pe, tangent: unit(sub(pe, ps)) },
                ]);
                eids.push(brep.add_edge(Edge::new(
                    EdgeGeometry::Nurbs(curve),
                    a,
                    b,
                    Some(vids[i]),
                    Some(vids[(i + 1) % n]),
                )));
            }
        }
        (brep, eids)
    }

    #[test]
    fn order_loops_tapered_cone_classifies_by_parametric_area_not_chord_length() {
        // THE TAPERED-TRIM CASE the chord-length proxy gets wrong (#8). On a
        // cone (radius ≈ 0.05·y, apex at origin) we place two disjoint bounded
        // loops:
        //   • OUTER — near the apex (radius ≈ 0.05, tiny chords) but spanning a
        //     LARGE parameter region: 5 rad of angle × 0.2 of axial height.
        //   • HOLE  — far out (radius ≈ 2.5, large chords) but a TINY parameter
        //     region: 0.2 × 0.2.
        // World-space chord length ranks the physically large hole above the
        // radially small boundary — so the old extent rule would pick the hole
        // as the outer bound. Parametric (u,v) area ranks them correctly.
        let k = 0.05; // radius / axial-height ⇒ cone half-angle
        let pt = |u: f64, v: f64| {
            let r = k * v;
            [r * u.cos(), v, r * u.sin()]
        };

        // Outer boundary: bottom rim u = 0..=5 at v = 1.0, then top rim back at
        // v = 1.2 — a closed 12-edge loop, each angular step (1 rad) < π so the
        // (u,v) seam-unwrap stays exact.
        let mut outer = Vec::new();
        for i in 0..=5 {
            outer.push(pt(i as f64, 1.0));
        }
        for i in (0..=5).rev() {
            outer.push(pt(i as f64, 1.2));
        }

        // Hole: a small 4-edge quad far from the apex.
        let hole = vec![
            pt(0.0, 50.0),
            pt(0.2, 50.0),
            pt(0.2, 50.2),
            pt(0.0, 50.2),
        ];

        let (brep, eids) = curved_face_rings(&[outer, hole]);
        let loops =
            order_loops(&brep, &eids, &cone_surface()).expect("must order into rings");

        // The fix: the radially small but parametrically large loop is the outer
        // bound; the physically large hole is inner.
        assert_eq!(loops.outer.len(), 12, "the large-(u,v)-area loop is the outer bound");
        assert_eq!(loops.inner.len(), 1, "exactly one inner hole loop");
        assert_eq!(loops.inner[0].len(), 4, "the small-(u,v)-area loop is the hole");

        // Demonstrate that the OLD chord-length proxy would have misranked: the
        // hole's total chord exceeds the outer boundary's.
        let outer_extent = ring_extent(&brep, &loops.outer);
        let hole_extent = ring_extent(&brep, &loops.inner[0]);
        assert!(
            hole_extent > outer_extent,
            "chord proxy would wrongly pick the hole as outer (hole {hole_extent} > outer {outer_extent})"
        );

        // Parametric (u,v) area ranks them the right way round.
        let outer_area = ring_uv_area(&brep, &loops.outer, &cone_surface()).unwrap().abs();
        let hole_area = ring_uv_area(&brep, &loops.inner[0], &cone_surface()).unwrap().abs();
        assert!(
            outer_area > hole_area,
            "(u,v) area ranks the boundary above the hole ({outer_area} > {hole_area})"
        );
    }

    #[test]
    fn order_loops_reflex_polygon_is_one_connected_ring() {
        // An L-shaped hexagon in the z=0 plane (CCW), with a REFLEX vertex at
        // (1,1). Edges are added scrambled and with mixed directions; the new
        // turning-angle walk must thread the reflex vertex and recover ONE
        // connected, CCW-wound ring of all 6 edges.
        let verts = [
            [0.0, 0.0, 0.0], // 0
            [2.0, 0.0, 0.0], // 1
            [2.0, 1.0, 0.0], // 2
            [1.0, 1.0, 0.0], // 3 reflex
            [1.0, 2.0, 0.0], // 4
            [0.0, 2.0, 0.0], // 5
        ];
        // Scrambled order + some reversed directions.
        let edges = [
            (1, 2), // right-low
            (3, 2), // reversed vs CCW (2→3)
            (0, 1), // bottom
            (4, 5), // reversed vs CCW (5→4)
            (3, 4), // reflex up
            (0, 5), // reversed vs CCW (5→0)
        ];
        let (brep, eids) = planar_face_edges(&verts, &edges);

        let loops =
            order_loops(&brep, &eids, &plane_xy()).expect("reflex polygon must order into a ring");
        assert!(loops.inner.is_empty(), "a simple polygon has no holes");
        assert_eq!(loops.outer.len(), 6, "all 6 boundary edges in the ring");

        let outer = Loop::new(loops.outer.clone());
        assert!(
            outer.is_connected_ring(&brep.edges),
            "reflex ring must be head-to-tail connected and closed"
        );
        // Outer loop winds CCW seen from +z (positive signed area).
        let area = ring_signed_area(&brep, &loops.outer, [0.0, 0.0, 1.0]);
        assert!(area > 0.0, "outer reflex loop must wind CCW (area {area})");
        // The reflex vertex (index 3) is visited exactly once.
        let mut visits = 0;
        for &(eid, rev) in &loops.outer {
            let e = brep.edge(eid);
            let start = if rev { e.end } else { e.start };
            if start == Some(VertexId(3)) {
                visits += 1;
            }
        }
        assert_eq!(visits, 1, "reflex vertex traversed exactly once");
    }

    #[test]
    fn order_loops_square_with_square_hole_splits_outer_and_inner() {
        // A planar face = big CCW square [0,4]² with a smaller square hole
        // around the centre. The two rings are disjoint; order_loops must
        // return one outer + one inner, both connected, with OPPOSITE winding
        // (outer CCW, hole CW) seen from +z.
        let verts = [
            // outer square (0..3)
            [0.0, 0.0, 0.0],
            [4.0, 0.0, 0.0],
            [4.0, 4.0, 0.0],
            [0.0, 4.0, 0.0],
            // inner square hole (4..7)
            [1.0, 1.0, 0.0],
            [3.0, 1.0, 0.0],
            [3.0, 3.0, 0.0],
            [1.0, 3.0, 0.0],
        ];
        // Scrambled; mixed directions across both rings.
        let edges = [
            (0, 1),
            (4, 5),
            (2, 3), // reversed vs CCW (3→2)... handled by flags
            (1, 2),
            (6, 7), // reversed vs hole CCW (7→6)
            (3, 0),
            (5, 6),
            (7, 4),
        ];
        let (brep, eids) = planar_face_edges(&verts, &edges);

        let loops = order_loops(&brep, &eids, &plane_xy()).expect("must order into rings");
        assert_eq!(loops.outer.len(), 4, "outer square has 4 edges");
        assert_eq!(loops.inner.len(), 1, "exactly one inner hole loop");
        assert_eq!(loops.inner[0].len(), 4, "inner square has 4 edges");

        let outer = Loop::new(loops.outer.clone());
        let inner = Loop::new(loops.inner[0].clone());
        assert!(outer.is_connected_ring(&brep.edges), "outer is a connected ring");
        assert!(inner.is_connected_ring(&brep.edges), "inner is a connected ring");

        let outer_area = ring_signed_area(&brep, &loops.outer, [0.0, 0.0, 1.0]);
        let inner_area = ring_signed_area(&brep, &loops.inner[0], [0.0, 0.0, 1.0]);
        assert!(outer_area > 0.0, "outer winds CCW (area {outer_area})");
        assert!(inner_area < 0.0, "hole winds CW (area {inner_area})");
        // Outer encloses more area than the hole.
        assert!(outer_area.abs() > inner_area.abs());
    }

    /// Assert every face loop of a woven body is a connected ring. Used as the
    /// connectivity gate for watertight bodies (cube / box-with-hole / etc.).
    fn assert_all_loops_connected(brep: &BRep) {
        for (i, f) in brep.faces.iter().enumerate() {
            assert!(
                f.outer.is_connected_ring(&brep.edges),
                "face {i} (branch {}) outer loop is not a connected ring",
                f.branch_id.as_u32()
            );
            for (j, lp) in f.inner.iter().enumerate() {
                assert!(
                    lp.is_connected_ring(&brep.edges),
                    "face {i} (branch {}) inner[{j}] is not a connected ring",
                    f.branch_id.as_u32()
                );
            }
        }
    }

    #[test]
    fn cube_all_face_loops_are_connected_rings() {
        let body = box_body([1.0, 1.0, 1.0]);
        let octree = Octree::build(&body, 0.25);
        let brep = weave(&body, &octree);
        assert_all_loops_connected(&brep);
    }

    #[test]
    fn box_with_through_hole_has_outer_and_one_inner_loop() {
        // A 2×2×2 box drilled by a cylinder (axis = Y, r = 0.5) through the ±y
        // faces. HONEST FINDING (the octree weaver does NOT split the holed
        // face): each of the two pierced planar faces (+y, −y) becomes ONE
        // planar face with an OUTER rectangle loop + exactly ONE INNER circular
        // hole loop — it is not split into separate faces. The bore-wall
        // cylinder face is bounded by the two trim circles (one outer, one
        // inner under the length rule). STEP then emits one FACE_OUTER_BOUND +
        // one FACE_BOUND for each holed face.
        let outer = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [1.0, 1.0, 1.0] },
        };
        let drill = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Cylinder { center: [0.0, 0.0, 0.0], radius: 0.5, half_height: 2.0 },
        };
        let body = BoundedCsg::from_csg(&CsgNode::Difference(Box::new(outer), Box::new(drill)));
        let octree = Octree::build(&body, 0.25);
        let brep = weave(&body, &octree);

        assert_watertight_solid(&brep);
        assert_all_loops_connected(&brep);

        // Exactly the two ±y faces carry a hole (outer rect + one inner loop).
        let holed: Vec<&Face> = brep
            .faces
            .iter()
            .filter(|f| !f.inner.is_empty() && is_analytic_plane(f))
            .collect();
        assert_eq!(holed.len(), 2, "the two pierced planar faces carry the hole");
        for f in &holed {
            assert_eq!(f.outer.edges.len(), 4, "outer is the 4-edge rectangle");
            assert_eq!(f.inner.len(), 1, "exactly one inner hole loop");
            // The inner loop is the single closed trim circle.
            assert_eq!(f.inner[0].edges.len(), 1, "hole is one closed circle edge");
            // Outer rectangle is CCW (positive area) about the plane normal.
            let SurfaceGeometry::Analytic {
                surface: HalfSpace::Plane { normal, .. },
                transform,
            } = &f.surface
            else {
                unreachable!()
            };
            let wn = crate::quat_rotate(transform.rotation, *normal);
            let n = [wn[0] as f64, wn[1] as f64, wn[2] as f64];
            let area = ring_signed_area(&brep, &f.outer.edges, n);
            assert!(area > 0.0, "holed face outer loop winds CCW (area {area})");
        }

        // STEP emits one FACE_OUTER_BOUND + one FACE_BOUND for each holed face.
        let step = crate::brep::step_export::export_step(&brep, "drill");
        assert_eq!(
            crate::brep::step_export::validate_step(&step),
            Ok(()),
            "drilled-box STEP must validate"
        );
        let outer_bounds = step.matches("FACE_OUTER_BOUND").count();
        let inner_bounds = step.matches("FACE_BOUND(").count();
        // 7 faces → 7 outer bounds; 2 holed planar faces + 1 cylinder wall (its
        // second trim circle is an inner bound) → 3 inner bounds.
        assert_eq!(outer_bounds, 7, "one FACE_OUTER_BOUND per face");
        assert_eq!(inner_bounds, 3, "two plate holes + one cylinder inner rim");
    }
}
