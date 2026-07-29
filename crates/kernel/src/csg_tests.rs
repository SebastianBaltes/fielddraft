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
            wall_subs: None,
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

/// A body-offset modifier: the sentinel `Offset(Empty, δ)` node on a
/// `Remove` feature targeting `target`, exactly as `OffsetFeature` emits it.
fn offset_feature(id: u32, target: u32, delta: f32) -> DecodedFeature {
    DecodedFeature {
        id: FeatureId::from_u32(id),
        name: format!("Offset {id}"),
        op: BoolOp::Remove,
        blend_radius: 0.0,
        blend_mode: BlendMode::Fillet,
        target_ids: vec![FeatureId::from_u32(target)],
        transform: Transform::IDENTITY,
        node: CsgNode::Offset(Box::new(CsgNode::Empty), delta),
    }
}

#[test]
fn offset_sentinel_dilates_and_erodes_the_target_body() {
    // A 20³ box (face at x=10). +3 mm offset moves the face to x=13; the
    // expansion passes keep the field exact there. −4 mm erodes to x=6.
    for (delta, face) in [(3.0_f32, 13.0_f32), (-4.0, 6.0)] {
        let features = vec![
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            offset_feature(2, 1, delta),
        ];
        let tree = CsgNode::from_decoded(&features)
            .push_down_offsets()
            .fold_offset_leaves();
        let on_face = tree.sdf([face, 0.0, 0.0], EvalMode::Logical).abs();
        assert!(on_face < 0.01, "delta {delta}: face at x={face} expected ~0, got {on_face}");
        // Still solid just inside, hollow-free just outside.
        assert!(tree.sdf([face - 0.5, 0.0, 0.0], EvalMode::Logical) < 0.0, "delta {delta}: inside");
        assert!(tree.sdf([face + 0.5, 0.0, 0.0], EvalMode::Logical) > 0.0, "delta {delta}: outside");
    }
}

#[test]
fn scoped_offset_leaves_a_sibling_body_untouched() {
    // Two disjoint boxes; offset scoped to body #1 only. Body #2's face at
    // x=25 must stay put while body #1's face at x=10 grows to x=13.
    let features = vec![
        box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
        box_feature(2, BoolOp::New, [40.0, 0.0, 0.0], [15.0, 10.0, 10.0]),
        offset_feature(3, 1, 3.0),
    ];
    let tree = CsgNode::from_decoded(&features)
        .push_down_offsets()
        .fold_offset_leaves();
    // Body #1 grew.
    assert!(tree.sdf([13.0, 0.0, 0.0], EvalMode::Logical).abs() < 0.01, "body #1 face moved to 13");
    // Body #2's near face (x=25) is unchanged: boundary on it, and the gap
    // just outside it (x=24) stays hollow — a wrongly-dilated body #2 would
    // have moved its face to x=22 and made x=24 solid.
    assert!(tree.sdf([25.0, 0.0, 0.0], EvalMode::Logical).abs() < 0.01, "body #2 face still at 25");
    assert!(tree.sdf([24.0, 0.0, 0.0], EvalMode::Logical) > 0.0, "body #2 not dilated past 25");
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
                branch: BranchId::from_feature_sub(&FeatureId::from_u32(1), 4).as_u64(),
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
                branch: BranchId::from_feature_sub(&FeatureId::from_u32(1), 0).as_u64(),
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
    assert_ne!(outer.as_u64(), inner.as_u64(), "inner wall is a distinct id");
    assert_eq!(inner.feature_id(), outer.feature_id(), "feature_id preserved");
    assert_eq!(inner.feature_id(), 7);
    assert_eq!(inner.inner_wall().as_u64(), inner.as_u64(), "idempotent");

    // The two extrude caps live below the inner-wall flag, so a cap and its
    // inner wall are distinct and the flag never collides with a cap.
    const { assert!(BranchId::EXTRUDE_CAP_TOP_SUB < BranchId::INNER_WALL_FLAG) };
    const { assert!(BranchId::EXTRUDE_CAP_BOTTOM_SUB < BranchId::INNER_WALL_FLAG) };
    let cap = BranchId::from_feature_sub(&FeatureId::from_u32(7), BranchId::EXTRUDE_CAP_TOP_SUB);
    assert!(!cap.is_inner_wall());
    assert_ne!(cap.as_u64(), cap.inner_wall().as_u64());
}

/// #424: the occurrence field is a real fourth field — it packs, unpacks,
/// survives a pattern instance shift, and does not disturb the three fields
/// beside it.
#[test]
fn branch_id_carries_the_wall_occurrence_ordinal() {
    // A wall name is `occurrence << SUB_BITS | sub`; `from_feature_sub` splits it.
    let name = (2 << BranchId::SUB_BITS) | 45;
    let b = BranchId::from_feature_sub(&FeatureId::from_u32(9), name);
    assert_eq!(b.feature_id(), 9);
    assert_eq!(b.sub(), 45);
    assert_eq!(b.occurrence(), 2);
    assert_eq!(b.instance(), 0);
    // A plain sub is simply occurrence 0 — every pre-#424 caller passes one.
    let plain = BranchId::from_feature_sub(&FeatureId::from_u32(9), 45);
    assert_eq!(plain.occurrence(), 0);
    assert_ne!(plain.as_u64(), b.as_u64(), "the ordinal is part of the name");

    // A pattern copy moves the body, not the profile's contours: `qInstance`
    // shifts the instance field and must leave the occurrence alone.
    let copy = b.on_instance(9, 3);
    assert_eq!(copy.instance(), 3);
    assert_eq!(copy.occurrence(), 2);
    assert_eq!(copy.sub(), 45);

    // The layout stays feature-major, which the weave's coincidence classes
    // sort on: a later feature outranks any occurrence of an earlier one.
    let later = BranchId::from_feature_sub(&FeatureId::from_u32(10), 0);
    let high = BranchId::from_feature_sub(
        &FeatureId::from_u32(9),
        (BranchId::OCCURRENCE_MAX << BranchId::SUB_BITS) | BranchId::SUB_MASK_U32,
    );
    assert!(high.as_u64() < later.as_u64(), "feature-major ordering");

    // …and the whole word stays an exact JavaScript integer (the wire is a
    // plain JSON number).
    const { assert!(BranchId::LAYOUT_BITS < 53) };

    // The inner-wall flag rides inside the SUB field, below the occurrence.
    const { assert!(BranchId::INNER_WALL_FLAG < 1 << BranchId::SUB_BITS) };
}

// ─── #384 Phase D2: closed-shell → complement-duality expansion ──────────────

fn box_leaf(id: u32, half: [f32; 3]) -> CsgNode {
    CsgNode::Leaf {
        id: FeatureId::from_u32(id),
        primitive: Primitive::Box {
            center: [0.0, 0.0, 0.0],
            half_extents: half,
        },
    }
}

#[test]
fn expand_shells_closed_field_matches_raw_shell_before_folding() {
    // A closed shell dualises to `Difference(body, InnerWall(Offset(body, −t)))`,
    // whose field (BEFORE folding) is bit-for-bit the old `max(d, −(d+t))`
    // modifier: `Difference` = `max(dA, −dB)`, and `Offset(body, −t)` evaluates
    // to `d − (−t) = d + t`, so the whole node is `max(d, −(d+t))`.
    let t = 2.0;
    let inner = box_leaf(1, [10.0, 10.0, 10.0]);
    let shell = CsgNode::Shell(Box::new(inner.clone()), t, vec![]);
    let expanded = shell.clone().expand_shells();

    // Structural: the expansion is exactly the documented complement duality.
    match &expanded {
        CsgNode::Difference(a, b) => {
            assert!(matches!(**a, CsgNode::Leaf { .. }));
            match &**b {
                CsgNode::InnerWall(off) => {
                    assert!(matches!(**off, CsgNode::Offset(_, d) if d == -t));
                }
                other => panic!("expected InnerWall(Offset), got {other:?}"),
            }
        }
        other => panic!("expected Difference, got {other:?}"),
    }

    for p in [
        [0.0, 0.0, 0.0],
        [9.0, 0.0, 0.0],
        [8.0, 0.0, 0.0],
        [5.0, 3.0, -7.0],
        [8.5, 8.5, 0.0],
        [12.0, 0.0, 0.0],
    ] {
        let d = inner.sdf(p, EvalMode::Logical);
        let raw = d.max(-(d + t));
        let shell_field = shell.sdf(p, EvalMode::Logical);
        let expanded_field = expanded.sdf(p, EvalMode::Logical);
        assert!(
            (expanded_field - raw).abs() < 1e-5,
            "at {p:?}: expanded {expanded_field} != raw max(d,-(d+t)) {raw}"
        );
        assert!(
            (expanded_field - shell_field).abs() < 1e-5,
            "at {p:?}: expanded {expanded_field} != Shell node {shell_field}"
        );
    }
}

#[test]
fn expand_shells_then_fold_makes_the_inner_wall_metrically_exact() {
    // The dual erosion `Offset(box, −t)` folds to a smaller box leaf (Phase A2),
    // so the inner wall of a box shell becomes the EXACT euclidean box-8 surface
    // instead of the SDF-offset `−(d+t)` approximation. They diverge in the wall
    // corner region, where the box's own SDF and its inward-offset disagree.
    let t = 2.0;
    let shell = CsgNode::Shell(Box::new(box_leaf(1, [10.0, 10.0, 10.0])), t, vec![]);
    let folded = shell
        .clone()
        .expand_shells()
        .push_down_offsets()
        .fold_offset_leaves();

    // Corner wall point: inside box-10 (d = −1.5), outside box-8 by √0.5.
    let p = [8.5, 8.5, 0.0];
    let raw = shell.sdf(p, EvalMode::Logical); // max(−1.5, −(−1.5+2)) = −0.5
    let fold = folded.sdf(p, EvalMode::Logical); // max(−1.5, −√0.5)   = −√0.5
    let truth = -(0.5f32.sqrt()); // exact: nearest surface is the inner box-8 corner

    assert!((raw - (-0.5)).abs() < 1e-4, "raw shell field is the inexact −0.5, got {raw}");
    assert!(
        (fold - truth).abs() < 1e-4,
        "folded inner wall should equal the euclidean {truth}, got {fold}"
    );
    // And it is measurably closer to the analytic truth than the raw modifier.
    assert!(
        (fold - truth).abs() + 0.1 < (raw - truth).abs(),
        "fold ({fold}) must beat raw ({raw}) vs truth {truth}"
    );
}

#[test]
fn expand_shells_leaves_open_shells_on_the_field_path() {
    // An OPEN shell (non-empty open_faces) has no clean complement duality
    // (the retained-face masking is a per-sub-face field mask) — it must stay a
    // `CsgNode::Shell` so it keeps the existing open-face field path.
    let open = vec![OpenSurface {
        branch: BranchId::from_feature_sub(&FeatureId::from_u32(1), 4).as_u64(),
        surface: HalfSpace::Plane { normal: [0.0, 0.0, 1.0], offset: 10.0 },
        placement: Transform::IDENTITY,
    }];
    let shell = CsgNode::Shell(Box::new(box_leaf(1, [10.0, 10.0, 10.0])), 2.0, open);
    let expanded = shell.expand_shells();
    match expanded {
        CsgNode::Shell(_, _, ref o) => assert_eq!(o.len(), 1, "opening intact"),
        other => panic!("open shell must stay a Shell, got {other:?}"),
    }
}

#[test]
fn expanded_closed_shell_tags_inner_wall_contributors() {
    // The `InnerWall` wrapper remaps the eroded copy's contributors onto the
    // DL-75 inner-wall band, so a point on the cavity surface reports an
    // inner-wall branch while a point on the outer surface does not.
    let shell = CsgNode::Shell(Box::new(box_leaf(1, [10.0, 10.0, 10.0])), 2.0, vec![]);
    let folded = shell
        .expand_shells()
        .push_down_offsets()
        .fold_offset_leaves();

    // Inner cavity face at x = 8: the InnerWall term wins the combine_max.
    let (_, c_in) = folded.evaluate([8.0, 0.0, 0.0], 0.1, EvalMode::Logical);
    assert!(
        c_in.as_slice().iter().any(|b| b.is_inner_wall()),
        "inner-wall point must carry an inner-wall contributor, got {:?}",
        c_in.as_slice()
    );

    // Outer face at x = 10: the outer body term wins — no inner-wall flag.
    let (_, c_out) = folded.evaluate([10.0, 0.0, 0.0], 0.1, EvalMode::Logical);
    assert!(!c_out.as_slice().is_empty(), "outer face has a contributor");
    assert!(
        c_out.as_slice().iter().all(|b| !b.is_inner_wall()),
        "outer-wall point must have no inner-wall contributor, got {:?}",
        c_out.as_slice()
    );
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
/// The sub a wall gets when its profile has NO wall-sub table: the positional
/// boundary-group index under [`BranchId::POSITIONAL_WALL_FLAG`] (#445). Every
/// hand-built `CsgNode::Extrude { wall_subs: None, .. }` in these tests mints
/// them, and the flag is what keeps `qWall(op, entity)` from mistaking one for
/// a sketch-entity ordinal.
fn positional_wall(group: u32) -> u32 {
    BranchId::POSITIONAL_WALL_FLAG | group
}

fn branches(raw: &[(u32, u32)]) -> Vec<BranchId> {
    let mut v: Vec<BranchId> = raw
        .iter()
        .map(|&(f, sub)| BranchId::from_feature_sub(&FeatureId::from_u32(f), sub))
        .collect();
    v.sort_by_key(|b| b.as_u64());
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
    got.sort_by_key(|b| b.as_u64());
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
    got.sort_by_key(|b| b.as_u64());
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
    got.sort_by_key(|b| b.as_u64());
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
    got.sort_by_key(|x| x.as_u64());
    assert_eq!(
        got,
        branches(&[(1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (6, 0), (7, 0), (8, 0)])
    );
}

// ─── extrude / revolve ─────────────────────────────────────────

use crate::sketch2d::{Sketch2d, Sketch2dPrimitive, SketchSegment};

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
    CsgNode::Helix { wall_subs: None,
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
    let extrude = CsgNode::Extrude { wall_subs: None,
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
    let extrude = CsgNode::Extrude { wall_subs: None,
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
    let extrude = CsgNode::Extrude { wall_subs: None,
        id: FeatureId::from_u32(42),
        sketch: Box::new(rect_sketch(0.0, 0.0, 5.0, 2.0)),
        half_height: 3.0,
    };
    let (d, c) = extrude.evaluate([5.0, 0.0, 0.0], 0.5, EvalMode::Visual);
    assert!(d.abs() < 1e-4);
    // No wall-sub table → the wall's sub is POSITIONAL and carries the flag
    // that keeps `qWall` off it (#445).
    assert_eq!(c.as_slice(), branches(&[(42, positional_wall(0))]).as_slice());
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
    let extrude = CsgNode::Extrude { wall_subs: None,
        id: FeatureId::from_u32(fid),
        sketch: Box::new(path),
        half_height: 3.0,
    };
    // Mid-wall probes with a tight band: each maps to exactly its own
    // wall group (the DL-65 contributor band only ties at a corner/rim, so
    // a clearly-nearest wall stays a single contributor). +x wall → group 1.
    let (_, c) = extrude.evaluate([4.0, 0.0, 0.0], 0.05, EvalMode::Visual);
    assert_eq!(c.as_slice(), branches(&[(fid, positional_wall(1))]).as_slice());
    // Bottom (−y) wall → group 0.
    let (_, c) = extrude.evaluate([0.0, -4.0, 0.0], 0.05, EvalMode::Visual);
    assert_eq!(c.as_slice(), branches(&[(fid, positional_wall(0))]).as_slice());
    // Top (+y) wall → group 2 — distinct sub-id.
    let (_, c) = extrude.evaluate([0.0, 4.0, 0.0], 0.05, EvalMode::Visual);
    assert_eq!(c.as_slice(), branches(&[(fid, positional_wall(2))]).as_slice());
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
    let extrude = CsgNode::Extrude { wall_subs: None,
        id: FeatureId::from_u32(fid),
        sketch: Box::new(path),
        half_height: 3.0,
    };

    // DL-65: at the (+x,+y) sketch corner the band ties the right (1) and
    // top (2) walls → a vertical wall∩wall edge reports BOTH branches, so
    // the both-branches gate sees a real edge instead of a singleton.
    let (_, c) = extrude.evaluate([1.0, 1.0, 0.0], 0.25, EvalMode::Visual);
    let s: std::collections::HashSet<u64> =
        c.as_slice().iter().map(|b| b.as_u64()).collect();
    assert!(
        s.contains(
            &BranchId::from_feature_sub(&FeatureId::from_u32(fid), positional_wall(1)).as_u64()
        ) && s.contains(
            &BranchId::from_feature_sub(&FeatureId::from_u32(fid), positional_wall(2)).as_u64()
        ),
        "corner must report both adjoining walls, got {s:?}"
    );
    assert_eq!(c.element(), ProxyElement::Edge);

    // At the top rim (mid-wall, z = +half_height) the wall group and the
    // top cap tie → a wall∩cap edge reports both branches.
    let (_, c) = extrude.evaluate([1.0, 0.0, 3.0], 0.25, EvalMode::Visual);
    let s: std::collections::HashSet<u64> =
        c.as_slice().iter().map(|b| b.as_u64()).collect();
    assert!(
        s.contains(
            &BranchId::from_feature_sub(&FeatureId::from_u32(fid), positional_wall(1)).as_u64()
        ) && s.contains(
            &BranchId::from_feature_sub(
                &FeatureId::from_u32(fid),
                BranchId::EXTRUDE_CAP_TOP_SUB
            )
            .as_u64()
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
    let tube = CsgNode::Revolve { wall_subs: None,
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

// ─── #390: axis-touching revolve — no phantom zero-set on the axis ───
//
// The profile boundary that lies ON the revolve axis is interior after
// revolution, not surface. Before the fix the compiled field carried a
// phantom zero-set along the whole axis (a5_axis_scan in the #384
// iter3 spike): inside a revolved cylinder the field on the axis read
// ~0 instead of the true −min(radius, cap distance). The strip runs at
// `CsgNode::from_parts` (and in the features pipeline), so these tests
// build via `Primitive::Revolve` exactly like the product.

/// Lower a `Primitive::Revolve` the way the product does (via
/// `from_parts`, which enforces the axis-strip invariant).
fn revolve_node(sketch: Sketch2d) -> CsgNode {
    CsgNode::from_decoded(&[DecodedFeature::from_parts(
        FeatureId::from_u32(1),
        "revolve".to_string(),
        BoolOp::New,
        Primitive::Revolve {
            sketch,
            center: [0.0, 0.0, 0.0],
            wall_subs: None,
        },
        Transform::IDENTITY,
        PrimitiveModifiers::NONE,
        vec![],
    )])
}

#[test]
fn axis_touching_rectangle_revolve_is_the_exact_cylinder_sdf() {
    // Rectangle u ∈ [0, 30], v ∈ [−20, 20] revolved around Y → the
    // cylinder r = 30, |y| ≤ 20 (the a5 spike scene).
    let node = revolve_node(rect_sketch(15.0, 0.0, 15.0, 20.0));
    let truth = |r: f32, y: f32| -> f32 {
        let dr = r - 30.0;
        let dy = y.abs() - 20.0;
        if dr > 0.0 || dy > 0.0 {
            (dr.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt()
        } else {
            dr.max(dy)
        }
    };
    // Radial scan through the axis at y = 0 — the a5 line scan. Before
    // the fix f(x→0) → 0 (phantom wall); the true value is −20.
    for i in 0..=100 {
        let x = 29.9 * i as f32 / 100.0;
        let f = node.sdf([x, 0.0, 0.0], EvalMode::Visual);
        let t = truth(x, 0.0);
        assert!((f - t).abs() < 1e-3, "x={x}: f={f}, truth={t}");
    }
    // Directly ON the axis: deep inside, the caps dominate.
    for y in [0.0f32, 10.0, -19.0, 19.5] {
        let f = node.sdf([0.0, y, 0.0], EvalMode::Visual);
        let t = truth(0.0, y);
        assert!((f - t).abs() < 1e-3, "axis y={y}: f={f}, truth={t}");
        assert!(f < -0.4, "axis y={y} must be clearly inside, got {f}");
    }
    // Above the cap on the axis: outside, distance to the cap plane.
    let f = node.sdf([0.0, 25.0, 0.0], EvalMode::Visual);
    assert!((f - 5.0).abs() < 1e-3, "axis above cap: f={f}, truth=5");
}

#[test]
fn axis_diameter_sector_revolve_is_the_exact_sphere_sdf() {
    // Half-disk (pie wedge from −90° to +90°, diameter on the axis)
    // revolved around Y → the sphere r = 10. The sector's two radial
    // edges lie on the axis and must be stripped.
    let node = revolve_node(Sketch2d::Prim(Sketch2dPrimitive::Sector {
        center: [0.0, 0.0],
        radius: 10.0,
        start_angle: -std::f32::consts::FRAC_PI_2,
        sweep_angle: std::f32::consts::PI,
    }));
    for (p, t) in [
        ([0.0f32, 0.0, 0.0], -10.0f32), // centre
        ([0.0, 6.0, 0.0], -4.0),        // on the axis, inside
        ([0.0, -6.0, 0.0], -4.0),
        ([5.0, 0.0, 0.0], -5.0),
        ([0.0, 14.0, 0.0], 4.0),        // on the axis, above the pole
        ([0.0, 0.0, 10.0], 0.0),        // equator surface
    ] {
        let f = node.sdf(p, EvalMode::Visual);
        assert!((f - t).abs() < 1e-3, "p={p:?}: f={f}, truth={t}");
    }
}

#[test]
fn axis_touching_difference_revolve_has_no_phantom_needle_in_the_bore() {
    // (rect r ∈ [0,30]) − (rect r ∈ [0,10], taller) revolved → tube
    // with inner radius 10. The subtracted region's axis edge must be
    // stripped too, or the field reads 0 along the axis inside the
    // bore where the true distance is +10.
    let profile = Sketch2d::Difference(
        Box::new(rect_sketch(15.0, 0.0, 15.0, 20.0)),
        Box::new(rect_sketch(5.0, 0.0, 5.0, 25.0)),
    );
    let node = revolve_node(profile);
    let f_axis = node.sdf([0.0, 0.0, 0.0], EvalMode::Visual);
    assert!(
        (f_axis - 10.0).abs() < 1e-3,
        "axis inside the bore must be +10 to the inner wall, got {f_axis}"
    );
    // The inner wall itself.
    assert!(node.sdf([10.0, 0.0, 0.0], EvalMode::Visual).abs() < 1e-3);
    // Between the walls: inside.
    assert!(node.sdf([20.0, 0.0, 0.0], EvalMode::Visual) < -9.0);
}

#[test]
fn off_axis_profiles_are_untouched_by_the_axis_strip() {
    // The strip must be a no-op for profiles that keep clear of the
    // axis — off-axis revolve exactness is measured and locked.
    let ring = rect_sketch(25.0, 0.0, 7.0, 10.0);
    assert_eq!(
        crate::sketch::strip_revolve_axis_segments(ring.clone()),
        ring
    );
    let circle = circle_sketch(8.0, 0.0, 2.0);
    assert_eq!(
        crate::sketch::strip_revolve_axis_segments(circle.clone()),
        circle
    );
}

#[test]
fn axis_strip_converts_touching_rectangle_to_a_three_wall_path() {
    let stripped =
        crate::sketch::strip_revolve_axis_segments(rect_sketch(15.0, 0.0, 15.0, 20.0));
    let Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed: true }) = &stripped
    else {
        panic!("expected a closed Path, got {stripped:?}");
    };
    assert_eq!(segments.len(), 3, "left wall dropped, got {segments:?}");
    // No kept segment may touch the axis.
    for s in segments {
        let SketchSegment::Line { start, end } = s else {
            panic!("rect walls are lines")
        };
        assert!(start[0].abs() > 1e-4 || end[0].abs() > 1e-4);
    }
}

#[test]
fn revolve_aabb_is_the_radial_sweep_of_the_sketch() {
    let tube = CsgNode::Revolve { wall_subs: None,
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
    let washer = CsgNode::Extrude { wall_subs: None,
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
    // A union round adds fillet material at the CONCAVE seam — here where
    // box a's top face (y=10, x<0) meets box b's side face (x=0, y>10).
    // Probe just outside that corner, equidistant to both faces (da=db=1,
    // inside the blend radius): the smooth field must drop below the hard
    // min. (The old probe point (10,10,0) sat 10 mm deep inside box b —
    // outside the blend's arc region — and only passed because the legacy
    // circular-geometrical smin inflates interior values without bound;
    // the #384 Circular-CD smin is exactly `min` there.)
    let p = [-1.0, 11.0, 0.0];
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

/// #384 Rollout-Auflage (SYNTHESIS §5 Exp 1 / §6 Stufe 1): under the
/// Circular-CD smin the blend sits `(1 − 1/√2)·blend_r ≈ 0.29·blend_r` below
/// the hard min on the seam bisector EVEN AT the mask-tube boundary (the CD
/// under-estimation is non-local), so a hard switch there would tear a real
/// value jump into the field. This pins down both halves of the Auflage:
/// (1) the cross-fade (`masked_blend_combine`) keeps the field CONTINUOUS
/// across both band edges (tube boundary and inner band edge), values
/// point-wise compared on either side; (2) the CPU tree and the compiled
/// opcode tape — the field the TSL shader mirrors term-for-term — produce
/// the SAME field on all three zones (outside / band / deep inside).
#[test]
fn masked_blend_crossfade_is_continuous_and_mirrored() {
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
    let blend_r = 4.0_f32;
    let masked = CsgNode::MaskedBlend {
        a: Box::new(a.clone()),
        b: Box::new(b.clone()),
        mask_polyline: vec![[10.0, -15.0, 0.0], [10.0, 15.0, 0.0]],
        mask_radius: 5.0,
        blend_radius: blend_r,
        blend_mode: super::BlendMode::Fillet,
        is_difference: false,
        provenance: super::BlendProvenance::NONE,
    };
    let compiled = crate::sdf_compiler::compile_body(&crate::bvh::BoundedCsg::from_csg(&masked));

    // Probe line: y = 12 (2 mm above both top faces, so da = db = 2 — the
    // seam bisector, worst case for the CD undershoot), z = 0, x ∈ [4, 9.6].
    // The mask distance there is (10 − x) − 5: tube boundary at x = 5, inner
    // band edge (band = blend_r) at x = 9.
    let f = |x: f32| masked.sdf([x, 12.0, 0.0], EvalMode::Visual);

    // The CD gap the fade has to bridge is REAL at the tube boundary:
    // hard = 2, blended = smin(2, 2, 4) = 2 − (1 − 1/√2)·4 ≈ 0.83.
    let gap = (1.0 - std::f32::consts::FRAC_1_SQRT_2) * blend_r;
    let smin_bis = super::smin_poly(2.0, 2.0, blend_r);
    assert!(
        ((2.0 - smin_bis) - gap).abs() < 1e-5,
        "bisector undershoot {} != (1 − 1/√2)·r {}",
        2.0 - smin_bis,
        gap
    );

    // (1a) Continuity: walk across both band edges; the field's slope along
    // x is bounded (≤ 1 from the operands + gap·∇w ≤ 1.17·0.375 from the
    // fade), so consecutive samples may differ by ~1.5·step — a hard switch
    // would show the full ≈1.17 jump at x = 5.
    let step = 0.005_f32;
    let mut x = 4.0_f32;
    let mut prev = f(x);
    while x < 9.6 {
        x += step;
        let cur = f(x);
        assert!(
            (cur - prev).abs() <= 2.0 * step,
            "field jumps by {} at x = {x} — cross-fade torn",
            (cur - prev).abs()
        );
        prev = cur;
    }

    // (1b) Point-wise on either side of both band edges.
    for edge in [5.0_f32, 9.0] {
        let (lo, hi) = (f(edge - 1e-3), f(edge + 1e-3));
        assert!(
            (hi - lo).abs() < 5e-3,
            "band edge x = {edge}: {lo} vs {hi} — discontinuous"
        );
    }
    // Outside the tube the field IS the hard combine; deep inside the band
    // it IS the blend.
    assert!((f(4.5) - 2.0).abs() < 1e-5, "outside tube must be hard min");
    assert!((f(9.5) - smin_bis).abs() < 1e-5, "deep inside must be the blend");

    // (2) The compiled tape (mirrored 1:1 by the TSL shader) computes the
    // SAME field in all three zones — including inside the fade band.
    for &x in &[4.0, 4.9, 5.1, 6.0, 7.0, 8.5, 8.9, 9.1, 9.5] {
        for &(y, z) in &[(12.0_f32, 0.0_f32), (11.0, 3.0), (9.0, 0.0)] {
            let p = [x, y, z];
            let d_tree = masked.sdf(p, EvalMode::Visual);
            let d_tape = crate::sdf_compiler::eval_opcodes(&compiled.opcodes, p);
            assert!(
                (d_tree - d_tape).abs() < 1e-4,
                "tree {d_tree} != tape {d_tape} at {p:?}"
            );
        }
    }
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
        cutter: None,
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
        cutter: None,
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
            cutter: None,
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

// ─── #384 Stufe 3a: Offset-Pushdown (exakte Minkowski-Fälle) ────────────────

/// Sphere leaf helper for the pushdown tests — spheres are offset-closed
/// (`offset(sphere r, δ) = sphere r+δ`), which gives an analytic ground
/// truth for the field comparison.
fn pd_sphere(id: u32, center: [f32; 3], radius: f32) -> CsgNode {
    CsgNode::Leaf {
        id: FeatureId::from_u32(id),
        primitive: Primitive::Sphere { center, radius },
    }
}

/// Deterministic probe cloud spanning inside, surface band and far field.
fn pd_cloud() -> Vec<[f32; 3]> {
    let mut pts = Vec::new();
    let mut i = 0u32;
    for x in -8..=8 {
        for y in -8..=8 {
            for z in -8..=8 {
                // Blue-noise-ish jitter from a tiny LCG so no probe sits on
                // a min/max tie by construction.
                i = i.wrapping_mul(1664525).wrapping_add(1013904223);
                let j = (i >> 16) as f32 / 65536.0 - 0.5;
                pts.push([x as f32 * 3.0 + j, y as f32 * 3.0 - j, z as f32 * 3.0 + j]);
            }
        }
    }
    pts
}

/// Field identity |f_a − f_b| < tol on the probe cloud.
fn pd_assert_fields_equal(a: &CsgNode, b: &CsgNode, tol: f32, what: &str) {
    for p in pd_cloud() {
        let da = a.sdf(p, EvalMode::Visual);
        let db = b.sdf(p, EvalMode::Visual);
        assert!(
            (da - db).abs() < tol,
            "{what}: field mismatch at {p:?}: {da} vs {db}",
        );
    }
}

#[test]
fn offset_pushdown_union_outward_lands_on_leaves_and_matches_ground_truth() {
    let a = pd_sphere(1, [0.0, 0.0, 0.0], 10.0);
    let b = pd_sphere(2, [12.0, 0.0, 0.0], 6.0);
    let delta = 2.5;
    let orig = CsgNode::Offset(
        Box::new(CsgNode::Union(Box::new(a.clone()), Box::new(b.clone()))),
        delta,
    );
    let pushed = orig.clone().push_down_offsets();

    // Structure: Union(Offset(Leaf), Offset(Leaf)) — the offset reached the leaves.
    match &pushed {
        CsgNode::Union(l, r) => {
            assert!(
                matches!(**l, CsgNode::Offset(ref c, d) if d == delta && matches!(**c, CsgNode::Leaf { .. })),
                "left arm should be Offset(Leaf, +δ), got {l:?}",
            );
            assert!(
                matches!(**r, CsgNode::Offset(ref c, d) if d == delta && matches!(**c, CsgNode::Leaf { .. })),
                "right arm should be Offset(Leaf, +δ), got {r:?}",
            );
        }
        other => panic!("expected Union at root after pushdown, got {other:?}"),
    }

    // Field: identical to the un-pushed tree AND to the analytic ground
    // truth (union of the analytically dilated spheres).
    pd_assert_fields_equal(&orig, &pushed, 1e-4, "union outward, pushed vs orig");
    let ground_truth = CsgNode::Union(
        Box::new(pd_sphere(1, [0.0, 0.0, 0.0], 10.0 + delta)),
        Box::new(pd_sphere(2, [12.0, 0.0, 0.0], 6.0 + delta)),
    );
    pd_assert_fields_equal(&pushed, &ground_truth, 1e-4, "union outward, pushed vs analytic");
}

#[test]
fn offset_pushdown_difference_inward_flips_subtrahend_and_matches_ground_truth() {
    let a = pd_sphere(1, [0.0, 0.0, 0.0], 10.0);
    let b = pd_sphere(2, [8.0, 0.0, 0.0], 4.0);
    let delta = -1.5; // inward
    let orig = CsgNode::Offset(
        Box::new(CsgNode::Difference(Box::new(a.clone()), Box::new(b.clone()))),
        delta,
    );
    let pushed = orig.clone().push_down_offsets();

    // Structure: Difference(Offset(Leaf, −δ), Offset(Leaf, +δ)) — the
    // subtrahend takes the OPPOSITE offset (erode minuend, dilate subtrahend).
    match &pushed {
        CsgNode::Difference(l, r) => {
            assert!(
                matches!(**l, CsgNode::Offset(_, d) if d == delta),
                "minuend should keep the inward offset −1.5, got {l:?}",
            );
            assert!(
                matches!(**r, CsgNode::Offset(_, d) if d == -delta),
                "subtrahend should take the flipped offset +1.5, got {r:?}",
            );
        }
        other => panic!("expected Difference at root after pushdown, got {other:?}"),
    }

    pd_assert_fields_equal(&orig, &pushed, 1e-4, "difference inward, pushed vs orig");
    // Analytic ground truth: erode(A \ B, 1.5) = erode(A, 1.5) \ dilate(B, 1.5).
    let ground_truth = CsgNode::Difference(
        Box::new(pd_sphere(1, [0.0, 0.0, 0.0], 10.0 - 1.5)),
        Box::new(pd_sphere(2, [8.0, 0.0, 0.0], 4.0 + 1.5)),
    );
    pd_assert_fields_equal(&pushed, &ground_truth, 1e-4, "difference inward, pushed vs analytic");
}

#[test]
fn offset_pushdown_intersection_inward_distributes() {
    let a = pd_sphere(1, [0.0, 0.0, 0.0], 10.0);
    let b = pd_sphere(2, [6.0, 0.0, 0.0], 10.0);
    let orig = CsgNode::Offset(
        Box::new(CsgNode::Intersection(Box::new(a), Box::new(b))),
        -2.0,
    );
    let pushed = orig.clone().push_down_offsets();
    match &pushed {
        CsgNode::Intersection(l, r) => {
            assert!(matches!(**l, CsgNode::Offset(_, d) if d == -2.0));
            assert!(matches!(**r, CsgNode::Offset(_, d) if d == -2.0));
        }
        other => panic!("expected Intersection at root, got {other:?}"),
    }
    pd_assert_fields_equal(&orig, &pushed, 1e-4, "intersection inward");
}

#[test]
fn offset_pushdown_stops_at_non_exact_boolean_cases() {
    let a = pd_sphere(1, [0.0, 0.0, 0.0], 10.0);
    let b = pd_sphere(2, [6.0, 0.0, 0.0], 8.0);

    // Outward over Intersection: NOT exact (channel bodies at the seam) —
    // must keep today's `f − δ` at the boolean.
    let inter_out = CsgNode::Offset(
        Box::new(CsgNode::Intersection(Box::new(a.clone()), Box::new(b.clone()))),
        2.0,
    );
    assert!(
        matches!(
            inter_out.clone().push_down_offsets(),
            CsgNode::Offset(ref c, d) if d == 2.0 && matches!(**c, CsgNode::Intersection(_, _))
        ),
        "outward offset must stop above Intersection",
    );

    // Inward over Union: NOT exact — stop.
    let union_in = CsgNode::Offset(
        Box::new(CsgNode::Union(Box::new(a.clone()), Box::new(b.clone()))),
        -2.0,
    );
    assert!(
        matches!(
            union_in.clone().push_down_offsets(),
            CsgNode::Offset(ref c, d) if d == -2.0 && matches!(**c, CsgNode::Union(_, _))
        ),
        "inward offset must stop above Union",
    );

    // Outward over Difference: NOT exact — stop.
    let diff_out = CsgNode::Offset(
        Box::new(CsgNode::Difference(Box::new(a.clone()), Box::new(b.clone()))),
        2.0,
    );
    assert!(
        matches!(
            diff_out.clone().push_down_offsets(),
            CsgNode::Offset(ref c, d) if d == 2.0 && matches!(**c, CsgNode::Difference(_, _))
        ),
        "outward offset must stop above Difference",
    );

    // smin: no exact commutation — stop.
    let smin = CsgNode::Offset(
        Box::new(CsgNode::SmoothUnion(
            Box::new(a.clone()),
            Box::new(b.clone()),
            3.0,
            BlendMode::Fillet,
        )),
        2.0,
    );
    assert!(
        matches!(
            smin.clone().push_down_offsets(),
            CsgNode::Offset(ref c, d) if d == 2.0 && matches!(**c, CsgNode::SmoothUnion(_, _, _, _))
        ),
        "offset must stop above SmoothUnion",
    );

    // Regression: the stop cases keep today's field bit-for-bit.
    for (node, what) in [
        (&inter_out, "intersection outward"),
        (&union_in, "union inward"),
        (&diff_out, "difference outward"),
        (&smin, "smooth union"),
    ] {
        pd_assert_fields_equal(node, &node.clone().push_down_offsets(), 1e-6, what);
    }
}

#[test]
fn offset_pushdown_recurses_into_flipped_subtrahend_union() {
    // erode(A \ (B ∪ C)): the subtrahend flips to +δ, and +δ keeps pushing
    // through the inner Union.
    let a = pd_sphere(1, [0.0, 0.0, 0.0], 12.0);
    let b = pd_sphere(2, [5.0, 0.0, 0.0], 4.0);
    let c = pd_sphere(3, [-5.0, 0.0, 0.0], 4.0);
    let orig = CsgNode::Offset(
        Box::new(CsgNode::Difference(
            Box::new(a),
            Box::new(CsgNode::Union(Box::new(b), Box::new(c))),
        )),
        -1.0,
    );
    let pushed = orig.clone().push_down_offsets();
    match &pushed {
        CsgNode::Difference(l, r) => {
            assert!(matches!(**l, CsgNode::Offset(_, d) if d == -1.0));
            match &**r {
                CsgNode::Union(u, v) => {
                    assert!(matches!(**u, CsgNode::Offset(_, d) if d == 1.0));
                    assert!(matches!(**v, CsgNode::Offset(_, d) if d == 1.0));
                }
                other => panic!("subtrahend should stay a Union of dilated leaves, got {other:?}"),
            }
        }
        other => panic!("expected Difference at root, got {other:?}"),
    }
    pd_assert_fields_equal(&orig, &pushed, 1e-4, "difference with union subtrahend");
}

#[test]
fn offset_pushdown_passes_rigid_transform_but_stops_at_scale() {
    let leaf = pd_sphere(1, [0.0, 0.0, 0.0], 5.0);
    let rigid = Transform {
        translate: [3.0, 1.0, -2.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
    };
    let orig = CsgNode::Offset(
        Box::new(CsgNode::Transform(
            rigid,
            Box::new(CsgNode::Union(
                Box::new(leaf.clone()),
                Box::new(pd_sphere(2, [4.0, 0.0, 0.0], 3.0)),
            )),
        )),
        1.0,
    );
    let pushed = orig.clone().push_down_offsets();
    assert!(
        matches!(
            pushed,
            CsgNode::Transform(_, ref c) if matches!(**c, CsgNode::Union(_, _))
        ),
        "rigid transform must be traversed, got {pushed:?}",
    );
    pd_assert_fields_equal(&orig, &orig.clone().push_down_offsets(), 1e-4, "rigid transform");

    let scaled = Transform {
        translate: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [2.0, 1.0, 1.0],
    };
    let orig_scaled = CsgNode::Offset(
        Box::new(CsgNode::Transform(scaled, Box::new(leaf))),
        1.0,
    );
    assert!(
        matches!(
            orig_scaled.clone().push_down_offsets(),
            CsgNode::Offset(ref c, _) if matches!(**c, CsgNode::Transform(_, _))
        ),
        "non-unit scale must stop the pushdown",
    );
}

#[test]
fn offset_pushdown_merges_stacked_offsets() {
    // Offset(Offset(U, +1), +2) evaluates as f − 3 today; the pushdown must
    // fold the sum and distribute it.
    let u = CsgNode::Union(
        Box::new(pd_sphere(1, [0.0, 0.0, 0.0], 8.0)),
        Box::new(pd_sphere(2, [10.0, 0.0, 0.0], 5.0)),
    );
    let orig = CsgNode::Offset(Box::new(CsgNode::Offset(Box::new(u), 1.0)), 2.0);
    let pushed = orig.clone().push_down_offsets();
    match &pushed {
        CsgNode::Union(l, _) => {
            assert!(matches!(**l, CsgNode::Offset(_, d) if (d - 3.0).abs() < 1e-6));
        }
        other => panic!("expected Union after merging stacked offsets, got {other:?}"),
    }
    pd_assert_fields_equal(&orig, &pushed, 1e-4, "stacked offsets");
}

// ─── offset_tree Phase A2 (#384): Erosions-Blatt-Einfalten ──────────────

fn fold_leaf(id: u32, primitive: Primitive) -> CsgNode {
    CsgNode::Leaf { id: FeatureId::from_u32(id), primitive }
}

/// Erosions-Fold der analytischen Familien: Parameter schrumpfen in-family,
/// die Blatt-Id bleibt, das Feld ist das exakte SDF des erodierten Körpers.
#[test]
fn fold_erosion_analytic_families_shrink_in_family() {
    let a = 1.5f32;
    // Kugel
    let folded = CsgNode::Offset(
        Box::new(fold_leaf(1, Primitive::Sphere { center: [1.0, 2.0, 3.0], radius: 7.25 })),
        -a,
    )
    .fold_offset_leaves();
    match &folded {
        CsgNode::Leaf { id, primitive: Primitive::Sphere { center, radius } } => {
            assert_eq!(id, &FeatureId::from_u32(1));
            assert_eq!(center, &[1.0, 2.0, 3.0]);
            assert_eq!(radius.to_bits(), (7.25f32 - a).to_bits());
        }
        other => panic!("sphere erosion must fold, got {other:?}"),
    }
    // Zylinder: Radius UND Halbhöhe schrumpfen.
    let folded = CsgNode::Offset(
        Box::new(fold_leaf(2, Primitive::Cylinder { center: [0.0; 3], radius: 10.0, half_height: 6.0 })),
        -a,
    )
    .fold_offset_leaves();
    match &folded {
        CsgNode::Leaf { primitive: Primitive::Cylinder { radius, half_height, .. }, .. } => {
            assert_eq!(radius.to_bits(), (10.0f32 - a).to_bits());
            assert_eq!(half_height.to_bits(), (6.0f32 - a).to_bits());
        }
        other => panic!("cylinder erosion must fold, got {other:?}"),
    }
    // Box: geschrumpfte Box (KEIN neuer Variant).
    let folded = CsgNode::Offset(
        Box::new(fold_leaf(3, Primitive::Box { center: [0.0; 3], half_extents: [10.0, 8.0, 4.0] })),
        -a,
    )
    .fold_offset_leaves();
    match &folded {
        CsgNode::Leaf { primitive: Primitive::Box { half_extents, .. }, .. } => {
            assert_eq!(half_extents[0].to_bits(), (10.0f32 - a).to_bits());
            assert_eq!(half_extents[1].to_bits(), (8.0f32 - a).to_bits());
            assert_eq!(half_extents[2].to_bits(), (4.0f32 - a).to_bits());
        }
        other => panic!("box erosion must fold, got {other:?}"),
    }
    // Torus: nur der Rohrradius schrumpft.
    let folded = CsgNode::Offset(
        Box::new(fold_leaf(4, Primitive::Torus { center: [0.0; 3], major_radius: 20.0, minor_radius: 5.0 })),
        -a,
    )
    .fold_offset_leaves();
    match &folded {
        CsgNode::Leaf { primitive: Primitive::Torus { major_radius, minor_radius, .. }, .. } => {
            assert_eq!(major_radius.to_bits(), 20.0f32.to_bits());
            assert_eq!(minor_radius.to_bits(), (5.0f32 - a).to_bits());
        }
        other => panic!("torus erosion must fold, got {other:?}"),
    }
    // Feld-Identität: gefaltete Kugel == analytisch geschrumpfte Kugel;
    // Nullmenge identisch mit f − δ (auf exakten Blättern nullmengen-exakt).
    let orig = CsgNode::Offset(
        Box::new(fold_leaf(1, Primitive::Sphere { center: [0.0; 3], radius: 7.25 })),
        -a,
    );
    let folded = orig.clone().fold_offset_leaves();
    pd_assert_fields_equal(&folded, &pd_sphere(1, [0.0; 3], 7.25 - a), 1e-5, "eroded sphere");
    pd_assert_fields_equal(&orig, &folded, 1e-4, "sphere: f−δ == gefaltet (exaktes Blatt)");
}

/// Kegelstumpf: Wandneigung bleibt erhalten, Deckel und Wand rücken exakt
/// um `a` ein — Nullmenge identisch zu `f + a` (Innenfeld des Frustums ist
/// exakt), Parameter aus der Halbraum-Erosions-Identität.
#[test]
fn fold_erosion_cone_shifts_apex_keeps_slope() {
    let (rb, rt, hh, a) = (10.0f32, 4.0f32, 6.0f32, 1.0f32);
    let orig = CsgNode::Offset(
        Box::new(fold_leaf(9, Primitive::Cone { r_bottom: rb, r_top: rt, half_height: hh })),
        -a,
    );
    let folded = orig.clone().fold_offset_leaves();
    let (rb2, rt2, hh2) = match &folded {
        CsgNode::Leaf { primitive: Primitive::Cone { r_bottom, r_top, half_height }, .. } => {
            (*r_bottom, *r_top, *half_height)
        }
        other => panic!("cone erosion must fold, got {other:?}"),
    };
    assert_eq!(hh2.to_bits(), (hh - a).to_bits());
    // Neigung erhalten (reell exakt, f32 bis auf Rundung).
    let slope0 = (rb - rt) / (2.0 * hh);
    let slope1 = (rb2 - rt2) / (2.0 * hh2);
    assert!((slope0 - slope1).abs() < 1e-6, "slope {slope0} vs {slope1}");
    // Nullmengen-Abgleich an Wand und Deckeln: f+a und das gefaltete Feld
    // verschwinden an denselben Punkten.
    let wall_mid = 0.5 * (rb2 + rt2);
    for p in [
        [wall_mid, 0.0, 0.0],
        [0.0, hh2, 0.0],
        [0.0, -hh2, 0.0],
    ] {
        let df = folded.sdf(p, EvalMode::Visual);
        let du = orig.sdf(p, EvalMode::Visual);
        assert!(df.abs() < 1e-4, "folded cone surface at {p:?}: {df}");
        assert!(du.abs() < 1e-4, "unfolded cone surface at {p:?}: {du}");
    }
}

// ─── offset_tree Phase C1 (#384): Blatt-Parameter-Kollaps bei Erosion ────

/// C1: Erosion über die Trägergröße hinaus kollabiert das Blatt zu
/// [`CsgNode::Empty`] — parametrisch exakt, kein `f − δ`-Fallback.
#[test]
fn fold_erosion_collapse_yields_empty_leaf() {
    // Kugel r=5, Erosion 5 → leer (r − a ≤ Degenerationsgrenze).
    let folded = CsgNode::Offset(
        Box::new(fold_leaf(1, Primitive::Sphere { center: [1.0, 2.0, 3.0], radius: 5.0 })),
        -5.0,
    )
    .fold_offset_leaves();
    assert!(matches!(folded, CsgNode::Empty), "sphere must collapse, got {folded:?}");

    // Box: die KLEINSTE Halbausdehnung entscheidet.
    let folded = CsgNode::Offset(
        Box::new(fold_leaf(2, Primitive::Box { center: [0.0; 3], half_extents: [10.0, 1.5, 8.0] })),
        -1.5,
    )
    .fold_offset_leaves();
    assert!(matches!(folded, CsgNode::Empty), "box must collapse, got {folded:?}");

    // Zylinder: Höhen-Slab kollabiert vor dem Radius.
    let folded = CsgNode::Offset(
        Box::new(fold_leaf(3, Primitive::Cylinder { center: [0.0; 3], radius: 9.0, half_height: 2.0 })),
        -2.0,
    )
    .fold_offset_leaves();
    assert!(matches!(folded, CsgNode::Empty), "cylinder must collapse, got {folded:?}");

    // Knapp UNTER der Grenze: weiterhin normales In-family-Falten.
    let folded = CsgNode::Offset(
        Box::new(fold_leaf(4, Primitive::Sphere { center: [0.0; 3], radius: 5.0 })),
        -4.99,
    )
    .fold_offset_leaves();
    assert!(
        matches!(&folded, CsgNode::Leaf { primitive: Primitive::Sphere { .. }, .. }),
        "borderline erosion must still fold in-family, got {folded:?}"
    );
}

/// C1, „Erosion bis zum Verschwinden" (Plan §7-Test) am PROFIL-BOOLEAN:
/// das Union-Profil hat kein exaktes 2D-SDF (min unterschätzt die Tiefe
/// an der inneren Naht), `f − δ` löscht Material daher VERFRÜHT; der
/// geometrische Fold hält es exakt — und cavalier entscheidet den
/// echten Kollaps topologisch exakt → leeres Blatt statt Raten.
#[test]
fn fold_erosion_union_profile_exact_vanish_point() {
    use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};
    // Peanut-Profil: zwei Kreise r=3 um (±2, 0). Wahre Taillen-Tiefe bei
    // (0,0): √(9−4) ≈ 2.236; naives min meldet dort nur 1.0.
    let profile = Sketch2d::Union(
        Box::new(Sketch2d::Prim(Sketch2dPrimitive::Circle { center: [-2.0, 0.0], radius: 3.0 })),
        Box::new(Sketch2d::Prim(Sketch2dPrimitive::Circle { center: [2.0, 0.0], radius: 3.0 })),
    );
    let extrude = |id: u32| CsgNode::Extrude {
        id: FeatureId::from_u32(id),
        sketch: Box::new(profile.clone()),
        half_height: 10.0,
        wall_subs: None,
    };
    // a = 2.0 < wahre Taillen-Tiefe: der Fold hält die Taille als
    // Material; das naive f − δ hätte sie verfrüht gelöscht.
    let folded = CsgNode::Offset(Box::new(extrude(7)), -2.0).fold_offset_leaves();
    assert!(matches!(&folded, CsgNode::Extrude { .. }), "moderate erosion must fold");
    let waist = [0.0f32, 0.0, 0.0];
    let df = folded.sdf(waist, EvalMode::Visual);
    assert!(df < -0.1, "folded field must keep the waist as material ({df})");
    let naive = CsgNode::Offset(Box::new(extrude(7)), -2.0);
    let dn = naive.sdf(waist, EvalMode::Visual);
    assert!(
        dn > 0.5,
        "fixture self-check: naive f−δ erases the waist prematurely ({dn})"
    );
    // a = 3.05 > größter Innkreis (r=3): wahre Erosion leer → Empty (C1).
    let gone = CsgNode::Offset(Box::new(extrude(8)), -3.05).fold_offset_leaves();
    assert!(matches!(gone, CsgNode::Empty), "over-erosion must collapse to Empty");
}

/// C1, Plan §7-Test „Shell mit t > lokale Wandstärke": die
/// Shell-Expansions-Bauart `Difference(S, Offset(S, −t))` — bei t über
/// der halben Plattendicke kollabiert die Kavität zu Empty, die Bohrung
/// „schließt sich" korrekt (kein Geisterkörper in der Kavität, Feld ≡ S).
#[test]
fn shell_expansion_overthick_wall_collapses_cavity() {
    let plate = |id: u32| {
        CsgNode::Difference(
            Box::new(fold_leaf(id, Primitive::Box { center: [0.0; 3], half_extents: [30.0, 3.0, 20.0] })),
            Box::new(CsgNode::Transform(
                Transform::IDENTITY,
                Box::new(fold_leaf(id + 1, Primitive::Cylinder {
                    center: [0.0; 3],
                    radius: 4.0,
                    half_height: 10.0,
                })),
            )),
        )
    };
    // t = 3.5 > halbe Plattendicke 3: erode(Box) kollabiert; die Kavität
    // Offset(S,−t) = erode(Box) ∖ dilate(Cyl) wird leer.
    let cavity = CsgNode::Offset(Box::new(plate(1)), -3.5)
        .push_down_offsets()
        .fold_offset_leaves();
    let shell = CsgNode::Difference(Box::new(plate(1)), Box::new(cavity));
    let solid = plate(1);
    let mut rng_state = 0x1234_5678_u64;
    let mut rnd = move || {
        rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((rng_state >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
    };
    for _ in 0..2000 {
        let p = [rnd() * 32.0, rnd() * 5.0, rnd() * 22.0];
        let a = shell.sdf(p, EvalMode::Visual);
        let b = solid.sdf(p, EvalMode::Visual);
        assert!(
            (a - b).abs() < 1e-4,
            "over-thick shell must equal the solid at {p:?}: {a} vs {b}"
        );
    }
}

/// Blocker 2 (Audit): Fold-Arithmetik ≡ `offset_halfspace_inward` bit-exakt
/// je Carrier-Familie — dieselbe SSOT-Funktion speist beide Seiten.
#[test]
fn fold_matches_offset_halfspace_inward_bit_exact_per_family() {
    let a = 0.7f32;
    let cases: Vec<Primitive> = vec![
        Primitive::Sphere { center: [0.0; 3], radius: 7.25 },
        Primitive::Cylinder { center: [0.0; 3], radius: 9.1, half_height: 5.3 },
        Primitive::Box { center: [0.0; 3], half_extents: [10.0, 8.0, 4.0] },
        Primitive::Torus { center: [0.0; 3], major_radius: 20.0, minor_radius: 5.5 },
    ];
    for prim in cases {
        let folded = match CsgNode::Offset(Box::new(fold_leaf(1, prim.clone())), -a)
            .fold_offset_leaves()
        {
            CsgNode::Leaf { primitive, .. } => primitive,
            other => panic!("{prim:?} must fold, got {other:?}"),
        };
        let hs0 = primitive_halfspaces(&prim).expect("enumerable");
        let hs1 = primitive_halfspaces(&folded).expect("enumerable");
        assert_eq!(hs0.faces().len(), hs1.faces().len(), "face count preserved");
        for (f0, f1) in hs0.faces().iter().zip(hs1.faces()) {
            assert_eq!(f0.sub, f1.sub, "sub-face numbering preserved ({prim:?})");
            let shifted = offset_halfspace_inward(&f0.half_space, a)
                .expect("family supports inward offset");
            // Bit-exakter Parameter-Vergleich über die rmp-Serialisierung
            // (f32-bit-treu, gleiche Kodierung wie der Body-Cache-Hash).
            let a_bytes = rmp_serde::to_vec(&shifted).unwrap();
            let b_bytes = rmp_serde::to_vec(&f1.half_space).unwrap();
            assert_eq!(a_bytes, b_bytes, "carrier drift on {prim:?} sub {}", f0.sub);
        }
    }
    // Cone: Deckel-Ebenen bit-exakt; die Wand hat KEIN offset_halfspace_-
    // inward-Gegenstück (Nappen trennen sich — dokumentierte Familienlücke),
    // der Fold bleibt trotzdem in-family (eigene Apex-Verschiebung).
    let cone = Primitive::Cone { r_bottom: 10.0, r_top: 4.0, half_height: 6.0 };
    let hs = primitive_halfspaces(&cone).unwrap();
    assert!(offset_halfspace_inward(&hs.faces()[0].half_space, a).is_none());
}

/// Blocker 3 (Audit): der Fold erhält FeatureId-Schlüssel
/// (`leaf_primitives`) und den BranchId-Raum (`branch_halfspace_map`);
/// die Carrier des gefalteten Blatts sind bit-gleich zu denen, die der
/// Registry-Offset-Arm für den ungefalteten Baum verschiebt.
#[test]
fn fold_preserves_leaf_keys_and_branch_map_keys() {
    let tree = CsgNode::Union(
        Box::new(CsgNode::Offset(
            Box::new(fold_leaf(1, Primitive::Box { center: [0.0; 3], half_extents: [10.0, 8.0, 6.0] })),
            -1.25,
        )),
        Box::new(pd_sphere(2, [30.0, 0.0, 0.0], 5.0)),
    );
    let folded = tree.clone().fold_offset_leaves();

    let keys = |n: &CsgNode| {
        let mut v: Vec<u32> = n.leaf_primitives().keys().copied().collect();
        v.sort_unstable();
        v
    };
    assert_eq!(keys(&tree), keys(&folded), "leaf_primitives keys must survive the fold");

    let map_pre = crate::brep::weaving::branch_halfspace_map(&crate::bvh::BoundedCsg::from_csg(&tree));
    let map_post =
        crate::brep::weaving::branch_halfspace_map(&crate::bvh::BoundedCsg::from_csg(&folded));
    let mut k_pre: Vec<u64> = map_pre.keys().copied().collect();
    let mut k_post: Vec<u64> = map_post.keys().copied().collect();
    k_pre.sort_unstable();
    k_post.sort_unstable();
    assert_eq!(k_pre, k_post, "BranchId space must survive the fold");
    for (k, (hs_pre, _)) in &map_pre {
        let (hs_post, _) = &map_post[k];
        let a_bytes = rmp_serde::to_vec(hs_pre).unwrap();
        let b_bytes = rmp_serde::to_vec(hs_post).unwrap();
        assert_eq!(a_bytes, b_bytes, "carrier of branch {k} drifted between offset arm and fold");
    }
}

/// Verweigerungs-Doktrin: Familien außerhalb des A2-Scopes und Dilation
/// bleiben unangetastet (`f − δ` steht). Erosion ÜBER die Trägergröße ist
/// seit Phase C1 KEIN Verweigerungsfall mehr, sondern der exakte
/// parametrische Kollaps (leeres Blatt, Plan §7.1).
#[test]
fn fold_refuses_degenerate_out_of_scope_and_dilation() {
    // Erosion ≥ Radius: Phase C1 — kollabiert exakt zu Empty.
    let n = CsgNode::Offset(
        Box::new(fold_leaf(1, Primitive::Sphere { center: [0.0; 3], radius: 2.0 })),
        -2.5,
    )
    .fold_offset_leaves();
    assert!(matches!(n, CsgNode::Empty), "over-erosion collapses to Empty (C1)");
    // Out-of-scope-Familie (Capsule ist nicht in der Audit-Liste).
    let n = CsgNode::Offset(
        Box::new(fold_leaf(2, Primitive::Capsule { radius: 5.0, half_height: 4.0 })),
        -1.0,
    )
    .fold_offset_leaves();
    assert!(matches!(n, CsgNode::Offset(_, _)), "capsule is out of scope for A2");
    // Dilation bleibt f − δ (auf exakten Blättern bereits exakt).
    let n = CsgNode::Offset(
        Box::new(fold_leaf(3, Primitive::Sphere { center: [0.0; 3], radius: 2.0 })),
        1.5,
    )
    .fold_offset_leaves();
    assert!(matches!(n, CsgNode::Offset(_, d) if d == 1.5), "dilation must stay f − δ");
    // Erosions-Stopp-Knoten (Union) wird nicht angefasst — Phase B.
    let n = CsgNode::Offset(
        Box::new(CsgNode::Union(
            Box::new(pd_sphere(1, [0.0; 3], 10.0)),
            Box::new(pd_sphere(2, [12.0, 0.0, 0.0], 10.0)),
        )),
        -2.0,
    )
    .fold_offset_leaves();
    assert!(matches!(n, CsgNode::Offset(_, _)), "union at erosion is a Phase-B stop");
}

/// Union-PROFIL-Erosion (der nullmengen-korrigierende Leitfall): zwei
/// überlappende Kreise als Extrude-Profil, Erosion 2 mm. `f − δ` legt die
/// Nullmenge auf die Vereinigung der r−2-Kreise (Halsbereich fälschlich
/// ausgehöhlt); der Fold liefert die wahre Minkowski-Erosion mit
/// Join-Bögen an den Reflex-Kreuzungen — und propagiert `wall_subs`.
#[test]
fn fold_extrude_union_profile_corrects_zero_set_and_wall_subs() {
    let sketch = ProtoSketch2d::Union(
        Box::new(ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Circle {
            center: [-8.0, 0.0],
            radius: 10.0,
        })),
        Box::new(ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Circle {
            center: [8.0, 0.0],
            radius: 10.0,
        })),
    );
    let orig = CsgNode::Offset(
        Box::new(CsgNode::Extrude {
            id: FeatureId::from_u32(5),
            sketch: Box::new(sketch),
            half_height: 12.0,
            wall_subs: Some(std::sync::Arc::new(vec![7, 9])),
        }),
        -2.0,
    );
    let folded = orig.clone().fold_offset_leaves();
    let table = match &folded {
        CsgNode::Extrude { id, half_height, wall_subs, .. } => {
            assert_eq!(id, &FeatureId::from_u32(5), "FeatureId bleibt");
            assert_eq!(*half_height, 10.0, "half_height − |δ|");
            wall_subs.as_ref().expect("propagierte Tabelle").as_ref().clone()
        }
        other => panic!("union profile erosion must fold, got {other:?}"),
    };
    // Zwei Quellwände (Subs 7 und 9) + zwei Join-Bögen aus dem Band.
    let mut t = table.clone();
    t.sort_unstable();
    assert_eq!(t, vec![7, 9, 0x700, 0x701], "wall_subs propagation: {table:?}");

    // Nullmengen-Korrektur am Hals: die wahre Erosionsgrenze liegt bei
    // (0, ±4) (Join-Bogen r=2 um die Kreuzung (0, ±6)); der Ursprung liegt
    // 4 mm TIEF im erodierten Körper. f − δ legt ihn fälschlich AUF die
    // Nullmenge (min-Feld sieht die weggeschnittene Kreiswand).
    let du = orig.sdf([0.0, 0.0, 0.0], EvalMode::Visual);
    assert!(du.abs() < 1e-4, "f−δ places the origin ON its zero set: {du}");
    // Innen-Probe knapp neben der Symmetriezeile y=0 (exakt auf y=0 liegen
    // die Bogen-Nahtpunkte der rekonstruierten Arcs — die Even-Odd-Parität
    // von Path-Bögen ist auf der Nahtzeile eine bekannte, vorbestehende
    // Fragilität): wahre Tiefe = |p − (0,6)| − 2.
    let p_neck = [0.37f32, 0.11, 0.0];
    let true_depth = (0.37f32.powi(2) + (6.0 - 0.11f32).powi(2)).sqrt() - 2.0;
    let df = folded.sdf(p_neck, EvalMode::Visual);
    assert!(
        (df + true_depth).abs() < 1e-3,
        "true erosion depth at the neck: {df} vs −{true_depth}",
    );
    let du_neck = orig.sdf(p_neck, EvalMode::Visual);
    assert!(du_neck > -0.5, "f−δ misreads the neck as near-boundary: {du_neck}");
    for p in [[0.0f32, 4.0, 0.0], [0.0, -4.0, 0.0], [-16.0, 0.0, 0.0], [16.0, 0.0, 0.0]] {
        let d = folded.sdf(p, EvalMode::Visual);
        assert!(d.abs() < 1e-3, "eroded boundary at {p:?}: {d}");
    }
    // Deckel: an z = ±10 endet der erodierte Körper.
    let d = folded.sdf([-8.0, 1.3, 10.0], EvalMode::Visual);
    assert!(d.abs() < 1e-3, "eroded cap at z=10: {d}");
}

/// Differenz-Profil (Leitszene iter3_extrude_bool_profile): Rechteck minus
/// Randkreis, Erosion 2 mm. Die gespaltene Oberkante behält ZWEIMAL die
/// stabile Sub der Quellwand (gleiche Carrier-Semantik wie eine durch
/// einen Slot gespaltene Box-Fläche), der Kerb-Bogen behält die Kreis-Sub.
#[test]
fn fold_extrude_difference_profile_propagates_split_wall_subs() {
    let sketch = ProtoSketch2d::Difference(
        Box::new(ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [30.0, 15.0],
        })),
        Box::new(ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Circle {
            center: [0.0, 15.0],
            radius: 10.0,
        })),
    );
    let orig = CsgNode::Offset(
        Box::new(CsgNode::Extrude {
            id: FeatureId::from_u32(3),
            sketch: Box::new(sketch),
            half_height: 12.0,
            wall_subs: Some(std::sync::Arc::new(vec![10, 11, 12, 13, 20])),
        }),
        -2.0,
    );
    let folded = orig.clone().fold_offset_leaves();
    let table = match &folded {
        CsgNode::Extrude { half_height, wall_subs, .. } => {
            assert_eq!(*half_height, 10.0);
            wall_subs.as_ref().expect("Tabelle").as_ref().clone()
        }
        other => panic!("difference profile erosion must fold, got {other:?}"),
    };
    let mut t = table.clone();
    t.sort_unstable();
    assert_eq!(
        t,
        vec![10, 11, 12, 12, 13, 20],
        "split top wall keeps its sub twice, notch arc keeps the circle sub: {table:?}",
    );
    // Geometrie: Boden bei y=−13, rechte Wand x=28, Kerb-Bogen r=12 →
    // tiefster Punkt (0, 3).
    for p in [[0.0f32, -13.0, 0.0], [28.0, 0.0, 0.0], [0.0, 3.0, 0.0]] {
        let d = folded.sdf(p, EvalMode::Visual);
        assert!(d.abs() < 1e-3, "eroded boundary at {p:?}: {d}");
    }
    assert!(folded.sdf([0.0, -5.0, 0.0], EvalMode::Visual) < -1.0, "interior stays solid");
}

/// `Sketch2d::Offset` im Profil wird VOR dem Fold geometrisch aufgelöst
/// (Leitszene iter3_extrude_offset_profile: L-Polygon +4 dilatiert), dann
/// erodiert — Wände liegen bei Netto +3, Verrundungsbögen an den konvexen
/// Ecken bekommen deterministische Band-Subs.
#[test]
fn fold_extrude_resolves_sketch_offset_before_folding() {
    let l_poly = ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Polygon {
        vertices: vec![
            [-20.0, -20.0],
            [20.0, -20.0],
            [20.0, 0.0],
            [0.0, 0.0],
            [0.0, 20.0],
            [-20.0, 20.0],
        ],
    });
    let sketch = ProtoSketch2d::Offset { inner: Box::new(l_poly), distance: 4.0 };
    let orig = CsgNode::Offset(
        Box::new(CsgNode::Extrude {
            id: FeatureId::from_u32(8),
            sketch: Box::new(sketch),
            half_height: 10.0,
            wall_subs: None,
        }),
        -1.0,
    );
    let folded = orig.clone().fold_offset_leaves();
    let table = match &folded {
        CsgNode::Extrude { half_height, wall_subs, .. } => {
            assert_eq!(*half_height, 9.0);
            wall_subs.as_ref().expect("Tabelle").as_ref().clone()
        }
        other => panic!("offset profile erosion must fold, got {other:?}"),
    };
    // Positionale Original-Subs (Polygon-Gruppen 0..5) bleiben erhalten;
    // Join-Bögen liegen im Band.
    let originals: Vec<u32> = table.iter().copied().filter(|&s| s < 0x700).collect();
    let joins: Vec<u32> = table.iter().copied().filter(|&s| s >= 0x700).collect();
    let mut o = originals.clone();
    o.sort_unstable();
    o.dedup();
    assert_eq!(o, vec![0, 1, 2, 3, 4, 5], "alle sechs L-Wände benannt: {table:?}");
    assert!(!joins.is_empty(), "konvexe Ecken müssen Verrundungs-Subs tragen");
    // Netto-Offset +3: rechte Wand des unteren Arms bei x = 23, Boden bei
    // y = −23; Verrundungsbogen r=3 um die Ecke (20, −20).
    for p in [[23.0f32, -10.0, 0.0], [0.0, -23.0, 0.0]] {
        let d = folded.sdf(p, EvalMode::Visual);
        assert!(d.abs() < 1e-3, "net +3 boundary at {p:?}: {d}");
    }
    let s = 3.0 / 2.0f32.sqrt();
    let d = folded.sdf([20.0 + s, -20.0 - s, 0.0], EvalMode::Visual);
    assert!(d.abs() < 1e-3, "corner join arc r=3 at (20,−20): {d}");
}

/// Extrude-Verweigerungen: QuadBezier-Profil und Erosion ≥ Halbhöhe.
#[test]
fn fold_extrude_refusals_keep_f_minus_delta() {
    let bez = ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Path {
        segments: vec![
            fielddraft_protocol::sketch2d::SketchSegment::Line { start: [0.0, 0.0], end: [4.0, 0.0] },
            fielddraft_protocol::sketch2d::SketchSegment::QuadBezier {
                p0: [4.0, 0.0],
                p1: [4.0, 4.0],
                p2: [0.0, 4.0],
            },
            fielddraft_protocol::sketch2d::SketchSegment::Line { start: [0.0, 4.0], end: [0.0, 0.0] },
        ],
        closed: true,
    });
    let n = CsgNode::Offset(
        Box::new(CsgNode::Extrude {
            id: FeatureId::from_u32(1),
            sketch: Box::new(bez),
            half_height: 5.0,
            wall_subs: None,
        }),
        -0.5,
    )
    .fold_offset_leaves();
    assert!(matches!(n, CsgNode::Offset(_, _)), "QuadBezier profile must refuse");

    let circle = ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Circle { center: [0.0, 0.0], radius: 10.0 });
    let n = CsgNode::Offset(
        Box::new(CsgNode::Extrude {
            id: FeatureId::from_u32(1),
            sketch: Box::new(circle),
            half_height: 5.0,
            wall_subs: None,
        }),
        -5.5,
    )
    .fold_offset_leaves();
    // Phase C1: Erosion ≥ half_height ist der exakte Slab-Kollaps
    // (Erosion distribuiert über die Prisma-Intersektion) → Empty.
    assert!(matches!(n, CsgNode::Empty), "erosion ≥ half_height collapses to Empty (C1)");
}

/// In-family-Schnellpfad: Kreis-/Rechteck-Profile schrumpfen ohne Umbau,
/// `wall_subs` bleiben unverändert (Gruppenstruktur identisch).
#[test]
fn fold_extrude_in_family_circle_keeps_wall_subs() {
    let ws = std::sync::Arc::new(vec![42u32]);
    let n = CsgNode::Offset(
        Box::new(CsgNode::Extrude {
            id: FeatureId::from_u32(1),
            sketch: Box::new(ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Circle {
                center: [2.0, 3.0],
                radius: 10.0,
            })),
            half_height: 5.0,
            wall_subs: Some(ws.clone()),
        }),
        -1.5,
    )
    .fold_offset_leaves();
    match &n {
        CsgNode::Extrude { sketch, half_height, wall_subs, .. } => {
            assert_eq!(*half_height, 3.5);
            match sketch.as_ref() {
                ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Circle { center, radius }) => {
                    assert_eq!(center, &[2.0, 3.0]);
                    assert_eq!(radius.to_bits(), (10.0f32 - 1.5).to_bits());
                }
                other => panic!("circle must stay a circle, got {other:?}"),
            }
            assert!(std::sync::Arc::ptr_eq(wall_subs.as_ref().unwrap(), &ws), "wall_subs untouched");
        }
        other => panic!("circle extrude must fold in-family, got {other:?}"),
    }
}

/// Revolve: achsenfernes Ring-Profil (Leitszene iter3_revolve_ring_arcs)
/// erodiert im Meridian; achsberührendes Profil verweigert (#390-Gate).
#[test]
fn fold_revolve_ring_erodes_meridian_and_gates_axis() {
    use fielddraft_protocol::sketch2d::SketchSegment as Seg;
    use std::f32::consts::PI;
    let seg_line = |a: [f32; 2], b: [f32; 2]| Seg::Line { start: a, end: b };
    let seg_arc = |c: [f32; 2], r: f32, a0: f32, sw: f32| Seg::Arc {
        center: c,
        radius: r,
        start_angle: a0,
        sweep_angle: sw,
    };
    // Gerundetes Rechteck u ∈ [18, 32], v ∈ [−10, 10], Ecken r=3.
    let ring = ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Path {
        segments: vec![
            seg_line([21.0, -10.0], [29.0, -10.0]),
            seg_arc([29.0, -7.0], 3.0, -PI / 2.0, PI / 2.0),
            seg_line([32.0, -7.0], [32.0, 7.0]),
            seg_arc([29.0, 7.0], 3.0, 0.0, PI / 2.0),
            seg_line([29.0, 10.0], [21.0, 10.0]),
            seg_arc([21.0, 7.0], 3.0, PI / 2.0, PI / 2.0),
            seg_line([18.0, 7.0], [18.0, -7.0]),
            seg_arc([21.0, -7.0], 3.0, PI, PI / 2.0),
        ],
        closed: true,
    });
    let orig = CsgNode::Offset(
        Box::new(CsgNode::Revolve {
            id: FeatureId::from_u32(2),
            sketch: Box::new(ring),
            wall_subs: None,
        }),
        -1.0,
    );
    let folded = orig.clone().fold_offset_leaves();
    let table = match &folded {
        CsgNode::Revolve { id, wall_subs, .. } => {
            assert_eq!(id, &FeatureId::from_u32(2));
            wall_subs.as_ref().expect("Tabelle").as_ref().clone()
        }
        other => panic!("off-axis revolve erosion must fold, got {other:?}"),
    };
    // 8 G1-Gruppen (Linie/Bogen wechseln Carrier), alle konvex → keine
    // Join-Bögen; die Tabelle ist eine Permutation der Originalgruppen.
    let mut t = table.clone();
    t.sort_unstable();
    assert_eq!(t, (0u32..8).collect::<Vec<_>>(), "table: {table:?}");
    // Erodierter Ring: u ∈ [19, 31] → Punkte auf Innen-/Außenwand.
    for p in [[19.0f32, 0.0, 0.0], [31.0, 0.0, 0.0], [25.0, 9.0, 0.0]] {
        let d = folded.sdf(p, EvalMode::Visual);
        assert!(d.abs() < 2e-3, "eroded revolve boundary at {p:?}: {d}");
    }
    assert!(folded.sdf([25.0, 0.0, 0.0], EvalMode::Visual) < -5.0, "core stays solid");

    // Achsberührendes Profil: Fold verweigern (Phantom-Nullmengen-Gate).
    let solid = ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Rectangle {
        center: [15.0, 0.0],
        half_extents: [15.0, 20.0],
    });
    let n = CsgNode::Offset(
        Box::new(CsgNode::Revolve {
            id: FeatureId::from_u32(2),
            sketch: Box::new(solid),
            wall_subs: None,
        }),
        -2.0,
    )
    .fold_offset_leaves();
    assert!(matches!(n, CsgNode::Offset(_, _)), "axis-touching revolve must refuse");
}

/// Pushdown + Fold als Pipeline (wie in den Body-Baupfaden): die Erosion
/// einer Differenz landet nach dem Pushdown an den Blättern und faltet
/// dort in-family; das Feld bleibt punktweise das der ungefalteten Kette.
#[test]
fn pushdown_then_fold_pipeline_on_difference() {
    let orig = CsgNode::Offset(
        Box::new(CsgNode::Difference(
            Box::new(pd_sphere(1, [0.0; 3], 10.0)),
            Box::new(pd_sphere(2, [8.0, 0.0, 0.0], 4.0)),
        )),
        -1.5,
    );
    let processed = orig.clone().push_down_offsets().fold_offset_leaves();
    match &processed {
        CsgNode::Difference(l, r) => {
            // Minuend: Erosion → gefaltetes Blatt r − 1.5.
            assert!(
                matches!(**l, CsgNode::Leaf { primitive: Primitive::Sphere { radius, .. }, .. } if radius == 8.5),
                "minuend must fold to r−1.5, got {l:?}",
            );
            // Subtrahend: Dilation (+1.5) bleibt f − δ.
            assert!(
                matches!(**r, CsgNode::Offset(_, d) if d == 1.5),
                "subtrahend dilation must stay f − δ, got {r:?}",
            );
        }
        other => panic!("expected Difference, got {other:?}"),
    }
    pd_assert_fields_equal(&orig, &processed, 1e-4, "pushdown+fold difference");
}

/// Blocker 1 + 6 (Audit): der Weave-Baupfad (`build_bounded_for_body_local`)
/// fährt dieselbe Pushdown+Fold-Normalisierung wie der Mesher-/Opcode-Pfad.
/// Beweis am nullmengen-korrigierenden Leitfall (Union-Profil-Erosion, wo
/// f − δ und wahres Offset am Hals 4 mm auseinanderliegen): JEDE gewobene
/// B-Rep-Kante liegt auf der Nullmenge des kompilierten Opcode-Felds
/// (|f| < 0.02 mm — die ON_SURFACE_TOL-Klasse, unter der Feature-Snap-Pins
/// still verworfen würden und CGAL-Schutzkanten neben der Fläche lägen).
#[test]
fn weave_edges_lie_on_the_folded_opcode_field() {
    let sketch = ProtoSketch2d::Union(
        Box::new(ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Circle {
            center: [-8.0, 0.0],
            radius: 10.0,
        })),
        Box::new(ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Circle {
            center: [8.0, 0.0],
            radius: 10.0,
        })),
    );
    let feature = DecodedFeature {
        id: FeatureId::from_u32(1),
        name: "eroded peanut".into(),
        op: BoolOp::New,
        blend_radius: 0.0,
        blend_mode: BlendMode::Fillet,
        target_ids: vec![],
        transform: Transform::IDENTITY,
        node: CsgNode::Offset(
            Box::new(CsgNode::Extrude {
                id: FeatureId::from_u32(1),
                sketch: Box::new(sketch),
                half_height: 12.0,
                wall_subs: Some(std::sync::Arc::new(vec![7, 9])),
            }),
            -2.0,
        ),
    };
    let features = vec![feature];
    let parts = crate::body::partition(&features);
    assert_eq!(parts.len(), 1);
    let mut body = crate::body::Body {
        id: parts[0].seed_id(&features),
        feature_indices: parts[0].feature_indices.clone(),
        aabb: parts[0].aabb,
        payload_hash: 0,
        geometry_hash: 0,
        transform_hash: 0,
        mesh_resolution: 0,
        mesh: crate::dc::Mesh::default(),
        version: 1,
        transform_delta: None,
        last_mesh_transforms: Vec::new(),
        compiled_sdf: None,
        placement: Transform::IDENTITY,
        brep: None,
    };
    body.ensure_brep(&features, true);
    let brep = body.brep_cached().expect("weave produced a B-Rep");
    assert!(!brep.edges.is_empty(), "eroded extrude must weave edges (cap rims)");

    // Opcode-Feld aus dem Mesher-Baupfad (mit Pushdown + Fold).
    let (bounded, _) =
        crate::body::build_bounded_with_primitives(&features, &body.feature_indices, crate::offset_channel::CHANNEL_TOL_RENDER);
    let compiled = crate::sdf_compiler::compile_body(&bounded);
    assert!(compiled.volumes.is_empty());

    // Placement ist Identity — Body-lokale Kantenpunkte SIND Weltpunkte.
    assert_eq!(body.placement.translate, [0.0, 0.0, 0.0]);
    let mut checked = 0usize;
    for e in &brep.edges {
        for p in crate::brep::edge_sampling::sample_edge(&e.geometry) {
            let w = [p[0] as f32, p[1] as f32, p[2] as f32];
            let f = crate::sdf_compiler::eval_opcodes(&compiled.opcodes, w);
            assert!(
                f.abs() < 0.02,
                "B-Rep edge point {w:?} lies {f} mm off the folded field (weave/field split)",
            );
            checked += 1;
        }
    }
    assert!(checked > 16, "expected a real edge sampling, got {checked} points");

    // Und die Kanten beschreiben den ERODIERTEN Körper: alle Punkte liegen
    // innerhalb der erodierten Hülle (|x| ≤ 16 + ε), keiner auf der alten
    // r=10-Kontur (max |x| = 18).
    for e in &brep.edges {
        for p in crate::brep::edge_sampling::sample_edge(&e.geometry) {
            assert!(
                p[0].abs() <= 16.0 + 1e-3,
                "edge point {p:?} lies outside the eroded body (unfolded weave?)",
            );
        }
    }
}

// ─── #449: the edge-cutter gate must not call a convex edge concave ───────

/// #449: `edge_cutter_frame` refuses a blend that is bigger than the wall it
/// sits on — rightly, since nothing can be cut out of material that is not
/// there — but it used to report that refusal as `Concave`, i.e. as an INSIDE
/// corner. The two are different facts, and the difference is measurable:
/// convexity is a property of the DIHEDRAL, so a verdict that flips when only
/// `reach` changes cannot be one.
///
/// That mislabel is what sent #449 looking for an inside corner in the #416
/// document. There is none: the document chamfers a 5 mm bevel onto a 3 mm
/// plate, and `Chamfer 1`'s two edges are plainly convex outer edges (adjacent
/// normals (−1, 0, 0) and (0, 0, 1)).
///
/// The gate is unchanged in what it ACCEPTS — this test pins both halves:
/// the accept/reject boundary (2 mm passes, 3 mm does not, on a 3 mm plate)
/// and which REASON each side of it gets.
#[test]
fn f449_edge_cutter_gate_separates_concave_from_oversized() {
    let plate = |hx: f32| CsgNode::Leaf {
        id: FeatureId::from_u32(1),
        primitive: Primitive::Box { center: [0.0; 3], half_extents: [hx, 40.0, 40.0] },
    };
    // The edge between the +x face and the +z face of a plate: an OUTER,
    // convex edge, running along y. Its dihedral does not depend on `hx`.
    let gate = |node: &CsgNode, hx: f32, reach: f32| {
        edge_cutter_frame(node, [hx, -20.0, 40.0], [hx, 20.0, 40.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], reach)
    };

    // A 3 mm plate (hx = 1.5). The rolling-ball centre sits `reach` inside BOTH
    // faces, so it leaves the plate through the far face at reach > 1.5 mm —
    // and 0.5·reach of margin is required on top, which is why 2 mm still
    // passes (the centre is 0.5 mm outside the +x face but 2 mm inside the very
    // large −z half-space, and the box SDF reads the nearer of the two).
    let thin = plate(1.5);
    for reach in [0.5f32, 1.0, 1.5, 2.0] {
        assert!(gate(&thin, 1.5, reach).is_ok(), "3 mm plate, {reach} mm cutter: must still fit");
    }
    for reach in [3.0f32, 5.0] {
        assert_eq!(
            gate(&thin, 1.5, reach).err(),
            Some(EdgeCutterReject::LargerThanMaterial),
            "3 mm plate, {reach} mm cutter: too big for the wall — and NOT concave",
        );
    }

    // The SAME edge on a thick plate takes every one of those cutters. This is
    // the control that makes the point: nothing about the corner changed.
    let thick = plate(10.0);
    for reach in [0.5f32, 1.0, 1.5, 2.0, 3.0, 5.0] {
        assert!(gate(&thick, 10.0, reach).is_ok(), "20 mm plate, {reach} mm cutter: must fit");
    }

    // And a genuinely CONCAVE edge stays refused as concave at every size,
    // including one far smaller than the material — the property the gate
    // exists for (a hard cutter there would carve material out of an inside
    // corner). Two slabs meeting in an inside corner at (1.5, ·, 1.5).
    let corner = CsgNode::Union(
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box { center: [20.0, 0.0, 0.0], half_extents: [20.0, 40.0, 1.5] },
        }),
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Box { center: [0.0, 0.0, 20.0], half_extents: [1.5, 40.0, 20.0] },
        }),
    );
    for reach in [0.5f32, 1.0, 5.0] {
        assert_eq!(
            edge_cutter_frame(
                &corner,
                [1.5, -20.0, 1.5],
                [1.5, 20.0, 1.5],
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                reach,
            )
            .err(),
            Some(EdgeCutterReject::Concave),
            "inside corner, {reach} mm cutter: concave at every size",
        );
    }
}

// ─── #450: a hard cutter for a dihedral that is NOT a right angle ─────────

/// The #416 wedge tip, reduced to its dihedral.
///
/// `Extrude 9` of that document ends in a wedge whose tip edge runs between the
/// part's outer wall (normal `(1, 0, 0)`) and the wedge's long underside
/// (normal `(−0.126219, 0, −0.992002)`) — a CONVEX edge with an interior angle
/// of 82.75°, and Onshape rounds it with a 1.3648 mm fillet. Until #450 that
/// fillet was a silent no-op: `edge_cutter_frame`'s P1 gate took right angles
/// only (`|n_a·n_b| ≤ 2e-2`), and the `smin` fallback needs a Boolean junction
/// between two DIFFERENT features, which a wedge's own tip does not have (the
/// #406 mechanism). The tip stayed sharp and the part's `z min` reached
/// 0.1675 mm past Onshape's.
///
/// Everything asserted here is the rolling-ball definition, in the wedge's own
/// bisector frame, and holds for any dihedral:
///
///  - the corner is gone, and gone by exactly `|C| − r` (the ball's clearance);
///  - both TANGENCY points still lie on the surface — the blend meets each face
///    smoothly instead of stepping;
///  - material PAST the tangency points is untouched, which is what a cutter
///    that is too wide would eat.
#[test]
fn f450_non_orthogonal_convex_edge_gets_a_hard_fillet_cutter() {
    let na = [1.0f32, 0.0, 0.0];
    let nb = [-0.126218598895937f32, 0.0, -0.9920024522614582];
    let r = 1.3647571568806202f32;

    // The wedge: {x ≤ 0} ∩ {n_b·p ≤ 0}, 5 mm deep along y — the two half-spaces
    // as boxes, so the edge at the origin belongs to no Boolean junction that
    // the mask fallback could rewrite.
    let cos_t = nb[0];
    let rot_y = [0.0f32, ((1.0 - cos_t) * 0.5).sqrt(), 0.0, ((1.0 + cos_t) * 0.5).sqrt()];
    let wedge = CsgNode::Intersection(
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box { center: [-20.0, 0.0, 0.0], half_extents: [20.0, 2.5, 40.0] },
        }),
        Box::new(CsgNode::Transform(
            Transform {
                translate: [-30.0 * nb[0], 0.0, -30.0 * nb[2]],
                rotation: rot_y,
                scale: [1.0; 3],
            },
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(2),
                primitive: Primitive::Box { center: [0.0; 3], half_extents: [30.0, 2.5, 40.0] },
            }),
        )),
    );
    let sdf = |n: &CsgNode, p: [f32; 3]| n.sdf(p, EvalMode::Visual);

    // The dihedral is the one the gate used to refuse outright.
    let cdot = na[0] * nb[0] + na[2] * nb[2];
    assert!((cdot + 0.126218598895937).abs() < 1e-6, "cdot {cdot}");
    let frame = edge_cutter_frame(&wedge, [0.0, -2.5, 0.0], [0.0, 2.5, 0.0], na, nb, r)
        .expect("an 82.75° convex edge must take a cutter (#450)");
    assert!((frame.cdot - cdot).abs() < 1e-5);

    // Sharp before: the corner sits ON the surface.
    assert!(sdf(&wedge, [0.0, 0.0, 0.0]).abs() < 1e-3, "corner must start sharp");

    let fillet = EdgeFillet {
        fillet_id: FeatureId::from_u32(9),
        // Deliberately the SAME feature on both sides: that is the wedge's own
        // tip, and it is what makes the mask fallback provably powerless here.
        feature_a: FeatureId::from_u32(1),
        feature_b: FeatureId::from_u32(1),
        branch_a: 0,
        branch_b: 0,
        mask_polyline: vec![[0.0, -2.5, 0.0], [0.0, 2.5, 0.0]],
        mask_radius: 3.0 * r,
        blend_radius: r,
        blend_mode: BlendMode::Fillet,
        cutter: Some(FilletCutter {
            normal_a: na,
            normal_b: nb,
            edge_start: [0.0, -2.5, 0.0],
            edge_end: [0.0, 2.5, 0.0],
            radius: r,
        }),
    };
    let rounded = wedge.clone().apply_edge_fillets(&[fillet]);

    // 1. The corner is cut back to the ball: |C| − r along the bisector.
    let c = frame.inner;
    let c_len = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
    let at_corner = sdf(&rounded, [0.0, 0.0, 0.0]);
    assert!(
        (at_corner - (c_len - r)).abs() < 0.01,
        "corner clearance {at_corner} vs expected {}",
        c_len - r,
    );

    // 2/3. Both tangency points still lie on the surface, and so does material
    //      a further 2 mm along each face — the cutter stops at the tangency.
    //      `t_a` runs along face A away from the corner: −n_b, projected.
    let along = |n: [f32; 3], other: [f32; 3]| {
        let d = [
            -(other[0] - cdot * n[0]),
            -(other[1] - cdot * n[1]),
            -(other[2] - cdot * n[2]),
        ];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        [d[0] / l, d[1] / l, d[2] / l]
    };
    for (n, other, label) in [(na, nb, "face A"), (nb, na, "face B")] {
        let t = [c[0] + r * n[0], c[1] + r * n[1], c[2] + r * n[2]];
        assert!(sdf(&rounded, t).abs() < 0.01, "{label}: tangency point off the surface");
        let dir = along(n, other);
        for extra in [0.5f32, 2.0, 6.0] {
            let p = [
                t[0] + extra * dir[0],
                t[1] + extra * dir[1],
                t[2] + extra * dir[2],
            ];
            assert!(
                sdf(&rounded, p).abs() < 0.01,
                "{label}: the face is eaten {extra} mm past the tangency point",
            );
            let inside = [p[0] - 0.5 * n[0], p[1] - 0.5 * n[1], p[2] - 0.5 * n[2]];
            assert!(sdf(&rounded, inside) < -0.4, "{label}: material missing behind the face");
        }
    }

    // 4. And the fillet really is a ROUNDING, not a bevel: every point at
    //    distance r from the ball centre, between the two tangency directions,
    //    is on the surface.
    for f in [0.1f32, 0.3, 0.5, 0.7, 0.9] {
        let d = [
            na[0] + f * (nb[0] - na[0]),
            na[1] + f * (nb[1] - na[1]),
            na[2] + f * (nb[2] - na[2]),
        ];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let p = [c[0] + r * d[0] / l, c[1] + r * d[1] / l, c[2] + r * d[2] / l];
        assert!(sdf(&rounded, p).abs() < 0.01, "arc point at f={f} is not on the surface");
    }
}

/// The chamfer's bevel is the same generalisation, one line of trigonometry
/// apart: EQUAL_OFFSETS cuts `d` back along BOTH faces, so the bevel plane sits
/// `d·sin φ` from the edge along the bisector (`d/√2` at a right angle — the
/// only case P1 could do). Same wedge, same edge.
#[test]
fn f450_non_orthogonal_convex_edge_gets_a_hard_chamfer_cutter() {
    let na = [1.0f32, 0.0, 0.0];
    let nb = [-0.126218598895937f32, 0.0, -0.9920024522614582];
    let d = 1.5f32;
    let cos_t = nb[0];
    let rot_y = [0.0f32, ((1.0 - cos_t) * 0.5).sqrt(), 0.0, ((1.0 + cos_t) * 0.5).sqrt()];
    let wedge = CsgNode::Intersection(
        Box::new(CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box { center: [-20.0, 0.0, 0.0], half_extents: [20.0, 2.5, 40.0] },
        }),
        Box::new(CsgNode::Transform(
            Transform {
                translate: [-30.0 * nb[0], 0.0, -30.0 * nb[2]],
                rotation: rot_y,
                scale: [1.0; 3],
            },
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(2),
                primitive: Primitive::Box { center: [0.0; 3], half_extents: [30.0, 2.5, 40.0] },
            }),
        )),
    );
    let cut = wedge.apply_edge_chamfers(&[EdgeChamfer {
        chamfer_id: FeatureId::from_u32(9),
        normal_a: na,
        normal_b: nb,
        edge_start: [0.0, -2.5, 0.0],
        edge_end: [0.0, 2.5, 0.0],
        distance: d,
    }]);
    let sdf = |p: [f32; 3]| cut.sdf(p, EvalMode::Visual);
    let cdot = na[0] * nb[0] + na[2] * nb[2];
    let sin_half = ((1.0 - cdot) * 0.5).sqrt();

    // The corner is cut back to the bevel plane, `d·sin φ` along the bisector.
    let at_corner = sdf([0.0, 0.0, 0.0]);
    assert!(
        (at_corner - d * sin_half).abs() < 0.01,
        "corner clearance {at_corner} vs expected {}",
        d * sin_half,
    );
    // Both offset points — `d` along each face from the edge — sit exactly on
    // the new bevel's rim, so the bevel is EQUAL-offset at this dihedral too.
    for (n, other, label) in [(na, nb, "face A"), (nb, na, "face B")] {
        let dir = {
            let v = [
                -(other[0] - cdot * n[0]),
                -(other[1] - cdot * n[1]),
                -(other[2] - cdot * n[2]),
            ];
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / l, v[1] / l, v[2] / l]
        };
        let p = [d * dir[0], d * dir[1], d * dir[2]];
        assert!(sdf(p).abs() < 0.01, "{label}: offset point is not on the chamfered surface");
        let further = [p[0] + 2.0 * dir[0], p[1] + 2.0 * dir[1], p[2] + 2.0 * dir[2]];
        assert!(sdf(further).abs() < 0.01, "{label}: the face is eaten past the offset point");
    }
}
