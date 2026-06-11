//! Sketch-wall collection and the DL-50 lift of extrude/revolve profiles
//! into analytic 3D half-spaces.

use crate::brep::BranchSurfaceMap;

use crate::compose_transforms;
use crate::csg::BranchId;
use crate::sketch2d::{BoundedSketch2d, Sketch2dPrimitive};
use fielddraft_protocol::sketch2d::{g1_group_segments, SketchSegment, PATH_G1_COS_THRESHOLD};
use fielddraft_protocol::{HalfSpace, Transform};

// ─── small vector helpers (f64): shared, see brep::vec3 ─────────────────────

/// A 2D sketch boundary group's analytic geometry, used to lift each
/// wall of an [`BoundedCsg::Extrude`] / [`BoundedCsg::Revolve`] into a
/// 3D [`HalfSpace`]. Only straight lines and full circles are
/// analytically representable here; curved (quad-bézier / arc / sector)
/// groups yield `None` and fall back to a tessellated face (DL-50).
pub(super) enum SketchWall {
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
pub(super) fn collect_sketch_walls(sketch: &BoundedSketch2d, base: u32, out: &mut Vec<(u32, SketchWall)>) {
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
pub(super) fn collect_prim_walls(prim: &Sketch2dPrimitive, base: u32, out: &mut Vec<(u32, SketchWall)>) {
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
        // A closed Path's straight / circular G1 groups lift exactly like
        // Polygon edges / Circle rims (ticket 9zlq: a face-extrude profile is
        // always a Path, and skipping it sent the whole prism to the octree,
        // whose sample fits bowed every edge). G1 grouping can merge
        // *near*-collinear segments (cos > 0.999 ≈ 2.5° per joint), so a Line
        // group is lifted only when every endpoint sits on the group chord
        // within a tight tolerance; an Arc group only when all arcs share one
        // circle. Curved (Bézier / mixed) groups are skipped and keep their
        // tessellated faces (DL-50).
        Sketch2dPrimitive::Path { segments, closed } => {
            if !*closed || segments.is_empty() {
                return;
            }
            let groups = g1_group_segments(segments, PATH_G1_COS_THRESHOLD);
            let ccw = path_chord_signed_area(segments) > 0.0;
            let n_groups = groups.iter().copied().max().unwrap_or(0) + 1;
            for g in 0..n_groups {
                let segs: Vec<&SketchSegment> = segments
                    .iter()
                    .zip(&groups)
                    .filter(|&(_, &gi)| gi == g)
                    .map(|(s, _)| s)
                    .collect();
                if let Some(wall) = lift_path_group(&segs, ccw) {
                    out.push((base + g, wall));
                }
            }
        }
        // Sector mixes radial lines + an arc into a single boundary group 0,
        // which no single analytic surface matches → tessellated (DL-50).
        Sketch2dPrimitive::Sector { .. } => {}
    }
}

/// Lift one G1 path group to an analytic wall: an all-`Line` group that is
/// genuinely straight becomes a plane wall, an all-`Arc` group on one common
/// circle becomes a cylinder wall. Anything else returns `None` (tessellated).
pub(super) fn lift_path_group(segs: &[&SketchSegment], ccw: bool) -> Option<SketchWall> {
    if segs.iter().all(|s| matches!(s, SketchSegment::Line { .. })) {
        let a = segs.first()?.start_point();
        let b = segs.last()?.end_point();
        let ex = b[0] - a[0];
        let ey = b[1] - a[1];
        let len = (ex * ex + ey * ey).sqrt();
        if len < 1e-9 {
            return None;
        }
        // Reject a G1 chain that merely *bends slowly*: every interior joint
        // must lie on the chord a→b.
        let tol = (len * 1e-5).max(1e-6);
        for s in segs {
            for p in [s.start_point(), s.end_point()] {
                let d = ((p[0] - a[0]) * ey - (p[1] - a[1]) * ex).abs() / len;
                if d > tol {
                    return None;
                }
            }
        }
        let (nx, ny) = if ccw { (ey / len, -ex / len) } else { (-ey / len, ex / len) };
        let offset = nx * a[0] + ny * a[1];
        return Some(SketchWall::Line { normal: [nx, ny], offset });
    }
    if segs.iter().all(|s| matches!(s, SketchSegment::Arc { .. })) {
        let SketchSegment::Arc { center, radius, .. } = segs[0] else {
            unreachable!("all-Arc group");
        };
        let tol = (radius * 1e-5).max(1e-6);
        for s in segs {
            let SketchSegment::Arc { center: c, radius: r, .. } = s else {
                unreachable!("all-Arc group");
            };
            if (c[0] - center[0]).abs() > tol
                || (c[1] - center[1]).abs() > tol
                || (r - radius).abs() > tol
            {
                return None;
            }
        }
        return Some(SketchWall::Circle { center: *center, radius: *radius });
    }
    None
}

/// Winding sign of a closed path via the shoelace area of its segment-endpoint
/// chords. Arc bulges shift the area but not its sign for any sane (simple,
/// non-self-intersecting) profile, which is all the outward-normal choice needs.
pub(super) fn path_chord_signed_area(segments: &[SketchSegment]) -> f32 {
    let mut area = 0.0f32;
    for s in segments {
        let a = s.start_point();
        let b = s.end_point();
        area += a[0] * b[1] - b[0] * a[1];
    }
    area * 0.5
}

pub(super) fn polygon_signed_area(v: &[[f32; 2]]) -> f32 {
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
pub(super) fn collect_extrude_halfspaces(
    id: &fielddraft_protocol::FeatureId,
    sketch: &BoundedSketch2d,
    half_height: f32,
    world_transform: &Transform,
    out: &mut BranchSurfaceMap,
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
pub(super) fn collect_revolve_halfspaces(
    id: &fielddraft_protocol::FeatureId,
    sketch: &BoundedSketch2d,
    world_transform: &Transform,
    out: &mut BranchSurfaceMap,
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
