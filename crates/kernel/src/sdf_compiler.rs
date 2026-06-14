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

use fielddraft_protocol::{BakedVolume, HalfSpace, Primitive, Transform};

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
pub const OP_PRIM_HALFSPACE: u32 = 0x0D; // +15: kind(1) + transform(10) + params(4) — one analytic half-space face (retained-boundary shell)

// 2D primitives — evaluate at (point.x, point.y), push onto stack.
pub const OP_PRIM2D_CIRCLE: u32  = 0x10; // +3: center(2) + radius(1)
pub const OP_PRIM2D_RECT: u32    = 0x11; // +4: center(2) + half_extents(2)
pub const OP_PRIM2D_POLY: u32    = 0x12; // +1+2N: vertex_count(1) + vertices(2*N)
pub const OP_PRIM2D_SECTOR: u32  = 0x13; // +5: center(2) + radius(1) + start(1) + sweep(1)
pub const OP_PRIM2D_PATH: u32    = 0x14; // +2+7N: seg_count(1) + closed(1) + records(7*N), record = kind(1) + payload(6); every record is y-monotone

// Binary operators — pop 2, push 1.
pub const OP_UNION: u32          = 0x20; // +0
pub const OP_DIFFERENCE: u32     = 0x21; // +0
pub const OP_INTERSECT: u32      = 0x22; // +0
pub const OP_SMOOTH_UNION: u32   = 0x23; // +2: radius(1) + mode(1)
pub const OP_SMOOTH_DIFF: u32    = 0x24; // +2: radius(1) + mode(1)

// Unary / compositor operators.
pub const OP_SHELL: u32          = 0x29; // +1: thickness — pops 1, closed inward shell max(d, −(d+t))
pub const OP_MORPH: u32          = 0x2A; // +1: t — pops 2, pushes lerp
pub const OP_DISPLACE_VAL: u32   = 0x2B; // +2: depth + freq — pops 1, uses point
pub const OP_EXTRUDE: u32        = 0x2C; // +3: half_height + scale + cap_mask(1=top,2=bottom retained) — pops d2d, uses point.z
pub const OP_REVOLVE_SCALE: u32  = 0x2D; // +1: scale — pops d2d, multiplies by scale
pub const OP_OFFSET_2D: u32      = 0x2E; // +1: distance — pops 1, pushes d - distance
pub const OP_NEG: u32            = 0x2F; // +0: pops 1, pushes −v — retained path fields are −unsigned distance

// Domain warps — modify the point register.
pub const OP_WARP_PUSH: u32      = 0x30; // +0: save point to point stack
pub const OP_WARP_POP: u32       = 0x31; // +0: restore point from point stack
pub const OP_WARP_TWIST: u32     = 0x32; // +1: rate — rotate XZ by rate*y
pub const OP_WARP_BEND: u32      = 0x33; // +1: rate — rotate XY by rate*x
pub const OP_WARP_TRANSFORM: u32 = 0x34; // +10: translate(3) + quat(4) + scale(3)
pub const OP_WARP_REVOLVE: u32   = 0x35; // +0: point ← (√(x²+z²), y, 0)
pub const OP_WARP_HELIX: u32     = 0x36; // +7: pitch + hand + turns + y_anchor + taper + r_inner + start_angle — point ← (r−taper·(y−y0), w, ell)
pub const OP_HELIX_SCALE: u32    = 0x37; // +1: scale (world·lipschitz, baked at emission) — pops d2d, composes with point.z (end caps), multiplies
pub const OP_WARP_SWEEP: u32     = 0x38; // +4 header (N, closed, total_len, twist) + N×10 (point3, n3, b3, arclen); nearest spine point → point ← (twist-rotated u, v, w_cap); compose via OP_HELIX_SCALE

// Masked blend — pops 2 (da, db), evaluates capsule mask, pushes 1.
pub const OP_MASKED_BLEND: u32   = 0x40; // +var: n + n×point(3) + mask_r+blend_r+mode+is_diff

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
    emit_node(node, &mut ctx, &crate::csg::OpenBranchSet::EMPTY);
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

fn emit_node(node: &BoundedCsg, ctx: &mut CompileCtx, open: &crate::csg::OpenBranchSet<'_>) {
    let out = &mut ctx.opcodes;
    match node {
        BoundedCsg::Empty => {
            out.push(op(OP_PRIM_CONST));
            out.push(f32::MAX);
        }
        BoundedCsg::Leaf {
            id,
            primitive,
            world_transform,
            ..
        } => {
            if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                emit_leaf_retained(id, primitive, world_transform, ctx, open);
            } else {
                emit_leaf_primitive(primitive, world_transform, ctx);
            }
        }
        BoundedCsg::Union { left, right, .. } => {
            emit_node(left, ctx, open);
            emit_node(right, ctx, open);
            ctx.opcodes.push(op(OP_UNION));
        }
        BoundedCsg::Difference { left, right, .. } => {
            emit_node(left, ctx, open);
            emit_node(right, ctx, open);
            ctx.opcodes.push(op(OP_DIFFERENCE));
        }
        BoundedCsg::Intersection { left, right, .. } => {
            emit_node(left, ctx, open);
            emit_node(right, ctx, open);
            ctx.opcodes.push(op(OP_INTERSECT));
        }
        BoundedCsg::SmoothUnion {
            left,
            right,
            radius,
            mode,
            ..
        } => {
            emit_node(left, ctx, open);
            emit_node(right, ctx, open);
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
            emit_node(left, ctx, open);
            emit_node(right, ctx, open);
            let out = &mut ctx.opcodes;
            out.push(op(OP_SMOOTH_DIFF));
            out.push(*radius);
            out.push(blend_mode_f32(*mode));
        }
        BoundedCsg::Twist { inner, rate, .. } => {
            out.push(op(OP_WARP_PUSH));
            out.push(op(OP_WARP_TWIST));
            out.push(*rate);
            emit_node(inner, ctx, open);
            ctx.opcodes.push(op(OP_WARP_POP));
        }
        BoundedCsg::Bend { inner, rate, .. } => {
            out.push(op(OP_WARP_PUSH));
            out.push(op(OP_WARP_BEND));
            out.push(*rate);
            emit_node(inner, ctx, open);
            ctx.opcodes.push(op(OP_WARP_POP));
        }
        BoundedCsg::Displace {
            inner, depth, freq, ..
        } => {
            emit_node(inner, ctx, open);
            let out = &mut ctx.opcodes;
            out.push(op(OP_DISPLACE_VAL));
            out.push(*depth);
            out.push(*freq);
        }
        BoundedCsg::Offset { inner, delta, .. } => {
            // `Offset` is `child − δ`. `OP_OFFSET_2D` is dimension-agnostic on the
            // distance stack (pops 1, pushes `d − δ`), so it serves the 3D node too.
            emit_node(inner, ctx, open);
            let out = &mut ctx.opcodes;
            out.push(op(OP_OFFSET_2D));
            out.push(*delta);
        }
        BoundedCsg::Shell {
            inner,
            thickness,
            open_faces,
            ..
        } => {
            // Retained-boundary shell: wall = max(d, −(d_ret + t)). The closed
            // case folds to the single-subtree OP_SHELL; an opened shell
            // compiles the inner subtree TWICE — the second copy under the
            // opened branches (its leaves emit retained-boundary fields) —
            // and combines with the existing stack ops:
            //   push d; push d_ret; OFFSET(−t) → d_ret + t; DIFFERENCE → wall.
            emit_node(inner, ctx, open);
            if open_faces.is_empty() {
                let out = &mut ctx.opcodes;
                out.push(op(OP_SHELL));
                out.push(*thickness);
            } else {
                let merged = crate::csg::OpenBranchSet::push(open, open_faces);
                emit_node(inner, ctx, &merged);
                let out = &mut ctx.opcodes;
                out.push(op(OP_OFFSET_2D));
                out.push(-*thickness);
                out.push(op(OP_DIFFERENCE));
            }
        }
        BoundedCsg::Morph {
            left, right, t, ..
        } => {
            emit_node(left, ctx, open);
            emit_node(right, ctx, open);
            let out = &mut ctx.opcodes;
            out.push(op(OP_MORPH));
            out.push(*t);
        }
        BoundedCsg::MaskedBlend {
            left,
            right,
            mask_polyline,
            mask_radius,
            blend_radius,
            blend_mode,
            is_difference,
            ..
        } => {
            emit_node(left, ctx, open);
            emit_node(right, ctx, open);
            let out = &mut ctx.opcodes;
            out.push(op(OP_MASKED_BLEND));
            // Count-prefixed polyline: n then n×[x,y,z]. A 1-point
            // polyline is a sphere, 2 points a single capsule, etc.
            out.push(mask_polyline.len() as f32);
            for pt in mask_polyline {
                out.extend_from_slice(pt);
            }
            out.push(*mask_radius);
            out.push(*blend_radius);
            out.push(blend_mode_f32(*blend_mode));
            out.push(if *is_difference { 1.0 } else { 0.0 });
        }
        BoundedCsg::Extrude {
            id,
            sketch,
            half_height,
            world_transform,
            ..
        } => {
            let excl = open
                .has_feature(id.as_u32_or_root().unwrap_or(0))
                .then_some((id, open));
            let mut cap_mask = 3.0_f32;
            if let Some((id, open)) = excl {
                if open.contains_sub(id, crate::csg::BranchId::EXTRUDE_CAP_TOP_SUB) {
                    cap_mask -= 1.0;
                }
                if open.contains_sub(id, crate::csg::BranchId::EXTRUDE_CAP_BOTTOM_SUB) {
                    cap_mask -= 2.0;
                }
            }
            out.push(op(OP_WARP_PUSH));
            emit_warp_transform(world_transform, out);
            emit_sketch(sketch, out, 0, excl);
            out.push(op(OP_EXTRUDE));
            out.push(*half_height);
            out.push(world_transform.min_scale());
            out.push(cap_mask);
            out.push(op(OP_WARP_POP));
        }
        BoundedCsg::Revolve {
            id,
            sketch,
            world_transform,
            ..
        } => {
            let excl = open
                .has_feature(id.as_u32_or_root().unwrap_or(0))
                .then_some((id, open));
            out.push(op(OP_WARP_PUSH));
            emit_warp_transform(world_transform, out);
            out.push(op(OP_WARP_REVOLVE));
            emit_sketch(sketch, out, 0, excl);
            out.push(op(OP_REVOLVE_SCALE));
            out.push(world_transform.min_scale());
            out.push(op(OP_WARP_POP));
        }
        BoundedCsg::Helix {
            id,
            sketch,
            pitch,
            turns,
            left_handed,
            taper,
            start_angle,
            y_anchor,
            r_inner,
            world_transform,
            ..
        } => {
            // Helical unwrap into the profile frame, then the shared 2D
            // sketch eval, then the per-point Lipschitz normalisation —
            // mirrors `crate::helix_profile_point` (and the web GPU
            // interpreter's OP_WARP_HELIX / OP_HELIX_SCALE arms).
            let excl = open
                .has_feature(id.as_u32_or_root().unwrap_or(0))
                .then_some((id, open));
            out.push(op(OP_WARP_PUSH));
            emit_warp_transform(world_transform, out);
            out.push(op(OP_WARP_HELIX));
            out.push(pitch.max(1e-3));
            out.push(if *left_handed { -1.0 } else { 1.0 });
            out.push(turns.max(0.0));
            out.push(*y_anchor);
            out.push(*taper);
            out.push(*r_inner);
            out.push(*start_angle);
            emit_sketch(sketch, out, 0, excl);
            // Lipschitz scale is a per-feature constant (see
            // `helix_profile_point`) — bake it into the multiplier.
            let tau = std::f32::consts::TAU;
            let p_eff = pitch.max(1e-3);
            let rr = r_inner.max(p_eff / tau * 0.25).max(1e-3);
            let q = p_eff / (tau * rr);
            let lip = 1.0 / (2.25 + q * q + taper * taper).sqrt();
            out.push(op(OP_HELIX_SCALE));
            out.push(world_transform.min_scale() * lip);
            out.push(op(OP_WARP_POP));
        }
        BoundedCsg::Sweep { id, sketch, path, world_transform, .. } => {
            // Nearest-spine-point unwrap into the profile's (u, v) frame, the
            // shared 2D sketch eval, then the IQ extrude cap-compose (reuses
            // OP_HELIX_SCALE — point.z carries the cap coordinate w). Mirrors
            // `crate::sweep_path::SweepPath::sdf`.
            let excl = open
                .has_feature(id.as_u32_or_root().unwrap_or(0))
                .then_some((id, open));
            out.push(op(OP_WARP_PUSH));
            emit_warp_transform(world_transform, out);
            out.push(op(OP_WARP_SWEEP));
            out.push(path.points.len() as f32);
            out.push(if path.closed { 1.0 } else { 0.0 });
            out.push(path.total_len);
            out.push(path.twist);
            for i in 0..path.points.len() {
                let p = path.points[i];
                let f = &path.frames[i];
                out.extend_from_slice(&[p[0], p[1], p[2]]);
                out.extend_from_slice(&[f.n[0], f.n[1], f.n[2]]);
                out.extend_from_slice(&[f.b[0], f.b[1], f.b[2]]);
                out.push(path.arclen[i]);
            }
            emit_sketch(sketch, out, 0, excl);
            out.push(op(OP_HELIX_SCALE));
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

// ── Shell open-face surface kinds (mirror in `web/src/sdfRenderer.ts`'s
//    `fd_open_gate`). Each open face is a fixed 15-float record so both VMs can
//    pre-skip the operand block and loop at a constant stride:
//    `kind(1) + transform(10) + params(4, zero-padded)`. ──
pub(crate) const OPEN_KIND_PLANE: f32 = 0.0;
pub(crate) const OPEN_KIND_CYLINDER: f32 = 1.0;
pub(crate) const OPEN_KIND_SPHERE: f32 = 2.0;
pub(crate) const OPEN_KIND_CONE: f32 = 3.0;
pub(crate) const OPEN_KIND_TORUS: f32 = 4.0;
/// A non-analytic surface (TPMS / profile) — never emitted as a retained face.
pub(crate) const OPEN_KIND_NEVER: f32 = 5.0;

/// Retained-boundary leaf: `max` over the primitive's half-space faces with
/// the opened ones removed (the GPU mirror of `csg::leaf_sdf_retained`). Each
/// retained face is one `OP_PRIM_HALFSPACE` record folded with
/// `OP_INTERSECT`; an all-faces-open leaf degenerates to `−∞` (no retained
/// boundary → everything is cavity).
fn emit_leaf_retained(
    id: &fielddraft_protocol::FeatureId,
    prim: &Primitive,
    world_transform: &Transform,
    ctx: &mut CompileCtx,
    open: &crate::csg::OpenBranchSet<'_>,
) {
    let Some(hs) = crate::csg::primitive_halfspaces(prim) else {
        // No half-space split (Sphere-as-mesh etc. never reach here; TPMS /
        // imported meshes have nothing to retain) — exclusion is ignored.
        emit_leaf_primitive(prim, world_transform, ctx);
        return;
    };
    let retained: Vec<_> =
        hs.faces().iter().filter(|f| !open.contains_sub(id, f.sub)).collect();
    if retained.len() == hs.faces().len() {
        emit_leaf_primitive(prim, world_transform, ctx);
        return;
    }
    let out = &mut ctx.opcodes;
    if retained.is_empty() {
        out.push(op(OP_PRIM_CONST));
        out.push(f32::NEG_INFINITY);
        return;
    }
    for (i, f) in retained.iter().enumerate() {
        // Face frame: world ∘ translate(center + local_offset), the same DL-20
        // fold the weaver uses.
        let translate = [
            hs.center[0] + f.local_offset[0],
            hs.center[1] + f.local_offset[1],
            hs.center[2] + f.local_offset[2],
        ];
        let face_xf = crate::compose_transforms(
            world_transform,
            &Transform { translate, ..Transform::IDENTITY },
        );
        emit_halfspace_record(&f.half_space, &face_xf, out);
        if i > 0 {
            out.push(op(OP_INTERSECT));
        }
    }
}

/// One `OP_PRIM_HALFSPACE` record: kind + transform(10) + params(4).
fn emit_halfspace_record(surface: &HalfSpace, placement: &Transform, out: &mut Vec<f32>) {
    let kind = match surface {
        HalfSpace::Plane { .. } => OPEN_KIND_PLANE,
        HalfSpace::InfiniteCylinder { .. } => OPEN_KIND_CYLINDER,
        HalfSpace::Sphere { .. } => OPEN_KIND_SPHERE,
        HalfSpace::Cone { .. } => OPEN_KIND_CONE,
        HalfSpace::Torus { .. } => OPEN_KIND_TORUS,
        HalfSpace::Gyroid { .. } => OPEN_KIND_NEVER,
        HalfSpace::RevolvedProfile { .. } => OPEN_KIND_NEVER,
    };
    out.push(op(OP_PRIM_HALFSPACE));
    out.push(kind);
    out.extend_from_slice(&placement.translate);
    out.extend_from_slice(&placement.rotation);
    out.extend_from_slice(&placement.scale);
    // Four param slots, zero-padded; layout matches the WGSL mirror.
    let params = match surface {
        HalfSpace::Plane { normal, offset } => [normal[0], normal[1], normal[2], *offset],
        HalfSpace::InfiniteCylinder { radius } | HalfSpace::Sphere { radius } => {
            [*radius, 0.0, 0.0, 0.0]
        }
        HalfSpace::Cone { half_angle } => [*half_angle, 0.0, 0.0, 0.0],
        HalfSpace::Torus { major_radius, minor_radius } => {
            [*major_radius, *minor_radius, 0.0, 0.0]
        }
        HalfSpace::Gyroid { .. } | HalfSpace::RevolvedProfile { .. } => [0.0; 4],
    };
    out.extend_from_slice(&params);
}

/// Raw local SDF of one analytic half-space face, mirroring
/// `csg::sdf_halfspace` and the WGSL `fd_halfspace_local`. The caller scales
/// by the placement's min-scale, the same convention as every other leaf
/// primitive opcode.
fn halfspace_local(local: [f32; 3], kind: f32, p: [f32; 4]) -> f32 {
    if kind < 0.5 {
        // Plane: normal = (p0, p1, p2), offset = p3.
        local[0] * p[0] + local[1] * p[1] + local[2] * p[2] - p[3]
    } else if kind < 1.5 {
        // Infinite cylinder, radius p0, axis Y.
        (local[0] * local[0] + local[2] * local[2]).sqrt() - p[0]
    } else if kind < 2.5 {
        // Sphere, radius p0.
        (local[0] * local[0] + local[1] * local[1] + local[2] * local[2]).sqrt() - p[0]
    } else if kind < 3.5 {
        // Cone, half-angle p0, apex at origin, axis Y.
        let rho = (local[0] * local[0] + local[2] * local[2]).sqrt();
        let (sin_a, cos_a) = p[0].sin_cos();
        rho * cos_a - local[1].abs() * sin_a
    } else if kind < 4.5 {
        // Torus, major p0, minor p1.
        let rho = (local[0] * local[0] + local[2] * local[2]).sqrt();
        let q0 = rho - p[0];
        (q0 * q0 + local[1] * local[1]).sqrt() - p[1]
    } else {
        // Non-analytic (TPMS / profile) — never emitted as retained faces.
        f32::INFINITY
    }
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
        Primitive::Extrude { .. } | Primitive::Revolve { .. } | Primitive::Helix { .. } => {
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

/// `excl` carries the owning sweep's feature id + opened branch set; `base` is
/// this subtree's first global boundary-group id (right-boolean children start
/// past the left child's `edge_count`), matching the kernel's retained walk.
fn emit_sketch(
    sketch: &BoundedSketch2d,
    out: &mut Vec<f32>,
    base: u32,
    excl: Option<(&fielddraft_protocol::FeatureId, &crate::csg::OpenBranchSet<'_>)>,
) {
    match sketch {
        BoundedSketch2d::Empty => {
            out.push(op(OP_PRIM_CONST));
            out.push(f32::MAX);
        }
        BoundedSketch2d::Prim { primitive, .. } => {
            emit_sketch_primitive(primitive, out, base, excl);
        }
        BoundedSketch2d::Union { left, right, .. } => {
            emit_sketch(left, out, base, excl);
            emit_sketch(right, out, base + left.edge_count(), excl);
            out.push(op(OP_UNION));
        }
        BoundedSketch2d::Difference { left, right, .. } => {
            emit_sketch(left, out, base, excl);
            emit_sketch(right, out, base + left.edge_count(), excl);
            out.push(op(OP_DIFFERENCE));
        }
        BoundedSketch2d::Intersection { left, right, .. } => {
            emit_sketch(left, out, base, excl);
            emit_sketch(right, out, base + left.edge_count(), excl);
            out.push(op(OP_INTERSECT));
        }
        BoundedSketch2d::Offset {
            inner, distance, ..
        } => {
            emit_sketch(inner, out, base, excl);
            out.push(op(OP_OFFSET_2D));
            out.push(*distance);
        }
    }
}

fn emit_sketch_primitive(
    prim: &Sketch2dPrimitive,
    out: &mut Vec<f32>,
    base: u32,
    excl: Option<(&fielddraft_protocol::FeatureId, &crate::csg::OpenBranchSet<'_>)>,
) {
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
        Sketch2dPrimitive::Path { segments, closed } => {
            // Retained-boundary exclusion (opened shell faces): the field is
            // the NEGATED unsigned distance to the retained groups — an OPEN
            // path record (unsigned) + NEG. Continuous through the opening;
            // see `Sketch2dPrimitive::sdf_retained`.
            if let Some((id, open)) = excl {
                let groups = fielddraft_protocol::sketch2d::g1_group_segments(
                    segments,
                    fielddraft_protocol::sketch2d::PATH_G1_COS_THRESHOLD,
                );
                if groups.iter().any(|g| open.contains_sub(id, base + g)) {
                    let retained: Vec<SketchSegment> = segments
                        .iter()
                        .zip(&groups)
                        .filter(|(_, g)| !open.contains_sub(id, base + **g))
                        .map(|(s, _)| s.clone())
                        .collect();
                    emit_path_record(&retained, false, out);
                    out.push(op(OP_NEG));
                    return;
                }
            }
            emit_path_record(segments, *closed, out);
        }
    }
}

/// One `OP_PRIM2D_PATH` record. The stream carries the analytic segments
/// themselves — exact distance and exact even-odd parity on the GPU. (The
/// former polyline flattening closed the contour with a synthetic edge; for
/// an axis-stripped revolve profile that edge lay ON the axis and Shell
/// inflated it into a phantom 2 mm rod.) Records are pre-split y-monotone so
/// the shader's crossing test is the plain half-open endpoint rule with one
/// closed-form root.
fn emit_path_record(segments: &[SketchSegment], closed: bool, out: &mut Vec<f32>) {
    let records: Vec<SketchSegment> = segments
        .iter()
        .flat_map(fielddraft_protocol::sketch2d::split_y_monotone)
        .collect();
    out.push(op(OP_PRIM2D_PATH));
    out.push(records.len() as f32);
    out.push(if closed { 1.0 } else { 0.0 });
    for seg in &records {
        match seg {
            SketchSegment::Line { start, end } => {
                out.push(0.0);
                out.extend_from_slice(start);
                out.extend_from_slice(end);
                out.extend_from_slice(&[0.0, 0.0]);
            }
            SketchSegment::QuadBezier { p0, p1, p2 } => {
                out.push(1.0);
                out.extend_from_slice(p0);
                out.extend_from_slice(p1);
                out.extend_from_slice(p2);
            }
            SketchSegment::Arc {
                center,
                radius,
                start_angle,
                sweep_angle,
            } => {
                out.push(2.0);
                out.extend_from_slice(center);
                out.push(*radius);
                out.push(*start_angle);
                out.push(*sweep_angle);
                out.push(0.0);
            }
        }
    }
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
            OP_PRIM_HALFSPACE => {
                let kind = rd!();
                let (local, scale) = read_transform_and_localise(opcodes, &mut pc, pt);
                let params = [rd!(), rd!(), rd!(), rd!()];
                push_v!(halfspace_local(local, kind, params) * scale);
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
            OP_PRIM2D_PATH => {
                let n = rd!() as usize;
                let closed = rd!() != 0.0;
                let mut segs = Vec::with_capacity(n);
                for _ in 0..n {
                    let kind = rd!();
                    let a = rd!();
                    let b = rd!();
                    let c = rd!();
                    let d = rd!();
                    let e = rd!();
                    let f = rd!();
                    segs.push(match kind as u32 {
                        0 => SketchSegment::Line { start: [a, b], end: [c, d] },
                        1 => SketchSegment::QuadBezier { p0: [a, b], p1: [c, d], p2: [e, f] },
                        _ => SketchSegment::Arc {
                            center: [a, b],
                            radius: c,
                            start_angle: d,
                            sweep_angle: e,
                        },
                    });
                }
                push_v!(fielddraft_protocol::sketch2d::sdf_path(
                    [pt[0], pt[1]],
                    &segs,
                    closed,
                ));
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
                // Closed inward shell. Opened shells compile to two subtrees
                // + OFFSET(−t) + DIFFERENCE instead (retained-boundary
                // erosion), so this op carries only the thickness.
                let thickness = rd!();
                let d = pop_v!();
                push_v!(d.max(-(d + thickness)));
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
                let cap_mask = rd!() as u32;
                let d2d = pop_v!();
                if cap_mask == 3 {
                    // Both caps retained: exact-corner composition (IQ).
                    let wz = pt[2].abs() - half_h;
                    let outside = (d2d.max(0.0).powi(2) + wz.max(0.0).powi(2)).sqrt();
                    let inside = d2d.max(wz).min(0.0);
                    push_v!((outside + inside) * scale);
                } else {
                    // Opened cap(s): plain max over the retained terms
                    // (mirrors `csg::extrude_retained_combine`).
                    let mut d = d2d;
                    if cap_mask & 1 != 0 {
                        d = d.max(pt[2] - half_h);
                    }
                    if cap_mask & 2 != 0 {
                        d = d.max(-pt[2] - half_h);
                    }
                    push_v!(d * scale);
                }
            }
            OP_REVOLVE_SCALE => {
                let scale = rd!();
                let d = pop_v!();
                push_v!(d * scale);
            }
            OP_HELIX_SCALE => {
                let scale = rd!();
                // pt[2] carries the signed end-overshoot chord stashed
                // by OP_WARP_HELIX; compose like `crate::helix_compose`.
                let d2 = pop_v!();
                let ell = pt[2];
                let outside = (d2.max(0.0)).hypot(ell.max(0.0));
                let inside = d2.max(ell).min(0.0);
                push_v!((outside + inside) * scale);
            }
            OP_OFFSET_2D => {
                let dist = rd!();
                let d = pop_v!();
                push_v!(d - dist);
            }
            OP_NEG => {
                let v = pop_v!();
                push_v!(-v);
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
            OP_WARP_HELIX => {
                let pitch = rd!();
                let hand = rd!();
                let turns = rd!();
                let y_anchor = rd!();
                let taper = rd!();
                let r_inner = rd!();
                let start_angle = rd!();
                // Mirrors `crate::helix_profile_point` — continuous
                // sweep-parameter clamp; the signed end-overshoot chord
                // rides along in pt[2] (2D sketch ops ignore z) until
                // OP_HELIX_SCALE composes it in.
                let tau = std::f32::consts::TAU;
                let r = (pt[0] * pt[0] + pt[2] * pt[2]).sqrt();
                let theta = pt[2].atan2(pt[0]);
                let th = hand * theta - start_angle;
                let n = ((pt[1] - y_anchor) / pitch - th / tau).round();
                let t_n = th + tau * n;
                let t_end = tau * turns;
                let t_c = t_n.clamp(0.0, t_end);
                let w = pt[1] - pitch * t_c / tau;
                let rp = r - taper * (pt[1] - y_anchor);
                let rr = r_inner.max(pitch / tau * 0.25).max(1e-3);
                let l_t = (t_n - t_end * 0.5).abs() - t_end * 0.5;
                let ell = l_t.signum()
                    * r.min(rr)
                    * (l_t.abs().min(std::f32::consts::PI) * 0.5).sin();
                pt = [rp, w, ell];
            }
            OP_WARP_SWEEP => {
                // Mirror of `SweepPath::cross_section`: nearest spine point,
                // project into the interpolated (n, b) frame → (u, v); the cap
                // coordinate w rides along in pt[2] until OP_HELIX_SCALE.
                use crate::sweep_path::{cross, dot, normalize, sub};
                let n = rd!() as usize;
                let closed = rd!() > 0.5;
                let total_len = rd!();
                let twist = rd!();
                let mut pts = Vec::with_capacity(n);
                let mut nrm = Vec::with_capacity(n);
                let mut bnm = Vec::with_capacity(n);
                let mut arc = Vec::with_capacity(n);
                for _ in 0..n {
                    pts.push(rd3!());
                    nrm.push(rd3!());
                    bnm.push(rd3!());
                    arc.push(rd!());
                }
                let seg_count = if closed { n } else { n - 1 };
                let mut best_d2 = f32::INFINITY;
                let mut best_seg = 0usize;
                let mut best_t = 0.0f32;
                for si in 0..seg_count {
                    let a = pts[si];
                    let b = pts[(si + 1) % n];
                    let ab = sub(b, a);
                    let denom = dot(ab, ab);
                    let t = if denom > 1e-12 {
                        (dot(sub(pt, a), ab) / denom).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let foot = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
                    let d = sub(pt, foot);
                    let d2 = dot(d, d);
                    if d2 < best_d2 {
                        best_d2 = d2;
                        best_seg = si;
                        best_t = t;
                    }
                }
                let i0 = best_seg;
                let i1 = (best_seg + 1) % n;
                let ab = sub(pts[i1], pts[i0]);
                let foot = [
                    pts[i0][0] + ab[0] * best_t,
                    pts[i0][1] + ab[1] * best_t,
                    pts[i0][2] + ab[2] * best_t,
                ];
                let lerp = |a: [f32; 3], b: [f32; 3]| {
                    normalize([
                        a[0] + (b[0] - a[0]) * best_t,
                        a[1] + (b[1] - a[1]) * best_t,
                        a[2] + (b[2] - a[2]) * best_t,
                    ])
                };
                let nf = lerp(nrm[i0], nrm[i1]);
                let bf = lerp(bnm[i0], bnm[i1]);
                let d = sub(pt, foot);
                let mut u = dot(d, nf);
                let mut v = dot(d, bf);
                let s_foot = arc[i0] + (arc[i1] - arc[i0]) * best_t;
                let w = if closed {
                    f32::NEG_INFINITY
                } else {
                    let at_end = (best_seg == 0 && best_t <= 1e-6)
                        || (best_seg == seg_count - 1 && best_t >= 1.0 - 1e-6);
                    if at_end {
                        let taxis = normalize(cross(nf, bf));
                        dot(d, taxis).abs()
                    } else {
                        -s_foot.min(total_len - s_foot)
                    }
                };
                // Twist: un-rotate (u, v) by θ = twist · s/L (mirror of
                // SweepPath::cross_section).
                if twist != 0.0 && total_len > 1e-6 {
                    let theta = -twist * s_foot / total_len;
                    let (sn, cs) = theta.sin_cos();
                    let (nu, nv) = (cs * u - sn * v, sn * u + cs * v);
                    u = nu;
                    v = nv;
                }
                pt = [u, v, w];
            }

            // ── Masked blend ───────────────────────────────────
            OP_MASKED_BLEND => {
                let n = rd!() as usize;
                let mut polyline: Vec<[f32; 3]> = Vec::with_capacity(n);
                for _ in 0..n {
                    polyline.push(rd3!());
                }
                let mask_r = rd!();
                let blend_r = rd!();
                let mode = rd!();
                let is_diff = rd!() > 0.5;
                let db = pop_v!();
                let da = pop_v!();
                let mask_d = crate::csg::sdf_polyline_tube(pt, &polyline, mask_r);
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
    // Lipschitz normalization: |∇f| peaks at sqrt(1 + (π·depth/pitch)²) because
    // the helix folds y into r_profile's phase; divide by it so the field is a
    // conservative ≤1 distance and the sphere-march never overshoots the groove
    // walls (cf. Gyroid 0.2887 / Cone wall_scale). See sdf_dual::Thread.
    let lip = (1.0 + (std::f32::consts::PI * depth / pitch.max(1e-6)).powi(2)).sqrt();
    d_radial.max(d_axial) / lip
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
            BoolOp::New,
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
            // Same-sign infinities agree (an emptied body reads +∞ on both
            // paths; the literal difference would be NaN).
            if expected.is_infinite() && got.is_infinite() && expected.signum() == got.signum() {
                continue;
            }
            assert!(
                (expected - got).abs() < tol,
                "mismatch at {p:?}: bvh={expected} compiled={got} (tol={tol})",
            );
        }
    }

    #[test]
    fn helix_opcode_stream_matches_tree_eval() {
        // Thread-like helix (rect profile r∈[8,10], pitch 2, 5 turns)
        // unioned onto a core cylinder — GPU/CPU opcode parity against
        // the BVH tree eval, including the Lipschitz normalisation.
        let features = vec![
            DecodedFeature::from_parts(
                FeatureId::from_u32(1),
                "Core".into(),
                BoolOp::New,
                Primitive::Cylinder { center: [0.0, 0.0, 0.0], radius: 8.2, half_height: 7.0 },
                Transform::IDENTITY,
                PrimitiveModifiers::NONE,
                vec![],
            ),
            DecodedFeature::from_parts(
                FeatureId::from_u32(2),
                "Helix".into(),
                BoolOp::Add,
                Primitive::Helix {
                    sketch: fielddraft_protocol::Sketch2d::Prim(
                        fielddraft_protocol::Sketch2dPrimitive::Rectangle {
                            center: [9.0, 0.0],
                            half_extents: [1.0, 0.6],
                        },
                    ),
                    center: [0.0, 0.0, 0.0],
                    pitch: 2.0,
                    turns: 5.0,
                    left_handed: false,
                    taper: 0.0,
                    // Non-zero so the parity check exercises the
                    // start-angle path of the opcode stream too.
                    start_angle: 0.6,
                },
                Transform::IDENTITY,
                PrimitiveModifiers::NONE,
                vec![],
            ),
        ];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
        let compiled = compile_body(&bvh);
        let points = [
            [9.0, 0.0, 0.0],
            [9.0, 1.0, 0.0],
            [0.0, 0.5, 9.0],
            [9.0, 2.0, 0.0],
            [9.0, 12.0, 0.0],
            [11.5, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            [4.0, 3.3, 4.0],
            [-9.0, 5.1, 0.2],
            // End-region seam pairs (the vtyh fix lives here).
            [-9.0, -1.0, 0.05],
            [-9.0, -1.0, -0.05],
            [-9.0, 11.0, 0.05],
            [-9.0, 11.0, -0.05],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-4);
    }

    #[test]
    fn sweep_opcode_stream_matches_tree_eval() {
        // Disk r=5 swept along a gentle 3D wave — opcode VM (the GPU mirror)
        // vs the BoundedCsg tree eval, exercising the nearest-segment unwrap,
        // interpolated RMF frame, and the end-cap composition.
        let path: Vec<[f32; 3]> = (0..=10)
            .map(|i| {
                let x = i as f32 * 5.0;
                [x, (x * 0.03).sin() * 5.0, (x * 0.02).cos() * 3.0]
            })
            .collect();
        let node = CsgNode::Sweep {
            id: FeatureId::from_u32(1),
            sketch: Box::new(fielddraft_protocol::Sketch2d::Prim(
                fielddraft_protocol::Sketch2dPrimitive::Circle {
                    center: [0.0, 0.0],
                    radius: 5.0,
                },
            )),
            path,
            closed: false,
            // Non-zero twist so the parity check exercises the rotation path.
            twist: 1.5,
        };
        let bvh = BoundedCsg::from_csg(&node);
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 3.0],
            [25.0, 0.0, 0.0],
            [25.0, 10.0, 0.0],
            [50.0, 0.0, 3.0],
            [52.0, 0.0, 3.0],
            [10.0, 8.0, 5.0],
            [40.0, -6.0, -2.0],
        ];
        assert_sdf_match(&bvh, &compiled, &points, 1e-3);
    }

    #[test]
    fn single_box_at_origin() {
        let features = vec![box_feature(
            1,
            BoolOp::New,
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
            BoolOp::New,
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
            BoolOp::New,
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
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            box_feature(2, BoolOp::New, [30.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
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
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            {
                let mut f = sphere_feature(2, [10.0, 10.0, 10.0], 8.0);
                f.op = BoolOp::Remove;
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
                BoolOp::New,
                Primitive::Cylinder {
                    center: [0.0, 0.0, 0.0],
                    radius: 5.0,
                    half_height: 15.0,
                },
                [0.0, 0.0, 0.0],
            ),
            translated_feature(
                2,
                BoolOp::New,
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
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            {
                let mut f = sphere_feature(2, [10.0, 10.0, 10.0], 8.0);
                f.op = BoolOp::Remove;
                f
            },
            box_feature(3, BoolOp::New, [-30.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
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
            BoolOp::New,
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
            BoolOp::New,
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
    fn shell_operator_curved_opening() {
        // A sphere body shelled with its own spherical face opened: the curved
        // gate must compile identically to the BVH path (validates the per-kind
        // open-surface VM record + `open_gate_local`/`fd_open_gate` mirror).
        use fielddraft_protocol::{HalfSpace, Transform};
        let sphere = CsgNode::from_decoded(&[DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "Sphere 1".into(),
            BoolOp::New,
            Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 10.0 },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )]);
        let shell = CsgNode::Shell(
            Box::new(sphere),
            1.0,
            vec![crate::csg::OpenSurface {
                branch: crate::csg::BranchId::from_feature_sub(
                    &FeatureId::from_u32(1),
                    0,
                )
                .as_u32(),
                surface: HalfSpace::Sphere { radius: 10.0 },
                placement: Transform::IDENTITY,
            }],
        );
        let bvh = BoundedCsg::from_csg(&shell);
        let compiled = compile_body(&bvh);
        let points = [
            [0.0, 0.0, 0.0],
            [9.5, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [0.0, 9.5, 0.0],
            [5.0, 5.0, 0.0],
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
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
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
