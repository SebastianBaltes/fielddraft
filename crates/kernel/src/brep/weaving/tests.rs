use super::*;
use super::loop_ordering::*;
use super::surface_registry::*;
use super::wireframes::*;
use crate::csg::CsgNode;
use fielddraft_protocol::{FeatureId, HalfSpace, Primitive, Transform};
use std::collections::HashSet;

/// DL-81: a sphere that pokes out of the box's −x face but whose intersection
/// circle stays in the face *interior* — closest approach ~0.44 mm shy of the
/// −x∩−z and −x∩+y box edges, never crossing them. At the web's resolution the
/// sphere-inflated AABB makes octree cells ~5 mm, so cells along those edges
/// see all three branches {−x, −z, sphere} and mint a `Vertex` cell. The
/// three surfaces have NO common zero (the sphere never reaches the edge line),
/// so the LSQ vertex is a phantom sitting ~0.1 mm inside the edge. Keeping it
/// bisected the straight box edge into two kinked fragments dragged toward the
/// sphere (the uploaded-project bug). `refine_triple_vertex` now reports such
/// an analytically-disproven junction as `Spurious` and the wireframe drops it,
/// so every box edge stays a single straight unsplit segment.
///
/// Geometry is the body-local frame of the real `.fdraft`.
#[test]
fn box_union_interior_sphere_keeps_box_edges_straight() {
    let csg = CsgNode::Union(
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [50.0, 50.0, 50.0],
            },
        }),
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Sphere {
                center: [-63.109, 0.598, -1.291],
                radius: 50.0,
            },
        }),
    );
    let body = BoundedCsg::from_csg(&csg);
    // Body-relative resolution exactly as the web path derives it: the
    // sphere-inflated AABB (~163 mm) over the DIVISOR of 32.
    let min_cell = 163.109 / 32.0;
    let octree = Octree::build(&body, min_cell);
    let brep = weave(&body, &octree);

    assert!(
        brep.validate().is_ok(),
        "B-Rep must be well-formed: {:?}",
        brep.validate()
    );

    // Box feature id 1 expands to the six half-space branches with subs
    // 0..=5 (+x −x +y −y +z −z); the 12 edges are every pair of NON-opposite
    // faces. Built through the constructor, never from a packed literal —
    // the field layout is BranchId's business (#429/E1).
    let bx = |sub: u32| BranchId::from_parts(1, 0, sub).as_u64();
    let (lo, hi) = (bx(0), bx(5));
    let opposite = |a: u64, b: u64| (a.min(b), a.max(b)) == (bx(0), bx(1))
        || (a.min(b), a.max(b)) == (bx(2), bx(3))
        || (a.min(b), a.max(b)) == (bx(4), bx(5));
    let is_box = |a: u64, b: u64| (lo..=hi).contains(&a) && (lo..=hi).contains(&b);

    // Each of the 12 adjacent box-face pairs must appear EXACTLY ONCE: a
    // duplicate signature means a phantom vertex split a straight edge in two.
    let box_edge_sigs: Vec<(u64, u64)> = brep
        .edges
        .iter()
        .filter_map(|e| {
            let (a, b) = (e.signature.0.as_u64(), e.signature.1.as_u64());
            is_box(a, b).then_some((a, b))
        })
        .collect();
    for a in lo..=hi {
        for b in (a + 1)..=hi {
            if opposite(a, b) {
                continue;
            }
            let n = box_edge_sigs.iter().filter(|&&s| s == (a, b)).count();
            assert_eq!(
                n, 1,
                "box edge ({a},{b}) should be a single segment, found {n} (split by a phantom vertex?); box edges {box_edge_sigs:?}"
            );
        }
    }

    // Every box edge must be geometrically straight: its sampled polyline may
    // not bow off the chord between its endpoints (the phantom kink reached
    // ~0.1 mm; a clean plane∩plane edge is exact to float noise).
    for e in &brep.edges {
        let (a, b) = (e.signature.0.as_u64(), e.signature.1.as_u64());
        if !is_box(a, b) {
            continue;
        }
        let pts = crate::brep::edge_sampling::sample_edge(&e.geometry);
        let (p0, pn) = (*pts.first().unwrap(), *pts.last().unwrap());
        let chord = sub(pn, p0);
        let cl = norm(chord);
        assert!(cl > 1.0, "degenerate box edge ({a},{b})");
        let u = [chord[0] / cl, chord[1] / cl, chord[2] / cl];
        let max_dev = pts
            .iter()
            .map(|&p| {
                let d = sub(p, p0);
                let t = d[0] * u[0] + d[1] * u[1] + d[2] * u[2];
                norm(sub(p, [p0[0] + u[0] * t, p0[1] + u[1] * t, p0[2] + u[2] * t]))
            })
            .fold(0.0f64, f64::max);
        assert!(
            max_dev < 1e-3,
            "box edge ({a},{b}) bows {max_dev:.4} mm off its chord — phantom-vertex kink"
        );
    }

    // The only vertices are the 8 box corners: the sphere's interior circle is
    // a closed loop with no corner, and no box edge is touched, so no genuine
    // triple junction exists. Any extra vertex is a surviving phantom.
    assert_eq!(
        brep.vertices.len(),
        8,
        "only the 8 box corners should remain; phantom triple junctions must be dropped"
    );
}

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

// ─── Offset-node spike (general SDF offset → field-based Thicken) ─────────
// Gate before parking the analytic `face_thicken`: prove a `CsgNode::Offset`
// (a) is exactly `child − δ` end to end, and (b) when its carrier is a
// NON-analytic sketch sweep, the dilated surface still weaves watertight into
// a SELECTABLE B-Rep face via the `Field::Leaf` carrier path — i.e. the
// general route reaches the curved faces the per-primitive analytic path
// (sphere/cylinder/torus/cone only) cannot.

/// A closed 4-quad-bézier "blob" extrusion. The blob is clearly non-circular,
/// so its swept lateral wall has no analytic half-space and is woven through
/// the dual-SDF leaf field — exactly the carrier the general offset must reach.
fn blob_extrude_node(id: u32, half_height: f32) -> CsgNode {
    use fielddraft_protocol::{Sketch2d, Sketch2dPrimitive, SketchSegment};
    let qb = |p0: [f32; 2], p1: [f32; 2], p2: [f32; 2]| SketchSegment::QuadBezier { p0, p1, p2 };
    let sketch = Sketch2d::Prim(Sketch2dPrimitive::Path {
        segments: vec![
            qb([3.0, 0.0], [4.0, 4.0], [0.0, 3.0]),
            qb([0.0, 3.0], [-4.0, 4.0], [-3.0, 0.0]),
            qb([-3.0, 0.0], [-4.0, -4.0], [0.0, -3.0]),
            qb([0.0, -3.0], [4.0, -4.0], [3.0, 0.0]),
        ],
        closed: true,
    });
    CsgNode::Extrude { wall_subs: None,
        id: FeatureId::from_u32(id),
        sketch: Box::new(sketch),
        half_height,
    }
}

fn has_tessellated_face(brep: &BRep) -> bool {
    brep.faces
        .iter()
        .any(|f| matches!(f.surface, SurfaceGeometry::Tessellated { .. }))
}

/// DL-77: a closed swept (Bézier) extrude weaves a **clean** B-Rep — its rim
/// (curved wall ∩ analytic cap) consolidates to ONE arc per wall, bounded by
/// exact rim corners, instead of shattering into ~70 octree-fragment loops.
/// This is the prerequisite for the curved-face edge-loop sweep (Thicken M1):
/// every face boundary must be a clean Kantenzug.
#[test]
fn swept_extrude_rim_consolidates_to_clean_loops() {
    let body = BoundedCsg::from_csg(&blob_extrude_node(1, 2.0));
    let octree = Octree::build(&body, 0.2);
    let brep = weave(&body, &octree);

    assert!(brep.validate().is_ok(), "swept extrude B-Rep must be well-formed");
    assert!(
        brep.shells.iter().any(|s| s.is_closed),
        "swept extrude must weave a closed (watertight) shell"
    );
    // 4 Bézier-wall faces + 2 caps; 8 rim corners; 4 vertical wall∩wall seams
    // + 4 top + 4 bottom rim arcs = 12 edges. No fragments, no doubled corners.
    assert_eq!(brep.face_count(), 6, "4 walls + 2 caps");
    assert_eq!(brep.vertex_count(), 8, "8 rim corners, exact (no doubled siblings)");
    assert_eq!(brep.edge_count(), 12, "4 vertical seams + 8 rim arcs, consolidated");
    // Every face is bounded by exactly its 4 edges, with NO spurious inner
    // loops (the pre-DL-77 weave left 70–140 single-edge rings per face).
    for f in &brep.faces {
        assert_eq!(f.outer.edges.len(), 4, "each face is a clean 4-edge loop");
        assert!(f.inner.is_empty(), "no fragmented inner loops");
    }
    // Each rim corner sits exactly on the cap plane (z = ±2) — the sketch
    // endpoint, not an octree-cell-quantised approximation.
    for v in &brep.vertices {
        assert!(
            (v.position[2].abs() - 2.0).abs() < 1e-6,
            "rim corner must lie exactly on a cap plane, got z = {}",
            v.position[2]
        );
    }
}

/// Regression (original bug, DL-78): a box unioned with a curved partner
/// drops the corner vertices on the side FAR from the partner. Root cause is
/// the seed search in `project_branch`: at such a convex corner every in-cell
/// probe sits on the multi-branch fault, so a side branch's Newton projection
/// slides onto an adjacent face — two tangent planes collapse (rank-deficient
/// vertex solve) and the corner vertex + its incident edges are dropped. The
/// far corners MUST survive the generic octree weave WITHOUT any analytic
/// half-space projection (that was the rejected DL-75-broadening fix).
#[test]
fn box_union_sphere_keeps_far_corners() {
    let csg = CsgNode::Union(
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [1.0, 1.0, 1.0],
            },
        }),
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            // Pokes a bump out of the +x face centre; leaves the -x corners
            // (and most of the box) untouched.
            primitive: Primitive::Sphere {
                center: [1.4, 0.0, 0.0],
                radius: 0.9,
            },
        }),
    );
    let body = BoundedCsg::from_csg(&csg);
    let octree = Octree::build(&body, 0.1);
    let brep = weave(&body, &octree);

    assert!(
        brep.validate().is_ok(),
        "B-Rep must be well-formed: {:?}",
        brep.validate()
    );

    // The four box corners at x = -1 are pure box-plane triple points, far
    // from the sphere bump on the +x face. Every one must be a B-Rep vertex.
    let far_corners = [
        [-1.0, -1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, 1.0, 1.0],
    ];
    for c in far_corners {
        let found = brep.vertices.iter().any(|v| {
            let d2 = (v.position[0] - c[0]).powi(2)
                + (v.position[1] - c[1]).powi(2)
                + (v.position[2] - c[2]).powi(2);
            d2 < 0.04 // 0.2 mm
        });
        assert!(
            found,
            "far box corner {c:?} missing from weave; got vertices {:?}",
            brep.vertices.iter().map(|v| v.position).collect::<Vec<_>>()
        );
    }
}

#[test]
fn offset_node_is_child_minus_delta_end_to_end() {
    use crate::csg::EvalMode;
    let node = blob_extrude_node(1, 2.0);
    let carrier = BoundedCsg::from_csg(&node);
    let offset = BoundedCsg::from_csg(&CsgNode::Offset(Box::new(node), 1.0));
    // The general operator: `Offset(child, δ).sdf == child.sdf − δ`, exactly,
    // through the BVH build and at any point (inside, on the wall, outside).
    for p in [
        [0.0, 0.0, 0.0],
        [2.5, 0.0, 0.0],
        [0.0, 0.0, 1.5],
        [5.0, 1.0, 0.5],
        [-3.5, -2.0, 3.0],
    ] {
        let c = carrier.sdf(p, EvalMode::Logical);
        let o = offset.sdf(p, EvalMode::Logical);
        assert!(
            (o - (c - 1.0)).abs() < 1e-5,
            "offset must be carrier − δ at {p:?}: {o} vs {}",
            c - 1.0
        );
    }
}

#[test]
fn offset_of_non_analytic_carrier_weaves_selectable_dilated_face() {
    use crate::csg::EvalMode;
    let node = blob_extrude_node(1, 2.0);
    let carrier = BoundedCsg::from_csg(&node);

    // 1. The carrier really is non-analytic: its curved wall is Tessellated.
    let c_oct = Octree::build(&carrier, 0.2);
    let c_brep = weave(&carrier, &c_oct);
    assert!(
        has_tessellated_face(&c_brep),
        "carrier blob must have a non-analytic (Tessellated) wall — else this \
         would secretly test the analytic path"
    );

    // 2. Dilate it by δ = 1 mm and weave the offset body.
    let delta = 1.0_f32;
    let offset = BoundedCsg::from_csg(&CsgNode::Offset(Box::new(node), delta));
    let oct = Octree::build(&offset, 0.2);
    let brep = weave(&offset, &oct);

    // Watertight, well-formed, and the non-analytic wall survived the offset…
    assert!(brep.validate().is_ok(), "offset body B-Rep must be well-formed");
    assert!(brep.solid_count() >= 1, "offset body must weave a solid");
    assert!(
        brep.shells.iter().any(|s| s.is_closed),
        "dilated body must weave a closed (watertight) shell"
    );
    let tess: Vec<_> = brep
        .faces
        .iter()
        .enumerate()
        .filter(|(_, f)| matches!(f.surface, SurfaceGeometry::Tessellated { .. }))
        .collect();
    assert!(
        !tess.is_empty(),
        "the offset of a curved carrier must still weave a curved (Tessellated) wall"
    );
    // …and that wall is SELECTABLE — it has a stable topological handle.
    let (fi, _) = tess[0];
    let handle =
        crate::brep::naming::TopoRef::from_face(&brep, crate::brep::FaceId(fi as u32)).id_hash;
    assert_ne!(handle, 0, "the dilated curved face must carry a qFace handle");

    // 3. The dilation is geometrically exact along the surface normal: find a
    //    point P on the carrier's +x wall, then the offset surface sits δ mm
    //    further out along the carrier normal there.
    let sdf_c = |p: [f32; 3]| carrier.sdf(p, EvalMode::Logical);
    let sdf_o = |p: [f32; 3]| offset.sdf(p, EvalMode::Logical);
    // Bisect along +x (z = 0, a side wall not a cap) to the carrier surface.
    let (mut lo, mut hi) = (0.0_f32, 8.0_f32);
    assert!(sdf_c([lo, 0.0, 0.0]) < 0.0 && sdf_c([hi, 0.0, 0.0]) > 0.0);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if sdf_c([mid, 0.0, 0.0]) < 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let px = 0.5 * (lo + hi);
    let p = [px, 0.0, 0.0];
    // Outward unit normal = ∇(carrier) via central differences.
    let h = 1e-3_f32;
    let g = [
        (sdf_c([p[0] + h, p[1], p[2]]) - sdf_c([p[0] - h, p[1], p[2]])) / (2.0 * h),
        (sdf_c([p[0], p[1] + h, p[2]]) - sdf_c([p[0], p[1] - h, p[2]])) / (2.0 * h),
        (sdf_c([p[0], p[1], p[2] + h]) - sdf_c([p[0], p[1], p[2] - h])) / (2.0 * h),
    ];
    let gl = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
    let n = [g[0] / gl, g[1] / gl, g[2] / gl];
    // P sits exactly δ inside the dilated body…
    assert!(
        (sdf_o(p) + delta).abs() < 0.05,
        "carrier-surface point must be δ inside the offset body: {} vs {}",
        sdf_o(p),
        -delta
    );
    // …and the dilated surface is δ out along the normal.
    let q = [p[0] + delta * n[0], p[1] + delta * n[1], p[2] + delta * n[2]];
    assert!(
        sdf_o(q).abs() < 0.05,
        "offset surface must sit δ out along the carrier normal: sdf_o(q) = {}",
        sdf_o(q)
    );
}

#[test]
fn thicken_band_is_shell_of_offset_on_a_closed_non_analytic_carrier() {
    // M0 core claim: a thin wall of a *closed* non-analytic face is
    // `Shell(Offset(carrier, out), out+in)` — the carrier dilated by `out`
    // then hollowed inward by `out+in`, so the band lies between
    // `carrier+out` (outer wall) and `carrier−in` (inner wall). This reuses
    // the proven Shell thin-wall weave + inner-wall (0x800) classification
    // and the spike's general Offset, with NO new face machinery and no
    // dual-offset branch collision.
    use crate::csg::EvalMode;
    let (out, inn) = (0.8_f32, 0.6_f32);
    let carrier_node = blob_extrude_node(1, 2.0);
    let carrier = BoundedCsg::from_csg(&carrier_node);

    let band_node = CsgNode::Shell(
        Box::new(CsgNode::Offset(Box::new(carrier_node), out)),
        out + inn,
        Vec::new(),
    );
    let band = BoundedCsg::from_csg(&band_node);

    // 1. Field identity: outer wall at carrier = +out, inner wall at
    //    carrier = −in, solid in between, hollow inside the inner wall.
    let sdf_c = |p: [f32; 3]| carrier.sdf(p, EvalMode::Logical);
    let sdf_b = |p: [f32; 3]| band.sdf(p, EvalMode::Logical);
    // March +x (z=0 side wall) to the carrier surface, then probe the band
    // across the wall thickness along the outward normal.
    let (mut lo, mut hi) = (0.0_f32, 8.0_f32);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if sdf_c([mid, 0.0, 0.0]) < 0.0 { lo = mid } else { hi = mid }
    }
    let px = 0.5 * (lo + hi);
    let n = {
        let h = 1e-3_f32;
        let g = [
            (sdf_c([px + h, 0.0, 0.0]) - sdf_c([px - h, 0.0, 0.0])) / (2.0 * h),
            (sdf_c([px, h, 0.0]) - sdf_c([px, -h, 0.0])) / (2.0 * h),
            (sdf_c([px, 0.0, h]) - sdf_c([px, 0.0, -h])) / (2.0 * h),
        ];
        let l = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
        [g[0] / l, g[1] / l, g[2] / l]
    };
    let at = |t: f32| [px + t * n[0], t * n[1], t * n[2]];
    assert!(sdf_b(at(out)).abs() < 0.05, "outer wall must sit at carrier+out");
    assert!(sdf_b(at(-inn)).abs() < 0.05, "inner wall must sit at carrier−in");
    assert!(sdf_b(at(0.0)) < -0.05, "carrier surface itself is inside the wall");
    assert!(sdf_b(at(-(inn + 0.4))) > 0.05, "interior past the inner wall is hollow");
    assert!(sdf_b(at(out + 0.4)) > 0.05, "outside past the outer wall is empty");

    // 2. The band weaves into a watertight B-Rep with selectable walls.
    let oct = Octree::build(&band, 0.15);
    let brep = weave(&band, &oct);
    assert!(brep.validate().is_ok(), "band B-Rep must be well-formed");
    assert!(
        brep.shells.iter().any(|s| s.is_closed),
        "thin-wall band must weave a closed (watertight) shell"
    );
    // The non-analytic carrier means both walls are Tessellated; each is a
    // selectable face (stable id_hash). Closed carrier → two disjoint walls.
    let tess: Vec<_> = (0..brep.faces.len())
        .filter(|&i| matches!(brep.faces[i].surface, SurfaceGeometry::Tessellated { .. }))
        .collect();
    assert!(
        tess.len() >= 2,
        "a closed non-analytic band has an inner AND an outer curved wall, got {}",
        tess.len()
    );
    for &i in &tess {
        let h = crate::brep::naming::TopoRef::from_face(&brep, crate::brep::FaceId(i as u32))
            .id_hash;
        assert_ne!(h, 0, "each woven band wall must carry a qFace handle");
    }
}

#[test]
fn thicken_band_folds_analytic_sphere_walls_when_carrier_is_a_sphere() {
    // M0 gate evidence "analytisch exakt wo faltbar": the SAME general
    // `Shell(Offset(carrier, out), out+in)` band, given an analytic Sphere
    // carrier, folds to the exact concentric analytic faces the per-primitive
    // path emits — Sphere(r+out) outer, Sphere(r−in) inner — via the
    // branch-map Offset/Shell arms, with no thicken-specific code.
    let (r, out, inn) = (5.0_f32, 0.8_f32, 0.6_f32);
    let sphere = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: r },
    };
    let band = BoundedCsg::from_csg(&CsgNode::Shell(
        Box::new(CsgNode::Offset(Box::new(sphere), out)),
        out + inn,
        Vec::new(),
    ));
    let radii: Vec<f32> = branch_halfspace_map(&band)
        .values()
        .filter_map(|(hs, _)| match hs {
            HalfSpace::Sphere { radius } => Some(*radius),
            _ => None,
        })
        .collect();
    assert!(
        radii.iter().any(|&x| (x - (r + out)).abs() < 1e-4),
        "outer analytic wall must be Sphere(r+out)={}, got {radii:?}",
        r + out
    );
    assert!(
        radii.iter().any(|&x| (x - (r - inn)).abs() < 1e-4),
        "inner analytic wall must be Sphere(r−in)={}, got {radii:?}",
        r - inn
    );
}

fn union_box_sphere(box_half: [f32; 3], sph_center: [f32; 3], sph_r: f32) -> BoundedCsg {
    let boxn = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box {
            center: [0.0, 0.0, 0.0],
            half_extents: box_half,
        },
    };
    let sph = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Sphere {
            center: sph_center,
            radius: sph_r,
        },
    };
    BoundedCsg::from_csg(&CsgNode::Union(Box::new(boxn), Box::new(sph)))
}

/// A regeneration-stable fingerprint of a woven B-Rep's wireframe: the
/// sorted multiset of edges (canonical signature, endpoint *positions*,
/// sampled polyline) plus the sorted vertex positions. Edge emission order
/// and endpoint `VertexId`s are themselves order-dependent, so we key on
/// geometry, never on ids.
fn wireframe_fingerprint(brep: &BRep) -> (Vec<String>, Vec<String>) {
    let q = |x: f64| format!("{x:.4}");
    let vpos = |vid: Option<VertexId>| match vid {
        Some(VertexId(i)) => {
            let p = brep.vertices[i as usize].position;
            format!("[{},{},{}]", q(p[0]), q(p[1]), q(p[2]))
        }
        None => "none".to_string(),
    };
    let mut edges: Vec<String> = brep
        .edges
        .iter()
        .map(|e| {
            let poly: String = crate::brep::edge_sampling::sample_edge(&e.geometry)
                .iter()
                .map(|p| format!("{},{},{};", q(p[0]), q(p[1]), q(p[2])))
                .collect();
            format!(
                "{}-{}|{}|{}|{poly}",
                e.signature.0.as_u64(),
                e.signature.1.as_u64(),
                vpos(e.start),
                vpos(e.end),
            )
        })
        .collect();
    edges.sort();
    let mut verts: Vec<String> = brep
        .vertices
        .iter()
        .map(|v| format!("[{},{},{}]", q(v.position[0]), q(v.position[1]), q(v.position[2])))
        .collect();
    verts.sort();
    (edges, verts)
}

/// Sorted multiset of the stable element handles (`id_hash`) for a woven
/// B-Rep's faces / edges / vertices — exactly what `qFace`/`qEdge`/`qVertex`
/// address. Independent of arena order.
fn topo_handles(brep: &BRep) -> (Vec<u64>, Vec<u64>, Vec<u64>) {
    use crate::brep::naming::TopoRef;
    use crate::brep::{EdgeId, FaceId, VertexId};
    let mut faces: Vec<u64> = (0..brep.faces.len())
        .map(|k| TopoRef::from_face(brep, FaceId(k as u32)).id_hash)
        .collect();
    let mut edges: Vec<u64> = (0..brep.edges.len())
        .map(|k| TopoRef::from_edge(brep, EdgeId(k as u32)).id_hash)
        .collect();
    let mut verts: Vec<u64> = (0..brep.vertices.len())
        .map(|k| TopoRef::from_vertex(brep, VertexId(k as u32)).id_hash)
        .collect();
    faces.sort_unstable();
    edges.sort_unstable();
    verts.sort_unstable();
    (faces, edges, verts)
}

#[test]
fn parametric_box_matches_octree_topology_and_handles() {
    // A non-cube box so the "exact corner" check is discriminating.
    let body = box_body([2.0, 1.0, 0.5]);

    let octree = Octree::build(&body, 0.125);
    let sdf = weave(&body, &octree);
    let par = weave_parametric(&body).expect("a lone Box is parametric-weavable");

    // Same cube topology, both watertight.
    assert_eq!(par.faces.len(), 6, "box has 6 faces");
    assert_eq!(par.edges.len(), 12, "box has 12 edges");
    assert_eq!(par.vertices.len(), 8, "box has 8 corners");
    assert!(par.shells[0].is_closed, "parametric box is watertight");
    assert_eq!(sdf.faces.len(), 6);
    assert_eq!(sdf.edges.len(), 12);
    assert_eq!(sdf.vertices.len(), 8);

    // The corners are EXACT — every sign combination of the half-extents to
    // full f64 precision (the octree path only reaches these within a
    // sampling tolerance). This is the precision win of milestone 1.
    let mut corners: Vec<[f64; 3]> = par.vertices.iter().map(|v| v.position).collect();
    corners.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut expect: Vec<[f64; 3]> = Vec::new();
    for &sx in &[-2.0, 2.0] {
        for &sy in &[-1.0, 1.0] {
            for &sz in &[-0.5, 0.5] {
                expect.push([sx, sy, sz]);
            }
        }
    }
    expect.sort_by(|a, b| a.partial_cmp(b).unwrap());
    for (got, want) in corners.iter().zip(&expect) {
        for k in 0..3 {
            assert!(
                (got[k] - want[k]).abs() < 1e-12,
                "corner {got:?} is not exactly {want:?}"
            );
        }
    }

    // The element handles (qFace/qEdge/qVertex id_hash) are byte-identical
    // on both routes — toggling parametric weaving never breaks a saved
    // selection. This also proves the SDF path produced the same clean
    // 6/12/8 topology (different handle sets would fail here).
    assert_eq!(
        topo_handles(&par),
        topo_handles(&sdf),
        "parametric and octree weave must yield identical element handles"
    );
}

fn cylinder_body(radius: f32, half_height: f32) -> BoundedCsg {
    let csg = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Cylinder { center: [0.0, 0.0, 0.0], radius, half_height },
    };
    BoundedCsg::from_csg(&csg)
}

#[test]
fn parametric_cylinder_matches_octree_topology_and_handles() {
    // A non-unit radius/height so the exactness check is discriminating.
    let body = cylinder_body(1.5, 0.5);

    let octree = Octree::build(&body, 0.0625);
    let sdf = weave(&body, &octree);
    let par = weave_parametric(&body).expect("a lone Cylinder is parametric-weavable");

    // A cylinder is 3 faces (wall + 2 caps), 2 rim edges, NO vertices.
    assert_eq!(par.faces.len(), 3, "cylinder has wall + 2 caps");
    assert_eq!(par.edges.len(), 2, "cylinder has 2 rim circles");
    assert_eq!(par.vertices.len(), 0, "a cylinder has no corner vertices");
    assert!(par.shells[0].is_closed, "parametric cylinder is watertight");
    assert_eq!(sdf.faces.len(), 3);
    assert_eq!(sdf.edges.len(), 2);
    assert_eq!(sdf.vertices.len(), 0);

    // Both rims are exact rational-quadratic circles: every rim is a degree-2
    // NURBS whose four axis control points (the even indices) lie EXACTLY on
    // radius 1.5 at the cap plane y = ±0.5 — resolution-independent, the M1
    // precision win (the octree fit only reaches this within a sample tol).
    for edge in &par.edges {
        let EdgeGeometry::Nurbs(curve) = &edge.geometry else {
            panic!("rim must be a NURBS circle");
        };
        assert_eq!(curve.degree, 2, "exact circle is rational quadratic");
        let y = curve.control_points[0][1];
        assert!((y.abs() - 0.5).abs() < 1e-12, "rim sits on a cap plane");
        for &p in curve.control_points.iter().step_by(2) {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            assert!((r - 1.5).abs() < 1e-6, "axis point off radius 1.5: r={r}");
            assert!((p[1] - y).abs() < 1e-12, "rim is planar");
        }
    }

    // Element handles (qFace/qEdge id_hash) are byte-identical on both routes,
    // so a saved selection survives toggling parametric weaving.
    assert_eq!(
        topo_handles(&par),
        topo_handles(&sdf),
        "parametric and octree weave must yield identical element handles"
    );
}

fn torus_body(major_radius: f32, minor_radius: f32) -> BoundedCsg {
    let csg = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Torus { center: [0.0, 0.0, 0.0], major_radius, minor_radius },
    };
    BoundedCsg::from_csg(&csg)
}

#[test]
fn parametric_edgeless_grundforms_match_octree_topology_and_handles() {
    // Sphere and torus are each ONE closed analytic face: no edges, no
    // vertices. The parametric route emits an empty wireframe and lets
    // `finish_weave` step (2b) materialise the lone closed face — identical
    // to the octree route, which produces no edge cells either.
    for (label, body) in
        [("sphere", sphere_body([0.0, 0.0, 0.0], 1.3)), ("torus", torus_body(2.0, 0.5))]
    {
        let octree = Octree::build(&body, 0.0625);
        let sdf = weave(&body, &octree);
        let par = weave_parametric(&body)
            .unwrap_or_else(|| panic!("a lone {label} is parametric-weavable"));

        assert_eq!(par.faces.len(), 1, "{label} is one closed face");
        assert_eq!(par.edges.len(), 0, "{label} has no edges");
        assert_eq!(par.vertices.len(), 0, "{label} has no vertices");
        assert!(par.shells[0].is_closed, "parametric {label} is a closed shell");
        assert_eq!(sdf.faces.len(), 1, "octree {label} is one closed face");
        assert_eq!(sdf.edges.len(), 0);
        assert_eq!(sdf.vertices.len(), 0);

        // The single face's analytic surface must be the right kind — a true
        // Sphere/Torus half-space, not a tessellated fallback.
        let SurfaceGeometry::Analytic { surface, .. } = &par.faces[0].surface else {
            panic!("{label} face must be analytic");
        };
        match (label, surface) {
            ("sphere", HalfSpace::Sphere { .. }) | ("torus", HalfSpace::Torus { .. }) => {}
            other => panic!("{label} got wrong analytic surface: {other:?}"),
        }

        assert_eq!(
            topo_handles(&par),
            topo_handles(&sdf),
            "{label}: parametric and octree weave must yield identical handles"
        );
    }
}

#[test]
fn parametric_weave_handles_boolean_and_cone_via_seed_path() {
    // Phase 2b: a boolean union is no longer a fall-back — the branch-seed
    // layer of `weave_parametric` weaves it analytically, watertight and
    // handle-identical to the octree route.
    let union = union_box_sphere([1.0, 1.0, 1.0], [1.0, 0.0, 0.0], 0.8);
    let oct = Octree::build(&union, 0.0625);
    let sdf = weave(&union, &oct);
    let par = weave_parametric(&union).expect("union now weaves via the seed path");
    assert!(par.shells[0].is_closed, "seed-woven union is watertight");
    assert_eq!(
        topo_handles(&par),
        topo_handles(&sdf),
        "union: parametric (seed) and octree handles must match"
    );

    // The Cone (frustum) — left to the octree in M1 (DL-76 apex convention) —
    // is also picked up by the seed path now, into the same watertight 3-face
    // frustum with identical handles.
    let frustum = BoundedCsg::from_csg(&CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Cone { r_bottom: 1.5, r_top: 0.8, half_height: 0.6 },
    });
    let oct = Octree::build(&frustum, 0.1);
    let sdf = weave(&frustum, &oct);
    let par = weave_parametric(&frustum).expect("cone now weaves via the seed path");
    assert!(par.shells[0].is_closed, "seed-woven frustum is watertight");
    assert_eq!(
        topo_handles(&par),
        topo_handles(&sdf),
        "frustum: parametric (seed) and octree handles must match"
    );
}

#[test]
fn parametric_weave_falls_back_for_out_of_scope() {
    // The genuine fall-backs are bodies outside the analytic scope: domain
    // warps (the half-space map describes the un-warped primitive) and
    // non-Lipschitz lattices (no clean Euclidean distance). Both → None.
    let warp = BoundedCsg::from_csg(&CsgNode::Twist(
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [1.0, 1.0, 1.0],
            },
        }),
        0.5,
    ));
    assert!(weave_parametric(&warp).is_none(), "a domain warp falls back to octree");

    let gyroid = BoundedCsg::from_csg(&CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Gyroid { cell_size: 4.0, thickness: 1.0 },
    });
    assert!(weave_parametric(&gyroid).is_none(), "a non-Lipschitz lattice falls back");
}

fn cone_body(r_bottom: f32, r_top: f32, half_height: f32) -> BoundedCsg {
    let csg = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Cone { r_bottom, r_top, half_height },
    };
    BoundedCsg::from_csg(&csg)
}

// ── DL-76: a truncated cone octree-weaves into a watertight 3-face frustum ──
#[test]
fn frustum_octree_weaves_watertight_with_apex_corrected_cone_wall() {
    // r_bottom=1.5 @ y=−0.6, r_top=0.8 @ y=+0.6 ⇒ extrapolated apex (r=0) at
    // y = half_height·(r_bottom+r_top)/(r_bottom−r_top) = 0.6·2.3/0.7 ≈ 1.9714.
    let body = cone_body(1.5, 0.8, 0.6);
    let octree = Octree::build(&body, 0.1);
    let brep = weave(&body, &octree);

    // Topology: wall + 2 caps = 3 faces; 2 rim circles = 2 edges; no corners.
    assert_eq!(brep.face_count(), 3, "frustum has wall + 2 caps");
    assert_eq!(brep.edge_count(), 2, "frustum has 2 rim circles");
    assert_eq!(brep.vertex_count(), 0, "rim circles are closed — no corners");
    assert_eq!(brep.shell_count(), 1);
    assert_eq!(brep.solid_count(), 1);
    assert_eq!(brep.validate(), Ok(()), "frustum B-Rep must be well-formed");
    assert!(
        brep.shells[0].is_closed,
        "DL-76: rim seams must be accepted → watertight frustum (was 3 faces, 0 edges)"
    );

    // Every rim edge is shared by exactly two loops — the annular wall carries
    // one rim in its outer loop and the other as an inner (hole) loop, like a
    // cylinder side, so both face loops must be counted.
    let mut uses = vec![0u32; brep.edge_count()];
    for f in &brep.faces {
        for lp in std::iter::once(&f.outer).chain(f.inner.iter()) {
            for &(EdgeId(idx), _) in &lp.edges {
                uses[idx as usize] += 1;
            }
        }
    }
    assert!(uses.iter().all(|&u| u == 2), "every rim edge used by exactly two loops");

    // The wall is an analytic cone whose transform origin sits at the TRUE
    // apex (≈1.9714 up the +Y axis), not at the frustum centre (DL-76).
    let wall = brep
        .faces
        .iter()
        .find(|f| {
            matches!(
                f.surface,
                SurfaceGeometry::Analytic { surface: HalfSpace::Cone { .. }, .. }
            )
        })
        .expect("frustum has an analytic cone wall");
    let SurfaceGeometry::Analytic { transform, .. } = &wall.surface else {
        unreachable!()
    };
    assert!(
        (transform.translate[1] - 1.971_428_5).abs() < 1e-3,
        "cone wall apex at y≈1.9714, got {}",
        transform.translate[1]
    );
}

/// Regenerating the B-Rep from the *same* geometry must yield an identical
/// wireframe. The std `HashMap` seed advances between weaves in one process
/// (on wasm it is a per-allocation counter, on native fresh entropy), so
/// iterating the signature groups in raw `HashMap` order made extraction
/// order-dependent: edges snapped to different vertices and reflex corners
/// reconstructed differently, run to run — the "erratic B-Rep" bug. We weave
/// several times so the seed varies, and require every weave to match the
/// first.
/// DL-67: at a *coarse* octree resolution the sphere∩box-face seam used to
/// shatter — too few `Edge` cells along the curve, with some failing to
/// extract, so neither the Kåsa circle fit nor gap-chaining could rebuild it
/// and the seam fragmented into many short unbounded pieces (faces then fell
/// back to unordered loops). The analytic circular seam makes it one clean
/// arc at any resolution. Here we weave the corner case at res=1.0 (which
/// fragmented before) and require the exact, resolution-independent topology.
#[test]
fn coarse_sphere_seam_is_one_analytic_arc_not_fragments() {
    let body = union_box_sphere([10.0, 10.0, 10.0], [10.0, 10.0, 10.0], 8.0);
    let octree = Octree::build(&body, 1.0);
    let brep = weave(&body, &octree);

    // The three sphere-cap seams (signature carrying the sphere branch) are
    // each exactly ONE edge, bounded by two triple vertices — not a spray of
    // short None-None fragments.
    let sphere_branch = BranchId::from_parts(2, 0, 0).as_u64();
    let seams: Vec<_> = brep
        .edges
        .iter()
        .filter(|e| e.signature.0.as_u64() == sphere_branch || e.signature.1.as_u64() == sphere_branch)
        .collect();
    assert_eq!(seams.len(), 3, "expected 3 single sphere-cap seams, got {}", seams.len());
    for e in &seams {
        assert!(
            e.start.is_some() && e.end.is_some(),
            "sphere seam must be bounded by its triple vertices, got {:?}..{:?}",
            e.start,
            e.end
        );
    }

    // Every face orders into connected rings — no unordered fallback.
    for (fi, f) in brep.faces.iter().enumerate() {
        assert!(
            f.outer.is_connected_ring(&brep.edges),
            "face {fi} (branch {}) outer loop is not a connected ring",
            f.branch_id.as_u64()
        );
    }
}

#[test]
fn weave_is_deterministic_across_regenerations() {
    let body = union_box_sphere([10.0, 10.0, 10.0], [10.0, 10.0, 10.0], 8.0);
    let octree = Octree::build(&body, 0.5);
    let first = wireframe_fingerprint(&weave(&body, &octree));
    for run in 1..6 {
        let again = wireframe_fingerprint(&weave(&body, &octree));
        assert_eq!(
            first, again,
            "weave #{run} differs from weave #0 — B-Rep extraction is not deterministic"
        );
    }
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
    let mut groups = order_loop_groups(&brep, &[e0, e2, e3, e1], &surface, false)
        .expect("should close into a cycle");
    assert_eq!(groups.len(), 1, "one connected boundary region");
    let loops = groups.remove(0);
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
    let branches: HashSet<u64> = brep.faces.iter().map(|f| f.branch_id.as_u64()).collect();
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

#[test]
fn shell_adds_inner_wall_halfspaces_under_inner_branch_ids() {
    // DL-75 Step 3: a closed-shell box keeps its 6 outer planes and gains 6
    // inner-wall planes — each outer face offset inward by the thickness,
    // keyed by the outer branch's inner-wall id, with `feature_id` preserved.
    let half = 5.0_f32;
    let thickness = 1.0_f32;
    let csg = CsgNode::Shell(
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [half, half, half],
            },
        }),
        thickness,
        Vec::new(),
    );
    let map = branch_halfspace_map(&BoundedCsg::from_csg(&csg));
    assert_eq!(map.len(), 12, "6 outer + 6 inner-wall planes");

    let inner: Vec<_> = map
        .iter()
        .filter(|(k, _)| BranchId(**k).is_inner_wall())
        .collect();
    assert_eq!(inner.len(), 6, "one inner wall per outer box face");
    for (k, (hs, _)) in inner {
        assert_eq!(
            BranchId(*k).feature_id(),
            1,
            "inner wall stays selectable as its owning feature"
        );
        match hs {
            HalfSpace::Plane { offset, .. } => assert!(
                (offset - (half - thickness)).abs() < 1e-4,
                "inner plane is the outer face pushed in by the thickness"
            ),
            other => panic!("box inner wall must be a plane, got {other:?}"),
        }
    }
}

#[test]
fn shell_weave_produces_inner_wall_faces() {
    // DL-75 Step 4: weaving a closed-shell box yields the 6 outer faces AND
    // 6 inner-wall faces (the cavity), the latter carrying inner-wall branch
    // ids that still resolve to the owning feature for selection.
    let half = 2.0_f32;
    let thickness = 0.5_f32;
    let csg = CsgNode::Shell(
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [half, half, half],
            },
        }),
        thickness,
        Vec::new(),
    );
    let body = BoundedCsg::from_csg(&csg);
    let octree = Octree::build(&body, 0.1);
    let brep = weave(&body, &octree);
    assert_eq!(brep.validate(), Ok(()), "shelled B-Rep must be well-formed");

    let inner: Vec<_> = brep
        .faces
        .iter()
        .filter(|f| f.branch_id.is_inner_wall())
        .collect();
    let outer = brep.faces.iter().filter(|f| !f.branch_id.is_inner_wall()).count();
    assert_eq!(outer, 6, "6 outer faces preserved");
    assert_eq!(inner.len(), 6, "6 inner-wall faces (the cavity)");
    for f in inner {
        assert_eq!(f.branch_id.feature_id(), 1, "inner wall stays selectable");
        assert!(
            matches!(
                f.surface,
                SurfaceGeometry::Analytic { surface: HalfSpace::Plane { .. }, .. }
            ),
            "inner wall is an analytic offset plane, got {:?}",
            f.surface
        );
    }
}

#[test]
fn dualised_closed_shell_weaves_inner_wall_faces() {
    // #384 D2: run a closed shell through the build-time complement expansion
    // (`expand_shells → push_down_offsets → fold_offset_leaves`), so it becomes
    // `Difference(box, InnerWall(box_eroded))` instead of a `BoundedCsg::Shell`.
    // The weave must STILL surface 6 outer + 6 inner-wall faces: the `InnerWall`
    // wrapper remaps the eroded copy's branches/carriers into the DL-75
    // inner-wall band (branch_pairs + surface_registry + parity), reproducing
    // the pre-D2 `Shell` weave through the new structural path.
    let half = 2.0_f32;
    let thickness = 0.5_f32;
    let csg = CsgNode::Shell(
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [half, half, half],
            },
        }),
        thickness,
        Vec::new(),
    )
    .expand_shells()
    .push_down_offsets()
    .fold_offset_leaves();
    let body = BoundedCsg::from_csg(&csg);
    let octree = Octree::build(&body, 0.1);
    let brep = weave(&body, &octree);
    assert_eq!(brep.validate(), Ok(()), "dualised shell B-Rep must be well-formed");

    let inner = brep
        .faces
        .iter()
        .filter(|f| f.branch_id.is_inner_wall())
        .count();
    let outer = brep.faces.iter().filter(|f| !f.branch_id.is_inner_wall()).count();
    assert_eq!(outer, 6, "6 outer faces preserved");
    assert_eq!(inner, 6, "6 inner-wall faces (the cavity)");
    for f in brep.faces.iter().filter(|f| f.branch_id.is_inner_wall()) {
        assert_eq!(f.branch_id.feature_id(), 1, "inner wall stays selectable");
    }
}

#[test]
fn open_shell_weaves_inner_wall_and_keeps_outer_faces() {
    // DL-75: the realistic case — a box hollowed and opened at its +Z face.
    // The weave must stay watertight-valid and surface BOTH outer faces and
    // inner-wall faces (the cavity exposed through the opening), with the
    // inner walls offset inward and still resolving to the owning feature.
    let half = 2.0_f32;
    let thickness = 0.5_f32;
    let csg = CsgNode::Shell(
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [half, half, half],
            },
        }),
        thickness,
        // Open the +Z face: its outward gate is the plane z = +half.
        vec![crate::csg::OpenSurface {
            branch: crate::csg::BranchId::from_feature_sub(&FeatureId::from_u32(1), 4).as_u64(),
            surface: HalfSpace::Plane { normal: [0.0, 0.0, 1.0], offset: half },
            placement: Transform::IDENTITY,
        }],
    );
    let body = BoundedCsg::from_csg(&csg);
    let octree = Octree::build(&body, 0.1);
    let brep = weave(&body, &octree);
    assert_eq!(brep.validate(), Ok(()), "open-shell B-Rep must be well-formed");

    let inner = brep.faces.iter().filter(|f| f.branch_id.is_inner_wall()).count();
    let outer = brep.faces.iter().filter(|f| !f.branch_id.is_inner_wall()).count();
    assert!(outer >= 5, "outer faces preserved (got {outer})");
    assert!(inner >= 4, "inner-wall faces exposed through the opening (got {inner})");
    for f in brep.faces.iter().filter(|f| f.branch_id.is_inner_wall()) {
        assert_eq!(f.branch_id.feature_id(), 1, "inner wall stays selectable");
    }
}

// ── DL-50: analytic Extrude / Revolve faces ──────────────────────────

use crate::sketch2d::SketchSegment;
use fielddraft_protocol::Sketch2d as ProtoSketch2d;
use fielddraft_protocol::Sketch2dPrimitive as ProtoSketch2dPrim;

/// Build a `BoundedCsg::Extrude` body directly (mirrors `box_body`).
fn extrude_body(sketch: ProtoSketch2d, half_height: f32) -> BoundedCsg {
    let csg = CsgNode::Extrude { wall_subs: None,
        id: FeatureId::from_u32(1),
        sketch: Box::new(sketch),
        half_height,
    };
    BoundedCsg::from_csg(&csg)
}

fn revolve_body(sketch: ProtoSketch2d) -> BoundedCsg {
    let csg = CsgNode::Revolve { wall_subs: None,
        id: FeatureId::from_u32(1),
        sketch: Box::new(sketch),
    };
    BoundedCsg::from_csg(&csg)
}

fn sweep_body(sketch: ProtoSketch2d, path: Vec<[f32; 3]>, closed: bool) -> BoundedCsg {
    let csg = CsgNode::Sweep {
        id: FeatureId::from_u32(1),
        sketch: Box::new(sketch),
        path,
        closed,
        twist: 0.0,
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
    // incident edges, so the full 12 corners are present and `order_loop_groups`
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
    // `order_loop_groups` unordered fallback is emitted (DL-60).
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
fn parametric_sweep_traces_corner_curves() {
    // Ticket ifog: a sweep's seam edges are traced constructively from the
    // profile corners through the spine's RMF frames. Square profile (4
    // corners). Open path → 4 longitudinal + 4+4 cap rims = 12 edges, 8
    // vertices. Closed path → 4 closed longitudinal loops, no caps/vertices.
    let sq = ProtoSketch2d::Prim(ProtoSketch2dPrim::Polygon {
        vertices: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]],
    });
    let open = sweep_body(sq.clone(), vec![[0.0, 0.0, 0.0], [0.0, 0.0, 5.0], [0.0, 0.0, 10.0]], false);
    let brep = crate::brep::weaving::wireframes::weave_parametric(&open)
        .expect("open sweep weaves parametrically");
    assert_eq!(brep.edge_count(), 12, "4 longitudinal + 4 start + 4 end rim");
    assert_eq!(brep.vertex_count(), 8, "4 corners × start/end caps");

    let closed = sweep_body(sq, vec![[5.0, 0.0, 0.0], [0.0, 0.0, 5.0], [-5.0, 0.0, 0.0], [0.0, 0.0, -5.0]], true);
    let brep = crate::brep::weaving::wireframes::weave_parametric(&closed)
        .expect("closed sweep weaves parametrically");
    assert_eq!(brep.edge_count(), 4, "4 closed longitudinal loops");
    assert_eq!(brep.vertex_count(), 0, "closed loops have no vertices");
}

#[test]
fn helix_forward_map_inverts_profile_point() {
    // The swept-wireframe traces a profile corner via the FORWARD helix map;
    // it must invert `helix_profile_point` (the SDF's reverse map) so the edges
    // land exactly on the body surface.
    use std::f32::consts::TAU;
    let (pitch, turns, taper, y_anchor, r_inner, start_angle) =
        (10.0f32, 2.0f32, 0.1f32, 0.5f32, 1.0f32, 0.3f32);
    for &left_handed in &[false, true] {
        let hand = if left_handed { -1.0f32 } else { 1.0 };
        for &(cx, cy) in &[(5.0f32, 2.0f32), (3.5, -1.0), (8.0, 0.4)] {
            for &t in &[0.0f32, 1.5, 3.0, TAU * turns] {
                // Forward map (local frame), mirroring build_swept_wireframe.
                let theta = hand * (t + start_angle);
                let y = cy + pitch * t / TAU;
                let r = cx + taper * (y - y_anchor);
                let p = [r * theta.cos(), y, r * theta.sin()];
                let (prof, _ell, _scale, _cap) = crate::helix_profile_point(
                    p, pitch, turns, left_handed, taper, y_anchor, r_inner, start_angle,
                );
                assert!(
                    (prof[0] - cx).abs() < 1e-3 && (prof[1] - cy).abs() < 1e-3,
                    "round-trip lh={left_handed} (cx,cy)=({cx},{cy}) t={t}: got {prof:?}"
                );
            }
        }
    }
}

#[test]
fn parametric_revolve_traces_corner_circles() {
    // Ticket ifog: weave_parametric must produce the longitudinal circle edges
    // of a revolve constructively (one per profile corner), not rely on SSI.
    // Rectangle profile → 4 corners → 4 closed circles (matches the octree
    // revolve_rectangle test). Triangle polygon → 3.
    let rect = ProtoSketch2d::Prim(ProtoSketch2dPrim::Rectangle {
        center: [4.0, 0.0],
        half_extents: [1.0, 1.0],
    });
    let brep = crate::brep::weaving::wireframes::weave_parametric(&revolve_body(rect))
        .expect("rectangle revolve weaves parametrically");
    assert_eq!(brep.edge_count(), 4, "4 corner circles");
    assert_eq!(brep.face_count(), 4, "2 cylinders + 2 planes");
    assert_eq!(brep.vertex_count(), 0, "closed circles have no vertices");
    for e in &brep.edges {
        assert!(e.start.is_none() && e.end.is_none(), "circle = closed loop");
    }
    assert_watertight_solid(&brep);

    // A triangle profile (offset from the axis, all x>0) → 3 corner circles.
    let tri = ProtoSketch2d::Prim(ProtoSketch2dPrim::Polygon {
        vertices: vec![[3.0, -1.0], [5.0, 0.0], [3.0, 1.0]],
    });
    let brep = crate::brep::weaving::wireframes::weave_parametric(&revolve_body(tri))
        .expect("triangle revolve weaves parametrically");
    assert_eq!(brep.edge_count(), 3, "3 corner circles");
    assert_eq!(brep.face_count(), 3, "3 walls");
}

#[test]
fn oblique_triangle_extrude_seed_weaves_all_nine_edges() {
    // Ticket ifog: a triangle EXTRUDE showed only 6 edges (3 top + 3 bottom),
    // missing the 3 vertical wall-wall corner seams. The L-polygon test above
    // uses the OCTREE weave; the live app uses weave_parametric → the SEED
    // weaver, where `plane_plane`'s `line_point` dropped the seam line of two
    // OBLIQUE walls (it omitted the determinant division, valid only for
    // perpendicular planes). A triangle's corners are all oblique AND lie on
    // the body AABB boundary, so every seam was rejected. Guard the seed path.
    let tri = ProtoSketch2d::Prim(ProtoSketch2dPrim::Path {
        segments: vec![
            SketchSegment::Line { start: [-66.25, 384.19], end: [-30.61, 298.87] },
            SketchSegment::Line { start: [-30.61, 298.87], end: [-4.31, 499.70] },
            SketchSegment::Line { start: [-4.31, 499.70], end: [-66.25, 384.19] },
        ],
        closed: true,
    });
    let body = extrude_body(tri, 120.0);
    let brep = crate::brep::weaving::wireframes::weave_parametric(&body)
        .expect("triangle extrude must weave analytically (seed path)");
    assert_eq!(brep.face_count(), 5, "3 walls + 2 caps");
    assert_eq!(
        brep.edge_count(),
        9,
        "3 vertical wall-wall seams + 3 top + 3 bottom rim edges"
    );
}

#[test]
fn extrude_caps_use_reserved_sub_ids() {
    // The cap branch sub-ids must be the reserved 0x7FF / 0x7FE (top of
    // the outer sub range, below the inner-wall flag 0x800; DL-75).
    let sketch = ProtoSketch2d::Prim(ProtoSketch2dPrim::Rectangle {
        center: [0.0, 0.0],
        half_extents: [1.0, 1.0],
    });
    let body = extrude_body(sketch, 1.0);
    let map = branch_halfspace_map(&body);
    let top = BranchId::from_feature_sub(&FeatureId::from_u32(1), BranchId::EXTRUDE_CAP_TOP_SUB);
    let bottom =
        BranchId::from_feature_sub(&FeatureId::from_u32(1), BranchId::EXTRUDE_CAP_BOTTOM_SUB);
    assert!(map.contains_key(&top.as_u64()), "top cap present");
    assert!(map.contains_key(&bottom.as_u64()), "bottom cap present");
    // 4 walls + 2 caps.
    assert_eq!(map.len(), 6);
}

#[test]
fn curved_path_extrude_falls_back_to_tessellated() {
    // A closed path with a quad-bézier group stays tessellated for that
    // curved wall (DL-50 documented limit). The straight Line groups and
    // the caps lift analytically (ticket 9zlq).
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
    // 2 caps + 3 straight walls; the bézier group (G1 group 1) is absent.
    assert_eq!(map.len(), 5, "caps + straight walls analytic, bézier wall not");
    let top = BranchId::from_feature_sub(&FeatureId::from_u32(1), BranchId::EXTRUDE_CAP_TOP_SUB);
    assert!(map.contains_key(&top.as_u64()));
    let bezier = BranchId::from_feature_sub(&FeatureId::from_u32(1), 1);
    assert!(!map.contains_key(&bezier.as_u64()), "bézier wall must stay tessellated");
}

#[test]
fn straight_path_extrude_lifts_all_walls_as_planes() {
    // Ticket 9zlq: a face-extrude profile is a Path of straight Lines.
    // Every G1 line group must lift to an analytic plane wall with an
    // OUTWARD normal, so the prism weaves on the seed path instead of
    // bowing through the octree fallback.
    let path = ProtoSketch2d::Prim(ProtoSketch2dPrim::Path {
        segments: vec![
            SketchSegment::Line { start: [-1.0, -1.0], end: [1.0, -1.0] },
            SketchSegment::Line { start: [1.0, -1.0], end: [1.0, 1.0] },
            SketchSegment::Line { start: [1.0, 1.0], end: [-1.0, 1.0] },
            SketchSegment::Line { start: [-1.0, 1.0], end: [-1.0, -1.0] },
        ],
        closed: true,
    });
    let body = extrude_body(path, 1.0);
    let map = branch_halfspace_map(&body);
    assert_eq!(map.len(), 6, "4 line walls + 2 caps");
    // Group g is segment g here (every corner breaks G1). Expected
    // outward normals for the CCW square: −y, +x, +y, −x; offset 1.
    let expect = [[0.0, -1.0], [1.0, 0.0], [0.0, 1.0], [-1.0, 0.0]];
    for (g, want) in expect.iter().enumerate() {
        // No wall-sub table on a hand-built profile: the walls carry
        // POSITIONAL subs, marked out of the nameable range (#445).
        let b = BranchId::from_feature_sub(
            &FeatureId::from_u32(1),
            BranchId::POSITIONAL_WALL_FLAG | g as u32,
        );
        let (hs, _) = map.get(&b.as_u64()).expect("line wall lifted");
        let HalfSpace::Plane { normal, offset } = hs else {
            panic!("line wall must lift to a plane, got {hs:?}");
        };
        assert!(
            (normal[0] - want[0]).abs() < 1e-6
                && (normal[1] - want[1]).abs() < 1e-6
                && normal[2].abs() < 1e-6,
            "group {g}: outward normal {normal:?}, want {want:?}"
        );
        assert!((offset - 1.0).abs() < 1e-6, "group {g}: offset {offset}");
    }
}

#[test]
fn bent_g1_line_chain_is_not_lifted() {
    // Two lines bending by ~1.4° share a G1 group (cos > 0.999) but are
    // NOT one plane — the straightness gate must reject the lift. This is
    // the exact hazard the old blanket Path skip guarded against.
    let bend = 0.025f32; // radians ≈ 1.43°, cos ≈ 0.99969 > 0.999
    let mid = [1.0, 0.0];
    let end = [1.0 + bend.cos(), bend.sin()];
    let path = ProtoSketch2d::Prim(ProtoSketch2dPrim::Path {
        segments: vec![
            SketchSegment::Line { start: [0.0, 0.0], end: mid },
            SketchSegment::Line { start: mid, end },
            SketchSegment::Line { start: end, end: [1.0, 1.0] },
            SketchSegment::Line { start: [1.0, 1.0], end: [0.0, 1.0] },
            SketchSegment::Line { start: [0.0, 1.0], end: [0.0, 0.0] },
        ],
        closed: true,
    });
    let body = extrude_body(path, 1.0);
    let map = branch_halfspace_map(&body);
    // The bent chain is one G1 group and must be skipped; the remaining
    // 3 straight walls + 2 caps lift.
    let bent = BranchId::from_feature_sub(&FeatureId::from_u32(1), 0);
    assert!(
        !map.contains_key(&bent.as_u64()),
        "a bent G1 chain must not lift to a single plane"
    );
    assert_eq!(map.len(), 5, "3 straight walls + 2 caps");
}

// ── DL-52: within-body booleans (Difference / Intersect) ─────────────

/// Count edges by closed-loop status (start == end == None) and the number
/// of edges whose signature contains `branch`.
fn count_loops_for_branch(brep: &BRep, branch: u64) -> (usize, usize) {
    let mut loops = 0;
    let mut total = 0;
    for e in &brep.edges {
        if e.signature.0.as_u64() == branch || e.signature.1.as_u64() == branch {
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
    // The notched box is an L-prism: its 6-corner cross-section extrudes to
    // 6 side walls + 2 caps = 8 faces, and 6 vertical + 6 top + 6 bottom =
    // 18 straight edges. All edges consolidate to one per signature (DL-51),
    // no over-segmentation. (DL-79: the two convex corner edges where the cut
    // walls meet the box's outer +x / +y faces — signatures (1/0, 2/3) and
    // (4098,8193) at L-corners (1,0) and (0,1) — used to vanish because every
    // octree cell along them only grazed one of the two faces; the outward-
    // seed edge rescue now recovers them, completing the cross-section.)
    assert_eq!(brep.face_count(), 8, "notched box faces");
    assert_eq!(brep.edge_count(), 18, "notched box edges (one per signature)");
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
fn pure_octree_matches_normal_weave_on_plain_box() {
    // Phase 0b: for a non-shelled body the `HalfspaceMap` is empty on BOTH
    // paths (normal: `body_contains_shell` is false; pure: forced empty), so
    // `weave_pure_octree` must be structurally identical to `weave`.
    let csg = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [1.0, 1.0, 1.0] },
    };
    let body = BoundedCsg::from_csg(&csg);
    let octree = Octree::build(&body, 0.25);
    let normal = weave(&body, &octree);
    let pure = weave_pure_octree(&body, &octree);

    assert_eq!(pure.vertex_count(), normal.vertex_count(), "same vertex count");
    assert_eq!(pure.edge_count(), normal.edge_count(), "same edge count");
    assert_eq!(pure.face_count(), normal.face_count(), "same face count");
    assert!(normal.manifold_violations().is_empty(), "normal box manifold");
    assert!(pure.manifold_violations().is_empty(), "pure box manifold");
    assert_eq!(
        pure.euler_characteristic(),
        normal.euler_characteristic(),
        "same Euler characteristic"
    );
}

#[test]
fn pure_octree_matches_normal_weave_on_difference() {
    // Same invariant on a non-shelled Difference (box minus corner notch):
    // empty map on both paths ⇒ pure == normal structurally.
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
    let normal = weave(&body, &octree);
    let pure = weave_pure_octree(&body, &octree);

    assert_eq!(pure.vertex_count(), normal.vertex_count(), "same vertex count");
    assert_eq!(pure.edge_count(), normal.edge_count(), "same edge count");
    assert_eq!(pure.face_count(), normal.face_count(), "same face count");
    // Structural identity: the two paths produce the SAME manifold-violation
    // set (the notched box's normal weave leaves a couple of open boundary
    // edges where Phase-3 misses a vertex cell; the pure path, with the same
    // empty map, reproduces them exactly).
    assert_eq!(
        pure.manifold_violations(),
        normal.manifold_violations(),
        "same manifold violations"
    );
    assert_eq!(
        pure.euler_characteristic(),
        normal.euler_characteristic(),
        "same Euler characteristic"
    );
}

#[test]
fn pure_octree_weaves_shelled_box_without_panic() {
    // Phase 0b smoke test: on a shelled body the pure path deliberately
    // disables the DL-75 analytic fast-path (empty map), so it may diverge
    // from `weave` and need not be manifold — but it must run without panic
    // and still produce a non-empty B-Rep. We only assert liveness here and
    // REPORT whether the output is manifold (a finding, not a requirement).
    let half = 2.0_f32;
    let thickness = 0.5_f32;
    let csg = CsgNode::Shell(
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [half, half, half],
            },
        }),
        thickness,
        Vec::new(),
    );
    let body = BoundedCsg::from_csg(&csg);
    let octree = Octree::build(&body, 0.1);
    let pure = weave_pure_octree(&body, &octree);
    assert!(pure.edge_count() > 0, "pure shelled weave must produce edges");
    println!(
        "pure shelled: V={} E={} F={} manifold_violations={}",
        pure.vertex_count(),
        pure.edge_count(),
        pure.face_count(),
        pure.manifold_violations().len()
    );
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
    let wall = BranchId::from_feature_sub(&FeatureId::from_u32(2), 0).as_u64();
    let (loops, total) = count_loops_for_branch(&brep, wall);
    assert_eq!(total, 2, "bore wall bounded by exactly 2 edges");
    assert_eq!(loops, 2, "both trim edges are closed circular loops");
}

/// DL-68: the general SDF edge tracer recovers a **cylinder** trim seam
/// exactly and resolution-independently — the concrete generalisation past
/// DL-67, whose closed-form circle handled only plane∩sphere / sphere∩sphere.
/// At a coarse octree the bore-wall∩cap seam must still be exactly TWO clean
/// closed loops whose every sample lies on the cylinder of radius 0.5 (so the
/// edge comes from the surfaces, not from how many cells tagged the curve).
#[test]
fn coarse_cylinder_trim_seam_is_exact_resolution_independent() {
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
    // A different (coarser) density than the 0.25 sibling test, to show the
    // trim seam is identical topology regardless of octree spacing.
    let octree = Octree::build(&body, 0.4);
    let brep = weave(&body, &octree);

    let wall = BranchId::from_feature_sub(&FeatureId::from_u32(2), 0).as_u64();
    let (loops, total) = count_loops_for_branch(&brep, wall);
    assert_eq!(total, 2, "bore wall bounded by exactly 2 edges at coarse res");
    assert_eq!(loops, 2, "both trim seams are closed loops at coarse res");

    // Surface-exactness: every trim-seam sample is on the cylinder r=0.5
    // about the Y axis, regardless of the coarse octree spacing.
    for e in &brep.edges {
        if e.signature.0.as_u64() != wall && e.signature.1.as_u64() != wall {
            continue;
        }
        let EdgeGeometry::Hermite(samples) = &e.geometry else {
            continue;
        };
        for s in samples {
            let r = (s.point[0] * s.point[0] + s.point[2] * s.point[2]).sqrt();
            assert!(
                (r - 0.5).abs() < 1e-4,
                "trim sample off the bore cylinder: r={r}, p={:?}",
                s.point
            );
        }
    }
}

/// DL-69: the general SDF edge tracer's [`Field::Leaf`] path recovers a
/// **curved cross-feature** seam — one whose face has no analytic
/// half-space. A full-sweep `Sector` extruded is a cylinder evaluated as a
/// dual-SDF leaf (its wall is *not* in `branch_halfspace_map`); unioned with
/// an analytic cylinder about a perpendicular axis, the wall∩wall seam is a
/// non-planar Steinmetz curve (two closed loops). `fit_line` and `fit_circle`
/// BOTH fail on it (neither a line nor a planar circle), so a clean,
/// surface-exact, resolution-independent recovery is the tracer's doing —
/// the leaf field for the sector wall paired with the analytic field for the
/// cylinder. The same-feature guard keeps the sector's *own* wall∩cap creases
/// off the leaf path (they fall back to the sample fits).
#[test]
fn leaf_field_traces_curved_cross_feature_seam() {
    // Body A: full-sweep sector r=1 extruded ±1.5 → a cylinder about Z, but
    // NON-analytic (a sector wall is not enumerated in branch_halfspace_map),
    // so its wall branch resolves to a Field::Leaf over the extrude subtree.
    let sector = ProtoSketch2d::Prim(ProtoSketch2dPrim::Sector {
        center: [0.0, 0.0],
        radius: 1.0,
        start_angle: 0.0,
        sweep_angle: std::f32::consts::TAU,
    });
    let cyl_z = CsgNode::Extrude { wall_subs: None,
        id: FeatureId::from_u32(1),
        sketch: Box::new(sector),
        half_height: 1.5,
    };
    // Body B: an analytic cylinder r=0.8 about Y (caps at y = ±1.5), so the
    // seam stays clear of all four caps (|y| ≤ 1, |z| ≤ 0.8 on the curve).
    let cyl_y = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Cylinder {
            center: [0.0, 0.0, 0.0],
            radius: 0.8,
            half_height: 1.5,
        },
    };
    let body = BoundedCsg::from_csg(&CsgNode::Union(Box::new(cyl_z), Box::new(cyl_y)));

    // Seam membership: a point on BOTH cylinders — radius about Z is the
    // leaf cylinder A (r=1), radius about Y is the analytic cylinder B (r=0.8).
    let on_a = |p: [f64; 3]| (p[0] * p[0] + p[1] * p[1]).sqrt();
    let on_b = |p: [f64; 3]| (p[0] * p[0] + p[2] * p[2]).sqrt();

    for res in [0.3_f32, 0.4] {
        let octree = Octree::build(&body, res);
        let brep = build_wireframe(&body, &octree);

        let mut seam_edges = 0usize;
        let mut closed_seam_loops = 0usize;
        for e in &brep.edges {
            let EdgeGeometry::Hermite(samples) = &e.geometry else {
                continue;
            };
            // Classify (loose gate): an edge is on the Steinmetz seam iff
            // every sample lies on both cylinders.
            let is_seam = samples
                .iter()
                .all(|s| (on_a(s.point) - 1.0).abs() < 5e-2 && (on_b(s.point) - 0.8).abs() < 5e-2);
            if !is_seam {
                continue;
            }
            seam_edges += 1;
            // Surface-exact on BOTH the leaf cylinder and the analytic one,
            // independent of the octree spacing — the hallmark of the tracer
            // (a coarse-octree gap-chain fragments; fit_circle/fit_line cannot
            // represent this non-planar curve at all).
            for s in samples {
                assert!(
                    (on_a(s.point) - 1.0).abs() < 3e-3,
                    "seam off leaf cylinder A: r={} at res {res}, p={:?}",
                    on_a(s.point),
                    s.point
                );
                assert!(
                    (on_b(s.point) - 0.8).abs() < 3e-3,
                    "seam off analytic cylinder B: r={} at res {res}, p={:?}",
                    on_b(s.point),
                    s.point
                );
            }
            if e.start.is_none() && e.end.is_none() {
                closed_seam_loops += 1;
            }
        }
        assert_eq!(seam_edges, 2, "exactly two Steinmetz seam loops at res {res}");
        assert_eq!(
            closed_seam_loops, 2,
            "both seam loops are closed (no triple-junction vertices) at res {res}"
        );
    }
}

/// DL-69b: a **leaf↔leaf** cross-feature seam now CONSOLIDATES in
/// `build_wireframe`, not just at the tracer level. Leaf A: full-sweep sector
/// r=1 extruded → cylinder about Z (a leaf wall, no analytic half-space).
/// Leaf B: sector r=0.3 extruded then rotated +90° about Y → a thin drill
/// about X (also a leaf). The drill pierces A's wall transversally, so the
/// seam is two small CLOSED loops with NO tangent node (different radii). Both
/// fields are numeric dual SDFs — `fit_line`/`fit_circle` cannot represent the
/// non-planar curve and the legacy gap-chain fragments it. Before the adaptive
/// march-step refinement this fragmented at res 0.15 (≈78 stubs); now the two
/// fields trace two clean, surface-exact closed rings at BOTH resolutions.
/// (A coaxial equal-radius Steinmetz, by contrast, has a genuine degree-4 node
/// where ∇f_a×∇f_b→0; the continuity guard refuses to crawl across it, so that
/// case still falls back to the sample fits until the node is extracted as a
/// vertex — a separate piece.)
#[test]
fn leaf_leaf_transversal_seam_consolidates() {
    use fielddraft_protocol::{Sketch2d, Sketch2dPrimitive, Transform as XF};
    let sector = |r: f32| {
        Sketch2d::Prim(Sketch2dPrimitive::Sector {
            center: [0.0, 0.0],
            radius: r,
            start_angle: 0.0,
            sweep_angle: std::f32::consts::TAU,
        })
    };
    let cyl_z = CsgNode::Extrude { wall_subs: None,
        id: FeatureId::from_u32(1),
        sketch: Box::new(sector(1.0)),
        half_height: 1.5,
    };
    let drill_x = CsgNode::Transform(
        XF {
            translate: [0.0, 0.0, 0.0],
            rotation: [0.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, std::f32::consts::FRAC_1_SQRT_2],
            scale: [1.0, 1.0, 1.0],
        },
        Box::new(CsgNode::Extrude { wall_subs: None,
            id: FeatureId::from_u32(2),
            sketch: Box::new(sector(0.3)),
            half_height: 1.5,
        }),
    );
    let body = BoundedCsg::from_csg(&CsgNode::Union(Box::new(cyl_z), Box::new(drill_x)));

    // Seam membership: on leaf A's wall (radius-about-Z = 1) AND on leaf B's
    // wall (radius-about-X = 0.3).
    let on_a = |p: [f64; 3]| (p[0] * p[0] + p[1] * p[1]).sqrt();
    let on_b = |p: [f64; 3]| (p[1] * p[1] + p[2] * p[2]).sqrt();

    for res in [0.15_f32, 0.2] {
        let octree = Octree::build(&body, res);
        let brep = build_wireframe(&body, &octree);
        let mut seam_edges = 0usize;
        let mut closed_loops = 0usize;
        for e in &brep.edges {
            let EdgeGeometry::Hermite(samples) = &e.geometry else { continue };
            let is_seam = samples
                .iter()
                .all(|s| (on_a(s.point) - 1.0).abs() < 5e-2 && (on_b(s.point) - 0.3).abs() < 5e-2);
            if !is_seam {
                continue;
            }
            seam_edges += 1;
            // Surface-exact on BOTH leaf walls, independent of the octree
            // spacing — the hallmark of the tracer (a gap-chain fragments; the
            // line/circle fits cannot represent this non-planar curve at all).
            for s in samples {
                assert!(
                    (on_a(s.point) - 1.0).abs() < 3e-3,
                    "seam off leaf A wall: r={} at res {res}, p={:?}",
                    on_a(s.point),
                    s.point
                );
                assert!(
                    (on_b(s.point) - 0.3).abs() < 3e-3,
                    "seam off leaf B wall: r={} at res {res}, p={:?}",
                    on_b(s.point),
                    s.point
                );
            }
            if e.start.is_none() && e.end.is_none() {
                closed_loops += 1;
            }
        }
        assert_eq!(seam_edges, 2, "exactly two transversal seam loops at res {res}");
        assert_eq!(
            closed_loops, 2,
            "both leaf↔leaf seam loops are closed (consolidated, not fragmented) at res {res}"
        );
    }
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
/// vertices. The rings are disjoint (share no vertices), so `order_loop_groups`
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
fn order_loop_groups_tapered_cone_classifies_by_parametric_area_not_chord_length() {
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
    let mut groups =
        order_loop_groups(&brep, &eids, &cone_surface(), false).expect("must order into rings");
    assert_eq!(groups.len(), 1, "curved carriers classify into a single face");
    let loops = groups.remove(0);

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
fn order_loop_groups_reflex_polygon_is_one_connected_ring() {
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

    let mut groups = order_loop_groups(&brep, &eids, &plane_xy(), false)
        .expect("reflex polygon must order into a ring");
    assert_eq!(groups.len(), 1, "one connected boundary region");
    let loops = groups.remove(0);
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
fn order_loop_groups_square_with_square_hole_splits_outer_and_inner() {
    // A planar face = big CCW square [0,4]² with a smaller square hole
    // around the centre. The two rings are disjoint; order_loop_groups must
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

    let mut groups =
        order_loop_groups(&brep, &eids, &plane_xy(), false).expect("must order into rings");
    assert_eq!(groups.len(), 1, "hole nests under the outer ring, not a second region");
    let loops = groups.remove(0);
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
            f.branch_id.as_u64()
        );
        for (j, lp) in f.inner.iter().enumerate() {
            assert!(
                lp.is_connected_ring(&brep.edges),
                "face {i} (branch {}) inner[{j}] is not a connected ring",
                f.branch_id.as_u64()
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


// ── #423: a Sector profile's walls must lift analytically ────────────────

/// A pie-wedge extrude used to register only its two CAPS: `collect_prim_walls`
/// skipped `Sector` outright, so all three wall branches were missing from the
/// half-space map. Every candidate branch pair touching a wall then came back
/// `unresolved` from `seed_branch_pairs`, the seed wireframe was empty, and the
/// hybrid weave bailed the whole body to the octree (#423). Now the arc lifts to
/// an infinite cylinder about the extrusion axis and each radial flank to a
/// plane through the sector centre.
#[test]
fn sector_extrude_registers_arc_cylinder_and_both_flank_planes() {
    use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};
    use std::f32::consts::FRAC_PI_2;
    // Quarter wedge in the first quadrant, radius 10, extruded ±2 about z.
    let csg = CsgNode::Extrude {
        wall_subs: None,
        id: FeatureId::from_u32(7),
        sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Sector {
            center: [0.0, 0.0],
            radius: 10.0,
            start_angle: 0.0,
            sweep_angle: FRAC_PI_2,
        })),
        half_height: 2.0,
    };
    let body = BoundedCsg::from_csg(&csg);
    let map = branch_halfspace_map(&body);
    assert_eq!(map.len(), 5, "2 caps + 3 walls, got {:?}", map.len());

    // Positional wall subs — a bare `Sector` has no wall-sub table (#445).
    let sub = |n: u32| {
        let name = crate::csg::BranchId::POSITIONAL_WALL_FLAG | n;
        map.get(&crate::csg::BranchId::from_feature_sub(&FeatureId::from_u32(7), name).as_u64())
            .unwrap_or_else(|| panic!("wall sub {n} missing from the half-space map"))
    };
    // Group 0 — the arc → an infinite cylinder of the sector radius.
    match &sub(0).0 {
        HalfSpace::InfiniteCylinder { radius } => assert!((*radius - 10.0).abs() < 1e-5),
        other => panic!("arc wall must be a cylinder, got {other:?}"),
    }
    // Group 1 — the flank at start_angle = 0 (the +x axis). Its outward
    // normal points AWAY from the interior (which lies at positive y), i.e. −y.
    match &sub(1).0 {
        HalfSpace::Plane { normal, offset } => {
            assert!((normal[0]).abs() < 1e-5 && (normal[1] + 1.0).abs() < 1e-5, "{normal:?}");
            assert!((*offset).abs() < 1e-5, "line through the centre ⇒ offset 0, got {offset}");
        }
        other => panic!("flank must be a plane, got {other:?}"),
    }
    // Group 2 — the flank at 90° (the +y axis), outward normal −x.
    match &sub(2).0 {
        HalfSpace::Plane { normal, offset } => {
            assert!((normal[0] + 1.0).abs() < 1e-5 && normal[1].abs() < 1e-5, "{normal:?}");
            assert!((*offset).abs() < 1e-5, "line through the centre ⇒ offset 0, got {offset}");
        }
        other => panic!("flank must be a plane, got {other:?}"),
    }
}

/// The flank normals must point OUT of the wedge for either sweep sign — a
/// flipped normal would invert the face winding and the weave would classify
/// the sector's material as its complement.
#[test]
fn sector_flank_normals_point_out_of_the_wedge() {
    use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};
    use std::f32::consts::FRAC_PI_2;
    for sweep in [FRAC_PI_2, -FRAC_PI_2] {
        let csg = CsgNode::Extrude {
            wall_subs: None,
            id: FeatureId::from_u32(3),
            sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Sector {
                center: [1.0, 2.0],
                radius: 6.0,
                start_angle: 0.4,
                sweep_angle: sweep,
            })),
            half_height: 1.0,
        };
        let body = BoundedCsg::from_csg(&csg);
        let map = branch_halfspace_map(&body);
        // A point just inside the wedge, at mid-sweep and half the radius.
        let mid = 0.4 + sweep * 0.5;
        let inside = [1.0 + 3.0 * mid.cos(), 2.0 + 3.0 * mid.sin()];
        for n in [1u32, 2] {
            let name = crate::csg::BranchId::POSITIONAL_WALL_FLAG | n;
            let (hs, _) = map
                .get(&crate::csg::BranchId::from_feature_sub(&FeatureId::from_u32(3), name).as_u64())
                .expect("flank present");
            let HalfSpace::Plane { normal, offset } = hs else { panic!("flank is a plane") };
            let d = normal[0] * inside[0] + normal[1] * inside[1] - offset;
            assert!(d < 0.0, "sweep {sweep}: flank {n} normal faces inward (d = {d})");
        }
    }
}

/// #437 Klasse B, the MINIMAL case: a face on a coincidence-class carrier
/// whose edge list is not a ring because it also contains the carrier's
/// INTERIOR seams.
///
/// Two flush boxes, the second one narrower in x:
///
///     A   x ∈ [-1, 1]   y ∈ [-1, 1]   z ∈ [-1, 1]
///     B   x ∈ [ 0, 1]   y ∈ [ 1, 3]   z ∈ [-1, 1]
///
/// A's `+y` plane (y = 1) is therefore only PARTIALLY a membrane: over
/// x ∈ [0, 1] B sits on it and it is interior; over x ∈ [-1, 0] it is a real
/// exterior face. `cull_membrane_branches` is all-or-nothing per branch, so
/// the plane survives whole — and `materialize_faces` hands its face EVERY
/// edge whose signature names it, interior ones included. Measured here: the
/// face gets **7 edges** where its ring needs 4, with `(4098, 4100)` and
/// `(4098, 4101)` appearing TWICE (the ±z rims split at x = 0, one half on
/// each side of B) plus the divider `(4098, 8193)` running through the middle.
/// The ring walk consumes the divider in its first ring and then finds no
/// closure for the rest — the „could not order face loops" fallback.
///
/// That is exactly the #419 shape: branch 41087 there carries 7 edges with a
/// degree histogram of {2: 4, 3: 2} and one interior divider, and none of its
/// edges names 41087 in its signature. The reproduction settles that the cause
/// is a PARTIAL membrane on a coincident carrier and needs no #419 document.
///
/// This test is an INSTRUMENT, not a repair: it pins the defect so it cannot
/// drift unwatched. `open_face_count() == 1` and `is_closed == false` are the
/// current, wrong answers; the correct ones are 0 and `true`, and the fix
/// belongs to the follow-up (per-region membership attribution, which the
/// existing `reattribute_coincident_memberships` does not do — measured: it
/// leaves the count at 1). Both directions matter, so this asserts equality.
///
/// #438 measured two more shapes of the same phenomenon, and a repair has to
/// answer all three at once — see
/// `partial_membrane_with_two_kept_regions_is_one_unclosable_face` (the kept
/// region splits in two, so the branch owes TWO faces) and
/// `partial_membrane_from_flush_remove_leaves_four_faces_open` (on a cut the
/// surplus edges bound NOTHING and have to be dropped, not moved — and that,
/// not this one, is the shape the #419 document is made of).
#[test]
fn partial_membrane_on_coincident_carrier_leaves_face_boundary_open() {
    let a = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [1.0, 1.0, 1.0] },
    };
    let b = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Box { center: [0.5, 2.0, 0.0], half_extents: [0.5, 1.0, 1.0] },
    };
    let body = BoundedCsg::from_csg(&CsgNode::Union(Box::new(a), Box::new(b)));
    let brep = weave_parametric(&body).expect("two flush boxes are parametric-weavable");

    // The offending face: A's +y plane, the partial membrane.
    let bad: Vec<&Face> = brep
        .faces
        .iter()
        .filter(|f| !f.outer.is_connected_ring(&brep.edges))
        .collect();
    assert_eq!(bad.len(), 1, "exactly one face fails to close into a ring");
    assert_eq!(
        bad[0].outer.edges.len(),
        7,
        "the partial membrane's face collects both sides' rims plus the divider"
    );

    assert_eq!(
        brep.open_face_count(),
        1,
        "#437 Klasse B: the partial membrane's face has no traversable boundary"
    );
    assert!(
        !brep.shells[0].is_closed,
        "and the body is consequently not watertight"
    );

    // The union's TRUE topology is a 2×4×2 L-prism: 12 faces would already be
    // too many. Recorded so a repair that changes the face count is visible.
    assert_eq!(
        (brep.vertex_count(), brep.edge_count(), brep.face_count()),
        (14, 23, 11),
        "topology of the flush-box T-junction (instrument, not a target)"
    );
}

/// The same construction WITHOUT the partial membrane — B spans the full x
/// range, so A's `+y` plane is a membrane everywhere and gets culled whole.
/// This one weaves closed, which is what makes the test above a statement
/// about PARTIAL membranes rather than about flush contact in general.
#[test]
fn full_membrane_on_coincident_carrier_still_weaves_closed() {
    let a = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [1.0, 1.0, 1.0] },
    };
    let b = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Box { center: [0.0, 2.0, 0.0], half_extents: [1.0, 1.0, 1.0] },
    };
    let body = BoundedCsg::from_csg(&CsgNode::Union(Box::new(a), Box::new(b)));
    let brep = weave_parametric(&body).expect("two flush boxes are parametric-weavable");
    assert_eq!(brep.open_face_count(), 0, "every face closes into a ring");
    assert!(brep.shells[0].is_closed, "the flush stack is watertight");
}

/// #438 — the partial membrane where the kept region falls into TWO disjoint
/// strips: B straddles the MIDDLE of A's `+y` plane instead of one of its ends.
///
///     A   x ∈ [-1, 1]     y ∈ [-1, 1]   z ∈ [-1, 1]
///     B   x ∈ [-0.5, 0.5] y ∈ [ 1, 3]   z ∈ [-1, 1]
///
/// A's `+y` plane is boundary over x ∈ [-1, -0.5] and over x ∈ [0.5, 1] and
/// interior between them, so its branch owes TWO faces of four edges each. It
/// gets ONE face of TEN, because `materialize_faces` hands a branch every
/// edge whose signature names it and both ±z rims are split into three pieces
/// here, of which the middle one belongs to neither strip.
///
/// INSTRUMENT, not a target: the numbers below are the measured WRONG answers.
/// A repair turns them into 0 open faces, watertight, and `strips == [4, 4]`.
/// Recorded because a repair that only handles the one-strip case of
/// `partial_membrane_on_coincident_carrier_leaves_face_boundary_open` would
/// pass that test and still be wrong here.
#[test]
fn partial_membrane_with_two_kept_regions_is_one_unclosable_face() {
    let a = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [1.0, 1.0, 1.0] },
    };
    let b = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Box { center: [0.0, 2.0, 0.0], half_extents: [0.5, 1.0, 1.0] },
    };
    let body = BoundedCsg::from_csg(&CsgNode::Union(Box::new(a), Box::new(b)));
    let brep = weave_parametric(&body).expect("two flush boxes are parametric-weavable");

    let strips: Vec<usize> = brep
        .faces
        .iter()
        .filter(|f| f.branch_id == BranchId::from_parts(1, 0, 2))
        .map(|f| f.outer.edges.len())
        .collect();
    assert_eq!(strips, vec![10], "A’s +y plane is ONE face of ten edges (owes two of four)");
    assert_eq!(brep.open_face_count(), 1, "and that face has no traversable boundary");
    assert!(!brep.shells[0].is_closed, "so the body is not watertight");
}

/// #438 — the same partial membrane on a REMOVE instead of an add, and it is
/// NOT the same defect. A flush cut whose tool wall lands exactly on the
/// target's `+x` wall over part of its height:
///
///     A     x ∈ [-1, 1]  y ∈ [-1, 1]    z ∈ [-1, 1]
///     tool  x ∈ [ 0, 1]  y ∈ [0.5, 2.5] z ∈ [-2, 2]
///
/// This is the shape the #419 document is made of (`Extrude 8 ∖ Revolve 5`).
/// Measured difference to the union case: there the surplus edges are
/// MIS-ATTRIBUTED — each still bounds one of the carriers that names it — while
/// here they bound NOTHING. The cut removed the material they were supposed to
/// separate, so they are traces on a measure-zero `f = 0` sheet, and a repair
/// has to DROP them, not move them. `drop_fin_edges` does exactly that job but
/// never looks at them, because the membrane cull left them two NOMINAL
/// survivors.
///
/// INSTRUMENT: the correct answer is the notched box's exact topology —
/// 12 vertices, 18 edges, 8 faces (Euler 12 − 18 + 8 = 2), watertight. What the
/// weaver produces is pinned below: four faces without a traversable boundary,
/// the `+x` wall alone carrying seven edges where its ring needs four.
#[test]
fn partial_membrane_from_flush_remove_leaves_four_faces_open() {
    let a = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [1.0, 1.0, 1.0] },
    };
    let tool = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Box { center: [0.5, 1.5, 0.0], half_extents: [0.5, 1.0, 2.0] },
    };
    let body = BoundedCsg::from_csg(&CsgNode::Difference(Box::new(a), Box::new(tool)));
    let brep = weave_parametric(&body).expect("flush cut is parametric-weavable");

    assert_eq!(
        (brep.vertex_count(), brep.edge_count(), brep.face_count()),
        (14, 21, 8),
        "the notched box weaves 14/21/8 where its true topology is 12/18/8"
    );
    assert_eq!(brep.open_face_count(), 4, "four faces have no traversable boundary");
    assert_eq!(brep.manifold_violations().len(), 18, "and 18 edges are used wrongly");
    let plus_x = brep
        .faces
        .iter()
        .find(|f| f.branch_id == BranchId::from_parts(1, 0, 0))
        .expect("A's +x wall survives the cull (it is only partly a membrane)");
    assert_eq!(
        plus_x.outer.edges.len(),
        7,
        "the +x wall collects the notch rims too: 7 edges where its ring needs 4"
    );
}

/// #439 probe — a solid whose walls lie ENTIRELY inside another, with flush
/// caps. Instrument only; prints the face/branch inventory.
#[test]
#[ignore = "measurement harness (#439)"]
fn interior_wall_flush_cap_probe() {
    let a = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [2.0, 2.0, 1.0] },
    };
    let b = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [0.5, 0.5, 1.0] },
    };
    let body = BoundedCsg::from_csg(&CsgNode::Union(Box::new(a), Box::new(b)));
    let brep = weave_parametric(&body).expect("weavable");
    let faces: Vec<String> = brep
        .faces
        .iter()
        .enumerate()
        .map(|(i, f)| {
            format!(
                "{i}:b{} e{}{}",
                f.branch_id.as_u64(),
                f.outer.edges.len(),
                if f.outer.is_connected_ring(&brep.edges) { "" } else { "!" }
            )
        })
        .collect();
    println!("BOX-IN-BOX faces: {faces:?}");
    println!(
        "  open {} counts {:?} closed {}",
        brep.open_face_count(),
        (brep.vertex_count(), brep.edge_count(), brep.face_count()),
        brep.shells[0].is_closed
    );
    println!("  viol {}", brep.manifold_violations().len());
    let sigs: Vec<(u64, u64)> = brep
        .edges
        .iter()
        .map(|e| (e.signature.0.as_u64(), e.signature.1.as_u64()))
        .collect();
    println!("  edge signatures: {sigs:?}");
}

/// #439 — a union operand swallowed WHOLE by its partner must leave no trace
/// in the B-Rep. A 64-facet extrude sits entirely inside a bigger extrude and
/// shares its z = 0 cap plane, so every facet wall is interior and meets ∂body
/// only in the LINE where the cap plane crosses it. Those lines get traced
/// (both fields are zero there, both branches are contributors).
///
/// This is the minimal form of the #419 body-10 defect — and it does NOT
/// reproduce it: measured, this construction was already correct before #439,
/// because `finish_weave`'s manifold-driven repair stage re-attributed the
/// memberships and the extra faces fell out that way. What the real document
/// adds is a body the repair stage cannot rescue (11 more features, a removing
/// revolve and four holes), so there the scratches survived into the shell.
///
/// The test earns its place as a GUARD: the intermediate #439 state that
/// culled the interior carriers without dropping their scratches produced
/// 7 faces / 76 edges here, with the swallowed extrude's cap surviving as a
/// 64-edge face lying inside the big one. The pin is the correct answer on
/// every route — the union of the two IS the big prism.
#[test]
fn interior_extrude_swallowed_whole_leaves_no_face() {
    use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};
    let n = 64;
    let ring: Vec<[f32; 2]> = (0..n)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            [5.0 * a.cos(), 5.0 * a.sin()]
        })
        .collect();
    // Big: z ∈ [0, 15.85].  Small: z ∈ [0, 9.7], entirely inside in xy.
    let big = CsgNode::Transform(
        Transform { translate: [0.0, 0.0, 7.925], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        Box::new(CsgNode::Extrude {
            wall_subs: None,
            id: FeatureId::from_u32(1),
            sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
                center: [0.0, 0.0],
                half_extents: [20.0, 20.0],
            })),
            half_height: 7.925,
        }),
    );
    let small = CsgNode::Transform(
        Transform { translate: [0.0, 0.0, 4.85], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        Box::new(CsgNode::Extrude {
            wall_subs: None,
            id: FeatureId::from_u32(2),
            sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Polygon { vertices: ring })),
            half_height: 4.85,
        }),
    );
    let body = BoundedCsg::from_csg(&CsgNode::Union(Box::new(big), Box::new(small)));
    let brep = weave_parametric(&body).expect("weavable");

    // The seed wireframe DOES trace the interior facets — 140 edges, among
    // them the 64 cap-plane scratches. The repair is downstream of tracing.
    let seed = crate::brep::seed_weaver::seed_wireframe(&body).expect("seed route");
    assert!(
        seed.edges.iter().any(|e| {
            let (a, b) = (e.signature.0.feature_id(), e.signature.1.feature_id());
            a == 2 || b == 2
        }),
        "the tracer must still find the interior facet curves — the cull, not \
         the tracer, is what removes them",
    );

    let branches: std::collections::BTreeSet<u32> =
        brep.faces.iter().map(|f| f.branch_id.feature_id()).collect();
    assert_eq!(
        branches,
        [1u32].into_iter().collect::<std::collections::BTreeSet<u32>>(),
        "only the swallowing extrude may own faces, got {:?}",
        brep.faces.iter().map(|f| f.branch_id.as_u64()).collect::<Vec<_>>(),
    );
    assert_eq!(
        (brep.vertex_count(), brep.edge_count(), brep.face_count()),
        (8, 12, 6),
        "the union of the two IS the big prism",
    );
    assert_eq!(brep.open_face_count(), 0, "every face closes into a ring");
    assert!(brep.manifold_violations().is_empty(), "and it is manifold");
    assert!(brep.shells[0].is_closed, "and watertight");
}
