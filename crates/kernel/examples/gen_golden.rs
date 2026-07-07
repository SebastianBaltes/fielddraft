//! THROWAWAY: emit golden (opcodes, point, sdf) triples so the C++ port
//! of `eval_opcodes` (cgal-mesher/src/sdf_interp.h) can be cross-checked
//! bit-exactly against the authoritative Rust interpreter.
//!
//! Run: cargo run -p fielddraft-kernel --example gen_golden > /tmp/fd_golden.txt
//! Then: cgal-mesher/test/check_interp (reads /tmp/fd_golden.txt).
//! Delete this file once the port is validated.

use fielddraft_kernel::bvh::BoundedCsg;
use fielddraft_kernel::csg::{CsgNode, DecodedFeature, PrimitiveModifiers};
use fielddraft_kernel::sdf_compiler::{compile_body, eval_opcodes};
use fielddraft_protocol::sketch2d::{Sketch2d, Sketch2dPrimitive, SketchSegment};
use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

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
    let bvh = BoundedCsg::from_csg(&CsgNode::Extrude {
        id: FeatureId::from_u32(1),
        sketch: Box::new(washer),
        half_height: 3.0,
    });
    emit("extrude_washer", &bvh, &pts);

    let rect = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
        center: [8.0, 0.0],
        half_extents: [2.0, 5.0],
    });
    let bvh = BoundedCsg::from_csg(&CsgNode::Revolve {
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
    let bvh = BoundedCsg::from_csg(&CsgNode::Revolve {
        id: FeatureId::from_u32(1),
        sketch: Box::new(path_profile()),
    });
    emit("revolve_path", &bvh, &pts);

    let bvh = BoundedCsg::from_csg(&CsgNode::Extrude {
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
}
