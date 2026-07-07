use super::*;
use fielddraft_protocol::{
    BoolOp, FeatureId, Primitive, Sketch2d as ProtoSketch2d,
    Sketch2dPrimitive as ProtoSketch2dPrimitive, Transform,
};

// ─── K7.3: from_features → CsgNode::Extrude ────────────────

#[test]
fn from_features_lowers_primitive_extrude_to_csgnode_extrude() {
    // A radius-5 circle sketch extruded ±3 mm along Z
    // should produce a cylindrical SDF: a point at the
    // centre of the slab is inside, a point on the
    // circular boundary sits on the surface (SDF ≈ 0),
    // and a point far from the slab is outside.
    let circle_sketch = ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Circle {
        center: [0.0, 0.0],
        radius: 5.0,
    });
    let features = vec![DecodedFeature::from_parts(
        FeatureId::from_u32(1),
        "extrude".to_string(),
        BoolOp::New,
        Primitive::Extrude {
            sketch: circle_sketch,
            half_depth_z: 3.0,
            center: [0.0, 0.0, 0.0],
        },
        Transform::IDENTITY,
        PrimitiveModifiers::NONE,
        vec![],
    )];
    let csg = CsgNode::from_decoded(&features);
    // Inside at the origin.
    assert!(csg.sdf([0.0, 0.0, 0.0], EvalMode::Logical) < 0.0);
    // On the cylindrical surface at (5, 0, 0).
    let on_surface = csg.sdf([5.0, 0.0, 0.0], EvalMode::Logical).abs();
    assert!(on_surface < 0.01, "expected ~0, got {on_surface}");
    // Outside above the slab.
    assert!(csg.sdf([0.0, 0.0, 10.0], EvalMode::Logical) > 0.0);
    // The bounding AABB matches the cylinder extents.
    let aabb = csg.aabb();
    assert!((aabb.min[0] - (-5.0)).abs() < 1e-5);
    assert!((aabb.max[0] - 5.0).abs() < 1e-5);
    assert!((aabb.min[2] - (-3.0)).abs() < 1e-5);
    assert!((aabb.max[2] - 3.0).abs() < 1e-5);
}

fn box_feature(id: u32, op: BoolOp, center: [f32; 3], half: [f32; 3]) -> DecodedFeature {
    DecodedFeature::from_parts(
        FeatureId::from_u32(id),
        format!("Box {id}"),
        op,
        Primitive::Box {
            center,
            half_extents: half,
        },
        Transform::IDENTITY,
        PrimitiveModifiers::NONE,
        vec![],
    )
}

/// A body-shell modifier: the sentinel `Shell(Empty, t, open)` node on a
/// `Difference` feature targeting `target`, exactly as `ShellFeature`
/// emits it.
fn shell_feature(id: u32, target: u32, thickness: f32, open: Vec<OpenSurface>) -> DecodedFeature {
    DecodedFeature {
        id: FeatureId::from_u32(id),
        name: "Shell".to_string(),
        op: BoolOp::Remove,
        blend_radius: 0.0,
        blend_mode: BlendMode::Fillet,
        target_ids: vec![FeatureId::from_u32(target)],
        transform: Transform::IDENTITY,
        node: CsgNode::Shell(Box::new(CsgNode::Empty), thickness, open),
    }
}

#[test]
fn shell_wall_sdf_closed_is_an_inward_wall() {
    // Closed shell (d_ret = d) → wall from the surface (d = 0) inward to
    // d = −t, hollow beyond. Outer surface preserved.
    let t = 2.0;
    assert!((shell_wall_sdf(0.0, 0.0, t)).abs() < 1e-6, "surface on boundary");
    assert!(shell_wall_sdf(-1.0, -1.0, t) < 0.0, "mid-wall is solid");
    assert!((shell_wall_sdf(-2.0, -2.0, t)).abs() < 1e-6, "inner wall face");
    assert!(shell_wall_sdf(-5.0, -5.0, t) > 0.0, "deep interior is hollow");
}

#[test]
fn open_shell_breaks_through_the_open_face_only() {
    // 20³ box centred at origin, top (+Z) face removed, 2 mm wall.
    let features = vec![
        box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
        shell_feature(
            2,
            1,
            2.0,
            vec![OpenSurface {
                // The box's +Z face is sub 5−1… see `primitive_halfspaces`:
                // sub 4 = plane [0,0,1].
                branch: BranchId::from_feature_sub(&FeatureId::from_u32(1), 4).as_u32(),
                surface: HalfSpace::Plane { normal: [0.0, 0.0, 1.0], offset: 10.0 },
                placement: Transform::IDENTITY,
            }],
        ),
    ];
    let tree = CsgNode::from_decoded(&features);
    let s = |p: [f32; 3]| tree.sdf(p, EvalMode::Logical);

    // Hollow centre, open top, but the side and bottom walls remain solid.
    assert!(s([0.0, 0.0, 0.0]) > 0.0, "centre is hollow");
    assert!(s([0.0, 0.0, 9.0]) > 0.0, "just under the open top is open");
    assert!(s([9.0, 0.0, 0.0]) < 0.0, "+X side wall is solid");
    assert!(s([0.0, 0.0, -9.0]) < 0.0, "bottom wall is solid (closed face)");
    // The outer surface is preserved everywhere — and the rim BAND still
    // ends exactly on the original face plane (the face centre, in
    // contrast, is now plain void: the material there is gone).
    assert!(s([10.0, 0.0, 0.0]).abs() < 1e-4, "+X outer face preserved");
    assert!(s([9.0, 0.0, 10.0]).abs() < 1e-4, "rim band ends on the original face");
    assert!(s([0.0, 0.0, 10.0]) > 0.0, "open face centre is void");
    // Retained-boundary semantics: the wall keeps its FULL 2 mm thickness
    // right up to the opened face — no knife-edge taper at the rim. The
    // rim band on the open plane is solid from x = 8…10.
    assert!(s([9.0, 0.0, 9.9]) < 0.0, "rim band keeps full thickness");
    assert!(s([8.1, 0.0, 9.9]) < 0.0, "rim annulus inner edge at x = 8");
    assert!(s([7.9, 0.0, 9.9]) > 0.0, "cavity reaches the open plane at x < 8");
}

#[test]
fn open_shell_curved_face_leaves_the_retained_plates() {
    // Opening the LATERAL face of a cylinder (sub 0, the InfiniteCylinder
    // wall): the retained boundary is just the two caps, so the shell
    // degenerates to two 2 mm plates — the whole mid-band is open, the
    // material near each cap stays.
    let features = vec![
        DecodedFeature {
            id: FeatureId::from_u32(1),
            name: "Cylinder".to_string(),
            op: BoolOp::New,
            blend_radius: 0.0,
            blend_mode: BlendMode::Fillet,
            target_ids: vec![],
            transform: Transform::IDENTITY,
            node: CsgNode::Leaf {
                id: FeatureId::from_u32(1),
                primitive: Primitive::Cylinder {
                    center: [0.0, 0.0, 0.0],
                    radius: 10.0,
                    half_height: 10.0,
                },
            },
        },
        shell_feature(
            2,
            1,
            2.0,
            vec![OpenSurface {
                branch: BranchId::from_feature_sub(&FeatureId::from_u32(1), 0).as_u32(),
                surface: HalfSpace::InfiniteCylinder { radius: 10.0 },
                placement: Transform::IDENTITY,
            }],
        ),
    ];
    let tree = CsgNode::from_decoded(&features);
    let s = |p: [f32; 3]| tree.sdf(p, EvalMode::Logical);

    assert!(s([9.0, 0.0, 0.0]) > 0.0, "just inside the opened lateral wall is open");
    assert!(s([0.0, 0.0, 0.0]) > 0.0, "axis mid-height is open (no lateral wall left)");
    assert!(s([0.0, 9.0, 0.0]) < 0.0, "top cap plate is solid");
    assert!(s([0.0, -9.0, 0.0]) < 0.0, "bottom cap plate is solid");
    assert!(s([0.0, 7.0, 0.0]) > 0.0, "below the 2 mm top plate is hollow");
}

#[test]
fn open_surface_gate_is_world_distance_under_scaled_placement() {
    // A unit sphere gate carried by a uniform-scale-2 placement is the world
    // sphere of radius 2. The gate must report the *world* Euclidean distance
    // (Eikonal-normalised), not the unscaled local SDF, so the shell taper
    // keeps its true millimetre width regardless of the body's placement scale.
    let scaled = OpenSurface {
        branch: 0,
        surface: HalfSpace::Sphere { radius: 1.0 },
        placement: Transform {
            translate: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [2.0, 2.0, 2.0],
        },
    };
    // World point 2 mm outside the radius-2 surface ⇒ gate = +2.
    assert!((scaled.eval([4.0, 0.0, 0.0]) - 2.0).abs() < 1e-4, "uniform scale: exact world distance");
    // On the surface ⇒ gate ≈ 0.
    assert!(scaled.eval([2.0, 0.0, 0.0]).abs() < 1e-4, "on the scaled surface");

    // Non-uniform scale (x·2): on-axis the gate is still the true distance to
    // the ellipsoid's x semi-axis (= 2), proving the gradient normalisation
    // beats the raw local SDF (which would report 1).
    let aniso = OpenSurface {
        branch: 0,
        surface: HalfSpace::Sphere { radius: 1.0 },
        placement: Transform {
            translate: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [2.0, 1.0, 1.0],
        },
    };
    assert!((aniso.eval([4.0, 0.0, 0.0]) - 2.0).abs() < 1e-4, "non-uniform: on-axis world distance");
}

#[test]
fn offset_halfspace_inward_is_the_inner_wall_surface() {
    // For each analytic kind, a point exactly `t` inside the original surface
    // (its g = -t level set) must lie ON the offset surface (g_offset ≈ 0),
    // and a point on the original surface must read +t (it is `t` *outside*
    // the smaller inner wall). This is the defining property of the inner wall.
    let t = 2.0;
    let cases: Vec<(HalfSpace, [f32; 3], [f32; 3])> = vec![
        // Plane (normal +X, offset 10): surface at x=10, inner wall at x=8.
        (
            HalfSpace::Plane { normal: [1.0, 0.0, 0.0], offset: 10.0 },
            [8.0, 1.0, 1.0],  // on inner wall
            [10.0, 1.0, 1.0], // on original surface
        ),
        // Cylinder radius 10 (axis Y): inner wall radius 8.
        (
            HalfSpace::InfiniteCylinder { radius: 10.0 },
            [8.0, 3.0, 0.0],
            [10.0, 3.0, 0.0],
        ),
        // Sphere radius 10: inner wall radius 8.
        (HalfSpace::Sphere { radius: 10.0 }, [0.0, 8.0, 0.0], [0.0, 10.0, 0.0]),
        // Torus major 10, minor 4: inner wall minor 2, on the tube top.
        (
            HalfSpace::Torus { major_radius: 10.0, minor_radius: 4.0 },
            [10.0, 2.0, 0.0],
            [10.0, 4.0, 0.0],
        ),
    ];
    for (hs, on_inner, on_outer) in cases {
        let inner = offset_halfspace_inward(&hs, t).expect("analytic inner wall");
        assert!(
            sdf_halfspace(&inner, on_inner).abs() < 1e-4,
            "{hs:?}: inner-wall point should be on the offset surface"
        );
        assert!(
            (sdf_halfspace(&inner, on_outer) - t).abs() < 1e-4,
            "{hs:?}: original surface is t outside the inner wall"
        );
    }

    // Degenerate + non-analytic cases yield no inner-wall surface.
    assert!(offset_halfspace_inward(&HalfSpace::Sphere { radius: 1.5 }, 2.0).is_none(), "wall thicker than radius vanishes");
    assert!(offset_halfspace_inward(&HalfSpace::Cone { half_angle: 0.5 }, 2.0).is_none(), "cone nappes separate under offset");
    assert!(offset_halfspace_inward(&HalfSpace::Gyroid { cell_size: 5.0, thickness: 0.3 }, 2.0).is_none(), "TPMS has no analytic offset");
}

#[test]
fn inner_wall_branch_id_scheme() {
    // DL-75 Step 2: the inner-wall id is a distinct branch that preserves
    // `feature_id()` (so UI selection still resolves to the owning feature)
    // and never collides with the reserved extrude caps.
    let outer = BranchId::from_feature_sub(&FeatureId::from_u32(7), 3);
    let inner = outer.inner_wall();
    assert!(!outer.is_inner_wall());
    assert!(inner.is_inner_wall());
    assert_ne!(outer.as_u32(), inner.as_u32(), "inner wall is a distinct id");
    assert_eq!(inner.feature_id(), outer.feature_id(), "feature_id preserved");
    assert_eq!(inner.feature_id(), 7);
    assert_eq!(inner.inner_wall().as_u32(), inner.as_u32(), "idempotent");

    // The two extrude caps live below the inner-wall flag, so a cap and its
    // inner wall are distinct and the flag never collides with a cap.
    const { assert!(BranchId::EXTRUDE_CAP_TOP_SUB < BranchId::INNER_WALL_FLAG) };
    const { assert!(BranchId::EXTRUDE_CAP_BOTTOM_SUB < BranchId::INNER_WALL_FLAG) };
    let cap = BranchId::from_feature_sub(&FeatureId::from_u32(7), BranchId::EXTRUDE_CAP_TOP_SUB);
    assert!(!cap.is_inner_wall());
    assert_ne!(cap.as_u32(), cap.inner_wall().as_u32());
}

#[test]
fn empty_tree_has_infinite_sdf_and_empty_aabb() {
    let tree = CsgNode::from_decoded(&[]);
    assert!(tree.sdf([0.0, 0.0, 0.0], EvalMode::Logical).is_infinite());
    assert!(tree.aabb().is_empty());
}

#[test]
fn single_box_tree_matches_primitive_sdf() {
    let features = vec![box_feature(
        1,
        BoolOp::New,
        [0.0, 0.0, 0.0],
        [10.0, 10.0, 10.0],
    )];
    let tree = CsgNode::from_decoded(&features);
    // Direct geometric SDF of a box with half-extents [10,10,10] at
    // the origin: negative inside, 0 on the +X face, positive outside.
    let expected = [
        ([0.0, 0.0, 0.0], -10.0),
        ([10.0, 0.0, 0.0], 0.0),
        ([20.0, 0.0, 0.0], 10.0),
    ];
    for &(p, want) in &expected {
        let a = tree.sdf(p, EvalMode::Logical);
        assert!((a - want).abs() < 1e-4, "{a} vs {want} at {p:?}");
    }
}

#[test]
fn tree_sdf_matches_linear_fold_for_difference() {
    let features = vec![
        box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
        DecodedFeature::from_parts(
            FeatureId::from_u32(2),
            "hole".into(),
            BoolOp::Remove,
            Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius: 3.0,
            },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        ),
    ];
    let tree = CsgNode::from_decoded(&features);
    // Box half [10,10,10] at origin MINUS sphere radius 3 at origin.
    // Solid SDF = max(box_sdf, -sphere_sdf); values precomputed.
    let expected = [
        ([0.0, 0.0, 0.0], 3.0),
        ([5.0, 0.0, 0.0], -2.0),
        ([10.0, 0.0, 0.0], 0.0),
        ([-5.0, -5.0, -5.0], -5.0),
        ([50.0, 0.0, 0.0], 40.0),
    ];
    for (p, want) in expected {
        let a = tree.sdf(p, EvalMode::Logical);
        assert!(
            (a - want).abs() < 1e-4,
            "tree {a} vs expected {want} at {p:?}",
        );
    }
}

#[test]
fn aabb_of_box_matches_half_extents() {
    let tree = CsgNode::from_decoded(&[box_feature(
        1,
        BoolOp::New,
        [0.0, 0.0, 0.0],
        [5.0, 7.0, 9.0],
    )]);
    let bb = tree.aabb();
    assert_eq!(bb.min, [-5.0, -7.0, -9.0]);
    assert_eq!(bb.max, [5.0, 7.0, 9.0]);
}

#[test]
fn union_aabb_covers_both_children() {
    let features = vec![
        box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        box_feature(2, BoolOp::New, [20.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
    ];
    let bb = CsgNode::from_decoded(&features).aabb();
    assert_eq!(bb.min, [-5.0, -5.0, -5.0]);
    assert_eq!(bb.max, [25.0, 5.0, 5.0]);
}

// ─── provenance / evaluate ─────────────────────────────────────

fn cyl_feature(id: u32, center: [f32; 3], radius: f32, half_height: f32) -> DecodedFeature {
    DecodedFeature::from_parts(
        FeatureId::from_u32(id),
        format!("Cyl {id}"),
        BoolOp::New,
        Primitive::Cylinder {
            center,
            radius,
            half_height,
        },
        Transform::IDENTITY,
        PrimitiveModifiers::NONE,
        vec![],
    )
}

fn sphere_feature(id: u32, center: [f32; 3], radius: f32) -> DecodedFeature {
    DecodedFeature::from_parts(
        FeatureId::from_u32(id),
        format!("Sphere {id}"),
        BoolOp::New,
        Primitive::Sphere { center, radius },
        Transform::IDENTITY,
        PrimitiveModifiers::NONE,
        vec![],
    )
}

/// Tiny convenience used in the contributor-equality assertions:
/// build a `Vec<BranchId>` from `(feature, sub)` pairs. Sorted so
/// callers can compare against an order-independent expectation.
fn branches(raw: &[(u32, u32)]) -> Vec<BranchId> {
    let mut v: Vec<BranchId> = raw
        .iter()
        .map(|&(f, sub)| BranchId::from_feature_sub(&FeatureId::from_u32(f), sub))
        .collect();
    v.sort_by_key(|b| b.as_u32());
    v
}

#[test]
fn evaluate_on_solo_face_returns_single_contributor() {
    // Union(Box=1, Cylinder=2) with the cylinder poking out the
    // top of the box. Point on the box top, but outside the
    // cylinder footprint → only the box contributes.
    let features = vec![
        box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
        cyl_feature(2, [0.0, 0.0, 0.0], 10.0, 50.0),
    ];
    let tree = CsgNode::from_decoded(&features);
    let (d, c) = tree.evaluate([15.0, 20.0, 0.0], 0.5, EvalMode::Logical);
    assert!(d.abs() < 1e-4, "point should sit on box top, got d={d}");
    assert_eq!(c.element(), ProxyElement::Face);
    // Box +Y face is sub-index 2 (see `primitive_halfspaces`).
    assert_eq!(c.as_slice(), branches(&[(1, 2)]).as_slice());
}

#[test]
fn evaluate_on_emergent_edge_returns_two_contributors() {
    // The cylinder's lateral surface (x²+z²=100) meets the box's
    // top face (y=20) along the circle x²+z²=100 at y=20. At
    // (10, 20, 0) both leaves' individual SDFs are zero → the
    // point is an Edge in the B-Rep proxy.
    let features = vec![
        box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
        cyl_feature(2, [0.0, 0.0, 0.0], 10.0, 50.0),
    ];
    let tree = CsgNode::from_decoded(&features);
    let (d, c) = tree.evaluate([10.0, 20.0, 0.0], 0.5, EvalMode::Logical);
    assert!(d.abs() < 1e-4, "should lie on both surfaces, got d={d}");
    assert_eq!(c.element(), ProxyElement::Edge);
    let mut got: Vec<BranchId> = c.as_slice().to_vec();
    got.sort_by_key(|b| b.as_u32());
    // box +Y face (sub 2) meets cylinder lateral wall (sub 0).
    assert_eq!(got, branches(&[(1, 2), (2, 0)]));
}

#[test]
fn evaluate_on_triple_intersection_returns_vertex() {
    // Three axis-aligned boxes meeting at a single corner (+x,+y,
    // +z). At the shared corner (10, 10, 10) all three box
    // surfaces pass through the same point → contributors = 3 →
    // classification = Vertex.
    let features = vec![
        box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
        box_feature(2, BoolOp::New, [20.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
        box_feature(3, BoolOp::New, [10.0, 20.0, 0.0], [10.0, 10.0, 10.0]),
    ];
    let tree = CsgNode::from_decoded(&features);
    // All three boxes meet at (10, 10, 10).
    let (d, c) = tree.evaluate([10.0, 10.0, 10.0], 0.5, EvalMode::Logical);
    assert!(d.abs() < 1e-4);
    // With per-face decomposition each box reports several of its
    // own faces at the shared corner, so the multiplicity is well
    // above three — still a Vertex. The provenance must name all
    // three feature ids (sub-index stripped).
    assert_eq!(c.element(), ProxyElement::Vertex);
    let mut feats: Vec<u32> = c.as_slice().iter().map(|b| b.feature_id()).collect();
    feats.sort_unstable();
    feats.dedup();
    assert_eq!(feats, vec![1, 2, 3]);
}

#[test]
fn difference_preserves_both_contributors_on_hole_rim() {
    // Box (id=1) minus a sphere (id=2) punched out of its corner.
    // On the circle where the sphere exits the box's +x face the
    // point lies on *both* the box's +x face and the sphere's
    // lateral surface → the difference SDF is zero via the tie
    // `max(box_sdf, -sphere_sdf)`.
    let features = vec![
        box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
        {
            let mut f = sphere_feature(2, [10.0, 0.0, 0.0], 4.0);
            f.op = BoolOp::Remove;
            f
        },
    ];
    let tree = CsgNode::from_decoded(&features);
    // (10, 0, 4) is on the box's +x face (x=10) AND on the
    // sphere's surface (distance from (10,0,0) = 4).
    let (d, c) = tree.evaluate([10.0, 0.0, 4.0], 0.5, EvalMode::Logical);
    assert!(d.abs() < 1e-4, "should lie on both boundaries, got d={d}");
    assert_eq!(c.element(), ProxyElement::Edge);
    let mut got: Vec<BranchId> = c.as_slice().to_vec();
    got.sort_by_key(|b| b.as_u32());
    // box +X face (sub 0) meets the sphere (un-split, sub 0).
    assert_eq!(got, branches(&[(1, 0), (2, 0)]));
}

#[test]
fn expand_evaluate_leaf_box_face_and_edge_subfaces() {
    // The B-Rep substrate: a single Box leaf, evaluated directly
    // through `expand_evaluate_leaf`, must hand back the per-face
    // BranchId(s) of the half-space decomposition.
    let id = FeatureId::from_u32(1);
    let prim = Primitive::Box {
        center: [0.0, 0.0, 0.0],
        half_extents: [10.0, 10.0, 10.0],
    };
    let eps = 0.25;

    // +X face centre → exactly one contributor: the +X face (sub 0).
    let (d, c) = expand_evaluate_leaf(&id, &prim, [10.0, 0.0, 0.0], eps);
    assert!(d.abs() < 1e-4, "on +X face, got d={d}");
    assert_eq!(c.element(), ProxyElement::Face);
    assert_eq!(c.as_slice(), branches(&[(1, 0)]).as_slice());

    // A point on the +X/+Y edge → the two adjoining face sub-ids
    // (sub 0 = +X, sub 2 = +Y), in some order.
    let (d, c) = expand_evaluate_leaf(&id, &prim, [10.0, 10.0, 0.0], eps);
    assert!(d.abs() < 1e-4, "on +X/+Y edge, got d={d}");
    assert_eq!(c.element(), ProxyElement::Edge);
    let mut got: Vec<BranchId> = c.as_slice().to_vec();
    got.sort_by_key(|b| b.as_u32());
    assert_eq!(got, branches(&[(1, 0), (1, 2)]));
}

#[test]
fn evaluate_far_from_surface_still_has_one_contributor() {
    // Inside a single leaf, nearer to one face than any other →
    // the dominant contributor is the unambiguous nearest face
    // (provenance still records which leaf owns that slab). A
    // point off-centre (closer to +Z) avoids the box-centre tie
    // where all six faces are equidistant.
    let features = vec![box_feature(
        1,
        BoolOp::New,
        [0.0, 0.0, 0.0],
        [10.0, 10.0, 10.0],
    )];
    let tree = CsgNode::from_decoded(&features);
    let (d, c) = tree.evaluate([0.0, 0.0, 5.0], 0.5, EvalMode::Logical);
    assert!(d < 0.0);
    // +Z face is sub-index 4.
    assert_eq!(c.as_slice(), branches(&[(1, 4)]).as_slice());
}

#[test]
fn contributors_push_dedupes_and_caps() {
    let b = |n: u32| BranchId::from_feature(&FeatureId::from_u32(n));
    let mut c = Contributors::empty();
    // Eight distinct ids fill the set, with a dup and an overflow.
    for n in [1u32, 2, 1, 3, 4, 5, 6, 7, 8, 9] {
        c.push(b(n));
    }
    assert_eq!(c.len(), MAX_CONTRIBUTORS);
    let mut got: Vec<BranchId> = c.as_slice().to_vec();
    got.sort_by_key(|x| x.as_u32());
    assert_eq!(
        got,
        branches(&[(1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (6, 0), (7, 0), (8, 0)])
    );
}

// ─── extrude / revolve ─────────────────────────────────────────

use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};

fn rect_sketch(cx: f32, cy: f32, hx: f32, hy: f32) -> Sketch2d {
    Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
        center: [cx, cy],
        half_extents: [hx, hy],
    })
}

fn circle_sketch(cx: f32, cy: f32, r: f32) -> Sketch2d {
    Sketch2d::Prim(Sketch2dPrimitive::Circle {
        center: [cx, cy],
        radius: r,
    })
}

/// Thread-like helix: rectangle profile r∈[8,10], y∈[−0.6,0.6],
/// pitch 2mm, 5 turns, right-handed, no taper.
fn test_helix() -> CsgNode {
    CsgNode::Helix {
        id: FeatureId::from_u32(1),
        sketch: Box::new(rect_sketch(9.0, 0.0, 1.0, 0.6)),
        pitch: 2.0,
        turns: 5.0,
        left_handed: false,
        taper: 0.0,
        start_angle: 0.0,
        y_anchor: 0.0,
        r_inner: 8.0,
    }
}

#[test]
fn sweep_gatekeeper_pass_emits_a_parametric_leaf() {
    // A gentle straight sweep clears the Gatekeeper → parametric leaf.
    let path: Vec<[f32; 3]> = (0..=8).map(|i| [i as f32 * 5.0, 0.0, 0.0]).collect();
    let node = CsgNode::Sweep {
        id: FeatureId::from_u32(1),
        sketch: Box::new(circle_sketch(0.0, 0.0, 5.0)),
        path,
        closed: false,
        twist: 0.0,
    };
    let info = crate::bvh::BoundedCsg::from_csg(&node).classify_for_hybrid();
    let leaf = info
        .leaves
        .iter()
        .find(|l| l.kind == "sweep")
        .expect("a passing sweep must emit a sweep leaf");
    let sweep = leaf.sweep.as_ref().expect("sweep leaf carries its spine");
    assert_eq!(sweep.points.len(), 9);
    assert!(!sweep.closed);
    assert!(leaf.contour.is_some(), "leaf carries the profile contour");
}

#[test]
fn self_intersecting_sweep_fails_the_gate_and_emits_no_leaf() {
    // Limaçon (inner loop crosses the origin) + thin profile → the
    // Gatekeeper's global check fails → no parametric leaf, SDF only.
    let path: Vec<[f32; 3]> = (0..120)
        .map(|i| {
            let th = i as f32 / 120.0 * std::f32::consts::TAU;
            let r = 20.0 + 50.0 * th.cos();
            [r * th.cos(), r * th.sin(), 0.0]
        })
        .collect();
    let node = CsgNode::Sweep {
        id: FeatureId::from_u32(1),
        sketch: Box::new(circle_sketch(0.0, 0.0, 0.5)),
        path,
        closed: true,
        twist: 0.0,
    };
    let info = crate::bvh::BoundedCsg::from_csg(&node).classify_for_hybrid();
    assert!(
        info.leaves.iter().all(|l| l.kind != "sweep"),
        "a self-intersecting sweep must NOT emit a parametric leaf"
    );
    assert!(info.needs_full_sdf, "it falls back to the SDF path");
}

#[test]
fn sweep_along_a_straight_path_is_a_capped_cylinder() {
    // Disk radius 5 swept along X from 0..40 → a capped cylinder.
    let path: Vec<[f32; 3]> = (0..=8).map(|i| [i as f32 * 5.0, 0.0, 0.0]).collect();
    let node = CsgNode::Sweep {
        id: FeatureId::from_u32(1),
        sketch: Box::new(circle_sketch(0.0, 0.0, 5.0)),
        path,
        closed: false,
        twist: 0.0,
    };
    // Unbounded (picking) eval — builds the SweepPath on the fly.
    let d = |p: [f32; 3]| node.sdf(p, EvalMode::Visual);
    assert!((d([20.0, 0.0, 0.0]) - (-5.0)).abs() < 1e-2, "axis → -5");
    assert!((d([20.0, 8.0, 0.0]) - 3.0).abs() < 1e-2, "3 outside the wall");
    assert!((d([42.0, 0.0, 0.0]) - 2.0).abs() < 1e-2, "2 past the cap");
    assert!(d([10.0, 5.0, 0.0]).abs() < 1e-2, "on the lateral surface");

    // Bounded (render) eval — uses the cached SweepPath built in `build`.
    let bounded = crate::bvh::BoundedCsg::from_csg(&node);
    let db = |p: [f32; 3]| bounded.sdf(p, EvalMode::Visual);
    assert!((db([20.0, 0.0, 0.0]) - (-5.0)).abs() < 1e-2);
    assert!((db([20.0, 8.0, 0.0]) - 3.0).abs() < 1e-2);
    assert!((db([42.0, 0.0, 0.0]) - 2.0).abs() < 1e-2);
}

#[test]
fn helix_sdf_winds_with_the_pitch() {
    let h = test_helix();
    let d = |p: [f32; 3]| h.sdf(p, EvalMode::Visual);

    // The sweep starts AT t=0 (θ=0, y=0) — the ridge centre there
    // lies ON the start cap (d ≈ 0). Probe a quarter turn in
    // (t = π/2, y advanced by pitch/4): solidly inside.
    assert!(d([0.0, 0.5, 9.0]) < 0.0, "ridge centre must be inside");
    // Half a pitch later at the same angle: the groove between turns.
    assert!(d([0.0, 1.5, 9.0]) > 0.0, "between turns must be outside");
    // Helical invariance away from the ends: a quarter turn (t=π/2)
    // vs half a turn (t=π) sit at the same profile depth.
    let d90 = d([0.0, 0.5, 9.0]);
    let d180 = d([-9.0, 1.0, 0.0]);
    assert!(
        (d90 - d180).abs() < 1e-4,
        "helical invariance violated: {d90} vs {d180}"
    );
    // Without the y-advance the quarter-turn point is off the ridge.
    assert!(d([0.0, 0.0, 9.0]) > d90 + 0.1);
    // Turn 1 instance exists one pitch up (t=2π, interior).
    assert!(d([9.0, 2.0, 0.0]) < 0.0);
    // Beyond the last turn: outside.
    assert!(d([9.0, 12.0, 0.0]) > 0.0);
    // Radially off the profile: outside on both sides.
    assert!(d([11.5, 1.0, 0.0]) > 0.0);
    assert!(d([6.5, 1.0, 0.0]) > 0.0);
    // On the axis: well outside (bore), field positive and finite.
    let axis = d([0.0, 0.0, 0.0]);
    assert!(axis > 0.0 && axis.is_finite());
}

/// Ticket vtyh: the finite sweep must stay CONTINUOUS across the
/// `atan2` branch seam (θ = ±π) in the end regions — the old
/// per-winding clamp jumped `w` by a full pitch there, conjuring a
/// phantom ribbon below the first winding (negative field where the
/// real solid has ended).
#[test]
fn helix_ends_are_continuous_across_the_seam() {
    let h = test_helix();
    let d = |p: [f32; 3]| h.sdf(p, EvalMode::Visual);
    let eps = 1e-3;
    // Pairs straddling the seam (z → ±0 at x < 0), below the start
    // (y = −1) and beyond the end (y = 11) at the profile radius.
    for y in [-1.0_f32, 11.0] {
        let a = d([-9.0, y, eps]);
        let b = d([-9.0, y, -eps]);
        assert!(
            (a - b).abs() < 1e-2,
            "seam discontinuity at y={y}: {a} vs {b}"
        );
        // …and both sides are OUTSIDE (no phantom material).
        assert!(a > 0.0 && b > 0.0, "phantom material at y={y}: {a}/{b}");
    }
    // Just behind the start cap (θ slightly negative, profile-deep
    // point): a thin positive gap, not phantom-inside.
    assert!(d([9.0, 0.0, -0.5]) > 0.0, "behind the start cap must be open");
    // Just past the start cap (θ slightly positive): inside.
    assert!(d([9.0, 0.05, 0.5]) < 0.0, "past the start cap must be solid");
}

/// vfqh: `start_angle` rotates the whole sweep around the axis —
/// the start cap moves to the given angle (measured in the winding
/// direction) and the ridge follows.
#[test]
fn helix_start_angle_rotates_the_sweep() {
    let mut h = test_helix();
    if let CsgNode::Helix { start_angle, .. } = &mut h {
        *start_angle = std::f32::consts::FRAC_PI_2;
    }
    let d = |p: [f32; 3]| h.sdf(p, EvalMode::Visual);
    // A quarter turn AFTER the rotated start (θ=π, axial advance
    // pitch/4): on the ridge.
    assert!(d([-9.0, 0.5, 0.0]) < 0.0, "rotated ridge must be inside");
    // A quarter turn BEFORE the rotated start (θ=0): off the sweep.
    assert!(d([9.0, 0.0, 0.0]) > 0.0, "before the rotated start must be open");
}

#[test]
fn helix_left_handed_flips_the_winding() {
    let mut h = test_helix();
    if let CsgNode::Helix { left_handed, .. } = &mut h {
        *left_handed = true;
    }
    let d = |p: [f32; 3]| h.sdf(p, EvalMode::Visual);
    // The right-handed quarter-turn ridge point is now off the
    // sweep entirely (its handed angle sits before the start)…
    assert!(d([0.0, 0.5, 9.0]) > 0.0);
    // …and the mirrored quarter turn (θ = −π/2 → handed t = π/2,
    // same +pitch/4 axial advance) is on the ridge.
    assert!(d([0.0, 0.5, -9.0]) < 0.0);
}

#[test]
fn helix_aabb_covers_all_turns() {
    let h = test_helix();
    let bb = h.aabb();
    // Radial: the profile reaches r=10.
    assert!(bb.min[0] <= -10.0 && bb.max[0] >= 10.0);
    assert!(bb.min[2] <= -10.0 && bb.max[2] >= 10.0);
    // Axial: turn 4's instance tops out near y = 8.6 plus the
    // in-turn rise; the AABB must cover it.
    assert!(bb.min[1] <= -0.6);
    assert!(bb.max[1] >= 10.0);
}

#[test]
fn extrude_of_rectangle_matches_3d_box_primitive() {
    // Extruding an XY rectangle of half-extents (hx, hy) by
    // half_height hh should give exactly the same signed
    // distance as a 3D Box primitive of half-extents
    // (hx, hy, hh). The corner-handling matches iq's
    // `opExtrusion` which computes the true Euclidean distance.
    let hx = 7.0;
    let hy = 4.0;
    let hh = 3.0;
    let sketch = rect_sketch(0.0, 0.0, hx, hy);
    let extrude = CsgNode::Extrude {
        id: FeatureId::from_u32(1),
        sketch: Box::new(sketch),
        half_height: hh,
    };
    let boxy = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Box {
            center: [0.0, 0.0, 0.0],
            half_extents: [hx, hy, hh],
        },
    };

    // Inside (centre), deep negative.
    let center = [0.0, 0.0, 0.0];
    assert!(
        (extrude.sdf(center, EvalMode::Visual) - boxy.sdf(center, EvalMode::Visual)).abs() < 1e-5,
    );
    // On each of the three face midpoints → 0.
    for p in [[hx, 0.0, 0.0], [0.0, hy, 0.0], [0.0, 0.0, hh]] {
        let a = extrude.sdf(p, EvalMode::Visual);
        let b = boxy.sdf(p, EvalMode::Visual);
        assert!((a - b).abs() < 1e-4, "surface mismatch at {p:?}: {a} vs {b}");
        assert!(a.abs() < 1e-4);
    }
    // Diagonal corner — true Euclidean distance to corner.
    let corner = [hx + 3.0, hy + 4.0, hh + 12.0];
    let a = extrude.sdf(corner, EvalMode::Visual);
    let b = boxy.sdf(corner, EvalMode::Visual);
    assert!((a - b).abs() < 1e-3, "corner mismatch: {a} vs {b}");
    assert!(a > 12.0); // definitely outside
}

#[test]
fn extrude_aabb_lifts_sketch_by_half_height() {
    let extrude = CsgNode::Extrude {
        id: FeatureId::from_u32(1),
        sketch: Box::new(rect_sketch(0.0, 0.0, 5.0, 2.0)),
        half_height: 3.0,
    };
    let bb = extrude.aabb();
    assert_eq!(bb.min, [-5.0, -2.0, -3.0]);
    assert_eq!(bb.max, [5.0, 2.0, 3.0]);
}

#[test]
fn extrude_returns_single_contributor() {
    let extrude = CsgNode::Extrude {
        id: FeatureId::from_u32(42),
        sketch: Box::new(rect_sketch(0.0, 0.0, 5.0, 2.0)),
        half_height: 3.0,
    };
    let (d, c) = extrude.evaluate([5.0, 0.0, 0.0], 0.5, EvalMode::Visual);
    assert!(d.abs() < 1e-4);
    assert_eq!(c.as_slice(), branches(&[(42, 0)]).as_slice());
    assert_eq!(c.element(), ProxyElement::Face);
}

#[test]
fn extrude_of_path_square_carries_per_segment_sub_ids() {
    use crate::sketch2d::SketchSegment;
    // Unit square as a 4-segment Path (CCW). Each side is its
    // own G1 group: bottom=0, right=1, top=2, left=3.
    let path = Sketch2d::Prim(Sketch2dPrimitive::Path {
        segments: vec![
            SketchSegment::Line { start: [-1.0, -1.0], end: [1.0, -1.0] },
            SketchSegment::Line { start: [1.0, -1.0], end: [1.0, 1.0] },
            SketchSegment::Line { start: [1.0, 1.0], end: [-1.0, 1.0] },
            SketchSegment::Line { start: [-1.0, 1.0], end: [-1.0, -1.0] },
        ],
        closed: true,
    });
    let fid = 9u32;
    let extrude = CsgNode::Extrude {
        id: FeatureId::from_u32(fid),
        sketch: Box::new(path),
        half_height: 3.0,
    };
    // Mid-wall probes with a tight band: each maps to exactly its own
    // wall group (the DL-65 contributor band only ties at a corner/rim, so
    // a clearly-nearest wall stays a single contributor). +x wall → group 1.
    let (_, c) = extrude.evaluate([4.0, 0.0, 0.0], 0.05, EvalMode::Visual);
    assert_eq!(c.as_slice(), branches(&[(fid, 1)]).as_slice());
    // Bottom (−y) wall → group 0.
    let (_, c) = extrude.evaluate([0.0, -4.0, 0.0], 0.05, EvalMode::Visual);
    assert_eq!(c.as_slice(), branches(&[(fid, 0)]).as_slice());
    // Top (+y) wall → group 2 — distinct sub-id.
    let (_, c) = extrude.evaluate([0.0, 4.0, 0.0], 0.05, EvalMode::Visual);
    assert_eq!(c.as_slice(), branches(&[(fid, 2)]).as_slice());
}

#[test]
fn extrude_corner_reports_both_walls_and_rim_reports_wall_and_cap() {
    use crate::sketch2d::SketchSegment;
    // Unit square Path (bottom=0, right=1, top=2, left=3), extruded ±3.
    let path = Sketch2d::Prim(Sketch2dPrimitive::Path {
        segments: vec![
            SketchSegment::Line { start: [-1.0, -1.0], end: [1.0, -1.0] },
            SketchSegment::Line { start: [1.0, -1.0], end: [1.0, 1.0] },
            SketchSegment::Line { start: [1.0, 1.0], end: [-1.0, 1.0] },
            SketchSegment::Line { start: [-1.0, 1.0], end: [-1.0, -1.0] },
        ],
        closed: true,
    });
    let fid = 9u32;
    let extrude = CsgNode::Extrude {
        id: FeatureId::from_u32(fid),
        sketch: Box::new(path),
        half_height: 3.0,
    };

    // DL-65: at the (+x,+y) sketch corner the band ties the right (1) and
    // top (2) walls → a vertical wall∩wall edge reports BOTH branches, so
    // the both-branches gate sees a real edge instead of a singleton.
    let (_, c) = extrude.evaluate([1.0, 1.0, 0.0], 0.25, EvalMode::Visual);
    let s: std::collections::HashSet<u32> =
        c.as_slice().iter().map(|b| b.as_u32()).collect();
    assert!(
        s.contains(&BranchId::from_feature_sub(&FeatureId::from_u32(fid), 1).as_u32())
            && s.contains(
                &BranchId::from_feature_sub(&FeatureId::from_u32(fid), 2).as_u32()
            ),
        "corner must report both adjoining walls, got {s:?}"
    );
    assert_eq!(c.element(), ProxyElement::Edge);

    // At the top rim (mid-wall, z = +half_height) the wall group and the
    // top cap tie → a wall∩cap edge reports both branches.
    let (_, c) = extrude.evaluate([1.0, 0.0, 3.0], 0.25, EvalMode::Visual);
    let s: std::collections::HashSet<u32> =
        c.as_slice().iter().map(|b| b.as_u32()).collect();
    assert!(
        s.contains(&BranchId::from_feature_sub(&FeatureId::from_u32(fid), 1).as_u32())
            && s.contains(
                &BranchId::from_feature_sub(
                    &FeatureId::from_u32(fid),
                    BranchId::EXTRUDE_CAP_TOP_SUB
                )
                .as_u32()
            ),
        "rim must report the wall group and the top cap, got {s:?}"
    );
    assert_eq!(c.element(), ProxyElement::Edge);
}

#[test]
fn revolve_of_offset_rectangle_gives_a_hollow_tube() {
    // A 4 mm × 20 mm rectangle parked at (r=5, y=0) revolves
    // around Y into a tube: inner radius 3, outer radius 7,
    // axial extent ±10.
    let rect = rect_sketch(5.0, 0.0, 2.0, 10.0);
    let tube = CsgNode::Revolve {
        id: FeatureId::from_u32(1),
        sketch: Box::new(rect),
    };

    // Outer surface (r=7, y=0).
    assert!(tube.sdf([7.0, 0.0, 0.0], EvalMode::Visual).abs() < 1e-4);
    // Inner surface (r=3, y=0) is also on the 2D rectangle's
    // inner edge → distance 0.
    assert!(tube.sdf([3.0, 0.0, 0.0], EvalMode::Visual).abs() < 1e-4);
    // Midway between the two walls → deep inside the tube.
    assert!(tube.sdf([5.0, 0.0, 0.0], EvalMode::Visual) < -1.9);
    // On the central axis → well outside the tube.
    assert!(tube.sdf([0.0, 0.0, 0.0], EvalMode::Visual) > 2.9);
    // Different angular direction — rotational symmetry check.
    assert!(tube.sdf([0.0, 0.0, 7.0], EvalMode::Visual).abs() < 1e-4);
    assert!(tube.sdf([0.0, 0.0, 3.0], EvalMode::Visual).abs() < 1e-4);
}

#[test]
fn revolve_aabb_is_the_radial_sweep_of_the_sketch() {
    let tube = CsgNode::Revolve {
        id: FeatureId::from_u32(1),
        sketch: Box::new(rect_sketch(5.0, 0.0, 2.0, 10.0)),
    };
    let bb = tube.aabb();
    // Sketch x in [3, 7] → radial extent r_max = 7.
    assert_eq!(bb.min, [-7.0, -10.0, -7.0]);
    assert_eq!(bb.max, [7.0, 10.0, 7.0]);
}

#[test]
fn difference_of_two_extrudes_carves_a_washer() {
    // Extrude-of-difference ≈ difference-of-extrudes here
    // because the sketch difference is the natural "ring"
    // region, and extruding the ring gives a washer.
    let outer = circle_sketch(0.0, 0.0, 10.0);
    let inner = circle_sketch(0.0, 0.0, 4.0);
    let ring = Sketch2d::Difference(Box::new(outer), Box::new(inner));
    let washer = CsgNode::Extrude {
        id: FeatureId::from_u32(1),
        sketch: Box::new(ring),
        half_height: 2.0,
    };
    // Between the two radii, on the mid-plane → inside.
    assert!(washer.sdf([7.0, 0.0, 0.0], EvalMode::Visual) < 0.0);
    // Inside the hole → outside the body.
    assert!(washer.sdf([0.0, 0.0, 0.0], EvalMode::Visual) > 0.0);
    // Top face at r=7 → on the surface. Note the extrude axis
    // is Z, not Y — [7, 0, 2] means "sketch point (7,0) lifted
    // to the top face of the slab".
    assert!(washer.sdf([7.0, 0.0, 2.0], EvalMode::Visual).abs() < 1e-4);
}

#[test]
fn difference_aabb_stays_with_left_operand() {
    let features = vec![
        box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
        DecodedFeature::from_parts(
            FeatureId::from_u32(2),
            "hole".into(),
            BoolOp::Remove,
            Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius: 3.0,
            },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        ),
    ];
    let bb = CsgNode::from_decoded(&features).aabb();
    assert_eq!(bb.min, [-10.0, -10.0, -10.0]);
    assert_eq!(bb.max, [10.0, 10.0, 10.0]);
}

// ─── K10.0: smooth blend ─────────────────────────────────

#[test]
fn smin_poly_degenerates_to_hard_min_outside_blend_zone() {
    assert_eq!(super::smin_poly(10.0, -5.0, 2.0), -5.0);
    assert_eq!(super::smin_poly(-5.0, 10.0, 2.0), -5.0);
}

#[test]
fn smin_poly_is_strictly_less_than_hard_min_in_blend_zone() {
    let da = 1.0;
    let db = 1.0;
    let r = 4.0;
    let smooth = super::smin_poly(da, db, r);
    let hard = da.min(db);
    assert!(smooth < hard, "smooth {smooth} not < hard {hard}");
}

/// c-M-b: the fillet cross-section is an EXACT circle. Between two half-spaces
/// with distances `(a, b)`, the zero level set of `smin_poly` is the arc of
/// radius `r` centred at `(r, r)`. Sample the quadrant facing the corner:
/// every point on that circle must read exactly 0.
#[test]
fn smin_poly_zero_set_is_an_exact_circle() {
    let r = 5.0_f32;
    for i in 0..=16 {
        // φ ∈ [π, 3π/2] parametrises the arc facing the origin.
        let phi = std::f32::consts::PI + std::f32::consts::FRAC_PI_2 * (i as f32 / 16.0);
        let a = r + r * phi.cos();
        let b = r + r * phi.sin();
        let f = super::smin_poly(a, b, r);
        assert!(f.abs() < 1e-4, "arc point (a={a}, b={b}) reads {f}, expected 0");
    }
    // The arc crosses the diagonal at a = b = r·(1 − 1/√2) ≈ 0.293 r. Just
    // inside reads negative, just outside positive (a true radius test).
    let cross = r * (1.0 - std::f32::consts::FRAC_1_SQRT_2);
    assert!(super::smin_poly(cross - 0.1, cross - 0.1, r) < 0.0, "inside the arc is negative");
    assert!(super::smin_poly(cross + 0.1, cross + 0.1, r) > 0.0, "outside the arc is positive");
}

/// d: `apply_global_round` rewrites every hard Boolean junction to its
/// smooth form (Union→SmoothUnion, Difference→SmoothDifference) and leaves
/// existing blends untouched.
#[test]
fn apply_global_round_smooths_all_hard_junctions() {
    let a = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box { center: [0.0; 3], half_extents: [10.0, 10.0, 10.0] },
    };
    let b = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Box { center: [12.0, 0.0, 0.0], half_extents: [10.0, 10.0, 10.0] },
    };
    let c = CsgNode::Leaf {
        id: FeatureId::from_u32(3),
        primitive: Primitive::Cylinder { center: [0.0; 3], radius: 3.0, half_height: 20.0 },
    };
    // (box ∪ box) − cylinder.
    let tree = CsgNode::Difference(
        Box::new(CsgNode::Union(Box::new(a), Box::new(b))),
        Box::new(c),
    );
    let rounded = tree.apply_global_round(2.0);
    match rounded {
        CsgNode::SmoothDifference(inner, _, r, BlendMode::Fillet) => {
            assert_eq!(r, 2.0);
            assert!(
                matches!(*inner, CsgNode::SmoothUnion(_, _, 2.0, BlendMode::Fillet)),
                "the inner union rounds too"
            );
        }
        other => panic!("top Difference must become SmoothDifference, got {other:?}"),
    }
    // r == 0 is a no-op.
    let flat = CsgNode::Union(
        Box::new(CsgNode::Leaf { id: FeatureId::from_u32(1), primitive: Primitive::Sphere { center: [0.0; 3], radius: 5.0 } }),
        Box::new(CsgNode::Leaf { id: FeatureId::from_u32(2), primitive: Primitive::Sphere { center: [4.0, 0.0, 0.0], radius: 5.0 } }),
    );
    assert!(matches!(flat.apply_global_round(0.0), CsgNode::Union(_, _)));
}

/// d: the global round actually rounds the geometry — at a convex union edge
/// the field is pulled below the hard `min` (a fillet of the requested radius).
#[test]
fn apply_global_round_rounds_a_union_edge() {
    let a = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box { center: [0.0; 3], half_extents: [10.0, 10.0, 10.0] },
    };
    let b = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Box { center: [10.0, 10.0, 0.0], half_extents: [10.0, 10.0, 10.0] },
    };
    let hard = CsgNode::Union(Box::new(a.clone()), Box::new(b.clone()));
    let rounded = hard.clone().apply_global_round(3.0);
    // The convex edge sits where the two top faces (y=10) and side faces
    // (x=10) meet, near (10, 10, 0). Just outside, the round pulls the field
    // more negative (adds the fillet material).
    let p = [10.0, 10.0, 0.0];
    let d_hard = hard.sdf(p, EvalMode::Visual);
    let d_round = rounded.sdf(p, EvalMode::Visual);
    assert!(d_round < d_hard, "round {d_round} not < hard {d_hard}");
}

#[test]
fn smooth_union_rounds_a_box_sphere_junction() {
    let a = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box {
            center: [0.0, 0.0, 0.0],
            half_extents: [10.0, 10.0, 10.0],
        },
    };
    let b = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Sphere {
            center: [10.0, 0.0, 0.0],
            radius: 8.0,
        },
    };
    let hard = CsgNode::Union(Box::new(a.clone()), Box::new(b.clone()));
    let smooth = CsgNode::SmoothUnion(Box::new(a), Box::new(b), 4.0, BlendMode::Fillet);

    // At the junction seam the smooth blend pulls the
    // distance further negative than the hard union.
    let p = [10.0, 8.0, 0.0];
    let d_hard = hard.sdf(p, EvalMode::Visual);
    let d_smooth = smooth.sdf(p, EvalMode::Visual);
    assert!(d_smooth < d_hard, "smooth {d_smooth} not < hard {d_hard}");
}

#[test]
fn masked_blend_fillets_only_inside_the_capsule() {
    // Two boxes side by side: a at origin, b shifted +X.
    let a = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box {
            center: [0.0, 0.0, 0.0],
            half_extents: [10.0, 10.0, 10.0],
        },
    };
    let b = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Box {
            center: [10.0, 0.0, 0.0],
            half_extents: [10.0, 10.0, 10.0],
        },
    };
    // Capsule mask along the Y axis at x=10 (the junction edge).
    let masked = CsgNode::MaskedBlend {
        a: Box::new(a.clone()),
        b: Box::new(b.clone()),
        mask_polyline: vec![[10.0, -15.0, 0.0], [10.0, 15.0, 0.0]],
        mask_radius: 5.0,
        blend_radius: 4.0,
        blend_mode: super::BlendMode::Fillet,
        is_difference: false,
        provenance: super::BlendProvenance::NONE,
    };
    let hard = CsgNode::Union(Box::new(a), Box::new(b));

    // Near the junction edge inside the mask → smooth blend
    // produces a different (more negative) distance than hard.
    let p_inside_mask = [10.0, 0.0, 8.0];
    let d_masked = masked.sdf(p_inside_mask, EvalMode::Visual);
    let d_hard = hard.sdf(p_inside_mask, EvalMode::Visual);
    assert!(
        d_masked < d_hard || (d_masked - d_hard).abs() < 1e-5,
        "masked {d_masked} should be <= hard {d_hard} inside the mask"
    );

    // Far from the mask → same as hard union.
    let p_outside_mask = [0.0, 0.0, 0.0];
    let d_masked_far = masked.sdf(p_outside_mask, EvalMode::Visual);
    let d_hard_far = hard.sdf(p_outside_mask, EvalMode::Visual);
    assert!(
        (d_masked_far - d_hard_far).abs() < 1e-5,
        "outside mask: masked {d_masked_far} != hard {d_hard_far}"
    );

    // Logical mode always uses hard ops.
    let d_logical = masked.sdf(p_inside_mask, EvalMode::Logical);
    assert_eq!(d_logical, d_hard);
}

#[test]
fn polyline_tube_two_points_equals_single_capsule() {
    // The reduction invariant: a 2-point polyline tube is exactly the
    // single-capsule SDF over those endpoints.
    let p0 = [10.0, -15.0, 0.0];
    let p1 = [10.0, 15.0, 0.0];
    let r = 5.0;
    let poly = [p0, p1];
    for p in [
        [10.0, 0.0, 8.0],
        [0.0, 0.0, 0.0],
        [12.0, 3.0, -4.0],
        [10.0, 20.0, 0.0],
        [-30.0, 0.0, 0.0],
    ] {
        let tube = sdf_polyline_tube(p, &poly, r);
        let cap = sdf_capsule_3d(p, p0, p1, r);
        assert!(
            (tube - cap).abs() < 1e-6,
            "tube {tube} != capsule {cap} at {p:?}",
        );
    }
}

#[test]
fn polyline_tube_degenerate_cases() {
    let r = 4.0;
    // Single point → sphere of radius r at that point.
    let one = [[2.0, 0.0, 0.0]];
    assert!((sdf_polyline_tube([2.0, 0.0, 0.0], &one, r) - (-r)).abs() < 1e-6);
    assert!((sdf_polyline_tube([2.0 + r, 0.0, 0.0], &one, r)).abs() < 1e-6);
    // Empty → never active (mask falls through to the hard combine).
    assert_eq!(sdf_polyline_tube([0.0, 0.0, 0.0], &[], r), f32::INFINITY);
    // A 3-point polyline tube is the min over both segments.
    let three = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]];
    let p = [10.0, 5.0, 0.0]; // closest to the 2nd segment
    let expect = sdf_capsule_3d(p, three[1], three[2], r)
        .min(sdf_capsule_3d(p, three[0], three[1], r));
    assert!((sdf_polyline_tube(p, &three, r) - expect).abs() < 1e-6);
}

#[test]
fn apply_edge_fillets_replaces_matching_junction() {
    let a = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box {
            center: [0.0, 0.0, 0.0],
            half_extents: [10.0, 10.0, 10.0],
        },
    };
    let b = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Sphere {
            center: [5.0, 0.0, 0.0],
            radius: 8.0,
        },
    };
    let tree = CsgNode::Union(Box::new(a), Box::new(b));
    let fillets = vec![EdgeFillet {
        fillet_id: FeatureId::from_u32(9),
        feature_a: FeatureId::from_u32(1),
        feature_b: FeatureId::from_u32(2),
        branch_a: 0,
        branch_b: 0,
        mask_polyline: vec![[5.0, -15.0, 0.0], [5.0, 15.0, 0.0]],
        mask_radius: 5.0,
        blend_radius: 3.0,
        blend_mode: BlendMode::Fillet,
    }];
    let result = tree.apply_edge_fillets(&fillets);
    // The top-level Union should have been replaced by
    // MaskedBlend.
    assert!(matches!(result, CsgNode::MaskedBlend { .. }));
}

#[test]
fn apply_edge_fillets_leaves_unmatched_junctions() {
    let a = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box {
            center: [0.0; 3],
            half_extents: [5.0; 3],
        },
    };
    let b = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Sphere {
            center: [0.0; 3],
            radius: 3.0,
        },
    };
    let tree = CsgNode::Difference(Box::new(a), Box::new(b));
    // Fillet references features 3 and 4 which don't exist
    // in the tree → no match, tree stays unchanged.
    let fillets = vec![EdgeFillet {
        fillet_id: FeatureId::from_u32(9),
        feature_a: FeatureId::from_u32(3),
        feature_b: FeatureId::from_u32(4),
        branch_a: 0,
        branch_b: 0,
        mask_polyline: vec![[0.0; 3], [0.0, 10.0, 0.0]],
        mask_radius: 5.0,
        blend_radius: 3.0,
        blend_mode: BlendMode::Fillet,
    }];
    let result = tree.apply_edge_fillets(&fillets);
    assert!(matches!(result, CsgNode::Difference(_, _)));
}

#[test]
fn fillet_all_edges_stress_test() {
    // Build a box with 4 holes subtracted — each junction
    // gets a fillet. Verifies that multiple MaskedBlend nodes
    // compose without panicking.
    let base = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box {
            center: [0.0; 3],
            half_extents: [50.0, 50.0, 50.0],
        },
    };
    let mut tree = base;
    for i in 2..=5 {
        let hole = CsgNode::Leaf {
            id: FeatureId::from_u32(i),
            primitive: Primitive::Cylinder {
                center: [(i as f32 - 3.5) * 20.0, 0.0, 0.0],
                radius: 5.0,
                half_height: 60.0,
            },
        };
        tree = CsgNode::Difference(Box::new(tree), Box::new(hole));
    }
    // Fillet every hole junction.
    let fillets: Vec<EdgeFillet> = (2..=5)
        .map(|i| EdgeFillet {
            fillet_id: FeatureId::from_u32(100 + i),
            feature_a: FeatureId::from_u32(1),
            feature_b: FeatureId::from_u32(i),
            branch_a: 0,
            branch_b: 0,
            mask_polyline: vec![
                [(i as f32 - 3.5) * 20.0, -55.0, 0.0],
                [(i as f32 - 3.5) * 20.0, 55.0, 0.0],
            ],
            mask_radius: 8.0,
            blend_radius: 2.0,
            blend_mode: BlendMode::Fillet,
        })
        .collect();
    let result = tree.apply_edge_fillets(&fillets);
    // Should produce exactly 4 MaskedBlend nodes (one per
    // hole junction). Test that the SDF evaluates without
    // panicking at a few sample points.
    for x in [-30.0, 0.0, 30.0] {
        let d = result.sdf([x, 0.0, 0.0], EvalMode::Visual);
        assert!(d.is_finite(), "SDF at ({x}, 0, 0) = {d}");
    }
}

#[test]
fn smooth_blend_logical_mode_degrades_to_hard_ops() {
    let a = CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Sphere {
            center: [0.0; 3],
            radius: 5.0,
        },
    };
    let b = CsgNode::Leaf {
        id: FeatureId::from_u32(2),
        primitive: Primitive::Sphere {
            center: [4.0, 0.0, 0.0],
            radius: 5.0,
        },
    };
    let hard_u = CsgNode::Union(Box::new(a.clone()), Box::new(b.clone()));
    let smooth_u = CsgNode::SmoothUnion(Box::new(a.clone()), Box::new(b.clone()), 3.0, BlendMode::Fillet);
    let hard_d = CsgNode::Difference(Box::new(a.clone()), Box::new(b.clone()));
    let smooth_d = CsgNode::SmoothDifference(Box::new(a), Box::new(b), 3.0, BlendMode::Fillet);

    let p = [2.0, 3.0, 0.0];
    assert_eq!(
        hard_u.sdf(p, EvalMode::Logical),
        smooth_u.sdf(p, EvalMode::Logical),
    );
    assert_eq!(
        hard_d.sdf(p, EvalMode::Logical),
        smooth_d.sdf(p, EvalMode::Logical),
    );
}

#[test]
fn targeted_intersect_only_wraps_target_subtree() {
    let sphere = DecodedFeature::from_parts(
        FeatureId::from_u32(1),
        "Sphere".into(),
        BoolOp::New,
        Primitive::Sphere { center: [0.0; 3], radius: 10.0 },
        Transform::IDENTITY,
        PrimitiveModifiers::NONE,
        vec![],
    );
    let cube = DecodedFeature::from_parts(
        FeatureId::from_u32(2),
        "Box".into(),
        BoolOp::New,
        Primitive::Box {
            center: [15.0, 0.0, 0.0],
            half_extents: [5.0; 3],
        },
        Transform::IDENTITY,
        PrimitiveModifiers::NONE,
        vec![],
    );
    let gyroid = DecodedFeature::from_parts(
        FeatureId::from_u32(3),
        "Gyroid".into(),
        BoolOp::Intersect,
        Primitive::Gyroid { cell_size: 5.0, thickness: 1.0 },
        Transform::IDENTITY,
        PrimitiveModifiers::NONE,
        vec![FeatureId::from_u32(1)],
    );

    let csg = CsgNode::from_decoded(&[sphere, cube, gyroid]);

    // Centre of the cube (15,0,0) must remain inside —
    // the Gyroid targets only the Sphere, not the Box.
    let cube_centre = csg.sdf([15.0, 0.0, 0.0], EvalMode::Visual);
    assert!(
        cube_centre < 0.0,
        "cube centre must stay solid, got sdf = {cube_centre}",
    );
}
