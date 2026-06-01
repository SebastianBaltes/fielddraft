//! SDF compiler: [`BoundedCsg`] → flat opcode buffer for GPU raymarching.
//!
//! The compiler traverses the BVH-wrapped CSG tree in postfix order and
//! emits a compact `Vec<f32>` that a GPU stack machine can interpret.
//! Each body gets its own opcode buffer.  2D sketch primitives (for
//! Extrude/Revolve) are inlined directly into the 3D stream — the
//! shader evaluates them at `(point.x, point.y)` in the local frame
//! established by a preceding `WarpTransform`.
//!
//! A CPU interpreter ([`eval_opcodes`]) mirrors the GPU logic 1:1 so
//! that every opcode path can be tested without a GPU.

use fielddraft_protocol::{BakedVolume, Primitive, Transform};

use crate::bvh::BoundedCsg;
use crate::csg::{Aabb, BlendMode};
use crate::sketch2d::{BoundedSketch2d, Sketch2dPrimitive, SketchSegment};
use crate::quat_rotate;

// ── Opcode tags ────────────────────────────────────────────────────
//
// Encoded as `f32::from_bits(tag)` in the buffer.  The GPU shader
// reads the f32, does `bitcast<u32>`, and dispatches on the tag.

// 3D primitives — push SDF(point) onto value stack.
// Transform (translate 3 + quat 4 + scale 3 = 10 floats) is embedded.
pub const OP_PRIM_BOX: u32       = 0x01; // +11: transform(8) + half_extents(3)
pub const OP_PRIM_SPHERE: u32    = 0x02; // +9:  transform(8) + radius(1)
pub const OP_PRIM_CYLINDER: u32  = 0x03; // +10: transform(8) + radius(1) + half_h(1)
pub const OP_PRIM_TORUS: u32     = 0x04; // +10: transform(8) + major_r(1) + minor_r(1)
pub const OP_PRIM_CONE: u32      = 0x05; // +11: transform(8) + r_bottom(1) + r_top(1) + half_h(1)
pub const OP_PRIM_CAPSULE: u32   = 0x06; // +10: transform(8) + radius(1) + half_h(1)
pub const OP_PRIM_HEX_PRISM: u32 = 0x07; // +10: transform(8) + apothem(1) + half_h(1)
pub const OP_PRIM_ELLIPSOID: u32 = 0x08; // +11: transform(8) + radii(3)
pub const OP_PRIM_CONST: u32     = 0x09; // +1:  constant distance
pub const OP_PRIM_GYROID: u32    = 0x0A; // +10: transform(8) + cell_size(1) + thickness(1)
pub const OP_PRIM_THREAD: u32    = 0x0B; // +12: transform(8) + nom_r + pitch + depth + half_h
pub const OP_PRIM_TEX3D: u32     = 0x0C; // +15: tex_idx(1) + transform(8) + aabb_min(3) + aabb_max(3)

// 2D primitives — evaluate at (point.x, point.y), push onto stack.
pub const OP_PRIM2D_CIRCLE: u32  = 0x10; // +3: center(2) + radius(1)
pub const OP_PRIM2D_RECT: u32    = 0x11; // +4: center(2) + half_extents(2)
pub const OP_PRIM2D_POLY: u32    = 0x12; // +1+2N: vertex_count(1) + vertices(2*N)
pub const OP_PRIM2D_SECTOR: u32  = 0x13; // +5: center(2) + radius(1) + start(1) + sweep(1)

// Binary operators — pop 2, push 1.
pub const OP_UNION: u32          = 0x20; // +0
pub const OP_DIFFERENCE: u32     = 0x21; // +0
pub const OP_INTERSECT: u32      = 0x22; // +0
pub const OP_SMOOTH_UNION: u32   = 0x23; // +2: radius(1) + mode(1)
pub const OP_SMOOTH_DIFF: u32    = 0x24; // +2: radius(1) + mode(1)

// Unary / compositor operators.
pub const OP_SHELL: u32          = 0x29; // +1: thickness
pub const OP_MORPH: u32          = 0x2A; // +1: t — pops 2, pushes lerp
pub const OP_DISPLACE_VAL: u32   = 0x2B; // +2: depth + freq — pops 1, uses point
pub const OP_EXTRUDE: u32        = 0x2C; // +2: half_height + scale — pops d2d, uses point.z
pub const OP_REVOLVE_SCALE: u32  = 0x2D; // +1: scale — pops d2d, multiplies by scale
pub const OP_OFFSET_2D: u32      = 0x2E; // +1: distance — pops 1, pushes d - distance

// Domain warps — modify the point register.
pub const OP_WARP_PUSH: u32      = 0x30; // +0: save point to point stack
pub const OP_WARP_POP: u32       = 0x31; // +0: restore point from point stack
pub const OP_WARP_TWIST: u32     = 0x32; // +1: rate — rotate XZ by rate*y
pub const OP_WARP_BEND: u32      = 0x33; // +1: rate — rotate XY by rate*x
pub const OP_WARP_TRANSFORM: u32 = 0x34; // +10: translate(3) + quat(4) + scale(3)
pub const OP_WARP_REVOLVE: u32   = 0x35; // +0: point ← (√(x²+z²), y, 0)

// Masked blend — pops 2 (da, db), evaluates capsule mask, pushes 1.
pub const OP_MASKED_BLEND: u32   = 0x40; // +10: p0(3)+p1(3)+mask_r+blend_r+mode+is_diff

pub const OP_END: u32            = 0xFF;

// ── Compiled output ────────────────────────────────────────────────

/// Flat opcode buffer compiled from a single body's CSG tree.
#[derive(Debug, Clone)]
pub struct CompiledSdf {
    pub opcodes: Vec<f32>,
    pub aabb: Aabb,
    pub volumes: Vec<BakedVolume>,
}

impl CompiledSdf {
    pub fn opcode_count(&self) -> usize {
        self.opcodes.len()
    }
}

// ── Compiler ───────────────────────────────────────────────────────

struct CompileCtx {
    opcodes: Vec<f32>,
    volumes: Vec<BakedVolume>,
}

const MESH_VOLUME_RESOLUTION: u32 = 64;

/// Compile a body's `BoundedCsg` tree into a flat opcode buffer.
pub fn compile_body(node: &BoundedCsg) -> CompiledSdf {
    let mut ctx = CompileCtx {
        opcodes: Vec::with_capacity(256),
        volumes: Vec::new(),
    };
    emit_node(node, &mut ctx);
    ctx.opcodes.push(f32::from_bits(OP_END));
    CompiledSdf {
        opcodes: ctx.opcodes,
        aabb: node.aabb(),
        volumes: ctx.volumes,
    }
}

fn op(tag: u32) -> f32 {
    f32::from_bits(tag)
}

fn blend_mode_f32(m: BlendMode) -> f32 {
    match m {
        BlendMode::Fillet => 0.0,
        BlendMode::Chamfer => 1.0,
    }
}

fn emit_node(node: &BoundedCsg, ctx: &mut CompileCtx) {
    let out = &mut ctx.opcodes;
    match node {
        BoundedCsg::Empty => {
            out.push(op(OP_PRIM_CONST));
            out.push(f32::MAX);
        }
        BoundedCsg::Leaf {
            primitive,
            world_transform,
            ..
        } => {
            emit_leaf_primitive(primitive, world_transform, ctx);
        }
        BoundedCsg::Union { left, right, .. } => {
            emit_node(left, ctx);
            emit_node(right, ctx);
            ctx.opcodes.push(op(OP_UNION));
        }
        BoundedCsg::Difference { left, right, .. } => {
            emit_node(left, ctx);
            emit_node(right, ctx);
            ctx.opcodes.push(op(OP_DIFFERENCE));
        }
        BoundedCsg::Intersection { left, right, .. } => {
            emit_node(left, ctx);
            emit_node(right, ctx);
            ctx.opcodes.push(op(OP_INTERSECT));
        }
        BoundedCsg::SmoothUnion {
            left,
            right,
            radius,
            mode,
            ..
        } => {
            emit_node(left, ctx);
            emit_node(right, ctx);
            let out = &mut ctx.opcodes;
            out.push(op(OP_SMOOTH_UNION));
            out.push(*radius);
            out.push(blend_mode_f32(*mode));
        }
        BoundedCsg::SmoothDifference {
            left,
            right,
            radius,
            mode,
            ..
        } => {
            emit_node(left, ctx);
            emit_node(right, ctx);
            let out = &mut ctx.opcodes;
            out.push(op(OP_SMOOTH_DIFF));
            out.push(*radius);
            out.push(blend_mode_f32(*mode));
        }
        BoundedCsg::Twist { inner, rate, .. } => {
            out.push(op(OP_WARP_PUSH));
            out.push(op(OP_WARP_TWIST));
            out.push(*rate);
            emit_node(inner, ctx);
            ctx.opcodes.push(op(OP_WARP_POP));
        }
        BoundedCsg::Bend { inner, rate, .. } => {
            out.push(op(OP_WARP_PUSH));
            out.push(op(OP_WARP_BEND));
            out.push(*rate);
            emit_node(inner, ctx);
            ctx.opcodes.push(op(OP_WARP_POP));
        }
        BoundedCsg::Displace {
            inner, depth, freq, ..
        } => {
            emit_node(inner, ctx);
            let out = &mut ctx.opcodes;
            out.push(op(OP_DISPLACE_VAL));
            out.push(*depth);
            out.push(*freq);
        }
        BoundedCsg::Shell {
            inner, thickness, ..
        } => {
            emit_node(inner, ctx);
            let out = &mut ctx.opcodes;
            out.push(op(OP_SHELL));
            out.push(*thickness);
        }
        BoundedCsg::Morph {
            left, right, t, ..
        } => {
            emit_node(left, ctx);
            emit_node(right, ctx);
            let out = &mut ctx.opcodes;
            out.push(op(OP_MORPH));
            out.push(*t);
        }
        BoundedCsg::MaskedBlend {
            left,
            right,
            mask_p0,
            mask_p1,
            mask_radius,
            blend_radius,
            blend_mode,
            is_difference,
            ..
        } => {
            emit_node(left, ctx);
            emit_node(right, ctx);
            let out = &mut ctx.opcodes;
            out.push(op(OP_MASKED_BLEND));
            out.extend_from_slice(mask_p0);
            out.extend_from_slice(mask_p1);
            out.push(*mask_radius);
            out.push(*blend_radius);
            out.push(blend_mode_f32(*blend_mode));
            out.push(if *is_difference { 1.0 } else { 0.0 });
        }
        BoundedCsg::Extrude {
            sketch,
            half_height,
            world_transform,
            ..
        } => {
            out.push(op(OP_WARP_PUSH));
            emit_warp_transform(world_transform, out);
            emit_sketch(sketch, out);
            out.push(op(OP_EXTRUDE));
            out.push(*half_height);
            out.push(world_transform.min_scale());
            out.push(op(OP_WARP_POP));
        }
        BoundedCsg::Revolve {
            sketch,
            world_transform,
            ..
        } => {
            out.push(op(OP_WARP_PUSH));
            emit_warp_transform(world_transform, out);
            out.push(op(OP_WARP_REVOLVE));
            emit_sketch(sketch, out);
            out.push(op(OP_REVOLVE_SCALE));
            out.push(world_transform.min_scale());
            out.push(op(OP_WARP_POP));
        }
    }
}

// ── Transform helpers ──────────────────────────────────────────────

fn emit_warp_transform(t: &Transform, out: &mut Vec<f32>) {
    out.push(op(OP_WARP_TRANSFORM));
    out.extend_from_slice(&t.translate);
    out.extend_from_slice(&t.rotation);
    out.extend_from_slice(&t.scale);
}

fn emit_transform_with_center(t: &Transform, center: &[f32; 3], out: &mut Vec<f32>) {
    let center_scaled = [
        center[0] * t.scale[0],
        center[1] * t.scale[1],
        center[2] * t.scale[2],
    ];
    let center_world = quat_rotate(t.rotation, center_scaled);
    out.push(t.translate[0] + center_world[0]);
    out.push(t.translate[1] + center_world[1]);
    out.push(t.translate[2] + center_world[2]);
    out.extend_from_slice(&t.rotation);
    out.extend_from_slice(&t.scale);
}

fn emit_transform(t: &Transform, out: &mut Vec<f32>) {
    out.extend_from_slice(&t.translate);
    out.extend_from_slice(&t.rotation);
    out.extend_from_slice(&t.scale);
}

// ── Leaf primitives ────────────────────────────────────────────────

fn emit_leaf_primitive(prim: &Primitive, t: &Transform, ctx: &mut CompileCtx) {
    let out = &mut ctx.opcodes;
    match prim {
        Primitive::Box {
            center,
            half_extents,
        } => {
            out.push(op(OP_PRIM_BOX));
            emit_transform_with_center(t, center, out);
            out.extend_from_slice(half_extents);
        }
        Primitive::Sphere { center, radius } => {
            out.push(op(OP_PRIM_SPHERE));
            emit_transform_with_center(t, center, out);
            out.push(*radius);
        }
        Primitive::Cylinder {
            center,
            radius,
            half_height,
        } => {
            out.push(op(OP_PRIM_CYLINDER));
            emit_transform_with_center(t, center, out);
            out.push(*radius);
            out.push(*half_height);
        }
        Primitive::Torus {
            center,
            major_radius,
            minor_radius,
        } => {
            out.push(op(OP_PRIM_TORUS));
            emit_transform_with_center(t, center, out);
            out.push(*major_radius);
            out.push(*minor_radius);
        }
        Primitive::Cone {
            r_bottom,
            r_top,
            half_height,
        } => {
            out.push(op(OP_PRIM_CONE));
            emit_transform(t, out);
            out.push(*r_bottom);
            out.push(*r_top);
            out.push(*half_height);
        }
        Primitive::Capsule {
            radius,
            half_height,
        } => {
            out.push(op(OP_PRIM_CAPSULE));
            emit_transform(t, out);
            out.push(*radius);
            out.push(*half_height);
        }
        Primitive::HexPrism {
            apothem,
            half_height,
        } => {
            out.push(op(OP_PRIM_HEX_PRISM));
            emit_transform(t, out);
            out.push(*apothem);
            out.push(*half_height);
        }
        Primitive::Ellipsoid { radii } => {
            out.push(op(OP_PRIM_ELLIPSOID));
            emit_transform(t, out);
            out.extend_from_slice(radii);
        }
        Primitive::Gyroid {
            cell_size,
            thickness,
        } => {
            out.push(op(OP_PRIM_GYROID));
            emit_transform(t, out);
            out.push(*cell_size);
            out.push(*thickness);
        }
        Primitive::Thread {
            nominal_radius,
            pitch,
            depth,
            half_height,
        } => {
            out.push(op(OP_PRIM_THREAD));
            emit_transform(t, out);
            out.push(*nominal_radius);
            out.push(*pitch);
            out.push(*depth);
            out.push(*half_height);
        }
        Primitive::Extrude { .. } | Primitive::Revolve { .. } => {
            out.push(op(OP_PRIM_CONST));
            out.push(0.0);
        }
        Primitive::ImportedMesh { mesh } => {
            let tex_idx = ctx.volumes.len() as f32;
            let volume = mesh.bake_volume(MESH_VOLUME_RESOLUTION);
            out.push(op(OP_PRIM_TEX3D));
            out.push(tex_idx);
            emit_transform(t, out);
            out.extend_from_slice(&volume.aabb_min);
            out.extend_from_slice(&volume.aabb_max);
            ctx.volumes.push(volume);
        }
    }
}

// ── 2D sketch inlining ────────────────────────────────────────────

fn emit_sketch(sketch: &BoundedSketch2d, out: &mut Vec<f32>) {
    match sketch {
        BoundedSketch2d::Empty => {
            out.push(op(OP_PRIM_CONST));
            out.push(f32::MAX);
        }
        BoundedSketch2d::Prim { primitive, .. } => {
            emit_sketch_primitive(primitive, out);
        }
        BoundedSketch2d::Union { left, right, .. } => {
            emit_sketch(left, out);
            emit_sketch(right, out);
            out.push(op(OP_UNION));
        }
        BoundedSketch2d::Difference { left, right, .. } => {
            emit_sketch(left, out);
            emit_sketch(right, out);
            out.push(op(OP_DIFFERENCE));
        }
        BoundedSketch2d::Intersection { left, right, .. } => {
            emit_sketch(left, out);
            emit_sketch(right, out);
            out.push(op(OP_INTERSECT));
        }
        BoundedSketch2d::Offset {
            inner, distance, ..
        } => {
            emit_sketch(inner, out);
            out.push(op(OP_OFFSET_2D));
            out.push(*distance);
        }
    }
}

fn emit_sketch_primitive(prim: &Sketch2dPrimitive, out: &mut Vec<f32>) {
    match prim {
        Sketch2dPrimitive::Circle { center, radius } => {
            out.push(op(OP_PRIM2D_CIRCLE));
            out.extend_from_slice(center);
            out.push(*radius);
        }
        Sketch2dPrimitive::Rectangle {
            center,
            half_extents,
        } => {
            out.push(op(OP_PRIM2D_RECT));
            out.extend_from_slice(center);
            out.extend_from_slice(half_extents);
        }
        Sketch2dPrimitive::Polygon { vertices } => {
            out.push(op(OP_PRIM2D_POLY));
            out.push(vertices.len() as f32);
            for v in vertices {
                out.extend_from_slice(v);
            }
        }
        Sketch2dPrimitive::Sector {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => {
            out.push(op(OP_PRIM2D_SECTOR));
            out.extend_from_slice(center);
            out.push(*radius);
            out.push(*start_angle);
            out.push(*sweep_angle);
        }
        Sketch2dPrimitive::Path { segments, .. } => {
            // DL-46 / DL-45: the GPU opcode stream stays curve-free.
            // A Path is flattened into a polyline and emitted as
            // OP_PRIM2D_POLY — the analytic segments are CPU/B-Rep
            // only. The even-odd polygon SDF then signs the outline.
            let verts = flatten_path_to_polyline(segments);
            out.push(op(OP_PRIM2D_POLY));
            out.push(verts.len() as f32);
            for v in &verts {
                out.extend_from_slice(v);
            }
        }
    }
}

/// Flatten a [`SketchSegment`] chain into a closed polyline (no
/// duplicated shared vertices) for the curve-free GPU opcode path.
/// Lines contribute their start vertex; curves are sampled at
/// [`PATH_FLATTEN_SAMPLES`] steps.
fn flatten_path_to_polyline(segments: &[SketchSegment]) -> Vec<[f32; 2]> {
    use fielddraft_protocol::sketch2d::PATH_FLATTEN_SAMPLES;
    let mut verts = Vec::new();
    for seg in segments {
        let steps = match seg {
            SketchSegment::Line { .. } => 1,
            _ => PATH_FLATTEN_SAMPLES,
        };
        for k in 0..steps {
            let t = k as f32 / steps as f32;
            verts.push(seg.point_at(t));
        }
    }
    verts
}

// ── CPU interpreter (test mirror of the GPU shader) ────────────────

const VSTACK_SIZE: usize = 8;
const PSTACK_SIZE: usize = 4;

/// Evaluate the compiled opcode buffer at a world-space point.
/// This is a 1:1 mirror of the WGSL stack machine and exists
/// solely so that every opcode path can be tested without a GPU.
pub fn eval_opcodes(opcodes: &[f32], world_point: [f32; 3]) -> f32 {
    eval_opcodes_with_volumes(opcodes, world_point, &[])
}

pub fn eval_opcodes_with_volumes(
    opcodes: &[f32],
    world_point: [f32; 3],
    volumes: &[BakedVolume],
) -> f32 {
    let mut vs = [0.0f32; VSTACK_SIZE];
    let mut vsp: usize = 0;
    let mut ps = [[0.0f32; 3]; PSTACK_SIZE];
    let mut psp: usize = 0;
    let mut pt = world_point;
    let mut pc: usize = 0;

    macro_rules! push_v {
        ($v:expr) => {{
            vs[vsp] = $v;
            vsp += 1;
        }};
    }
    macro_rules! pop_v {
        () => {{
            vsp -= 1;
            vs[vsp]
        }};
    }
    macro_rules! rd {
        () => {{
            let v = opcodes[pc];
            pc += 1;
            v
        }};
    }
    macro_rules! rd3 {
        () => {{
            let a = opcodes[pc];
            let b = opcodes[pc + 1];
            let c = opcodes[pc + 2];
            pc += 3;
            [a, b, c]
        }};
    }

    while pc < opcodes.len() {
        let tag = opcodes[pc].to_bits();
        pc += 1;

        match tag {
            // ── 3D primitives ──────────────────────────────────
            OP_PRIM_BOX => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let he = rd3!();
                push_v!(sdf_box_local(local, he) * scale);
            }
            OP_PRIM_SPHERE => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let r = rd!();
                push_v!(sdf_sphere_local(local, r) * scale);
            }
            OP_PRIM_CYLINDER => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let r = rd!();
                let hh = rd!();
                push_v!(sdf_cylinder_local(local, r, hh) * scale);
            }
            OP_PRIM_TORUS => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let major = rd!();
                let minor = rd!();
                push_v!(sdf_torus_local(local, major, minor) * scale);
            }
            OP_PRIM_CONE => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let rb = rd!();
                let rt = rd!();
                let hh = rd!();
                push_v!(sdf_cone_local(local, rb, rt, hh) * scale);
            }
            OP_PRIM_CAPSULE => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let r = rd!();
                let hh = rd!();
                push_v!(sdf_capsule_local(local, r, hh) * scale);
            }
            OP_PRIM_HEX_PRISM => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let ap = rd!();
                let hh = rd!();
                push_v!(sdf_hex_prism_local(local, ap, hh) * scale);
            }
            OP_PRIM_ELLIPSOID => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let radii = rd3!();
                push_v!(sdf_ellipsoid_local(local, radii) * scale);
            }
            OP_PRIM_CONST => {
                let v = rd!();
                push_v!(v);
            }
            OP_PRIM_GYROID => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let cs = rd!();
                let th = rd!();
                push_v!(sdf_gyroid_local(local, cs, th) * scale);
            }
            OP_PRIM_THREAD => {
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let nr = rd!();
                let pitch = rd!();
                let depth = rd!();
                let hh = rd!();
                push_v!(sdf_thread_local(local, nr, pitch, depth, hh) * scale);
            }

            // ── 2D primitives ──────────────────────────────────
            OP_PRIM2D_CIRCLE => {
                let cx = rd!();
                let cy = rd!();
                let r = rd!();
                let dx = pt[0] - cx;
                let dy = pt[1] - cy;
                push_v!((dx * dx + dy * dy).sqrt() - r);
            }
            OP_PRIM2D_RECT => {
                let cx = rd!();
                let cy = rd!();
                let hx = rd!();
                let hy = rd!();
                let qx = (pt[0] - cx).abs() - hx;
                let qy = (pt[1] - cy).abs() - hy;
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                let inside = qx.max(qy).min(0.0);
                push_v!(outside + inside);
            }
            OP_PRIM2D_POLY => {
                let n = rd!() as usize;
                let mut verts = Vec::with_capacity(n);
                for _ in 0..n {
                    let vx = rd!();
                    let vy = rd!();
                    verts.push([vx, vy]);
                }
                push_v!(sdf_polygon_2d([pt[0], pt[1]], &verts));
            }
            OP_PRIM2D_SECTOR => {
                let cx = rd!();
                let cy = rd!();
                let r = rd!();
                let start = rd!();
                let sweep = rd!();
                push_v!(sdf_sector_2d([pt[0], pt[1]], [cx, cy], r, start, sweep));
            }

            // ── Binary operators ───────────────────────────────
            OP_UNION => {
                let b = pop_v!();
                let a = pop_v!();
                push_v!(a.min(b));
            }
            OP_DIFFERENCE => {
                let b = pop_v!();
                let a = pop_v!();
                push_v!(a.max(-b));
            }
            OP_INTERSECT => {
                let b = pop_v!();
                let a = pop_v!();
                push_v!(a.max(b));
            }
            OP_SMOOTH_UNION => {
                let r = rd!();
                let mode = rd!();
                let b = pop_v!();
                let a = pop_v!();
                push_v!(blend_min_eval(a, b, r, mode));
            }
            OP_SMOOTH_DIFF => {
                let r = rd!();
                let mode = rd!();
                let b = pop_v!();
                let a = pop_v!();
                push_v!(-blend_min_eval(-a, b, r, mode));
            }

            // ── Unary / compositor operators ───────────────────
            OP_SHELL => {
                let thickness = rd!();
                let d = pop_v!();
                push_v!(d.abs() - thickness);
            }
            OP_MORPH => {
                let t = rd!();
                let b = pop_v!();
                let a = pop_v!();
                push_v!((1.0 - t) * a + t * b);
            }
            OP_DISPLACE_VAL => {
                let depth = rd!();
                let freq = rd!();
                let d = pop_v!();
                let disp = depth
                    * (pt[0] * freq).sin()
                    * (pt[1] * freq * 1.3).sin()
                    * (pt[2] * freq * 0.7).sin();
                push_v!(d - disp);
            }
            OP_EXTRUDE => {
                let half_h = rd!();
                let scale = rd!();
                let d2d = pop_v!();
                let wz = pt[2].abs() - half_h;
                let outside = (d2d.max(0.0).powi(2) + wz.max(0.0).powi(2)).sqrt();
                let inside = d2d.max(wz).min(0.0);
                push_v!((outside + inside) * scale);
            }
            OP_REVOLVE_SCALE => {
                let scale = rd!();
                let d = pop_v!();
                push_v!(d * scale);
            }
            OP_OFFSET_2D => {
                let dist = rd!();
                let d = pop_v!();
                push_v!(d - dist);
            }

            // ── Domain warps ───────────────────────────────────
            OP_WARP_PUSH => {
                ps[psp] = pt;
                psp += 1;
            }
            OP_WARP_POP => {
                psp -= 1;
                pt = ps[psp];
            }
            OP_WARP_TWIST => {
                let rate = rd!();
                let angle = rate * pt[1];
                let (s, c) = angle.sin_cos();
                let px = pt[0];
                pt[0] = c * px - s * pt[2];
                pt[2] = s * px + c * pt[2];
            }
            OP_WARP_BEND => {
                let rate = rd!();
                let angle = rate * pt[0];
                let (s, c) = angle.sin_cos();
                let px = pt[0];
                pt[0] = c * px - s * pt[1];
                pt[1] = s * px + c * pt[1];
            }
            OP_WARP_TRANSFORM => {
                let translate = rd3!();
                let quat = [rd!(), rd!(), rd!(), rd!()];
                let scale = [rd!(), rd!(), rd!()];
                pt = world_to_local_inline(pt, translate, quat, scale);
            }
            OP_WARP_REVOLVE => {
                let r = (pt[0] * pt[0] + pt[2] * pt[2]).sqrt();
                pt = [r, pt[1], 0.0];
            }

            // ── Masked blend ───────────────────────────────────
            OP_MASKED_BLEND => {
                let p0 = rd3!();
                let p1 = rd3!();
                let mask_r = rd!();
                let blend_r = rd!();
                let mode = rd!();
                let is_diff = rd!() > 0.5;
                let db = pop_v!();
                let da = pop_v!();
                let mask_d = sdf_capsule_3d(pt, p0, p1, mask_r);
                let result = if mask_d < 0.0 {
                    if is_diff {
                        -blend_min_eval(-da, db, blend_r, mode)
                    } else {
                        blend_min_eval(da, db, blend_r, mode)
                    }
                } else {
                    crate::csg::hard_combine(is_diff, da, db)
                };
                push_v!(result);
            }

            OP_PRIM_TEX3D => {
                let tex_idx = rd!() as usize;
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let aabb_min = rd3!();
                let aabb_max = rd3!();
                if let Some(vol) = volumes.get(tex_idx) {
                    push_v!(vol.sample_trilinear(local) * scale);
                } else {
                    push_v!(f32::MAX);
                }
                let _ = (aabb_min, aabb_max);
            }

            OP_END => break,
            _ => break,
        }
    }

    if vsp > 0 {
        vs[vsp - 1]
    } else {
        f32::MAX
    }
}

// ── Inline SDF helpers (mirror what the WGSL shader does) ──────────

fn read_transform_and_localise(
    buf: &[f32],
    pc: &mut usize,
    pt: [f32; 3],
) -> ([f32; 3], f32) {
    let translate = [buf[*pc], buf[*pc + 1], buf[*pc + 2]];
    let quat = [buf[*pc + 3], buf[*pc + 4], buf[*pc + 5], buf[*pc + 6]];
    let scale = [buf[*pc + 7], buf[*pc + 8], buf[*pc + 9]];
    *pc += 10;
    let local = world_to_local_inline(pt, translate, quat, scale);
    let min_s = scale[0].min(scale[1]).min(scale[2]);
    (local, min_s)
}

fn world_to_local_inline(
    p: [f32; 3],
    translate: [f32; 3],
    quat: [f32; 4],
    scale: [f32; 3],
) -> [f32; 3] {
    let centered = [
        p[0] - translate[0],
        p[1] - translate[1],
        p[2] - translate[2],
    ];
    let qx = -quat[0];
    let qy = -quat[1];
    let qz = -quat[2];
    let qw = quat[3];
    let t = [
        2.0 * (qy * centered[2] - qz * centered[1]),
        2.0 * (qz * centered[0] - qx * centered[2]),
        2.0 * (qx * centered[1] - qy * centered[0]),
    ];
    let rotated = [
        centered[0] + qw * t[0] + qy * t[2] - qz * t[1],
        centered[1] + qw * t[1] + qz * t[0] - qx * t[2],
        centered[2] + qw * t[2] + qx * t[1] - qy * t[0],
    ];
    [rotated[0] / scale[0], rotated[1] / scale[1], rotated[2] / scale[2]]
}

fn sdf_box_local(p: [f32; 3], half: [f32; 3]) -> f32 {
    let q = [
        p[0].abs() - half[0],
        p[1].abs() - half[1],
        p[2].abs() - half[2],
    ];
    let outside = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2) + q[2].max(0.0).powi(2)).sqrt();
    let inside = q[0].max(q[1]).max(q[2]).min(0.0);
    outside + inside
}

fn sdf_sphere_local(p: [f32; 3], r: f32) -> f32 {
    (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - r
}

fn sdf_cylinder_local(p: [f32; 3], r: f32, hh: f32) -> f32 {
    let radial = (p[0] * p[0] + p[2] * p[2]).sqrt() - r;
    let axial = p[1].abs() - hh;
    let outside = radial.max(0.0).hypot(axial.max(0.0));
    let inside = radial.max(axial).min(0.0);
    outside + inside
}

fn sdf_torus_local(p: [f32; 3], major: f32, minor: f32) -> f32 {
    let q0 = (p[0] * p[0] + p[2] * p[2]).sqrt() - major;
    (q0 * q0 + p[1] * p[1]).sqrt() - minor
}

fn sdf_cone_local(p: [f32; 3], rb: f32, rt: f32, hh: f32) -> f32 {
    let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
    let t = ((p[1] + hh) / (2.0 * hh)).clamp(0.0, 1.0);
    let r_at_y = rb * (1.0 - t) + rt * t;
    let dr = r - r_at_y;
    let da = p[1].abs() - hh;
    if dr > 0.0 && da > 0.0 {
        (dr * dr + da * da).sqrt()
    } else {
        dr.max(da)
    }
}

fn sdf_capsule_local(p: [f32; 3], r: f32, hh: f32) -> f32 {
    let rv = (p[0] * p[0] + p[2] * p[2]).sqrt();
    let yc = p[1].clamp(-hh, hh);
    let dy = p[1] - yc;
    (rv * rv + dy * dy).sqrt() - r
}

fn sdf_hex_prism_local(p: [f32; 3], apothem: f32, hh: f32) -> f32 {
    let qx = p[0].abs();
    let qz = p[2].abs();
    let d_hex = (qx * 0.866_025_4 + qz * 0.5).max(qz) - apothem;
    let d_axial = p[1].abs() - hh;
    if d_hex > 0.0 && d_axial > 0.0 {
        (d_hex * d_hex + d_axial * d_axial).sqrt()
    } else {
        d_hex.max(d_axial)
    }
}

fn sdf_ellipsoid_local(p: [f32; 3], radii: [f32; 3]) -> f32 {
    let r_min = radii[0].min(radii[1]).min(radii[2]);
    let q = [p[0] / radii[0], p[1] / radii[1], p[2] / radii[2]];
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
    (len - 1.0) * r_min
}

fn sdf_gyroid_local(p: [f32; 3], cell_size: f32, thickness: f32) -> f32 {
    let s = cell_size;
    let x = p[0] / s;
    let y = p[1] / s;
    let z = p[2] / s;
    let f = x.sin() * y.cos() + y.sin() * z.cos() + z.sin() * x.cos();
    (f.abs() - thickness) * s * 0.2887
}

fn sdf_thread_local(
    p: [f32; 3],
    nominal_radius: f32,
    pitch: f32,
    depth: f32,
    half_height: f32,
) -> f32 {
    let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
    let theta = p[2].atan2(p[0]);
    let y_unwrapped = p[1] - pitch * theta / std::f32::consts::TAU;
    let phase = (y_unwrapped / pitch) * std::f32::consts::TAU;
    let r_profile = nominal_radius - depth * 0.5 * (1.0 - phase.cos());
    let d_radial = r - r_profile;
    let d_axial = p[1].abs() - half_height;
    d_radial.max(d_axial)
}

fn sdf_polygon_2d(p: [f32; 2], verts: &[[f32; 2]]) -> f32 {
    if verts.len() < 3 {
        return f32::MAX;
    }
    let n = verts.len();
    let mut d = f32::MAX;
    let mut sign = 1.0f32;
    let mut j = n - 1;
    for i in 0..n {
        let vi = verts[i];
        let vj = verts[j];
        let e = [vj[0] - vi[0], vj[1] - vi[1]];
        let w = [p[0] - vi[0], p[1] - vi[1]];
        let t = (w[0] * e[0] + w[1] * e[1]) / (e[0] * e[0] + e[1] * e[1]);
        let t = t.clamp(0.0, 1.0);
        let dx = w[0] - e[0] * t;
        let dy = w[1] - e[1] * t;
        let dd = dx * dx + dy * dy;
        if dd < d {
            d = dd;
        }
        // Winding number (crossing test)
        let c1 = p[1] >= vi[1];
        let c2 = p[1] < vj[1];
        let c3 = e[0] * w[1] > e[1] * w[0];
        if (c1 && c2 && c3) || (!c1 && !c2 && !c3) {
            sign = -sign;
        }
        j = i;
    }
    sign * d.sqrt()
}

fn sdf_sector_2d(
    p: [f32; 2],
    center: [f32; 2],
    radius: f32,
    start_angle: f32,
    sweep_angle: f32,
) -> f32 {
    let dx = p[0] - center[0];
    let dy = p[1] - center[1];
    let r = (dx * dx + dy * dy).sqrt();
    let angle = dy.atan2(dx);
    let mid = start_angle + sweep_angle * 0.5;
    let half = sweep_angle.abs() * 0.5;
    let mut rel = angle - mid;
    while rel > std::f32::consts::PI {
        rel -= std::f32::consts::TAU;
    }
    while rel < -std::f32::consts::PI {
        rel += std::f32::consts::TAU;
    }
    let d_circ = r - radius;
    if rel.abs() <= half {
        d_circ
    } else {
        let edge_angle = if rel > 0.0 {
            start_angle + sweep_angle
        } else {
            start_angle
        };
        let ex = center[0] + radius * edge_angle.cos();
        let ey = center[1] + radius * edge_angle.sin();
        let to_edge = ((p[0] - ex).powi(2) + (p[1] - ey).powi(2)).sqrt();
        let to_center = r;
        d_circ.max(0.0).min(to_edge.min(to_center))
    }
}

fn blend_min_eval(a: f32, b: f32, r: f32, mode: f32) -> f32 {
    if mode < 0.5 {
        // Fillet (IQ cubic smin)
        let h = (0.5 + 0.5 * (b - a) / r).clamp(0.0, 1.0);
        b * (1.0 - h) + a * h - r * h * (1.0 - h)
    } else {
        // Chamfer
        a.min(b).min((a + b - r) * std::f32::consts::FRAC_1_SQRT_2)
    }
}

fn sdf_capsule_3d(p: [f32; 3], a: [f32; 3], b: [f32; 3], r: f32) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let dot_ab = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let t = if dot_ab > 0.0 {
        ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / dot_ab).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let dx = ap[0] - ab[0] * t;
    let dy = ap[1] - ab[1] * t;
    let dz = ap[2] - ab[2] * t;
    (dx * dx + dy * dy + dz * dz).sqrt() - r
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::{CsgNode, DecodedFeature, EvalMode, PrimitiveModifiers};
    use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

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

    fn sphere_feature(id: u32, center: [f32; 3], radius: f32) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Sphere {id}"),
            BoolOp::Union,
            Primitive::Sphere { center, radius },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    fn translated_feature(
        id: u32,
        op: BoolOp,
        primitive: Primitive,
        translate: [f32; 3],
    ) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("F{id}"),
            op,
            primitive,
            Transform {
                translate,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
            },
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    fn assert_sdf_match(bvh: &BoundedCsg, compiled: &CompiledSdf, points: &[[f32; 3]], tol: f32) {
        for &p in points {
            let expected = bvh.sdf(p, EvalMode::Visual);
            let got = eval_opcodes(&compiled.opcodes, p);
            assert!(
                (expected - got).abs() < tol,
                "mismatch at {p:?}: bvh={expected} compiled={got} (tol={tol})",
            );
        }
    }

    #[test]
    fn single_box_at_origin() {
        let features = vec![box_feature(
            1,
            BoolOp::Union,
            [0.0, 0.0, 0.0],
            [10.0, 10.0, 10.0],
        )];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [15.0, 0.0, 0.0],
            [5.0, 5.0, 5.0],
            [-10.0, -10.0, -10.0],
            [20.0, 20.0, 20.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-4);
    }

    #[test]
    fn box_with_offset_center() {
        let features = vec![box_feature(
            1,
            BoolOp::Union,
            [5.0, -3.0, 7.0],
            [4.0, 4.0, 4.0],
        )];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [5.0, -3.0, 7.0],
            [9.0, -3.0, 7.0],
            [0.0, 0.0, 0.0],
            [5.0, 1.0, 11.0],
            [10.0, 10.0, 10.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-4);
    }

    #[test]
    fn sphere_translated() {
        let features = vec![translated_feature(
            1,
            BoolOp::Union,
            Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius: 8.0,
            },
            [10.0, 20.0, 30.0],
        )];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [10.0, 20.0, 30.0],
            [18.0, 20.0, 30.0],
            [0.0, 0.0, 0.0],
            [10.0, 20.0, 38.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-4);
    }

    #[test]
    fn union_of_two_boxes() {
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            box_feature(2, BoolOp::Union, [30.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [15.0, 0.0, 0.0],
            [30.0, 0.0, 0.0],
            [20.0, 0.0, 0.0],
            [50.0, 50.0, 50.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-4);
    }

    #[test]
    fn difference_box_minus_sphere() {
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            {
                let mut f = sphere_feature(2, [10.0, 10.0, 10.0], 8.0);
                f.op = BoolOp::Difference;
                f
            },
        ];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [10.0, 10.0, 10.0],
            [20.0, 0.0, 0.0],
            [15.0, 5.0, 5.0],
            [-10.0, -10.0, -10.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-4);
    }

    #[test]
    fn cylinder_and_torus() {
        let features = vec![
            translated_feature(
                1,
                BoolOp::Union,
                Primitive::Cylinder {
                    center: [0.0, 0.0, 0.0],
                    radius: 5.0,
                    half_height: 15.0,
                },
                [0.0, 0.0, 0.0],
            ),
            translated_feature(
                2,
                BoolOp::Union,
                Primitive::Torus {
                    center: [0.0, 0.0, 0.0],
                    major_radius: 10.0,
                    minor_radius: 3.0,
                },
                [0.0, 0.0, 0.0],
            ),
        ];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [5.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [0.0, 15.0, 0.0],
            [13.0, 0.0, 0.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    #[test]
    fn extruded_rectangle() {
        use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};

        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [5.0, 3.0],
        });
        let csg = CsgNode::Extrude {
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
            half_height: 2.0,
        };
        let bvh = BoundedCsg::from_csg(&csg);
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [5.0, 0.0, 0.0],
            [0.0, 3.0, 0.0],
            [0.0, 0.0, 2.0],
            [5.0, 3.0, 2.0],
            [7.0, 0.0, 0.0],
            [10.0, 10.0, 10.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-4);
    }

    #[test]
    fn extruded_washer_ring() {
        use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};

        let outer = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 10.0,
        });
        let inner = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 4.0,
        });
        let washer = Sketch2d::Difference(Box::new(outer), Box::new(inner));
        let csg = CsgNode::Extrude {
            id: FeatureId::from_u32(1),
            sketch: Box::new(washer),
            half_height: 3.0,
        };
        let bvh = BoundedCsg::from_csg(&csg);
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [7.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [3.0, 0.0, 0.0],
            [0.0, 0.0, 3.0],
            [7.0, 0.0, 3.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    #[test]
    fn revolved_rectangle() {
        use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};

        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [8.0, 0.0],
            half_extents: [2.0, 5.0],
        });
        let csg = CsgNode::Revolve {
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
        };
        let bvh = BoundedCsg::from_csg(&csg);
        let compiled = compile_body(&bvh);
        let points = [
            [8.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [6.0, 0.0, 0.0],
            [0.0, 0.0, 8.0],
            [0.0, 6.0, 0.0],
            [0.0, 0.0, 0.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    #[test]
    fn mixed_scene_matches_bvh() {
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            {
                let mut f = sphere_feature(2, [10.0, 10.0, 10.0], 8.0);
                f.op = BoolOp::Difference;
                f
            },
            box_feature(3, BoolOp::Union, [-30.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [20.0, 0.0, 0.0],
            [10.0, 10.0, 10.0],
            [-30.0, 0.0, 0.0],
            [-60.0, 0.0, 0.0],
            [100.0, 100.0, 100.0],
            [15.0, 5.0, 5.0],
            [-4.0, -4.0, -4.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    #[test]
    fn twist_warp() {
        let f = DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "Box 1".into(),
            BoolOp::Union,
            Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [5.0, 20.0, 5.0],
            },
            Transform::IDENTITY,
            PrimitiveModifiers { twist_rate: 0.03, ..PrimitiveModifiers::NONE },
            vec![],
        );
        let features = vec![f];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [5.0, 0.0, 0.0],
            [0.0, 10.0, 0.0],
            [3.0, 5.0, 3.0],
            [10.0, 10.0, 10.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    #[test]
    fn shell_operator() {
        let f = DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "Sphere 1".into(),
            BoolOp::Union,
            Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius: 10.0,
            },
            Transform::IDENTITY,
            PrimitiveModifiers { shell_thickness: 1.0, ..PrimitiveModifiers::NONE },
            vec![],
        );
        let features = vec![f];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [9.5, 0.0, 0.0],
            [15.0, 0.0, 0.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    #[test]
    fn empty_tree_compiles() {
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[]));
        let compiled = compile_body(&bvh);
        let d = eval_opcodes(&compiled.opcodes, [0.0, 0.0, 0.0]);
        assert!(d > 1e30);
    }

    #[test]
    fn compiled_opcode_count_is_reasonable() {
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            sphere_feature(2, [0.0, 0.0, 0.0], 5.0),
        ];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        // box: 1 tag + 13 payload = 14 (scale 1→3: +2)
        // sphere: 1 tag + 11 payload = 12 (scale 1→3: +2)
        // union: 1
        // end: 1
        // total = 28
        assert_eq!(compiled.opcode_count(), 28);
    }
}
