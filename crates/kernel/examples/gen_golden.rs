//! Golden generator: emit (opcodes, point, sdf) triples so the C++ port
//! of `eval_opcodes` (cgal-mesher/src/sdf_interp.h) can be cross-checked
//! against the authoritative Rust interpreter. PERMANENT — the check runs
//! as a mandatory step inside the mesher Docker-image build (interpreter
//! drift fails the build); extend the scene list with every new opcode.
//!
//! Run: cargo run -p fielddraft-kernel --example gen_golden > /tmp/fd_golden.txt
//! Then: cgal-mesher/test/check_interp (reads /tmp/fd_golden.txt).

use fielddraft_kernel::bvh::BoundedCsg;
use fielddraft_kernel::csg::{BlendMode, BranchId, CsgNode, DecodedFeature, OpenSurface, PrimitiveModifiers};
use fielddraft_kernel::sdf_compiler::{compile_body, eval_opcodes};
use fielddraft_protocol::sketch2d::{Sketch2d, Sketch2dPrimitive, SketchSegment};
use fielddraft_protocol::{BoolOp, FeatureId, Primitive, SdfExprOp, Transform};

fn feat_m(
    id: u32,
    op: BoolOp,
    primitive: Primitive,
    t: Transform,
    mods: PrimitiveModifiers,
) -> DecodedFeature {
    DecodedFeature::from_parts(
        FeatureId::from_u32(id),
        format!("F{id}"),
        op,
        primitive,
        t,
        mods,
        vec![],
    )
}

fn feat(id: u32, op: BoolOp, primitive: Primitive, t: Transform) -> DecodedFeature {
    feat_m(id, op, primitive, t, PrimitiveModifiers::NONE)
}

fn xform(translate: [f32; 3], rotation: [f32; 4], scale: [f32; 3]) -> Transform {
    Transform { translate, rotation, scale }
}

// A non-trivial unit quaternion (rotation about a tilted axis) to exercise
// world_to_local with a real rotation.
fn tilt_quat() -> [f32; 4] {
    // axis ~ (1,1,0)/√2, angle 0.7 rad
    let half = 0.35f32;
    let (s, c) = half.sin_cos();
    let inv = 1.0 / 2.0f32.sqrt();
    [s * inv, s * inv, 0.0, c]
}

fn emit(name: &str, bvh: &BoundedCsg, pts: &[[f32; 3]]) {
    let compiled = compile_body(bvh);
    println!("SCENE {name}");
    let a = &compiled.aabb;
    println!(
        "AABB {} {} {} {} {} {}",
        a.min[0], a.min[1], a.min[2], a.max[0], a.max[1], a.max[2]
    );
    println!("NOPS {}", compiled.opcodes.len());
    for op in &compiled.opcodes {
        println!("OP {:08x}", op.to_bits());
    }
    println!("NPTS {}", pts.len());
    for &p in pts {
        let d = eval_opcodes(&compiled.opcodes, p);
        // hex-float-ish: print with full f32 precision via {:e}
        println!("PT {} {} {} {}", p[0], p[1], p[2], d);
    }
    println!("END");
}

fn grid_points() -> Vec<[f32; 3]> {
    let mut v = Vec::new();
    let coords = [-18.0f32, -7.3, -1.1, 0.0, 2.7, 6.5, 13.0, 21.0];
    for &x in &coords {
        for &y in &coords {
            for &z in &coords {
                v.push([x, y, z]);
            }
        }
    }
    v
}

fn main() {
    let id = [0.0, 0.0, 0.0, 1.0];
    let s1 = [1.0, 1.0, 1.0];
    let pts = grid_points();

    // Box, rotated + scaled.
    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[feat(
        1,
        BoolOp::Add,
        Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [6.0, 4.0, 8.0] },
        xform([1.0, -2.0, 0.5], tilt_quat(), [1.0, 1.3, 0.8]),
    )]));
    emit("box_rot_scale", &bvh, &pts);

    // Sphere translated.
    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[feat(
        1,
        BoolOp::Add,
        Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 7.0 },
        xform([3.0, 4.0, -2.0], id, s1),
    )]));
    emit("sphere", &bvh, &pts);

    // Cylinder + torus union, one rotated.
    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[
        feat(
            1,
            BoolOp::Add,
            Primitive::Cylinder { center: [0.0, 0.0, 0.0], radius: 4.0, half_height: 10.0 },
            xform([0.0, 0.0, 0.0], id, s1),
        ),
        feat(
            2,
            BoolOp::Add,
            Primitive::Torus { center: [0.0, 0.0, 0.0], major_radius: 8.0, minor_radius: 2.5 },
            xform([0.0, 0.0, 0.0], tilt_quat(), s1),
        ),
    ]));
    emit("cyl_torus", &bvh, &pts);

    // Box minus sphere (difference) plus a cone (union).
    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[
        feat(
            1,
            BoolOp::Add,
            Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [10.0, 10.0, 10.0] },
            xform([0.0, 0.0, 0.0], id, s1),
        ),
        feat(
            2,
            BoolOp::Remove,
            Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 6.0 },
            xform([6.0, 6.0, 6.0], id, s1),
        ),
        feat(
            3,
            BoolOp::Add,
            Primitive::Cone { r_bottom: 5.0, r_top: 1.0, half_height: 6.0 },
            xform([-14.0, 0.0, 0.0], id, s1),
        ),
    ]));
    emit("box_minus_sphere_cone", &bvh, &pts);

    // Capsule, hexprism, ellipsoid union.
    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[
        feat(
            1,
            BoolOp::Add,
            Primitive::Capsule { radius: 3.0, half_height: 7.0 },
            xform([-8.0, 0.0, 0.0], id, s1),
        ),
        feat(
            2,
            BoolOp::Add,
            Primitive::HexPrism { apothem: 5.0, half_height: 4.0 },
            xform([8.0, 0.0, 0.0], tilt_quat(), s1),
        ),
        feat(
            3,
            BoolOp::Add,
            Primitive::Ellipsoid { radii: [6.0, 3.0, 4.0] },
            xform([0.0, 10.0, 0.0], id, s1),
        ),
    ]));
    emit("capsule_hex_ellipsoid", &bvh, &pts);

    // Intersection of two boxes.
    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[
        feat(
            1,
            BoolOp::Add,
            Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [10.0, 6.0, 10.0] },
            xform([0.0, 0.0, 0.0], id, s1),
        ),
        feat(
            2,
            BoolOp::Intersect,
            Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [6.0, 10.0, 6.0] },
            xform([2.0, 0.0, 0.0], id, s1),
        ),
    ]));
    emit("intersect_boxes", &bvh, &pts);

    // Smooth union (fillet) and smooth difference (chamfer) via blend_radius.
    let mut a = feat(
        1,
        BoolOp::Add,
        Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 6.0 },
        xform([-3.0, 0.0, 0.0], id, s1),
    );
    let mut b = feat(
        2,
        BoolOp::Add,
        Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 6.0 },
        xform([3.0, 0.0, 0.0], id, s1),
    );
    a.blend_radius = 3.0;
    b.blend_radius = 3.0;
    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[a, b]));
    emit("smooth_union", &bvh, &pts);

    // Shell + twist on a box.
    let f = feat_m(
        1,
        BoolOp::Add,
        Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [5.0, 16.0, 5.0] },
        xform([0.0, 0.0, 0.0], id, s1),
        PrimitiveModifiers {
            shell_thickness: 1.0,
            twist_rate: 0.05,
            ..PrimitiveModifiers::NONE
        },
    );
    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[f]));
    emit("shell_twist", &bvh, &pts);

    // Bend + displace on a box.
    let f = feat_m(
        1,
        BoolOp::Add,
        Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [12.0, 4.0, 4.0] },
        xform([0.0, 0.0, 0.0], id, s1),
        PrimitiveModifiers {
            bend_rate: 0.04,
            displace_depth: 1.5,
            displace_freq: 0.3,
            ..PrimitiveModifiers::NONE
        },
    );
    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[f]));
    emit("bend_displace", &bvh, &pts);

    // Extruded washer (difference of circles) + revolved rectangle.
    let outer = Sketch2d::Prim(Sketch2dPrimitive::Circle { center: [0.0, 0.0], radius: 9.0 });
    let inner = Sketch2d::Prim(Sketch2dPrimitive::Circle { center: [0.0, 0.0], radius: 4.0 });
    let washer = Sketch2d::Difference(Box::new(outer), Box::new(inner));
    let bvh = BoundedCsg::from_csg(&CsgNode::Extrude { wall_subs: None,
        id: FeatureId::from_u32(1),
        sketch: Box::new(washer),
        half_height: 3.0,
    });
    emit("extrude_washer", &bvh, &pts);

    let rect = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
        center: [8.0, 0.0],
        half_extents: [2.0, 5.0],
    });
    let bvh = BoundedCsg::from_csg(&CsgNode::Revolve { wall_subs: None,
        id: FeatureId::from_u32(1),
        sketch: Box::new(rect),
    });
    emit("revolve_rect", &bvh, &pts);

    // Gyroid and thread.
    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[feat(
        1,
        BoolOp::Add,
        Primitive::Gyroid { cell_size: 5.0, thickness: 0.4 },
        xform([0.0, 0.0, 0.0], id, s1),
    )]));
    emit("gyroid", &bvh, &pts);

    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[feat(
        1,
        BoolOp::Add,
        Primitive::Thread { nominal_radius: 6.0, pitch: 2.0, depth: 1.0, half_height: 10.0 },
        xform([0.0, 0.0, 0.0], id, s1),
    )]));
    emit("thread", &bvh, &pts);

    // Analytic path profile (line + arc + quad Bézier, closed), revolved
    // AND extruded — exercises every OP_PRIM2D_PATH record kind plus the
    // extrude cap_mask payload. This is the vase-class opcode shape that
    // the C++ port initially lacked (returned FLT_MAX everywhere).
    let path_profile = || {
        Sketch2d::Prim(Sketch2dPrimitive::Path {
            segments: vec![
                SketchSegment::Line { start: [4.0, -8.0], end: [9.0, -8.0] },
                SketchSegment::Arc {
                    center: [9.0, -4.0],
                    radius: 4.0,
                    start_angle: -std::f32::consts::FRAC_PI_2,
                    sweep_angle: std::f32::consts::FRAC_PI_2,
                },
                SketchSegment::Line { start: [13.0, -4.0], end: [13.0, 2.0] },
                SketchSegment::QuadBezier { p0: [13.0, 2.0], p1: [6.0, 5.0], p2: [4.0, 10.0] },
                SketchSegment::Line { start: [4.0, 10.0], end: [4.0, -8.0] },
            ],
            closed: true,
        })
    };
    let bvh = BoundedCsg::from_csg(&CsgNode::Revolve { wall_subs: None,
        id: FeatureId::from_u32(1),
        sketch: Box::new(path_profile()),
    });
    emit("revolve_path", &bvh, &pts);

    let bvh = BoundedCsg::from_csg(&CsgNode::Extrude { wall_subs: None,
        id: FeatureId::from_u32(1),
        sketch: Box::new(path_profile()),
        half_height: 5.0,
    });
    emit("extrude_path", &bvh, &pts);

    // Smooth difference (chamfer mode) to hit OP_SMOOTH_DIFF + chamfer branch.
    let a = feat(
        1,
        BoolOp::Add,
        Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [9.0, 9.0, 9.0] },
        xform([0.0, 0.0, 0.0], id, s1),
    );
    let b = feat_m(
        2,
        BoolOp::Remove,
        Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 7.0 },
        xform([7.0, 0.0, 0.0], id, s1),
        PrimitiveModifiers {
            blend_radius: 2.5,
            chamfer_size: 2.5,
            ..PrimitiveModifiers::NONE
        },
    );
    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[a, b]));
    emit("smooth_diff_chamfer", &bvh, &pts);

    // ── Helix (OP_WARP_HELIX + OP_HELIX_SCALE) ─────────────────────────
    // Circle profile swept helically; a right- and a left-handed variant
    // with taper + start_angle to exercise every parameter slot.
    let helix_profile = || {
        Sketch2d::Prim(Sketch2dPrimitive::Circle { center: [6.0, 0.0], radius: 1.5 })
    };
    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[feat(
        1,
        BoolOp::Add,
        Primitive::Helix {
            sketch: helix_profile(),
            center: [1.0, -2.0, 0.5],
            pitch: 4.0,
            turns: 3.0,
            left_handed: false,
            taper: 0.05,
            start_angle: 0.4,
            wall_subs: None,
        },
        xform([0.0, 0.0, 0.0], id, s1),
    )]));
    emit("helix", &bvh, &pts);

    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[feat(
        1,
        BoolOp::Add,
        Primitive::Helix {
            sketch: helix_profile(),
            center: [0.0, 0.0, 0.0],
            pitch: 3.0,
            turns: 2.5,
            left_handed: true,
            taper: 0.0,
            start_angle: 0.0,
            wall_subs: None,
        },
        xform([2.0, 1.0, -1.0], tilt_quat(), s1),
    )]));
    emit("helix_left_tilted", &bvh, &pts);

    // ── Sweep (OP_WARP_SWEEP + OP_HELIX_SCALE) ─────────────────────────
    // Open swept tube with twist along a 3D polyline (end caps via the
    // w-coordinate), and a closed ring sweep (w = −∞ branch).
    let sweep_profile = || {
        Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [1.5, 0.8],
        })
    };
    let bvh = BoundedCsg::from_csg(&CsgNode::Sweep {
        id: FeatureId::from_u32(1),
        sketch: Box::new(sweep_profile()),
        path: vec![
            [-8.0, -3.0, 0.0],
            [-2.0, 2.0, 1.0],
            [4.0, 1.0, -2.0],
            [9.0, -4.0, 3.0],
        ],
        closed: false,
        twist: 1.2,
    });
    emit("sweep_open_twist", &bvh, &pts);

    let bvh = BoundedCsg::from_csg(&CsgNode::Sweep {
        id: FeatureId::from_u32(1),
        sketch: Box::new(sweep_profile()),
        path: vec![
            [8.0, 0.0, 0.0],
            [0.0, 0.0, 8.0],
            [-8.0, 0.0, 0.0],
            [0.0, 0.0, -8.0],
        ],
        closed: true,
        twist: 0.0,
    });
    emit("sweep_closed_ring", &bvh, &pts);

    // ── Masked blend (OP_MASKED_BLEND, count-prefixed polyline) ────────
    // The C++ port initially read a fixed 2-point capsule — a 3-point
    // polyline (fillet union) and a 1-point sphere mask (chamfer diff)
    // pin the count-prefixed layout.
    let box_node = || {
        CsgNode::from_decoded(&[feat(
            1,
            BoolOp::Add,
            Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [8.0, 6.0, 8.0] },
            xform([0.0, 0.0, 0.0], id, s1),
        )])
    };
    let sphere_node = || {
        CsgNode::from_decoded(&[feat(
            2,
            BoolOp::Add,
            Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 6.0 },
            xform([6.0, 4.0, 0.0], id, s1),
        )])
    };
    let bvh = BoundedCsg::from_csg(&CsgNode::MaskedBlend {
        a: Box::new(box_node()),
        b: Box::new(sphere_node()),
        mask_polyline: vec![[8.0, -2.0, -6.0], [8.0, 6.0, 0.0], [8.0, -2.0, 6.0]],
        mask_radius: 3.0,
        blend_radius: 1.5,
        blend_mode: BlendMode::Fillet,
        is_difference: false,
        provenance: fielddraft_kernel::csg::BlendProvenance::NONE,
    });
    emit("masked_blend_polyline", &bvh, &pts);

    let bvh = BoundedCsg::from_csg(&CsgNode::MaskedBlend {
        a: Box::new(box_node()),
        b: Box::new(sphere_node()),
        mask_polyline: vec![[7.0, 5.0, 0.0]],
        mask_radius: 4.0,
        blend_radius: 2.0,
        blend_mode: BlendMode::Chamfer,
        is_difference: true,
        provenance: fielddraft_kernel::csg::BlendProvenance::NONE,
    });
    emit("masked_blend_point_diff", &bvh, &pts);

    // ── Opened shells (OP_PRIM_HALFSPACE retained faces) ───────────────
    // Box with the +Y face opened → 5 retained plane half-spaces; a
    // cylinder with the top cap opened → cylinder-kind + plane records; a
    // cone with a cap opened → cone-kind record.
    let open = |fid: u32, sub: u32, surface: fielddraft_protocol::HalfSpace| OpenSurface {
        branch: BranchId::from_feature_sub(&FeatureId::from_u32(fid), sub).as_u32(),
        surface,
        placement: Transform::IDENTITY,
    };
    let boxed = CsgNode::from_decoded(&[feat(
        1,
        BoolOp::Add,
        Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [9.0, 7.0, 9.0] },
        xform([0.0, 0.0, 0.0], id, s1),
    )]);
    let shell = CsgNode::Shell(
        Box::new(boxed),
        1.5,
        vec![open(1, 2, fielddraft_protocol::HalfSpace::Plane {
            normal: [0.0, 1.0, 0.0],
            offset: 7.0,
        })],
    );
    let bvh = BoundedCsg::from_csg(&shell);
    emit("shell_open_box", &bvh, &pts);

    let cyl = CsgNode::from_decoded(&[feat(
        1,
        BoolOp::Add,
        Primitive::Cylinder { center: [0.0, 0.0, 0.0], radius: 8.0, half_height: 9.0 },
        xform([0.0, 1.0, 0.0], tilt_quat(), s1),
    )]);
    let shell = CsgNode::Shell(
        Box::new(cyl),
        1.0,
        vec![open(1, 1, fielddraft_protocol::HalfSpace::Plane {
            normal: [0.0, 1.0, 0.0],
            offset: 9.0,
        })],
    );
    let bvh = BoundedCsg::from_csg(&shell);
    emit("shell_open_cylinder", &bvh, &pts);

    let cone = CsgNode::from_decoded(&[feat(
        1,
        BoolOp::Add,
        Primitive::Cone { r_bottom: 9.0, r_top: 3.0, half_height: 8.0 },
        xform([0.0, 0.0, 0.0], id, s1),
    )]);
    let shell = CsgNode::Shell(
        Box::new(cone),
        1.0,
        vec![open(1, 1, fielddraft_protocol::HalfSpace::Plane {
            normal: [0.0, 1.0, 0.0],
            offset: 8.0,
        })],
    );
    let bvh = BoundedCsg::from_csg(&shell);
    emit("shell_open_cone", &bvh, &pts);

    // ── Custom SDF (OP_PRIM_CUSTOM, user expression tape) ──────────────
    // A sin-displaced sphere: sqrt(x²+y²+z²) − 8 + sin(x·0.9)·cos(y·1.1)
    // — exercises leaf pushes, const, mul, add/sub, sqrt, sin/cos, and the
    // max(expr·invL, box) clip; plus a tilted transform.
    use SdfExprOp as E;
    let ops = vec![
        E::X, E::X, E::Mul,
        E::Y, E::Y, E::Mul, E::Add,
        E::Z, E::Z, E::Mul, E::Add,
        E::Sqrt,
        E::Const { v: 8.0 }, E::Sub,
        E::X, E::Const { v: 0.9 }, E::Mul, E::Sin,
        E::Y, E::Const { v: 1.1 }, E::Mul, E::Cos,
        E::Mul, E::Add,
    ];
    let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[feat(
        1,
        BoolOp::Add,
        Primitive::CustomSdf {
            source: "sqrt(x*x+y*y+z*z)-8+sin(x*0.9)*cos(y*1.1)".into(),
            ops,
            lipschitz: 2.5,
            half_extents: [10.0, 10.0, 10.0],
        },
        xform([1.0, -2.0, 0.5], tilt_quat(), s1),
    )]));
    emit("custom_sdf_sinsphere", &bvh, &pts);

    // ── Kanal-Korrekturkörper (OP_ARC_CHANNEL, offset_tree Phase B) ────
    // Kleiner Steinmetz (+δ über Schnitt zweier orthogonaler Zylinder,
    // konvexer Keil-Abzug, gebogene Naht → echte Bögen) und ein
    // Union-Dieder-Notch (−δ, konkaver Sliver-Zusatz, gerade Naht →
    // Segmente). Beide durch die echte Synthese, damit der Strom exakt
    // die Produktions-Payloads trägt.
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let stein = CsgNode::Offset(
        Box::new(CsgNode::Intersection(
            Box::new(CsgNode::Transform(
                xform([0.0; 3], [half, 0.0, 0.0, half], s1),
                Box::new(CsgNode::Leaf {
                    id: FeatureId::from_u32(1),
                    primitive: Primitive::Cylinder {
                        center: [0.0; 3],
                        radius: 9.0,
                        half_height: 18.0,
                    },
                }),
            )),
            Box::new(CsgNode::Transform(
                xform([0.0; 3], [0.0, 0.0, -half, half], s1),
                Box::new(CsgNode::Leaf {
                    id: FeatureId::from_u32(2),
                    primitive: Primitive::Cylinder {
                        center: [0.0; 3],
                        radius: 6.0,
                        half_height: 18.0,
                    },
                }),
            )),
        )),
        1.5,
    );
    let bvh = fielddraft_kernel::offset_channel::synthesize_offset_channels(
        BoundedCsg::from_csg(&stein),
        0.01,
    );
    let (n_ch, _) = fielddraft_kernel::offset_channel::count_channels(&bvh);
    assert!(n_ch > 0, "golden arc_channel_steinmetz: Synthese lieferte keine Kanäle");
    emit("arc_channel_steinmetz", &bvh, &pts);

    let notch = CsgNode::Offset(
        Box::new(CsgNode::Union(
            Box::new(CsgNode::Transform(
                xform([0.0, -10.0, 0.0], id, s1),
                Box::new(CsgNode::Leaf {
                    id: FeatureId::from_u32(1),
                    primitive: Primitive::Box {
                        center: [0.0; 3],
                        half_extents: [20.0, 10.0, 12.0],
                    },
                }),
            )),
            Box::new(CsgNode::Transform(
                xform([10.0, 0.0, 0.0], [0.0, 0.0, half, half], s1),
                Box::new(CsgNode::Leaf {
                    id: FeatureId::from_u32(2),
                    primitive: Primitive::Box {
                        center: [0.0; 3],
                        half_extents: [20.0, 10.0, 12.0],
                    },
                }),
            )),
        )),
        -1.5,
    );
    let bvh = fielddraft_kernel::offset_channel::synthesize_offset_channels(
        BoundedCsg::from_csg(&notch),
        0.01,
    );
    let (n_ch, _) = fielddraft_kernel::offset_channel::count_channels(&bvh);
    assert!(n_ch > 0, "golden arc_channel_notch: Synthese lieferte keine Kanäle");
    emit("arc_channel_notch", &bvh, &pts);

    // ── Vertex-Kugel (OP_VERTEX_SPHERE, offset_tree Phase C4) ────
    // Konkave Raum-Ecke aus drei orthogonalen Platten, erodiert: die
    // Triple-Junctions synthetisieren Vertex-Kugeln (Rossignac–Requicha),
    // durch die echte Synthese, damit der Strom die Produktions-Payloads
    // trägt.
    let plate = |fid: u32, c: [f32; 3], h: [f32; 3]| {
        CsgNode::Transform(
            xform(c, id, s1),
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(fid),
                primitive: Primitive::Box { center: [0.0; 3], half_extents: h },
            }),
        )
    };
    let corner = CsgNode::Offset(
        Box::new(CsgNode::Union(
            Box::new(CsgNode::Union(
                Box::new(plate(1, [0.0, -5.0, 0.0], [20.0, 5.0, 20.0])),
                Box::new(plate(2, [-5.0, 0.0, 0.0], [5.0, 20.0, 20.0])),
            )),
            Box::new(plate(3, [0.0, 0.0, -5.0], [20.0, 20.0, 5.0])),
        )),
        -1.5,
    );
    let bvh = fielddraft_kernel::offset_channel::synthesize_offset_channels(
        BoundedCsg::from_csg(&corner),
        0.01,
    );
    let n_v = fielddraft_kernel::offset_channel::count_vertex_spheres(&bvh);
    assert!(n_v > 0, "golden vertex_sphere_corner: Synthese lieferte keine Vertex-Kugeln");
    emit("vertex_sphere_corner", &bvh, &pts);

    // #395 Kontakt-Prisma: zwei bündig gestapelte Boxen mit UNGLEICHEM
    // Querschnitt unter Erosion. Die Synthese hängt ein Kontakt-Prisma
    // (reguläres Extrude, KEIN neuer Opcode) als Union-Member VOR das −δ, das
    // den submergierten Phantom-Schlitz füllt. Der CGAL-C++-Interpreter rechnet
    // dieses Extrude 1:1 mit — diese Golden-Szene verankert die Parität.
    let stack = CsgNode::Offset(
        Box::new(CsgNode::Union(
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(1),
                primitive: Primitive::Box { center: [0.0; 3], half_extents: [12.0, 12.0, 12.0] },
            }),
            Box::new(CsgNode::Transform(
                xform([24.0, 3.0, 2.0], id, s1),
                Box::new(CsgNode::Leaf {
                    id: FeatureId::from_u32(2),
                    primitive: Primitive::Box { center: [0.0; 3], half_extents: [12.0, 9.0, 8.0] },
                }),
            )),
        )),
        -2.0,
    );
    let bvh = fielddraft_kernel::offset_channel::synthesize_offset_channels(
        BoundedCsg::from_csg(&stack),
        0.01,
    );
    emit("contact_prism_stack", &bvh, &pts);

    // #395 Gleicher Querschnitt: das Prisma IST die Union (S = U), die Erosion
    // ist metrisch exakt (eine geschrumpfte Doppel-Box).
    let equal = CsgNode::Offset(
        Box::new(CsgNode::Union(
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(1),
                primitive: Primitive::Box { center: [0.0; 3], half_extents: [12.0, 12.0, 12.0] },
            }),
            Box::new(CsgNode::Transform(
                xform([24.0, 0.0, 0.0], id, s1),
                Box::new(CsgNode::Leaf {
                    id: FeatureId::from_u32(2),
                    primitive: Primitive::Box { center: [0.0; 3], half_extents: [12.0, 12.0, 12.0] },
                }),
            )),
        )),
        -2.0,
    );
    let bvh = fielddraft_kernel::offset_channel::synthesize_offset_channels(
        BoundedCsg::from_csg(&equal),
        0.01,
    );
    emit("contact_prism_equal", &bvh, &pts);
}
