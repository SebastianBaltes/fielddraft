//! FieldDraft geometry kernel.
//!
//! After K1 the kernel owns the canonical `Document`, builds a CSG tree
//! from its feature spine, and meshes it with a naive uniform-grid
//! Dual Contouring pass. The mesh buffers are held inside the kernel
//! and exposed to JavaScript through a zero-copy pointer interface;
//! the old raymarcher-era packed feature buffer is still shipped for
//! the legacy TSL viewport path but is no longer the primary render
//! input.

pub mod body;
pub mod bvh;
pub mod csg;
pub mod dc;
pub mod expr;
pub mod features;
// K13: mesh_sdf moved to the protocol crate so
// Primitive::ImportedMesh can carry it serialised.
pub use fielddraft_protocol::mesh_sdf;
pub mod sketch_regions;
pub mod queries;
pub mod export;
pub mod raycast;
pub mod sketch2d;
pub mod sketch_features;

use body::{build_bounded_for_body, hash_body_payload, partition, Body};
use bvh::BoundedCsg;
use csg::CsgNode;
use dc::{dual_contour, Mesh};
use raycast::{raycast, RaycastParams};
use fielddraft_protocol::{BoolOp, Document, Feature, FeatureId, Primitive, Transform};
use wasm_bindgen::prelude::*;

/// Default voxel grid resolution for the *active* body — the one the
/// user is currently editing. 48 cells along the longest AABB axis
/// strikes a balance between rebuild latency (a handful of ms for a
/// small spine) and visible smoothness for a 300 mm build volume.
pub const DEFAULT_MESH_RESOLUTION: u32 = 48;

/// Default voxel grid resolution for *context* bodies — the ones
/// that aren't currently being edited. Half of the active
/// resolution means ≈ 8× fewer cells and ≈ 8× less meshing work
/// when the partition pass sweeps across them, while still looking
/// identifiable enough on screen to serve as background reference.
pub const DEFAULT_CONTEXT_MESH_RESOLUTION: u32 = 24;

/// Packed representation of one feature for the zero-copy bridge.
///
/// Layout (16 × f32, 64 bytes — exactly four `vec4f`s in a shader, with
/// no field straddling a vec4 boundary):
///
///   vec4 0  [0..=3]   = (kind_tag, center.x, center.y, center.z)
///   vec4 1  [4..=7]   = (param0, param1, param2, op_tag)
///   vec4 2  [8..=11]  = (translate.x, translate.y, translate.z, scale)
///   vec4 3  [12..=15] = (quat.x, quat.y, quat.z, quat.w)
///
/// Primitive parameters per kind:
///   box      → param = half-extents xyz
///   sphere   → param = (radius, 0, 0)
///   cylinder → param = (radius, half_height, 0)
///
/// Op tags: 0 = union, 1 = difference, 2 = intersect.
/// Kind tags: 0 = box, 1 = sphere, 2 = cylinder.
pub const FEATURE_STRIDE: usize = 16;

/// Maximum undo / redo depth. The whole document is snapshotted per
/// edit, so depth × document-size ≈ memory cost. For M2 (small linear
/// feature lists) 64 is plenty without being painful.
const HISTORY_DEPTH: usize = 64;

// ─── Pure-Rust SDF evaluation (used by tests and the future CPU mesher) ──

/// Rotate a vector by the conjugate of a unit quaternion `(x, y, z, w)`.
/// This is the inverse rotation of `q`, used to map a world-space point
/// into the primitive's local frame.
fn quat_inverse_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    // For unit q, q⁻¹ = (-x, -y, -z, w). We then apply the standard
    // p' = q⁻¹ * p * q rotation, optimised via the cross-product form.
    let qx = -q[0];
    let qy = -q[1];
    let qz = -q[2];
    let qw = q[3];
    let t = [
        2.0 * (qy * v[2] - qz * v[1]),
        2.0 * (qz * v[0] - qx * v[2]),
        2.0 * (qx * v[1] - qy * v[0]),
    ];
    [
        v[0] + qw * t[0] + qy * t[2] - qz * t[1],
        v[1] + qw * t[1] + qz * t[0] - qx * t[2],
        v[2] + qw * t[2] + qx * t[1] - qy * t[0],
    ]
}

/// Hamilton product of two unit quaternions, both in the project's
/// `(x, y, z, w)` storage order. Used when collapsing chains of
/// `CsgNode::Transform` into a single accumulated world transform at
/// each BVH leaf (see `bvh.rs`).
pub fn quat_multiply(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// Compose two affine transforms into one that applies `child` first,
/// then `parent`: the resulting transform is equivalent to the chain
/// `parent ∘ child`. Used to fold nested `CsgNode::Transform` nodes
/// into each leaf's accumulated world transform.
pub fn compose_transforms(parent: &Transform, child: &Transform) -> Transform {
    let scaled_child_t = [
        child.translate[0] * parent.scale,
        child.translate[1] * parent.scale,
        child.translate[2] * parent.scale,
    ];
    let rotated = quat_rotate(parent.rotation, scaled_child_t);
    Transform {
        translate: [
            parent.translate[0] + rotated[0],
            parent.translate[1] + rotated[1],
            parent.translate[2] + rotated[2],
        ],
        rotation: quat_multiply(parent.rotation, child.rotation),
        scale: parent.scale * child.scale,
    }
}

/// Rotate a vector by a unit quaternion `(x, y, z, w)`. Counterpart to
/// [`quat_inverse_rotate`] — this one applies the rotation itself,
/// used when mapping an already-local point (or an AABB corner) out to
/// world space.
pub fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let qx = q[0];
    let qy = q[1];
    let qz = q[2];
    let qw = q[3];
    let t = [
        2.0 * (qy * v[2] - qz * v[1]),
        2.0 * (qz * v[0] - qx * v[2]),
        2.0 * (qx * v[1] - qy * v[0]),
    ];
    [
        v[0] + qw * t[0] + qy * t[2] - qz * t[1],
        v[1] + qw * t[1] + qz * t[0] - qx * t[2],
        v[2] + qw * t[2] + qx * t[1] - qy * t[0],
    ]
}

/// Bring a world-space point into a feature's primitive-local frame
/// using its transform: untranslate, unrotate, unscale.
pub fn world_to_local(t: &Transform, p_world: [f32; 3]) -> [f32; 3] {
    let p = [
        p_world[0] - t.translate[0],
        p_world[1] - t.translate[1],
        p_world[2] - t.translate[2],
    ];
    let p = quat_inverse_rotate(t.rotation, p);
    let inv = 1.0 / t.scale;
    [p[0] * inv, p[1] * inv, p[2] * inv]
}

/// SDF of a single primitive at world point `p`.
pub fn sdf_primitive(primitive: &Primitive, p: [f32; 3]) -> f32 {
    match primitive {
        Primitive::Box { center, half_extents } => {
            let q = [
                (p[0] - center[0]).abs() - half_extents[0],
                (p[1] - center[1]).abs() - half_extents[1],
                (p[2] - center[2]).abs() - half_extents[2],
            ];
            let outside =
                (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2) + q[2].max(0.0).powi(2)).sqrt();
            let inside = q[0].max(q[1]).max(q[2]).min(0.0);
            outside + inside
        }
        Primitive::Sphere { center, radius } => {
            let dx = p[0] - center[0];
            let dy = p[1] - center[1];
            let dz = p[2] - center[2];
            (dx * dx + dy * dy + dz * dz).sqrt() - *radius
        }
        Primitive::Cylinder { center, radius, half_height } => {
            let dx = p[0] - center[0];
            let dy = p[1] - center[1];
            let dz = p[2] - center[2];
            let radial = (dx * dx + dz * dz).sqrt() - *radius;
            let axial = dy.abs() - *half_height;
            let outside = radial.max(0.0).hypot(axial.max(0.0));
            let inside = radial.max(axial).min(0.0);
            outside + inside
        }
        // K7.3: lift the 2D sketch SDF into 3D via IQ's extrude
        // composition — `(d_2d, |z| - half_depth_z)` treated as
        // a 2D box SDF. Matches what `CsgNode::Extrude` does at
        // the mesher level; duplicated here so the
        // non-CSG-tree `sdf_primitive` path (picking fallback)
        // still returns a sensible distance.
        Primitive::Extrude {
            sketch,
            half_depth_z,
            center,
        } => {
            let local_xy = [p[0] - center[0], p[1] - center[1]];
            let d_xy = sketch.sdf(local_xy);
            let d_z = (p[2] - center[2]).abs() - *half_depth_z;
            let outside = d_xy.max(0.0).hypot(d_z.max(0.0));
            let inside = d_xy.max(d_z).min(0.0);
            outside + inside
        }
        // K8.4: 2D sketch revolved around the local Y axis.
        // Map (x, y, z) to (r, y) where r = √((x - cx)² + (z - cz)²),
        // then evaluate the 2D sketch SDF. Mirrors
        // `CsgNode::Revolve::sdf` so the non-CSG-tree path
        // (picking fallback) returns a sensible distance.
        Primitive::Revolve { sketch, center } => {
            let dx = p[0] - center[0];
            let dz = p[2] - center[2];
            let r = (dx * dx + dz * dz).sqrt();
            sketch.sdf([r, p[1] - center[1]])
        }
        Primitive::Torus { center, major_radius, minor_radius } => {
            let dx = p[0] - center[0];
            let dy = p[1] - center[1];
            let dz = p[2] - center[2];
            let q = ((dx * dx + dz * dz).sqrt() - major_radius, dy);
            (q.0 * q.0 + q.1 * q.1).sqrt() - minor_radius
        }
        Primitive::Gyroid { cell_size, thickness } => {
            let s = *cell_size;
            let x = p[0] / s;
            let y = p[1] / s;
            let z = p[2] / s;
            let f = x.sin() * y.cos() + y.sin() * z.cos() + z.sin() * x.cos();
            f.abs() - *thickness
        }
        Primitive::Thread { nominal_radius, pitch, depth, half_height } => {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let theta = p[2].atan2(p[0]);
            // Unwrap helix: map (y, theta) to a linear phase
            let y_unwrapped = p[1] - *pitch * theta / std::f32::consts::TAU;
            // Distance to nearest thread crest (periodic)
            let phase = ((y_unwrapped / pitch).rem_euclid(1.0) - 0.5).abs() * pitch;
            // Axial bounds
            let d_axial = p[1].abs() - *half_height;
            // Cylinder surface + thread groove
            let d_radial = r - *nominal_radius;
            let d_groove = phase - *depth;
            // Thread = cylinder intersected with groove pattern, bounded axially
            let d_thread = d_radial.max(-d_groove);
            d_thread.max(d_axial)
        }
        Primitive::Cone { r_bottom, r_top, half_height } => {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let hh = *half_height;
            // Interpolated radius at p.y
            let t = ((p[1] + hh) / (2.0 * hh)).clamp(0.0, 1.0);
            let r_at_y = r_bottom * (1.0 - t) + r_top * t;
            // 2D distance to the trapezoid cross-section in (r, y) space
            let d_radial = r - r_at_y;
            let d_axial = p[1].abs() - hh;
            if d_radial > 0.0 && d_axial > 0.0 {
                (d_radial * d_radial + d_axial * d_axial).sqrt()
            } else {
                d_radial.max(d_axial)
            }
        }
        Primitive::Capsule { radius, half_height } => {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let y_clamped = p[1].clamp(-*half_height, *half_height);
            let dy = p[1] - y_clamped;
            (r * r + dy * dy).sqrt() - *radius
        }
        Primitive::HexPrism { apothem, half_height } => {
            // 2D hex distance in XZ plane.
            let qx = p[0].abs();
            let qz = p[2].abs();
            // Hex: max(qx * sqrt(3)/2 + qz * 0.5, qz) - apothem
            let d_hex = (qx * 0.866_025_4 + qz * 0.5).max(qz) - *apothem;
            let d_axial = p[1].abs() - *half_height;
            if d_hex > 0.0 && d_axial > 0.0 {
                (d_hex * d_hex + d_axial * d_axial).sqrt()
            } else {
                d_hex.max(d_axial)
            }
        }
        Primitive::ImportedMesh { mesh } => mesh.sdf(p),
    }
}

/// Apply one boolean op to the running accumulator.
pub fn apply_op(acc: f32, di: f32, op: BoolOp) -> f32 {
    match op {
        BoolOp::Union => acc.min(di),
        // SDF subtraction: max(a, -b). Pre-empty space (acc = +∞) means
        // subtracting first does nothing, which matches Onshape's "the
        // first feature must be additive" rule.
        BoolOp::Difference => acc.max(-di),
        BoolOp::Intersect => acc.max(di),
    }
}

/// SDF of one transformed feature at a world-space point.
pub fn sdf_feature(feature: &Feature, p_world: [f32; 3]) -> f32 {
    let p_local = world_to_local(&feature.transform, p_world);
    // Multiplying by the uniform scale restores the world-space distance
    // metric — without it the SDF would report distances in the
    // primitive's pre-scale frame.
    sdf_primitive(&feature.primitive, p_local) * feature.transform.scale
}

/// Composite SDF of an entire feature spine.
pub fn sdf_features(features: &[Feature], p: [f32; 3]) -> f32 {
    let mut acc = f32::INFINITY;
    for feature in features {
        let di = sdf_feature(feature, p);
        acc = apply_op(acc, di, feature.op);
    }
    acc
}

/// Pack one feature into a 16-float slot of the renderer buffer.
pub fn pack_feature(feature: &Feature, out: &mut [f32; FEATURE_STRIDE]) {
    *out = [0.0; FEATURE_STRIDE];
    out[0] = feature.primitive.kind_tag() as f32;
    match &feature.primitive {
        Primitive::Box { center, half_extents } => {
            out[1..4].copy_from_slice(center);
            out[4..7].copy_from_slice(half_extents);
        }
        Primitive::Sphere { center, radius } => {
            out[1..4].copy_from_slice(center);
            out[4] = *radius;
        }
        Primitive::Cylinder { center, radius, half_height } => {
            out[1..4].copy_from_slice(center);
            out[4] = *radius;
            out[5] = *half_height;
        }
        // K7.3: the old TSL raymarcher has no concept of
        // sketch extrusions. Pack the AABB extents as a proxy
        // box so the retired renderer at least draws something
        // at the right place if it ever gets reactivated.
        Primitive::Extrude {
            sketch,
            half_depth_z,
            center,
        } => {
            let bb2 = sketch.aabb();
            let half_x = if bb2.is_empty() {
                0.0
            } else {
                0.5 * (bb2.max[0] - bb2.min[0])
            };
            let half_y = if bb2.is_empty() {
                0.0
            } else {
                0.5 * (bb2.max[1] - bb2.min[1])
            };
            out[1..4].copy_from_slice(center);
            out[4] = half_x;
            out[5] = half_y;
            out[6] = *half_depth_z;
        }
        // K8.4: same story as Extrude — the retired TSL
        // raymarcher has no concept of revolves. Pack a proxy
        // cylinder with the sketch's radial and vertical extents.
        Primitive::Revolve { sketch, center } => {
            let bb2 = sketch.aabb();
            let (r_max, half_y) = if bb2.is_empty() {
                (0.0, 0.0)
            } else {
                (
                    bb2.max[0].abs().max(bb2.min[0].abs()),
                    0.5 * (bb2.max[1] - bb2.min[1]),
                )
            };
            out[1..4].copy_from_slice(center);
            out[4] = r_max;
            out[5] = half_y;
        }
        Primitive::Torus { center, major_radius, minor_radius } => {
            out[1..4].copy_from_slice(center);
            out[4] = *major_radius;
            out[5] = *minor_radius;
        }
        Primitive::Gyroid { cell_size, thickness } => {
            out[4] = *cell_size;
            out[5] = *thickness;
        }
        Primitive::Thread { nominal_radius, pitch, depth, half_height } => {
            out[4] = *nominal_radius;
            out[5] = *pitch;
            out[6] = *depth;
            out[7] = *half_height;
        }
        Primitive::Cone { r_bottom, r_top, half_height } => {
            out[4] = *r_bottom;
            out[5] = *r_top;
            out[6] = *half_height;
        }
        Primitive::Capsule { radius, half_height } => {
            out[4] = *radius;
            out[5] = *half_height;
        }
        Primitive::HexPrism { apothem, half_height } => {
            out[4] = *apothem;
            out[5] = *half_height;
        }
        Primitive::ImportedMesh { mesh } => {
            for i in 0..3 {
                out[1 + i] = (mesh.aabb_min[i] + mesh.aabb_max[i]) * 0.5;
            }
        }
    }
    out[7] = feature.op.tag() as f32;

    let t = &feature.transform;
    out[8..11].copy_from_slice(&t.translate);
    out[11] = t.scale;
    out[12..16].copy_from_slice(&t.rotation);
}

/// Pack an entire feature list to a flat `Vec<f32>`.
pub fn pack_features(features: &[Feature]) -> Vec<f32> {
    let mut buf = vec![0.0_f32; features.len() * FEATURE_STRIDE];
    for (i, feature) in features.iter().enumerate() {
        let slot: &mut [f32; FEATURE_STRIDE] =
            (&mut buf[i * FEATURE_STRIDE..(i + 1) * FEATURE_STRIDE])
                .try_into()
                .unwrap();
        pack_feature(feature, slot);
    }
    buf
}

// ─── wasm-bindgen surface ────────────────────────────────────────────────

/// Compact JS view of one feature for the outliner.
#[derive(serde::Serialize)]
struct FeatureSummary {
    id: FeatureId,
    name: String,
    op: &'static str,
    kind: &'static str,
    translate: [f32; 3],
    scale: f32,
    /// K8.7: mirrors `FeatureCall.suppressed`. The outliner
    /// reads this to render the eye-icon affordance and to
    /// grey out the row when the feature is skipped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    suppressed: bool,
}

/// Owned by the JS side. Holds the canonical document, the packed
/// renderer buffer, and the undo/redo stacks. The packed buffer's
/// pointer is stable until any mutation that may grow the underlying
/// `Vec` (which can move it on reallocation) — JS callers must
/// re-acquire `primitivesPtr()` after every mutating call.
#[wasm_bindgen]
pub struct Kernel {
    document: Document,
    packed: Vec<f32>,
    undo: Vec<Document>,
    redo: Vec<Document>,
    /// Multi-body mesh cache (K3 part 2). Each body has its own
    /// feature subset, BVH and mesh buffer. Mutations that only
    /// touch body `A` leave body `B`'s mesh — and its per-body
    /// `version` counter — untouched.
    bodies: Vec<Body>,
    /// Voxel grid resolution for the currently-edited body (the
    /// active body). Defaults to [`DEFAULT_MESH_RESOLUTION`]. The
    /// legacy `mesh_resolution()` / `setMeshResolution()` API maps
    /// onto this field so existing tests keep working.
    active_mesh_resolution: u32,
    /// Voxel grid resolution for bodies that aren't currently being
    /// edited. Defaults to [`DEFAULT_CONTEXT_MESH_RESOLUTION`] —
    /// half the active resolution, ≈ 8× fewer cells = a meaningful
    /// speedup on every re-mesh that touches a context body.
    context_mesh_resolution: u32,
    /// Id of the body the user is currently editing, if any. `None`
    /// disables LOD entirely: every body is treated as active and
    /// uses `active_mesh_resolution`. When a body id is set, only
    /// that body stays at active resolution; every other body
    /// drops to the context resolution.
    active_body_id: Option<FeatureId>,
    /// When `false`, `rebuild_mesh_internal` still partitions the
    /// feature spine into bodies but skips the Dual Contouring
    /// pass, leaving every body with an empty mesh buffer. The
    /// frontend flips this off at startup so the main-thread
    /// kernel only maintains document state (and the CSG tree
    /// used by `pick`), while a dedicated Web Worker owns a
    /// second kernel instance that actually does the meshing and
    /// streams the result back through the store.
    ///
    /// Default `true` so every Rust test keeps producing real
    /// meshes without needing to opt in.
    sync_meshing: bool,
    /// Global version counter bumped on every mesh rebuild (any body
    /// touched). Frontend uses this to detect "anything changed";
    /// per-body freshness is tracked by each `Body`'s own `version`.
    mesh_version: u32,
    /// K10.4: edge fillets collected from FilletEdgeFeature calls
    /// during the last regenerate pass. Applied as a post-processing
    /// step on the CsgNode tree before meshing.
    edge_fillets: Vec<csg::EdgeFillet>,
}

impl Kernel {
    fn snapshot(&mut self) {
        self.undo.push(self.document.clone());
        if self.undo.len() > HISTORY_DEPTH {
            // Keep only the most recent HISTORY_DEPTH snapshots.
            let drop = self.undo.len() - HISTORY_DEPTH;
            self.undo.drain(0..drop);
        }
        self.redo.clear();
    }

    fn rebuild_packed(&mut self) {
        self.packed = pack_features(&self.document.features);
        // K4.5.4c dual-write: rebuild the K4.5 Feature-as-Function
        // `feature_calls` mirror after every mutation that ran
        // through `rebuild_packed`. Every public mutator funnels
        // through this call (add / remove / nudge /
        // set_feature_{translate,transform,op} / reorder / undo /
        // redo / load_json), so the invariant "`features` and
        // `feature_calls` are in sync after any mutation returns"
        // holds automatically without instrumenting each path.
        self.document.sync_feature_calls();
        self.rebuild_mesh_internal();
    }

    /// Re-mesh the current document at the current resolution and
    /// bump `mesh_version`. Called by `rebuild_packed` (after every
    /// mutation) and by the explicit `setMeshResolution` API.
    ///
    /// K3 part 2: the pipeline is `features → partition → bodies`,
    /// and each body only gets re-meshed when its payload hash
    /// actually changed. Bodies whose seed feature disappeared are
    /// dropped; bodies whose hash matches an existing one keep their
    /// cached mesh verbatim.
    ///
    /// K3 part 4 adds per-body LOD on top: each body picks between
    /// `active_mesh_resolution` and `context_mesh_resolution` based
    /// on whether its id matches `active_body_id`. When no active
    /// body is set, every body runs at active resolution (backward
    /// compatible with the K3 part 1–3 behaviour). The cache reuse
    /// check compares both the payload hash *and* the resolution
    /// the cached mesh was built at, so switching the active body
    /// correctly re-meshes the two bodies that changed LOD and
    /// leaves the rest untouched.
    fn rebuild_mesh_internal(&mut self) {
        let partitions = partition(&self.document.features);

        // Build a lookup of existing bodies keyed by stable id so we
        // can tell which partitions just shuffled vs. which actually
        // changed. `std::mem::take` lets us move meshes out of the
        // old Vec without cloning their potentially-large buffers.
        let mut previous: std::collections::HashMap<FeatureId, Body> =
            std::mem::take(&mut self.bodies)
                .into_iter()
                .map(|b| (b.id.clone(), b))
                .collect();

        let mut new_bodies: Vec<Body> = Vec::with_capacity(partitions.len());
        for part in partitions {
            let id = part.seed_id(&self.document.features);
            let payload_hash =
                hash_body_payload(&self.document.features, &part.feature_indices);
            let target_resolution = self.target_resolution_for(&id);

            if let Some(mut prev) = previous.remove(&id) {
                if prev.payload_hash == payload_hash
                    && prev.mesh_resolution == target_resolution
                {
                    // Identical payload and LOD → reuse the cached
                    // mesh as-is. Feature indices may have shifted
                    // because other bodies were removed around it;
                    // refresh the bookkeeping without touching the
                    // mesh buffers or the per-body `version`.
                    prev.feature_indices = part.feature_indices;
                    prev.aabb = part.aabb;
                    new_bodies.push(prev);
                    continue;
                }
                // Either the payload *or* the LOD changed → re-mesh
                // and bump this body's version counter.
                let mesh = self.build_mesh_for(&part.feature_indices, target_resolution);
                prev.feature_indices = part.feature_indices;
                prev.aabb = part.aabb;
                prev.payload_hash = payload_hash;
                prev.mesh_resolution = target_resolution;
                prev.mesh = mesh;
                prev.version = prev.version.wrapping_add(1);
                new_bodies.push(prev);
            } else {
                // Fresh body — mesh from scratch at the target LOD.
                let mesh = self.build_mesh_for(&part.feature_indices, target_resolution);
                new_bodies.push(Body {
                    id,
                    feature_indices: part.feature_indices,
                    aabb: part.aabb,
                    payload_hash,
                    mesh_resolution: target_resolution,
                    mesh,
                    version: 1,
                });
            }
        }
        // Anything left in `previous` corresponds to bodies that no
        // longer exist (their seed was removed / merged). They're
        // dropped along with their meshes automatically when the
        // map goes out of scope.

        self.bodies = new_bodies;
        self.mesh_version = self.mesh_version.wrapping_add(1);
    }

    /// Build a mesh for a body's feature subset at the requested
    /// resolution — or short-circuit to an empty `Mesh` when
    /// `sync_meshing` is off, so the main-thread kernel stops
    /// paying the Dual Contouring cost while the web worker
    /// handles it out-of-process.
    fn build_mesh_for(&self, feature_indices: &[usize], resolution: u32) -> Mesh {
        if !self.sync_meshing {
            return Mesh::default();
        }
        let bounded = build_bounded_for_body(&self.document.features, feature_indices);
        dual_contour(&bounded, resolution)
    }

    /// Pick the voxel grid resolution a body should use right now:
    /// the active resolution when no LOD hint is set or when the
    /// body id matches `active_body_id`, the context resolution
    /// otherwise.
    fn target_resolution_for(&self, body_id: &FeatureId) -> u32 {
        match &self.active_body_id {
            Some(active_id) if active_id != body_id => self.context_mesh_resolution,
            _ => self.active_mesh_resolution,
        }
    }

    /// Internal active-body setter that works directly on
    /// `FeatureId` values (no wasm-bindgen u32 shim). All three
    /// public entry points — [`Kernel::set_active_body_id`],
    /// [`Kernel::set_active_body_by_feature`] and
    /// [`Kernel::clear_active_body_id`] — go through here so the
    /// rebuild / no-op rules live in exactly one place.
    fn set_active_body_id_internal(&mut self, new_active: Option<FeatureId>) -> bool {
        if new_active == self.active_body_id {
            return false;
        }
        self.active_body_id = new_active;
        self.rebuild_mesh_internal();
        true
    }

    /// Walk the current bodies and return the stable id of the one
    /// that owns `feature_id`, if any. O(bodies × features_per_body).
    fn body_containing_feature(&self, feature_id: &FeatureId) -> Option<FeatureId> {
        // Translate the FeatureId to an index into the document's
        // feature list first, then search the bodies for that index.
        let idx = self
            .document
            .features
            .iter()
            .position(|f| &f.id == feature_id)?;
        self.bodies
            .iter()
            .find(|b| b.feature_indices.iter().any(|&i| i == idx))
            .map(|b| b.id.clone())
    }

    fn op_label(op: BoolOp) -> &'static str {
        match op {
            BoolOp::Union => "union",
            BoolOp::Difference => "difference",
            BoolOp::Intersect => "intersect",
        }
    }

    fn kind_label(prim: &Primitive) -> &'static str {
        match prim {
            Primitive::Box { .. } => "box",
            Primitive::Sphere { .. } => "sphere",
            Primitive::Cylinder { .. } => "cylinder",
            Primitive::Extrude { .. } => "extrude",
            Primitive::Revolve { .. } => "revolve",
            Primitive::Gyroid { .. } => "gyroid",
            Primitive::Torus { .. } => "torus",
            Primitive::Thread { .. } => "thread",
            Primitive::Cone { .. } => "cone",
            Primitive::Capsule { .. } => "capsule",
            Primitive::HexPrism { .. } => "hex_prism",
            Primitive::ImportedMesh { .. } => "imported_mesh",
        }
    }

    /// K6b.3a: for a primitive-backed call the spine primitive is
    /// authoritative, but future non-primitive spine-backed
    /// features (like extrude, which currently lowers to a
    /// `Primitive::Box` placeholder) want the original call type
    /// to leak through to the UI. Prefer the call's `type_name`
    /// for anything that isn't a plain primitive kind.
    fn kind_label_for_call(
        type_name: &str,
        prim: &Primitive,
    ) -> &'static str {
        match type_name {
            "box" | "sphere" | "cylinder" | "torus" | "thread" | "cone" | "capsule" | "hex_prism" => Self::kind_label(prim),
            "imported_mesh" => "imported_mesh",
            "extrude" => "extrude",
            // K8.9: hole lowers to Primitive::Cylinder but must
            // keep its own outliner label so the row renders
            // "Hole" glyphs/colours instead of the cylinder one.
            "hole" => "hole",
            "gyroid" => "gyroid",
            "fillet_edge" => "fillet_edge",
            _ => Self::static_kind_name(type_name),
        }
    }

    fn static_kind_name(type_name: &str) -> &'static str {
        match type_name {
            "sketch" => "sketch",
            "box" => "box",
            "sphere" => "sphere",
            "cylinder" => "cylinder",
            "extrude" => "extrude",
            "revolve" => "revolve",
            "hole" => "hole",
            "gyroid" => "gyroid",
            "imported_mesh" => "imported_mesh",
            "fillet_edge" => "fillet_edge",
            "linear_pattern" => "linear_pattern",
            "circular_pattern" => "circular_pattern",
            "mirror" => "mirror",
            _ => "unknown",
        }
    }

    /// K6b.3a: synthesise a display name for a feature call that
    /// doesn't match a spine entry. `"Sketch 3"` for a sketch
    /// call with id 3, etc. Primitive calls never land here
    /// because they always have a spine entry.
    fn synth_name_for_call(
        call: &fielddraft_protocol::FeatureCall,
    ) -> String {
        // K8.5: honour a user-set `name` in the call's params
        // so rename through `replaceFeatureCallParams` sticks
        // for feature types that don't have a spine-level
        // `SpineFeature.name` (sketches, patterns).
        if let Some(name) = call.params.get("name").and_then(|v| v.as_str()) {
            if !name.is_empty() {
                return name.to_string();
            }
        }
        let label = match call.type_name.as_str() {
            "sketch" => "Sketch",
            "revolve" => "Revolve",
            "hole" => "Hole",
            "gyroid" => "Gyroid",
            "imported_mesh" => "Imported mesh",
            "fillet_edge" => "Fillet edge",
            "linear_pattern" => "Linear pattern",
            "circular_pattern" => "Circular pattern",
            "mirror" => "Mirror",
            other => other,
        };
        format!("{label} {}", call.id)
    }

    fn parse_op(tag: u32) -> Result<BoolOp, JsError> {
        match tag {
            BoolOp::TAG_UNION => Ok(BoolOp::Union),
            BoolOp::TAG_DIFFERENCE => Ok(BoolOp::Difference),
            BoolOp::TAG_INTERSECT => Ok(BoolOp::Intersect),
            other => Err(JsError::new(&format!("invalid op tag: {other}"))),
        }
    }
}

#[wasm_bindgen]
impl Kernel {
    /// Create a new kernel with an empty document.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        console_error_panic_hook::set_once();

        let document = Document::default();
        let packed = pack_features(&document.features);
        let mut k = Self {
            document,
            packed,
            undo: Vec::new(),
            redo: Vec::new(),
            bodies: Vec::new(),
            active_mesh_resolution: DEFAULT_MESH_RESOLUTION,
            context_mesh_resolution: DEFAULT_CONTEXT_MESH_RESOLUTION,
            active_body_id: None,
            sync_meshing: true,
            mesh_version: 0,
            edge_fillets: Vec::new(),
        };
        // Seed the mesh buffer (empty document → empty mesh, but we
        // still want the version counter and buffers initialised).
        k.rebuild_mesh_internal();
        k
    }

    // ── document I/O ──────────────────────────────────────────────────

    #[wasm_bindgen(js_name = loadJson)]
    pub fn load_json(&mut self, json: &str) -> Result<usize, JsError> {
        let document: Document =
            serde_json::from_str(json).map_err(|e| JsError::new(&e.to_string()))?;
        self.document = document;

        // K4.5.4d: if the loaded document uses the Feature-as-
        // Function wire form (`feature_calls` populated, `features`
        // empty), bootstrap the spine by running the calls through
        // the feature registry. Legacy K1 documents with populated
        // `features` keep their current loading behaviour
        // unchanged — the K4.5.4c dual-write hook in
        // `rebuild_packed` below still derives `feature_calls`
        // from `features` in that case. Any error from the
        // registry walk is surfaced as a load failure (fail-stop
        // semantics per §4.6).
        if self.document.features.is_empty()
            && !self.document.feature_calls.is_empty()
        {
            self.bootstrap_features_from_calls()
                .map_err(|e| JsError::new(&format!("feature regeneration failed: {e}")))?;
        }

        self.rebuild_packed();
        // Loading is a fresh start: history is irrelevant.
        self.undo.clear();
        self.redo.clear();
        Ok(self.document.features.len())
    }

    /// K4.5.4d internal helper. Takes the current
    /// `document.feature_calls` list, clears `document.features`,
    /// and regenerates the spine by walking each call through a
    /// fresh `FeatureRegistry::with_builtins()`. Used by
    /// [`Kernel::load_json`] when the loaded document uses the
    /// step-4d wire shape. Fail-stop per §4.6: the first
    /// registry error halts the walk and bubbles up as a
    /// `FeatureError`.
    ///
    /// The call list is temporarily `mem::take`n out of the
    /// document so the registry's mutable borrow can point at
    /// `self.document` without aliasing the call list itself;
    /// it's restored before the function returns. The subsequent
    /// `rebuild_packed` call (from `load_json`) overwrites it
    /// again via `sync_feature_calls`, but leaving the restore
    /// in place keeps this helper independent of its caller's
    /// sequencing.
    fn bootstrap_features_from_calls(&mut self) -> Result<(), features::FeatureError> {
        let registry = features::FeatureRegistry::with_builtins();
        let calls = std::mem::take(&mut self.document.feature_calls);
        self.document.features.clear();
        let result = {
            let mut ctx = features::Context::new(&mut self.document);
            let r = registry.regenerate(&mut ctx, &calls);
            // K10.4: capture edge fillets collected during regen.
            self.edge_fillets = ctx.edge_fillets;
            r
        };
        self.document.feature_calls = calls;
        result
    }

    #[wasm_bindgen(js_name = toJson)]
    pub fn to_json(&self) -> Result<String, JsError> {
        serde_json::to_string(&self.document).map_err(|e| JsError::new(&e.to_string()))
    }

    // ── packed buffer (zero-copy bridge) ──────────────────────────────

    #[wasm_bindgen(js_name = primitivesPtr)]
    pub fn primitives_ptr(&self) -> *const f32 {
        self.packed.as_ptr()
    }

    #[wasm_bindgen(js_name = featureCount)]
    pub fn feature_count(&self) -> usize {
        self.document.features.len()
    }

    #[wasm_bindgen(js_name = featureStride)]
    pub fn feature_stride() -> usize {
        FEATURE_STRIDE
    }

    // ── outliner queries ──────────────────────────────────────────────

    // ── K6a feature-schema lookup ────────────────────────────────────
    //
    // The FeatureDialog generator on the TypeScript side consumes
    // the JSON schema that the K4.5.4a `FeatureSchema` derive
    // emits for every definition struct. These two accessors
    // walk the built-in `FeatureRegistry` to expose the schema
    // and the list of known types to the wasm-bindgen surface;
    // the schemas are serialised as strings so wasm-bindgen
    // doesn't need to learn about `serde_json::Value`.

    /// K6a: list every feature type name the built-in registry
    /// knows about, sorted for deterministic UI rendering. The
    /// result is a JSON array of strings, e.g. `["box", "cylinder",
    /// "extrude", "sphere"]`.
    #[wasm_bindgen(js_name = listFeatureTypes)]
    pub fn list_feature_types(&self) -> Result<String, JsError> {
        let registry = features::FeatureRegistry::with_builtins();
        let mut names = registry.type_names();
        names.sort();
        serde_json::to_string(&names).map_err(|e| JsError::new(&e.to_string()))
    }

    /// K6a: look up the FeatureSchema-generated JSON schema for
    /// a feature type. Returns the serialised JSON string or an
    /// error if the type isn't registered.
    #[wasm_bindgen(js_name = schemaForType)]
    pub fn schema_for_type(&self, type_name: &str) -> Result<String, JsError> {
        let registry = features::FeatureRegistry::with_builtins();
        let value = registry
            .schema_for(type_name)
            .ok_or_else(|| JsError::new(&format!("unknown feature type `{type_name}`")))?;
        serde_json::to_string(&value).map_err(|e| JsError::new(&e.to_string()))
    }

    /// K6b.2: list every sketch-tool type name the kernel knows
    /// about. Sketch tools live in a separate
    /// [`sketch_features::SketchToolRegistry`] that the UI's
    /// sketch-mode toolbar queries to decide which tool buttons
    /// to render. JSON array of strings, sorted alphabetically
    /// for deterministic rendering.
    #[wasm_bindgen(js_name = listSketchToolTypes)]
    pub fn list_sketch_tool_types(&self) -> Result<String, JsError> {
        let tools = sketch_features::SketchToolRegistry::with_builtins();
        let mut names = tools.type_names();
        names.sort();
        serde_json::to_string(&names).map_err(|e| JsError::new(&e.to_string()))
    }

    /// K6b.2: look up the FeatureSchema-generated JSON schema for
    /// a sketch tool. Same contract as [`schema_for_type`] but
    /// routes through the sketch-tool registry so the disjoint
    /// namespaces stay disjoint.
    #[wasm_bindgen(js_name = schemaForSketchToolType)]
    pub fn schema_for_sketch_tool_type(
        &self,
        type_name: &str,
    ) -> Result<String, JsError> {
        let tools = sketch_features::SketchToolRegistry::with_builtins();
        let value = tools
            .schema_for(type_name)
            .ok_or_else(|| JsError::new(&format!("unknown sketch tool `{type_name}`")))?;
        serde_json::to_string(&value).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Return a JSON array of feature summaries (id, name, op, kind).
    ///
    /// K6b.3a: the list walks `feature_calls` — the K4.5 canonical
    /// wire form — instead of the spine directly. For primitive
    /// calls the summary is pulled from the matching spine entry
    /// (translate, scale, op are authoritative there). For
    /// non-spine calls like `sketch` the summary is synthesised
    /// from the call itself with neutral transform defaults, so
    /// the outliner sees a stable row per sketch even though
    /// sketches never push onto the spine.
    #[wasm_bindgen(js_name = listFeatures)]
    pub fn list_features(&self) -> Result<String, JsError> {
        let mut summaries: Vec<FeatureSummary> = Vec::with_capacity(
            self.document.feature_calls.len(),
        );
        for call in &self.document.feature_calls {
            if let Some(spine) = self
                .document
                .features
                .iter()
                .find(|f| f.id == call.id)
            {
                summaries.push(FeatureSummary {
                    id: spine.id.clone(),
                    name: spine.name.clone(),
                    op: Self::op_label(spine.op),
                    kind: Self::kind_label_for_call(&call.type_name, &spine.primitive),
                    translate: spine.transform.translate,
                    scale: spine.transform.scale,
                    suppressed: call.suppressed,
                });
            } else {
                // K8.9: hole pushes its spine entry under a
                // hierarchical child id (`<hole>/0`), so the
                // lookup above misses it. Surface the op that
                // matches the kind's known hard-coded semantics
                // here instead of defaulting to "union".
                let op = match call.type_name.as_str() {
                    "hole" => "difference",
                    "gyroid" => "intersect",
                    _ => "union",
                };
                // Try to pull the world-space translate from
                // the call's params so the outliner's ghost
                // meta badge shows the hole's real position.
                let translate = call
                    .params
                    .get("translate")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| {
                        if arr.len() != 3 {
                            return None;
                        }
                        let mut out = [0.0_f32; 3];
                        for (i, item) in arr.iter().enumerate() {
                            out[i] = item.as_f64()? as f32;
                        }
                        Some(out)
                    })
                    .unwrap_or([0.0, 0.0, 0.0]);
                summaries.push(FeatureSummary {
                    id: call.id.clone(),
                    name: Self::synth_name_for_call(call),
                    op,
                    kind: Self::static_kind_name(&call.type_name),
                    translate,
                    scale: 1.0,
                    suppressed: call.suppressed,
                });
            }
        }
        serde_json::to_string(&summaries).map_err(|e| JsError::new(&e.to_string()))
    }

    /// K6b.3a: commit a sketch from the UI's sub-spine draft. The
    /// caller hands in a plane query string plus a JSON array of
    /// `FeatureCall`s (the sketch-tool calls accumulated by
    /// `SketchSession.subCalls` in the TS store). The kernel
    /// pushes a `sketch`-type `FeatureCall` onto `feature_calls`,
    /// regenerates the document so the `sketch` feature's
    /// `execute` runs and stashes the solved `SketchState` in
    /// `Context::sketches`, and returns the allocated feature id.
    ///
    /// Regeneration uses the same `bootstrap_features_from_calls`
    /// path the load-from-JSON hook uses: clear `features`, walk
    /// `feature_calls`, let each registered `FeatureKind::execute`
    /// rebuild the spine as it goes. Primitive entries land in
    /// `features`, the new `sketch` entry contributes to the
    /// context-side `sketches` map. After the walk succeeds,
    /// `rebuild_packed` resyncs everything and the mesher sees
    /// the same spine it would have seen if the sketch had been
    /// there all along.
    #[wasm_bindgen(js_name = addSketch)]
    pub fn add_sketch(
        &mut self,
        plane_query: &str,
        sub_calls_json: &str,
    ) -> Result<u32, JsError> {
        let sub_calls: Vec<fielddraft_protocol::FeatureCall> =
            serde_json::from_str(sub_calls_json).map_err(|e| {
                JsError::new(&format!("invalid sub_calls JSON: {e}"))
            })?;

        self.snapshot();
        let id = self.document.allocate_id();
        let flat = id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");

        let params = serde_json::json!({
            "plane": plane_query,
            "sub_calls": sub_calls,
        });
        let call = fielddraft_protocol::FeatureCall::new(
            "sketch",
            id.clone(),
            params,
        );
        self.document.feature_calls.push(call);

        // Walk every feature call (primitives + the new sketch)
        // through the registry so the spine and context are
        // consistent. On failure we undo via the snapshot.
        if let Err(err) = self.bootstrap_features_from_calls() {
            // Undo: pop the feature id counter and the sketch call
            // we just pushed, then restore from the snapshot.
            self.undo();
            return Err(JsError::new(&format!(
                "sketch regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(flat)
    }

    /// K6b.3b: replace an existing sketch's plane and sub-spine
    /// in place, keeping its feature id stable. Used by the UI
    /// when the user re-enters a committed sketch to edit it —
    /// Finish should not create a new outliner row, it should
    /// update the existing one. Fails fast if the target id
    /// doesn't point at a sketch.
    #[wasm_bindgen(js_name = replaceSketch)]
    pub fn replace_sketch(
        &mut self,
        id: u32,
        plane_query: &str,
        sub_calls_json: &str,
    ) -> Result<(), JsError> {
        let target = FeatureId::from_u32(id);
        let sub_calls: Vec<fielddraft_protocol::FeatureCall> =
            serde_json::from_str(sub_calls_json).map_err(|e| {
                JsError::new(&format!("invalid sub_calls JSON: {e}"))
            })?;
        // Locate the sketch call.
        let idx = self
            .document
            .feature_calls
            .iter()
            .position(|c| c.id == target && c.type_name == "sketch")
            .ok_or_else(|| {
                JsError::new(&format!("feature {id} is not a sketch"))
            })?;

        self.snapshot();
        let params = serde_json::json!({
            "plane": plane_query,
            "sub_calls": sub_calls,
        });
        self.document.feature_calls[idx] = fielddraft_protocol::FeatureCall::new(
            "sketch",
            target,
            params,
        );
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "sketch regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(())
    }

    /// K7.4: append an `extrude` feature call that lowers the
    /// referenced sketch into a 3D body via the K7.3
    /// `Primitive::Extrude` pipeline. Convenience wrapper over
    /// the full FeatureCall shape so the UI doesn't have to
    /// hand-assemble the params JSON.
    ///
    /// `sketch_id` must point at an existing `sketch`
    /// FeatureCall. `depth_mm` is the total slab depth (the
    /// `Primitive::Extrude` half_depth_z becomes `depth_mm /
    /// 2`). `op` uses the same `BoolOp::TAG_*` tags as
    /// `add_primitive`. `tz` positions the extrude plane along
    /// the world Z axis.
    #[wasm_bindgen(js_name = addExtrude)]
    pub fn add_extrude(
        &mut self,
        sketch_id: u32,
        depth_mm: f32,
        op: u32,
        tz: f32,
    ) -> Result<u32, JsError> {
        let sketch_feature_id = FeatureId::from_u32(sketch_id);
        let exists = self
            .document
            .feature_calls
            .iter()
            .any(|c| c.id == sketch_feature_id && c.type_name == "sketch");
        if !exists {
            return Err(JsError::new(&format!(
                "feature {sketch_id} is not a sketch"
            )));
        }
        let bool_op = Self::parse_op(op)?;
        let op_wire = match bool_op {
            BoolOp::Union => "union",
            BoolOp::Difference => "difference",
            BoolOp::Intersect => "intersect",
        };

        self.snapshot();
        let id = self.document.allocate_id();
        let flat = id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");

        let params = serde_json::json!({
            "profiles": format!("qCreatedBy({sketch_feature_id})"),
            "depth": { "value": depth_mm, "unit": "mm" },
            "profile_half_x": 0.0,
            "profile_half_y": 0.0,
            "operation": op_wire,
            "center": [0.0, 0.0, tz],
        });
        let call = fielddraft_protocol::FeatureCall::new(
            "extrude",
            id.clone(),
            params,
        );
        self.document.feature_calls.push(call);

        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "extrude regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(flat)
    }

    /// K8.4: append a `revolve` feature call that consumes the
    /// sketch with id `sketch_id` and sweeps its profile around
    /// the local Y axis through `(cx, cy, cz)`. `op` uses the
    /// same `BoolOp::TAG_*` tags as `add_primitive`. Returns
    /// the new feature id.
    #[wasm_bindgen(js_name = addRevolve)]
    pub fn add_revolve(
        &mut self,
        sketch_id: u32,
        op: u32,
        cx: f32,
        cy: f32,
        cz: f32,
    ) -> Result<u32, JsError> {
        let sketch_feature_id = FeatureId::from_u32(sketch_id);
        let exists = self
            .document
            .feature_calls
            .iter()
            .any(|c| c.id == sketch_feature_id && c.type_name == "sketch");
        if !exists {
            return Err(JsError::new(&format!(
                "feature {sketch_id} is not a sketch"
            )));
        }
        let bool_op = Self::parse_op(op)?;
        let op_wire = match bool_op {
            BoolOp::Union => "union",
            BoolOp::Difference => "difference",
            BoolOp::Intersect => "intersect",
        };

        self.snapshot();
        let id = self.document.allocate_id();
        let flat = id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");

        let params = serde_json::json!({
            "profiles": format!("qCreatedBy({sketch_feature_id})"),
            "operation": op_wire,
            "center": [cx, cy, cz],
        });
        let call = fielddraft_protocol::FeatureCall::new(
            "revolve",
            id.clone(),
            params,
        );
        self.document.feature_calls.push(call);

        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "revolve regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(flat)
    }

    /// K8.9: append a `hole` feature call. Thin convenience
    /// wrapper that serialises the typed diameter/depth/translate
    /// into a FeatureCall and bootstraps. Returns the new id.
    #[wasm_bindgen(js_name = addHole)]
    pub fn add_hole(
        &mut self,
        diameter_mm: f32,
        depth_mm: f32,
        tx: f32,
        ty: f32,
        tz: f32,
    ) -> Result<u32, JsError> {
        self.snapshot();
        let id = self.document.allocate_id();
        let flat = id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");
        let params = serde_json::json!({
            "diameter": { "value": diameter_mm, "unit": "mm" },
            "depth": { "value": depth_mm, "unit": "mm" },
            "translate": [tx, ty, tz],
        });
        let call = fielddraft_protocol::FeatureCall::new("hole", id, params);
        self.document.feature_calls.push(call);
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "hole regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(flat)
    }

    /// K11.0: append a `gyroid` feature call. `cell_size_mm` is
    /// the cell period, `thickness_mm` the wall half-thickness.
    /// Default op is Intersect so the gyroid clips to any
    /// existing bodies. Returns the new id.
    #[wasm_bindgen(js_name = addGyroid)]
    pub fn add_gyroid(
        &mut self,
        cell_size_mm: f32,
        thickness_mm: f32,
        op: u32,
    ) -> Result<u32, JsError> {
        let bool_op = Self::parse_op(op)?;
        let op_wire = match bool_op {
            BoolOp::Union => "union",
            BoolOp::Difference => "difference",
            BoolOp::Intersect => "intersect",
        };
        self.snapshot();
        let id = self.document.allocate_id();
        let flat = id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");
        let params = serde_json::json!({
            "cell_size": { "value": cell_size_mm, "unit": "mm" },
            "thickness": { "value": thickness_mm, "unit": "mm" },
            "op": op_wire,
        });
        let call = fielddraft_protocol::FeatureCall::new("gyroid", id, params);
        self.document.feature_calls.push(call);
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "gyroid regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(flat)
    }

    /// K8.0: append a `linear_pattern` feature call that
    /// replicates the source feature along a direction
    /// K8.13: list the connected inside-regions of a sketch.
    /// Returns a JSON array of `{index, centroid, aabb, area}`
    /// objects. The sketch is identified by its feature id.
    #[wasm_bindgen(js_name = listSketchRegions)]
    pub fn list_sketch_regions(
        &self,
        sketch_id: u32,
        resolution: u32,
    ) -> Result<String, JsError> {
        let target = FeatureId::from_u32(sketch_id);
        let call = self
            .document
            .feature_calls
            .iter()
            .find(|c| c.id == target && c.type_name == "sketch")
            .ok_or_else(|| JsError::new(&format!("feature {sketch_id} is not a sketch")))?;

        let sub_calls: Vec<serde_json::Value> = call
            .params
            .get("sub_calls")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let sketch2d = crate::sketch_features::build_sketch2d_from_sub_calls_raw(&sub_calls);
        let regions = sketch_regions::find_regions(&sketch2d, resolution);
        serde_json::to_string(&regions).map_err(|e| JsError::new(&e.to_string()))
    }

    /// K13.1: import triangle data as a solid CSG leaf.
    #[wasm_bindgen(js_name = addImportedMesh)]
    pub fn add_imported_mesh(
        &mut self,
        positions: &[f32],
        indices: &[u32],
        op: u32,
    ) -> Result<u32, JsError> {
        let bool_op = Self::parse_op(op)?;
        let op_wire = match bool_op {
            BoolOp::Union => "union",
            BoolOp::Difference => "difference",
            BoolOp::Intersect => "intersect",
        };
        let mesh = fielddraft_protocol::MeshSdf::from_buffers(positions, indices);
        let mesh_json = serde_json::to_value(&mesh)
            .map_err(|e| JsError::new(&format!("mesh serialisation failed: {e}")))?;
        self.snapshot();
        let id = self.document.allocate_id();
        let flat = id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");
        let params = serde_json::json!({
            "op": op_wire,
            "__mesh": mesh_json,
        });
        let call = fielddraft_protocol::FeatureCall::new("imported_mesh", id, params);
        self.document.feature_calls.push(call);
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "imported mesh regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(flat)
    }

    /// vector. `source_id` must point at an existing spine
    /// feature; `count` is total instances including the
    /// source. Returns the allocated pattern feature id.
    #[wasm_bindgen(js_name = addLinearPattern)]
    pub fn add_linear_pattern(
        &mut self,
        source_id: u32,
        dx: f32,
        dy: f32,
        dz: f32,
        count: u32,
        spacing_mm: f32,
    ) -> Result<u32, JsError> {
        // Sanity-check the source exists on the spine now,
        // rather than at regen time — lets the UI surface a
        // clean error before any snapshot.
        let source_fid = FeatureId::from_u32(source_id);
        if !self.document.features.iter().any(|f| f.id == source_fid) {
            return Err(JsError::new(&format!(
                "source feature {source_id} not found on the spine"
            )));
        }

        self.snapshot();
        let pattern_id = self.document.allocate_id();
        let flat = pattern_id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");
        let params = serde_json::json!({
            "source": source_id,
            "direction": [dx, dy, dz],
            "count": count as f32,
            "spacing": { "value": spacing_mm, "unit": "mm" },
        });
        let call = fielddraft_protocol::FeatureCall::new(
            "linear_pattern",
            pattern_id,
            params,
        );
        self.document.feature_calls.push(call);
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "linear pattern regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(flat)
    }

    /// K8.1: append a `circular_pattern` feature call that
    /// replicates the source around an axis-through-center
    /// pivot. `angle_deg` is the full sweep (360° for a
    /// closed ring). Returns the new pattern feature id.
    #[wasm_bindgen(js_name = addCircularPattern)]
    pub fn add_circular_pattern(
        &mut self,
        source_id: u32,
        axis_x: f32,
        axis_y: f32,
        axis_z: f32,
        center_x: f32,
        center_y: f32,
        center_z: f32,
        count: u32,
        angle_deg: f32,
    ) -> Result<u32, JsError> {
        let source_fid = FeatureId::from_u32(source_id);
        if !self.document.features.iter().any(|f| f.id == source_fid) {
            return Err(JsError::new(&format!(
                "source feature {source_id} not found on the spine"
            )));
        }

        self.snapshot();
        let pattern_id = self.document.allocate_id();
        let flat = pattern_id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");
        let params = serde_json::json!({
            "source": source_id,
            "axis": [axis_x, axis_y, axis_z],
            "center": [center_x, center_y, center_z],
            "count": count as f32,
            "angle_deg": angle_deg,
        });
        let call = fielddraft_protocol::FeatureCall::new(
            "circular_pattern",
            pattern_id,
            params,
        );
        self.document.feature_calls.push(call);
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "circular pattern regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(flat)
    }

    /// K8.2: append a `mirror` feature call that reflects the
    /// source feature through a plane defined by a point and a
    /// normal. Returns the new mirror feature id.
    #[wasm_bindgen(js_name = addMirror)]
    pub fn add_mirror(
        &mut self,
        source_id: u32,
        normal_x: f32,
        normal_y: f32,
        normal_z: f32,
        point_x: f32,
        point_y: f32,
        point_z: f32,
    ) -> Result<u32, JsError> {
        let source_fid = FeatureId::from_u32(source_id);
        if !self.document.features.iter().any(|f| f.id == source_fid) {
            return Err(JsError::new(&format!(
                "source feature {source_id} not found on the spine"
            )));
        }

        self.snapshot();
        let mirror_id = self.document.allocate_id();
        let flat = mirror_id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");
        let params = serde_json::json!({
            "source": source_id,
            "plane_normal": [normal_x, normal_y, normal_z],
            "plane_point": [point_x, point_y, point_z],
        });
        let call = fielddraft_protocol::FeatureCall::new(
            "mirror",
            mirror_id,
            params,
        );
        self.document.feature_calls.push(call);
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "mirror regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(flat)
    }

    /// K7.5: return the current `params` JSON string for a
    /// feature call by id. Used by the Outliner's
    /// FeatureDialog to initialise its edit buffer with the
    /// feature's real values rather than a schema-default
    /// blob.
    #[wasm_bindgen(js_name = getFeatureCallParams)]
    pub fn get_feature_call_params(&self, id: u32) -> Result<String, JsError> {
        let target = FeatureId::from_u32(id);
        let call = self
            .document
            .feature_calls
            .iter()
            .find(|c| c.id == target)
            .ok_or_else(|| JsError::new(&format!("feature {id} not found")))?;
        serde_json::to_string(&call.params).map_err(|e| JsError::new(&e.to_string()))
    }

    /// K7.5: replace the `params` JSON of a feature call in
    /// place and regenerate the spine. Keeps the feature id
    /// stable so the outliner row doesn't jump, and re-runs
    /// the registry so downstream cached state (body meshes,
    /// packed buffer) reflects the new values.
    #[wasm_bindgen(js_name = replaceFeatureCallParams)]
    pub fn replace_feature_call_params(
        &mut self,
        id: u32,
        params_json: &str,
    ) -> Result<(), JsError> {
        let target = FeatureId::from_u32(id);
        let new_params: serde_json::Value =
            serde_json::from_str(params_json).map_err(|e| {
                JsError::new(&format!("invalid params JSON: {e}"))
            })?;
        let idx = self
            .document
            .feature_calls
            .iter()
            .position(|c| c.id == target)
            .ok_or_else(|| JsError::new(&format!("feature {id} not found")))?;

        self.snapshot();
        // K8.7: preserve the call's suppressed flag across a
        // params replace — otherwise toggling suppression off by
        // renaming or editing any other field would resurrect it.
        let old = &self.document.feature_calls[idx];
        let type_name = old.type_name.clone();
        let suppressed = old.suppressed;
        self.document.feature_calls[idx] = fielddraft_protocol::FeatureCall {
            type_name,
            id: target,
            params: new_params,
            suppressed,
        };
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "feature regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(())
    }

    /// K8.8: duplicate a feature call. Allocates a fresh id,
    /// clones the call's wire-form params, nudges any
    /// `translate` vector by `(+20, 0, 0)` so the clone doesn't
    /// sit exactly on top of the source, and pushes the new
    /// call to `feature_calls`. Returns the new id. Features
    /// without a translate field (patterns, mirror, sketch)
    /// are cloned verbatim — their geometry is derived from a
    /// referenced source, so the visual offset happens on the
    /// source side instead.
    #[wasm_bindgen(js_name = duplicateFeature)]
    pub fn duplicate_feature(&mut self, id: u32) -> Result<u32, JsError> {
        let target = FeatureId::from_u32(id);
        let idx = self
            .document
            .feature_calls
            .iter()
            .position(|c| c.id == target)
            .ok_or_else(|| JsError::new(&format!("feature {id} not found")))?;

        self.snapshot();
        let source = self.document.feature_calls[idx].clone();
        let new_id = self.document.allocate_id();
        let new_flat = new_id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");

        // Nudge translate along +X so the clone is visible.
        let mut new_params = source.params.clone();
        if let Some(obj) = new_params.as_object_mut() {
            if let Some(t) = obj.get_mut("translate").and_then(|v| v.as_array_mut()) {
                if t.len() == 3 {
                    if let Some(x) = t[0].as_f64() {
                        t[0] = serde_json::json!(x + 20.0);
                    }
                }
            }
        }

        let new_call = fielddraft_protocol::FeatureCall {
            type_name: source.type_name.clone(),
            id: new_id,
            params: new_params,
            suppressed: source.suppressed,
        };
        self.document.feature_calls.push(new_call);
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "duplicate regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(new_flat)
    }

    /// K8.10: upsert a document-level variable. Snapshots so
    /// undo/redo walks variable edits the same way it walks
    /// feature edits.
    ///
    /// K8.11: after the write, re-bootstraps from calls so
    /// every formula that references the variable picks up
    /// the new value on the next mesh pass. Pre-K8.11 this
    /// was a pure state edit with no geometry side effect.
    #[wasm_bindgen(js_name = setVariable)]
    pub fn set_variable(&mut self, name: &str, value: f32) -> Result<(), JsError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(JsError::new("variable name must not be empty"));
        }
        if !value.is_finite() {
            return Err(JsError::new(&format!(
                "variable `{trimmed}` must be a finite number"
            )));
        }
        self.snapshot();
        self.document
            .variables
            .insert(trimmed.to_string(), value);
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "variable set failed to regenerate: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(())
    }

    /// K8.10: remove a document-level variable. No-op if the
    /// name isn't in the table; returns `true` when it actually
    /// removed something.
    ///
    /// K8.11: re-bootstraps after the remove so any formula
    /// that referenced the departing variable surfaces as a
    /// regen error (which rolls the remove back).
    #[wasm_bindgen(js_name = removeVariable)]
    pub fn remove_variable(&mut self, name: &str) -> bool {
        if !self.document.variables.contains_key(name) {
            return false;
        }
        self.snapshot();
        self.document.variables.remove(name);
        if self.bootstrap_features_from_calls().is_err() {
            // A downstream formula broke — roll the remove
            // back. The caller gets `false` and the document
            // stays consistent.
            self.undo();
            return false;
        }
        self.rebuild_packed();
        true
    }

    /// K8.10: list every document-level variable as a JSON
    /// array of `{name, value}` pairs, sorted by name (the
    /// BTreeMap ordering).
    /// K9.1: return the overall scene AABB as a JSON object
    /// `{min: [x,y,z], max: [x,y,z], size: [w,h,d]}` in mm.
    /// Empty scenes return all-zero extents.
    #[wasm_bindgen(js_name = sceneAabb)]
    pub fn scene_aabb(&self) -> Result<String, JsError> {
        let tree = csg::CsgNode::from_features(&self.document.features)
            .apply_edge_fillets(&self.edge_fillets);
        let bb = tree.aabb();
        let size = bb.size();
        let result = serde_json::json!({
            "min": bb.min,
            "max": bb.max,
            "size": size,
        });
        serde_json::to_string(&result).map_err(|e| JsError::new(&e.to_string()))
    }

    #[wasm_bindgen(js_name = listVariables)]
    pub fn list_variables(&self) -> Result<String, JsError> {
        #[derive(serde::Serialize)]
        struct Entry<'a> {
            name: &'a str,
            value: f32,
        }
        let out: Vec<Entry> = self
            .document
            .variables
            .iter()
            .map(|(name, value)| Entry { name, value: *value })
            .collect();
        serde_json::to_string(&out).map_err(|e| JsError::new(&e.to_string()))
    }

    /// K8.7: toggle a feature call's suppressed flag and
    /// regenerate. A suppressed call is skipped by the
    /// registry's regen loop, so its geometry drops out of the
    /// CSG tree, but the call stays on `feature_calls` so the
    /// user can unsuppress it later. Fails if the id doesn't
    /// match any call or if regeneration errors out (e.g.
    /// unsuppressing a call whose upstream dependency is now
    /// broken).
    #[wasm_bindgen(js_name = setFeatureSuppressed)]
    pub fn set_feature_suppressed(
        &mut self,
        id: u32,
        suppressed: bool,
    ) -> Result<(), JsError> {
        let target = FeatureId::from_u32(id);
        let idx = self
            .document
            .feature_calls
            .iter()
            .position(|c| c.id == target)
            .ok_or_else(|| JsError::new(&format!("feature {id} not found")))?;
        if self.document.feature_calls[idx].suppressed == suppressed {
            return Ok(());
        }

        self.snapshot();
        self.document.feature_calls[idx].suppressed = suppressed;
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "suppress toggle failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(())
    }

    /// K6b.3a: fetch the sub-spine sub_calls for a sketch by
    /// feature id, so the UI can reload an existing sketch when
    /// the user re-enters it. Returns the JSON array string of
    /// `FeatureCall`s or an error if the id doesn't point at a
    /// sketch.
    #[wasm_bindgen(js_name = getSketchSubCalls)]
    pub fn get_sketch_sub_calls(&self, id: u32) -> Result<String, JsError> {
        let target = FeatureId::from_u32(id);
        let call = self
            .document
            .feature_calls
            .iter()
            .find(|c| c.id == target && c.type_name == "sketch")
            .ok_or_else(|| {
                JsError::new(&format!("feature {id} is not a sketch"))
            })?;
        let sub = call
            .params
            .get("sub_calls")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Array(vec![]));
        serde_json::to_string(&sub).map_err(|e| JsError::new(&e.to_string()))
    }

    // ── mutations ─────────────────────────────────────────────────────

    /// Append a primitive feature.
    ///
    /// The `tx`/`ty`/`tz` arguments populate the new feature's transform
    /// (`Transform.translate`); the primitive itself is created at its
    /// own origin so that future transform edits remain a single source
    /// of truth.
    ///
    /// `params` is interpreted per kind:
    ///   - box      → (half_x, half_y, half_z)
    ///   - sphere   → (radius, _, _)
    ///   - cylinder → (radius, half_height, _)
    #[wasm_bindgen(js_name = addPrimitive)]
    pub fn add_primitive(
        &mut self,
        kind: u32,
        op: u32,
        tx: f32,
        ty: f32,
        tz: f32,
        p0: f32,
        p1: f32,
        p2: f32,
    ) -> Result<u32, JsError> {
        let primitive = match kind {
            Primitive::KIND_BOX => Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [p0, p1, p2],
            },
            Primitive::KIND_SPHERE => Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius: p0,
            },
            Primitive::KIND_CYLINDER => Primitive::Cylinder {
                center: [0.0, 0.0, 0.0],
                radius: p0,
                half_height: p1,
            },
            Primitive::KIND_TORUS => Primitive::Torus {
                center: [0.0, 0.0, 0.0],
                major_radius: p0,
                minor_radius: p1,
            },
            Primitive::KIND_THREAD => Primitive::Thread {
                nominal_radius: p0,
                pitch: p1,
                depth: p2,
                half_height: 5.0,
            },
            Primitive::KIND_CONE => Primitive::Cone {
                r_bottom: p0,
                r_top: p1,
                half_height: p2,
            },
            Primitive::KIND_CAPSULE => Primitive::Capsule {
                radius: p0,
                half_height: p1,
            },
            Primitive::KIND_HEX_PRISM => Primitive::HexPrism {
                apothem: p0,
                half_height: p1,
            },
            other => return Err(JsError::new(&format!("invalid primitive kind: {other}"))),
        };
        let bool_op = Self::parse_op(op)?;

        self.snapshot();
        let id = self.document.allocate_id();
        let name = format!("{} {}", primitive.default_name(), id);
        let transform = Transform {
            translate: [tx, ty, tz],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: 1.0,
        };
        // K4.5.1 still issues flat-numeric FeatureIds, so the shim
        // call at the bottom is guaranteed to succeed. When the
        // Feature Registry starts issuing prefix-style ids in a
        // later slice, `addPrimitive` will need to move to a
        // String return value.
        let flat_u32 = id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");
        self.document.features.push(Feature {
            id,
            name,
            op: bool_op,
            primitive,
            transform,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        });
        self.rebuild_packed();
        Ok(flat_u32)
    }

    /// Replace the translate component of a feature's transform.
    #[wasm_bindgen(js_name = setFeatureTranslate)]
    pub fn set_feature_translate(
        &mut self,
        id: u32,
        tx: f32,
        ty: f32,
        tz: f32,
    ) -> bool {
        let id = FeatureId::from_u32(id);
        let pos = self.document.features.iter().position(|f| f.id == id);
        let Some(idx) = pos else { return false };
        if self.document.features[idx].transform.translate == [tx, ty, tz] {
            return false;
        }
        self.snapshot();
        self.document.features[idx].transform.translate = [tx, ty, tz];
        self.rebuild_packed();
        true
    }

    /// Add `(dx, dy, dz)` to a feature's translate. Convenience for
    /// keyboard nudging.
    #[wasm_bindgen(js_name = nudgeFeature)]
    pub fn nudge_feature(&mut self, id: u32, dx: f32, dy: f32, dz: f32) -> bool {
        let id = FeatureId::from_u32(id);
        let pos = self.document.features.iter().position(|f| f.id == id);
        let Some(idx) = pos else { return false };
        if dx == 0.0 && dy == 0.0 && dz == 0.0 {
            return false;
        }
        self.snapshot();
        let t = &mut self.document.features[idx].transform;
        t.translate[0] += dx;
        t.translate[1] += dy;
        t.translate[2] += dz;
        self.rebuild_packed();
        true
    }

    /// Replace the full transform (translate, quaternion, scale).
    #[wasm_bindgen(js_name = setFeatureTransform)]
    pub fn set_feature_transform(
        &mut self,
        id: u32,
        tx: f32,
        ty: f32,
        tz: f32,
        qx: f32,
        qy: f32,
        qz: f32,
        qw: f32,
        scale: f32,
    ) -> bool {
        let id = FeatureId::from_u32(id);
        let pos = self.document.features.iter().position(|f| f.id == id);
        let Some(idx) = pos else { return false };
        let new = Transform {
            translate: [tx, ty, tz],
            rotation: [qx, qy, qz, qw],
            scale,
        };
        if self.document.features[idx].transform == new {
            return false;
        }
        self.snapshot();
        self.document.features[idx].transform = new;
        self.rebuild_packed();
        true
    }

    /// Remove the feature with the given id. Returns true if anything
    /// was removed.
    #[wasm_bindgen(js_name = removeFeature)]
    pub fn remove_feature(&mut self, id: u32) -> bool {
        let id = FeatureId::from_u32(id);
        let pos = self.document.features.iter().position(|f| f.id == id);
        if let Some(idx) = pos {
            self.snapshot();
            self.document.features.remove(idx);
            self.rebuild_packed();
            true
        } else {
            false
        }
    }

    /// Change the boolean op of an existing feature.
    #[wasm_bindgen(js_name = setFeatureOp)]
    pub fn set_feature_op(&mut self, id: u32, op: u32) -> Result<bool, JsError> {
        let bool_op = Self::parse_op(op)?;
        let id = FeatureId::from_u32(id);
        let pos = self.document.features.iter().position(|f| f.id == id);
        if let Some(idx) = pos {
            if self.document.features[idx].op == bool_op {
                return Ok(false);
            }
            self.snapshot();
            self.document.features[idx].op = bool_op;
            self.rebuild_packed();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Move a feature to a new index. Indices clamp into bounds.
    #[wasm_bindgen(js_name = reorderFeature)]
    pub fn reorder_feature(&mut self, id: u32, target_index: usize) -> bool {
        let id = FeatureId::from_u32(id);
        let from = self.document.features.iter().position(|f| f.id == id);
        let Some(from) = from else { return false };
        let to = target_index.min(self.document.features.len().saturating_sub(1));
        if from == to {
            return false;
        }
        self.snapshot();
        let f = self.document.features.remove(from);
        self.document.features.insert(to, f);
        self.rebuild_packed();
        true
    }

    // ── undo / redo ───────────────────────────────────────────────────

    #[wasm_bindgen(js_name = canUndo)]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[wasm_bindgen(js_name = undoDepth)]
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    #[wasm_bindgen(js_name = canRedo)]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    #[wasm_bindgen(js_name = redoDepth)]
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    pub fn undo(&mut self) -> bool {
        if let Some(prev) = self.undo.pop() {
            let current = std::mem::replace(&mut self.document, prev);
            self.redo.push(current);
            self.rebuild_packed();
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some(next) = self.redo.pop() {
            let current = std::mem::replace(&mut self.document, next);
            self.undo.push(current);
            self.rebuild_packed();
            true
        } else {
            false
        }
    }

    // ── per-body mesh bridge (zero-copy) ──────────────────────────────
    //
    // The Dual Contouring mesher writes flat positions/normals/indices
    // into each `Body`'s own `Mesh` on every mutation. JS binds
    // `Float32Array`/`Uint32Array` views straight onto WASM linear
    // memory via the per-body pointers — callers MUST re-acquire the
    // views after every mutating call, because the underlying `Vec`s
    // can be reallocated.
    //
    // Aggregate counts (`meshVertexCount` / `meshIndexCount` /
    // `meshTriangleCount`) are provided as cheap sums over the body
    // list for callers that only need a total (tests, status bars).
    // There is no aggregate pointer buffer any more — the frontend
    // iterates `bodyCount()` and binds one `BufferGeometry` per body.

    /// Sum of vertex counts across every body. O(bodies).
    #[wasm_bindgen(js_name = meshVertexCount)]
    pub fn mesh_vertex_count(&self) -> usize {
        self.bodies.iter().map(|b| b.mesh.vertex_count()).sum()
    }

    /// Sum of index counts across every body. O(bodies).
    #[wasm_bindgen(js_name = meshIndexCount)]
    pub fn mesh_index_count(&self) -> usize {
        self.bodies.iter().map(|b| b.mesh.indices.len()).sum()
    }

    /// Sum of triangle counts across every body. O(bodies).
    #[wasm_bindgen(js_name = meshTriangleCount)]
    pub fn mesh_triangle_count(&self) -> usize {
        self.bodies.iter().map(|b| b.mesh.triangle_count()).sum()
    }

    // ── K7.2: STL export ─────────────────────────────────────────

    /// Export the current scene as an ASCII STL string.
    /// Iterates every body's DC mesh in the order `bodies`
    /// holds them; downstream slicers treat the whole blob as
    /// one `solid`.
    ///
    /// Takes `&mut self` because the worker-mode kernel keeps
    /// its main-thread `bodies` cache empty (the worker owns
    /// the meshes). Export temporarily forces a sync rebuild
    /// so the bytes come out non-empty, then leaves the sync
    /// state exactly as it found it.
    #[wasm_bindgen(js_name = exportStlAscii)]
    pub fn export_stl_ascii(&mut self) -> String {
        let prev = self.ensure_bodies_for_export();
        let out = {
            let meshes: Vec<&Mesh> = self.bodies.iter().map(|b| &b.mesh).collect();
            export::export_ascii(&meshes)
        };
        self.restore_sync_meshing(prev);
        out
    }

    /// Export the current scene as binary STL. Returns a
    /// JavaScript `Uint8Array` by virtue of wasm-bindgen's
    /// auto-conversion for `Vec<u8>`.
    ///
    /// Same `&mut self` + sync-rebuild dance as
    /// [`export_stl_ascii`].
    #[wasm_bindgen(js_name = exportStlBinary)]
    pub fn export_stl_binary(&mut self) -> Vec<u8> {
        let prev = self.ensure_bodies_for_export();
        let out = {
            let meshes: Vec<&Mesh> = self.bodies.iter().map(|b| &b.mesh).collect();
            export::export_binary(&meshes)
        };
        self.restore_sync_meshing(prev);
        out
    }

    /// Force the main-thread body cache to be populated so
    /// the STL exporter has something to walk. Returns the
    /// previous `sync_meshing` value so the caller can
    /// restore it after the export.
    fn ensure_bodies_for_export(&mut self) -> bool {
        let prev = self.sync_meshing;
        if !prev {
            self.sync_meshing = true;
            self.bodies.clear();
            self.rebuild_mesh_internal();
        }
        prev
    }

    /// Flip sync meshing back to whatever it was before
    /// [`ensure_bodies_for_export`] ran. When worker-mode was
    /// active, we drop the bodies we just built so the next
    /// mutation's worker response isn't shadowed by stale
    /// cache data.
    fn restore_sync_meshing(&mut self, prev: bool) {
        if prev != self.sync_meshing {
            self.sync_meshing = prev;
            if !prev {
                self.bodies.clear();
            }
        }
    }

    #[wasm_bindgen(js_name = bodyCount)]
    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }

    /// Stable identifier for the i-th body (= lowest `FeatureId` in
    /// its feature set). Panics only if `i` is out of range — tests
    /// and the wasm-bindgen bridge should always gate the call on
    /// `bodyCount()`.
    ///
    /// K4.5.1 compat shim: the return type stays `u32` because every
    /// id this slice issues is still flat-numeric. A later slice that
    /// introduces prefix-style ids will move this to `String`.
    #[wasm_bindgen(js_name = bodyId)]
    pub fn body_id(&self, i: usize) -> u32 {
        self.bodies[i]
            .id
            .as_u32_if_flat()
            .expect("K4.5.1 body ids are still flat-numeric")
    }

    /// Per-body version counter. Bumps only when the body's own
    /// mesh was actually rebuilt — not when unrelated bodies
    /// mutated around it.
    #[wasm_bindgen(js_name = bodyVersion)]
    pub fn body_version(&self, i: usize) -> u32 {
        self.bodies[i].version
    }

    #[wasm_bindgen(js_name = bodyVertexCount)]
    pub fn body_vertex_count(&self, i: usize) -> usize {
        self.bodies[i].mesh.vertex_count()
    }

    #[wasm_bindgen(js_name = bodyIndexCount)]
    pub fn body_index_count(&self, i: usize) -> usize {
        self.bodies[i].mesh.indices.len()
    }

    #[wasm_bindgen(js_name = bodyTriangleCount)]
    pub fn body_triangle_count(&self, i: usize) -> usize {
        self.bodies[i].mesh.triangle_count()
    }

    #[wasm_bindgen(js_name = bodyPositionsPtr)]
    pub fn body_positions_ptr(&self, i: usize) -> *const f32 {
        self.bodies[i].mesh.positions.as_ptr()
    }

    #[wasm_bindgen(js_name = bodyNormalsPtr)]
    pub fn body_normals_ptr(&self, i: usize) -> *const f32 {
        self.bodies[i].mesh.normals.as_ptr()
    }

    #[wasm_bindgen(js_name = bodyIndicesPtr)]
    pub fn body_indices_ptr(&self, i: usize) -> *const u32 {
        self.bodies[i].mesh.indices.as_ptr()
    }

    #[wasm_bindgen(js_name = bodyClassificationsPtr)]
    pub fn body_classifications_ptr(&self, i: usize) -> *const u8 {
        self.bodies[i].mesh.classifications.as_ptr()
    }

    #[wasm_bindgen(js_name = bodyContributorsPtr)]
    pub fn body_contributors_ptr(&self, i: usize) -> *const u32 {
        self.bodies[i].mesh.contributors.as_ptr()
    }

    /// Number of `u32` slots reserved per vertex in the per-body
    /// contributor flat buffer. Matches `csg::MAX_CONTRIBUTORS`.
    #[wasm_bindgen(js_name = meshContributorStride)]
    pub fn mesh_contributor_stride() -> usize {
        csg::MAX_CONTRIBUTORS
    }

    /// Monotonic counter bumped every time the mesh is rebuilt. JS can
    /// use it to debounce uploads ("only rebind the BufferGeometry when
    /// this changes since the last frame").
    #[wasm_bindgen(js_name = meshVersion)]
    pub fn mesh_version(&self) -> u32 {
        self.mesh_version
    }

    /// Voxel grid resolution for the active body. Aliased as
    /// `meshResolution` for backward compatibility with callers
    /// that don't care about LOD yet.
    #[wasm_bindgen(js_name = meshResolution)]
    pub fn mesh_resolution(&self) -> u32 {
        self.active_mesh_resolution
    }

    /// Change the active-body voxel grid resolution and re-mesh.
    /// No-op if the resolution is unchanged or zero. Returns true
    /// if the mesh was actually rebuilt. Aliased as
    /// `setMeshResolution` for backward compatibility.
    #[wasm_bindgen(js_name = setMeshResolution)]
    pub fn set_mesh_resolution(&mut self, resolution: u32) -> bool {
        if resolution == 0 || resolution == self.active_mesh_resolution {
            return false;
        }
        self.active_mesh_resolution = resolution;
        self.rebuild_mesh_internal();
        true
    }

    // ── LOD controls (K3 part 4) ──────────────────────────────────────

    #[wasm_bindgen(js_name = contextMeshResolution)]
    pub fn context_mesh_resolution(&self) -> u32 {
        self.context_mesh_resolution
    }

    /// Change the context-body voxel grid resolution and re-mesh
    /// every body that's currently running at context LOD. No-op
    /// if unchanged or zero.
    #[wasm_bindgen(js_name = setContextMeshResolution)]
    pub fn set_context_mesh_resolution(&mut self, resolution: u32) -> bool {
        if resolution == 0 || resolution == self.context_mesh_resolution {
            return false;
        }
        self.context_mesh_resolution = resolution;
        self.rebuild_mesh_internal();
        true
    }

    /// Currently active body id, or `0` when LOD is disabled (every
    /// body treated as active). The zero sentinel keeps the
    /// wasm-bindgen signature a simple `u32` — feature ids are
    /// allocated starting at `1` so `0` is unambiguous.
    #[wasm_bindgen(js_name = activeBodyId)]
    pub fn active_body_id(&self) -> u32 {
        self.active_body_id
            .as_ref()
            .and_then(|id| id.as_u32_if_flat())
            .unwrap_or(0)
    }

    /// Mark the given body id as the active one. Triggers a re-mesh
    /// that promotes `body_id` to active resolution and demotes
    /// whichever body was active before (if any) to context
    /// resolution. Passing `0` clears the active body.
    #[wasm_bindgen(js_name = setActiveBodyId)]
    pub fn set_active_body_id(&mut self, body_id: u32) -> bool {
        let new_active = if body_id == 0 {
            None
        } else {
            Some(FeatureId::from_u32(body_id))
        };
        self.set_active_body_id_internal(new_active)
    }

    /// Convenience: set the active body to whichever body owns the
    /// given feature. Returns true if the active body actually
    /// changed. No-op (returns false) when the feature doesn't
    /// exist or already lives in the active body.
    #[wasm_bindgen(js_name = setActiveBodyByFeature)]
    pub fn set_active_body_by_feature(&mut self, feature_id: u32) -> bool {
        let feature_id = FeatureId::from_u32(feature_id);
        match self.body_containing_feature(&feature_id) {
            Some(id) => self.set_active_body_id_internal(Some(id)),
            None => false,
        }
    }

    /// Clear the active body so every body runs at active LOD
    /// again. Equivalent to `setActiveBodyId(0)`.
    #[wasm_bindgen(js_name = clearActiveBodyId)]
    pub fn clear_active_body_id(&mut self) -> bool {
        self.set_active_body_id_internal(None)
    }

    // ── sync meshing toggle (K3 part 5) ───────────────────────────────

    /// Whether this kernel instance runs the Dual Contouring pass
    /// itself on every mutation. The frontend flips this off on the
    /// main-thread kernel once the mesher web worker is up — the
    /// worker owns its own kernel instance that meshes for real.
    #[wasm_bindgen(js_name = syncMeshing)]
    pub fn sync_meshing(&self) -> bool {
        self.sync_meshing
    }

    /// Enable or disable the synchronous DC pass. Toggling forces a
    /// rebuild so the body list's mesh buffers match the new
    /// setting: enabling meshes every body from scratch, disabling
    /// clears every mesh to the default empty buffer. No-op if the
    /// setting is already at the requested value.
    #[wasm_bindgen(js_name = setSyncMeshing)]
    pub fn set_sync_meshing(&mut self, enabled: bool) -> bool {
        if self.sync_meshing == enabled {
            return false;
        }
        self.sync_meshing = enabled;
        // Drop the body cache so `rebuild_mesh_internal` goes
        // through the fresh-mesh path for every body — otherwise
        // the cache-reuse check (`hash + resolution match`) would
        // keep populated meshes around after disabling, or empty
        // meshes after enabling.
        self.bodies.clear();
        self.rebuild_mesh_internal();
        true
    }

    // ── picking / provenance ──────────────────────────────────────────
    //
    // K2 wire-format: the kernel takes a world-space ray (origin +
    // direction) and returns a small JSON blob that the frontend can
    // decode without touching the provenance structs directly. On a
    // miss the payload is `{"hit":false}`.

    /// Sphere-trace a ray through the current CSG tree and return a
    /// JSON description of the first hit, if any. See `raycast.rs`
    /// for the numerical scheme.
    #[wasm_bindgen(js_name = pick)]
    pub fn pick(
        &self,
        ox: f32,
        oy: f32,
        oz: f32,
        dx: f32,
        dy: f32,
        dz: f32,
    ) -> String {
        let csg = CsgNode::from_features(&self.document.features)
            .apply_edge_fillets(&self.edge_fillets);
        let bounded = BoundedCsg::from_csg(&csg);
        let params = RaycastParams::default();
        match raycast(&bounded, [ox, oy, oz], [dx, dy, dz], params) {
            None => r#"{"hit":false}"#.to_string(),
            Some(hit) => {
                // `contributors` is a small inline array; serialise as
                // a JSON array of ids. Avoid serde_json::Value churn
                // and just format manually — the schema is tiny and
                // stable.
                // Serialise contributor ids as bare integers to stay
                // byte-compatible with the K2 frontend parser that
                // expects `contributors: number[]`. K4.5.1 still only
                // produces flat-numeric ids, so the `expect` never
                // fires; when a future slice introduces hierarchical
                // ids the JSON schema will have to widen to strings.
                let mut contributors = String::from("[");
                for (i, id) in hit.contributors.as_slice().iter().enumerate() {
                    if i > 0 {
                        contributors.push(',');
                    }
                    let flat = id
                        .as_u32_if_flat()
                        .expect("K4.5.1 pick hit has flat-numeric contributors");
                    contributors.push_str(&flat.to_string());
                }
                contributors.push(']');
                format!(
                    r#"{{"hit":true,"point":[{},{},{}],"t":{},"element":"{}","contributors":{}}}"#,
                    hit.point[0],
                    hit.point[1],
                    hit.point[2],
                    hit.t,
                    hit.element.label(),
                    contributors,
                )
            }
        }
    }

    // ── debug helper ──────────────────────────────────────────────────

    #[wasm_bindgen(js_name = evalSdf)]
    pub fn eval_sdf(&self, x: f32, y: f32, z: f32) -> f32 {
        sdf_features(&self.document.features, [x, y, z])
    }
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_feature(id: u32, op: BoolOp, center: [f32; 3], half: [f32; 3]) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Box {id}"),
            op,
            primitive: Primitive::Box { center, half_extents: half },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        }
    }

    fn sphere_feature(id: u32, op: BoolOp, center: [f32; 3], r: f32) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Sphere {id}"),
            op,
            primitive: Primitive::Sphere { center, radius: r },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        }
    }

    fn transformed_sphere(id: u32, translate: [f32; 3], r: f32) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Sphere {id}"),
            op: BoolOp::Union,
            primitive: Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: r },
            transform: Transform {
                translate,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: 1.0,
            },
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        }
    }

    // ─── primitive SDFs ──────────────────────────────────────────────

    #[test]
    fn box_sdf_zero_on_face() {
        let b = Primitive::Box {
            center: [0.0, 0.0, 0.0],
            half_extents: [1.0, 1.0, 1.0],
        };
        assert!((sdf_primitive(&b, [1.0, 0.0, 0.0])).abs() < 1e-5);
        assert!(sdf_primitive(&b, [0.0, 0.0, 0.0]) < 0.0);
        assert!(sdf_primitive(&b, [2.0, 0.0, 0.0]) > 0.0);
    }

    #[test]
    fn sphere_sdf_matches_radius() {
        let s = Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 5.0 };
        assert!((sdf_primitive(&s, [5.0, 0.0, 0.0])).abs() < 1e-5);
        assert!((sdf_primitive(&s, [0.0, 10.0, 0.0]) - 5.0).abs() < 1e-5);
        assert!((sdf_primitive(&s, [0.0, 0.0, 0.0]) + 5.0).abs() < 1e-5);
    }

    #[test]
    fn cylinder_sdf_radial_and_axial() {
        let c = Primitive::Cylinder {
            center: [0.0, 0.0, 0.0],
            radius: 2.0,
            half_height: 3.0,
        };
        assert!((sdf_primitive(&c, [2.0, 0.0, 0.0])).abs() < 1e-5);
        assert!((sdf_primitive(&c, [0.0, 3.0, 0.0])).abs() < 1e-5);
        assert!(sdf_primitive(&c, [0.0, 0.0, 0.0]) < 0.0);
    }

    // ─── boolean ops on a feature spine ──────────────────────────────

    #[test]
    fn empty_features_are_empty_space() {
        assert!(sdf_features(&[], [0.0, 0.0, 0.0]).is_infinite());
    }

    #[test]
    fn union_of_two_spheres_is_their_min() {
        let features = vec![
            sphere_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], 5.0),
            sphere_feature(2, BoolOp::Union, [20.0, 0.0, 0.0], 5.0),
        ];
        // Inside first sphere.
        assert!(sdf_features(&features, [0.0, 0.0, 0.0]) < 0.0);
        // Inside second sphere.
        assert!(sdf_features(&features, [20.0, 0.0, 0.0]) < 0.0);
        // Halfway between (10, 0, 0) — outside both, distance ≈ 5.
        assert!((sdf_features(&features, [10.0, 0.0, 0.0]) - 5.0).abs() < 1e-4);
    }

    #[test]
    fn difference_carves_a_hole() {
        // Big box minus a smaller centered box: the center is now outside.
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            box_feature(2, BoolOp::Difference, [0.0, 0.0, 0.0], [3.0, 3.0, 3.0]),
        ];
        // Center is inside the cavity → outside the body → SDF > 0.
        assert!(sdf_features(&features, [0.0, 0.0, 0.0]) > 0.0);
        // Edge of the outer box is still on the surface.
        assert!((sdf_features(&features, [10.0, 0.0, 0.0])).abs() < 1e-4);
        // A point well outside both is positive.
        assert!(sdf_features(&features, [50.0, 0.0, 0.0]) > 0.0);
    }

    #[test]
    fn intersect_keeps_only_overlap() {
        // Two boxes overlapping in a cube of half-extent 5 around origin.
        let features = vec![
            box_feature(1, BoolOp::Union, [-2.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            box_feature(2, BoolOp::Intersect, [2.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
        ];
        // Origin is inside both → inside intersection.
        assert!(sdf_features(&features, [0.0, 0.0, 0.0]) < 0.0);
        // (−10, 0, 0): inside first, outside second → outside intersection.
        assert!(sdf_features(&features, [-10.0, 0.0, 0.0]) > 0.0);
        // (10, 0, 0): inside second, outside first → outside intersection.
        assert!(sdf_features(&features, [10.0, 0.0, 0.0]) > 0.0);
    }

    // ─── packed buffer ───────────────────────────────────────────────

    #[test]
    fn pack_includes_op_tag_in_slot_seven() {
        let mut slot = [0.0_f32; FEATURE_STRIDE];
        let f = sphere_feature(1, BoolOp::Difference, [1.0, 2.0, 3.0], 4.0);
        pack_feature(&f, &mut slot);
        assert_eq!(slot[0], Primitive::KIND_SPHERE as f32);
        assert_eq!(&slot[1..4], &[1.0, 2.0, 3.0]);
        assert_eq!(slot[4], 4.0);
        assert_eq!(slot[7], BoolOp::TAG_DIFFERENCE as f32);
    }

    #[test]
    fn pack_writes_transform_into_tail() {
        let mut slot = [0.0_f32; FEATURE_STRIDE];
        let f = transformed_sphere(1, [10.0, 20.0, -5.0], 4.0);
        pack_feature(&f, &mut slot);
        // vec4 2: translate xyz + uniform scale
        assert_eq!(&slot[8..11], &[10.0, 20.0, -5.0]);
        assert_eq!(slot[11], 1.0);
        // vec4 3: quaternion xyzw
        assert_eq!(&slot[12..16], &[0.0, 0.0, 0.0, 1.0]);
    }

    // ─── transform-aware SDF ─────────────────────────────────────────

    #[test]
    fn translated_sphere_matches_origin_sphere() {
        // A sphere translated to (50, 0, 0) should give exactly the same
        // SDF at (55, 0, 0) as a sphere at the origin gives at (5, 0, 0).
        let translated = transformed_sphere(1, [50.0, 0.0, 0.0], 5.0);
        let at_origin = sphere_feature(2, BoolOp::Union, [0.0, 0.0, 0.0], 5.0);
        let d_t = sdf_feature(&translated, [55.0, 0.0, 0.0]);
        let d_o = sdf_feature(&at_origin, [5.0, 0.0, 0.0]);
        assert!((d_t - d_o).abs() < 1e-5, "{d_t} vs {d_o}");
        assert!(d_t.abs() < 1e-4, "should be on the surface");
    }

    #[test]
    fn uniform_scale_preserves_distance_property() {
        // A unit sphere scaled by 4 should behave like a radius-4 sphere.
        let scaled = Feature {
            id: FeatureId::from_u32(1),
            name: "S".into(),
            op: BoolOp::Union,
            primitive: Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 1.0 },
            transform: Transform {
                translate: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: 4.0,
            },
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        };
        // On the (scaled) surface.
        assert!(sdf_feature(&scaled, [4.0, 0.0, 0.0]).abs() < 1e-5);
        // 2 units inside.
        assert!((sdf_feature(&scaled, [2.0, 0.0, 0.0]) + 2.0).abs() < 1e-5);
        // 2 units outside.
        assert!((sdf_feature(&scaled, [6.0, 0.0, 0.0]) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn rotation_keeps_box_axis_aligned_to_local_frame() {
        // A unit box rotated 90° about Z should still report distance 0
        // at world point (0, 1, 0) — because in the rotated frame the
        // box still extends ±1 in every axis.
        // Quaternion for 90° about Z: (0, 0, sin45°, cos45°)
        let s = (std::f32::consts::FRAC_PI_4).sin();
        let c = (std::f32::consts::FRAC_PI_4).cos();
        let f = Feature {
            id: FeatureId::from_u32(1),
            name: "B".into(),
            op: BoolOp::Union,
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [1.0, 1.0, 1.0],
            },
            transform: Transform {
                translate: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, s, c],
                scale: 1.0,
            },
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        };
        // Surface in any axis direction.
        assert!((sdf_feature(&f, [1.0, 0.0, 0.0])).abs() < 1e-5);
        assert!((sdf_feature(&f, [0.0, 1.0, 0.0])).abs() < 1e-5);
        // Origin still inside.
        assert!(sdf_feature(&f, [0.0, 0.0, 0.0]) < 0.0);
    }

    #[test]
    fn translate_then_difference_carves_an_offset_hole() {
        // A big box at origin with a translated subtractive sphere at
        // its corner should have a hole biased toward that corner.
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            transformed_sphere(2, [10.0, 10.0, 10.0], 5.0),
        ];
        // At the corner the carved sphere has scooped material away.
        // Use Difference: replace second feature's op.
        let mut features = features;
        features[1].op = BoolOp::Difference;

        // Right at the center of the carving sphere we are outside the
        // remaining body.
        assert!(sdf_features(&features, [10.0, 10.0, 10.0]) > 0.0);
        // Origin of the box is still safely inside.
        assert!(sdf_features(&features, [0.0, 0.0, 0.0]) < 0.0);
    }

    #[test]
    fn pack_features_strides_correctly() {
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [1.0, 2.0, 3.0]),
            sphere_feature(2, BoolOp::Difference, [10.0, 0.0, 0.0], 5.0),
        ];
        let buf = pack_features(&features);
        assert_eq!(buf.len(), 2 * FEATURE_STRIDE);
        assert_eq!(buf[FEATURE_STRIDE], Primitive::KIND_SPHERE as f32);
        assert_eq!(buf[FEATURE_STRIDE + 7], BoolOp::TAG_DIFFERENCE as f32);
    }

    // ─── kernel mutations + undo/redo ────────────────────────────────

    // ─── K6b.3a: sketch commit path ─────────────────────────────────

    #[test]
    // ─── K8.10: variables ─────────────────────────────────────

    #[test]
    fn set_list_remove_variable_roundtrips_through_btreemap() {
        let mut k = Kernel::new();
        assert_eq!(k.list_variables().unwrap(), "[]");

        k.set_variable("width", 50.0).unwrap();
        k.set_variable("depth", 30.0).unwrap();
        // Inserted in reverse alphabetical order, but the
        // BTreeMap normalises to name-sorted on listing.
        let json = k.list_variables().unwrap();
        assert!(json.contains("\"name\":\"depth\""));
        assert!(json.contains("\"name\":\"width\""));
        let depth_idx = json.find("depth").unwrap();
        let width_idx = json.find("width").unwrap();
        assert!(depth_idx < width_idx);

        assert!(k.remove_variable("depth"));
        assert!(!k.remove_variable("depth"));
        let json = k.list_variables().unwrap();
        assert!(!json.contains("depth"));
        assert!(json.contains("width"));
    }

    // Error-path tests for set_variable are wasm-only —
    // JsError::new panics when built as a native binary, so
    // the empty-name / NaN / Infinity checks are covered by
    // the Playwright smoketest instead.

    #[test]
    fn set_variable_trims_whitespace_and_undo_rolls_back() {
        let mut k = Kernel::new();
        k.set_variable("  width  ", 42.0).unwrap();
        assert_eq!(k.document.variables.get("width"), Some(&42.0));
        // Undo removes the variable entirely.
        assert!(k.undo());
        assert!(k.document.variables.is_empty());
    }

    #[test]
    fn add_sketch_appends_a_sketch_feature_call_and_preserves_primitives() {
        let mut k = Kernel::new();
        // Start with a box so we can confirm the primitive survives
        // the sketch add + regeneration round trip.
        let box_id = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                10.0,
                10.0,
                10.0,
            )
            .unwrap();

        let sub_calls = r#"[
            {
                "type": "sketch_line",
                "id": "100",
                "params": { "start": [0.0, 0.0], "end": [20.0, 0.0] }
            }
        ]"#;
        let sketch_id = k
            .add_sketch("qCreatedBy(origin_plane_xy)", sub_calls)
            .unwrap();
        assert_ne!(sketch_id, box_id);

        // feature_calls: [box, sketch] — sketch is appended after
        // the primitive survivors.
        assert_eq!(k.document.feature_calls.len(), 2);
        assert_eq!(k.document.feature_calls[0].type_name, "box");
        assert_eq!(k.document.feature_calls[1].type_name, "sketch");

        // The primitive spine still has the box.
        assert_eq!(k.document.features.len(), 1);
        assert_eq!(k.document.features[0].id.as_u32_if_flat(), Some(box_id));

        // listFeatures reports both rows with the right kinds.
        let list_json = k.list_features().unwrap();
        let list: serde_json::Value = serde_json::from_str(&list_json).unwrap();
        let arr = list.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["kind"], serde_json::json!("box"));
        assert_eq!(arr[1]["kind"], serde_json::json!("sketch"));
        assert_eq!(arr[1]["name"], serde_json::json!(format!("Sketch {sketch_id}")));
    }

    #[test]
    fn add_sketch_survives_subsequent_primitive_mutations() {
        // K6b.3a regression guard: adding a sketch and then
        // adding a box must leave both entries in the feature
        // calls list. The old sync_feature_calls wipe-and-rebuild
        // would have dropped the sketch.
        let mut k = Kernel::new();
        let sketch_id = k.add_sketch("qCreatedBy(origin_plane_xy)", "[]").unwrap();
        let box_id = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                5.0,
                5.0,
                5.0,
            )
            .unwrap();
        assert_ne!(sketch_id, box_id);
        assert_eq!(k.document.feature_calls.len(), 2);
        assert!(k
            .document
            .feature_calls
            .iter()
            .any(|c| c.type_name == "sketch"));
        assert!(k
            .document
            .feature_calls
            .iter()
            .any(|c| c.type_name == "box"));
    }

    #[test]
    fn get_sketch_sub_calls_returns_the_committed_sub_spine() {
        let mut k = Kernel::new();
        let sub_calls = r#"[
            { "type": "sketch_point", "id": "100", "params": { "at": [1.0, 2.0] } },
            { "type": "sketch_point", "id": "101", "params": { "at": [3.0, 4.0] } }
        ]"#;
        let id = k
            .add_sketch("qCreatedBy(origin_plane_xy)", sub_calls)
            .unwrap();
        let back = k.get_sketch_sub_calls(id).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&back).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], serde_json::json!("sketch_point"));
        assert_eq!(arr[1]["type"], serde_json::json!("sketch_point"));
    }

    #[test]
    fn replace_sketch_updates_in_place_and_keeps_feature_id_stable() {
        let mut k = Kernel::new();
        let id = k.add_sketch("qCreatedBy(origin_plane_xy)", "[]").unwrap();
        assert_eq!(k.document.feature_calls.len(), 1);

        let new_calls = r#"[
            {
                "type": "sketch_point",
                "id": "100",
                "params": { "at": [5.0, 5.0] }
            }
        ]"#;
        k.replace_sketch(id, "qCreatedBy(origin_plane_xy)", new_calls)
            .unwrap();

        // Still one call, still the same id.
        assert_eq!(k.document.feature_calls.len(), 1);
        let call = &k.document.feature_calls[0];
        assert_eq!(call.type_name, "sketch");
        assert_eq!(call.id.as_u32_if_flat(), Some(id));
        // And the sub-spine has the new point in it.
        let sub = call.params.get("sub_calls").unwrap().as_array().unwrap();
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0]["type"], serde_json::json!("sketch_point"));
    }

    #[test]
    fn replace_sketch_preserves_surrounding_primitive_calls() {
        let mut k = Kernel::new();
        let box_id = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                1.0,
                1.0,
                1.0,
            )
            .unwrap();
        let sketch_id = k.add_sketch("qCreatedBy(origin_plane_xy)", "[]").unwrap();
        assert_eq!(k.document.feature_calls.len(), 2);

        k.replace_sketch(
            sketch_id,
            "qCreatedBy(origin_plane_xy)",
            r#"[{ "type": "sketch_point", "id": "100", "params": { "at": [1.0, 2.0] } }]"#,
        )
        .unwrap();

        // Still two calls, both ids intact.
        assert_eq!(k.document.feature_calls.len(), 2);
        assert!(k
            .document
            .feature_calls
            .iter()
            .any(|c| c.type_name == "box"));
        assert!(k
            .document
            .feature_calls
            .iter()
            .any(|c| c.type_name == "sketch"));
        // And the primitive spine still has the original box.
        assert_eq!(k.document.features.len(), 1);
        assert_eq!(k.document.features[0].id.as_u32_if_flat(), Some(box_id));
    }

    // Error-path tests for `add_sketch` and `get_sketch_sub_calls`
    // would construct a `JsError`, which panics in non-wasm test
    // runs because `wasm-bindgen` imports aren't available — the
    // existing kernel error paths (`parse_op`, malformed JSON in
    // `load_json`, …) rely on wasm-pack test coverage for the same
    // reason. Happy-path correctness here is covered above.

    // ─── K7.4: add_extrude ───────────────────────────────────────

    #[test]
    fn add_extrude_lowers_the_referenced_sketch_into_primitive_extrude() {
        // Commit a rectangle sketch, then ask add_extrude to
        // lift it into a 3D slab. The resulting spine feature
        // must be a `Primitive::Extrude` whose `sketch` aabb
        // matches the rectangle.
        let mut k = Kernel::new();
        let sketch_id = k
            .add_sketch(
                "qCreatedBy(origin_plane_xy)",
                r#"[
                    {
                        "type": "sketch_rectangle",
                        "id": "r1",
                        "params": {
                            "corner_a": [-10.0, -5.0],
                            "corner_b": [10.0, 5.0]
                        }
                    }
                ]"#,
            )
            .unwrap();

        let extrude_id = k
            .add_extrude(sketch_id, 16.0, BoolOp::TAG_UNION, 0.0)
            .unwrap();
        assert_ne!(extrude_id, sketch_id);

        // One primitive on the spine — the extrude.
        // (Sketches don't push onto `features`.)
        assert_eq!(k.document.features.len(), 1);
        let extrude_spine = k
            .document
            .features
            .iter()
            .find(|f| f.id.as_u32_if_flat() == Some(extrude_id))
            .expect("extrude spine entry");
        match &extrude_spine.primitive {
            Primitive::Extrude {
                sketch,
                half_depth_z,
                center: _,
            } => {
                assert!((half_depth_z - 8.0).abs() < 1e-5);
                let aabb = sketch.aabb();
                assert!((aabb.min[0] - (-10.0)).abs() < 1e-5);
                assert!((aabb.max[1] - 5.0).abs() < 1e-5);
            }
            other => panic!("expected Primitive::Extrude, got {other:?}"),
        }

        // feature_calls still has both: the sketch call and
        // the new extrude call with its qCreatedBy query.
        assert_eq!(k.document.feature_calls.len(), 2);
        let extrude_call = k
            .document
            .feature_calls
            .iter()
            .find(|c| c.type_name == "extrude")
            .expect("extrude feature call");
        let profiles = extrude_call.params.get("profiles").unwrap();
        assert_eq!(
            profiles.as_str().unwrap(),
            format!("qCreatedBy({sketch_id})")
        );
    }

    #[test]
    fn add_extrude_respects_the_bool_op_argument() {
        let mut k = Kernel::new();
        let sketch_id = k
            .add_sketch(
                "qCreatedBy(origin_plane_xy)",
                r#"[{
                    "type": "sketch_rectangle",
                    "id": "r1",
                    "params": { "corner_a": [-1.0, -1.0], "corner_b": [1.0, 1.0] }
                }]"#,
            )
            .unwrap();
        k.add_extrude(sketch_id, 4.0, BoolOp::TAG_DIFFERENCE, 0.0).unwrap();
        let extrude_spine = k
            .document
            .features
            .iter()
            .find(|f| matches!(f.primitive, Primitive::Extrude { .. }))
            .expect("extrude spine entry");
        assert_eq!(extrude_spine.op, BoolOp::Difference);
    }

    // ─── K8.0: add_linear_pattern + list_features round trip ──

    #[test]
    fn add_linear_pattern_populates_list_features_with_source_and_pattern() {
        let mut k = Kernel::new();
        let box_id = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                10.0,
                10.0,
                10.0,
            )
            .unwrap();
        let pattern_id = k.add_linear_pattern(box_id, 1.0, 0.0, 0.0, 3, 40.0).unwrap();
        assert_ne!(box_id, pattern_id);

        // listFeatures walks feature_calls; only top-level
        // entries show up, not the hierarchical clone spine
        // entries pattern execute pushes.
        let list_json = k.list_features().unwrap();
        let list: serde_json::Value = serde_json::from_str(&list_json).unwrap();
        let arr = list.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let kinds: Vec<&str> = arr.iter().map(|f| f["kind"].as_str().unwrap()).collect();
        assert!(kinds.contains(&"box"));
        assert!(kinds.contains(&"linear_pattern"));

        // The spine has 3 entries: the box + two clones.
        assert_eq!(k.document.features.len(), 3);
    }

    // ─── K7.5: editable feature params ──────────────────────────

    #[test]
    fn replace_feature_call_params_updates_box_half_extents_in_place() {
        let mut k = Kernel::new();
        let id = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                10.0,
                10.0,
                10.0,
            )
            .unwrap();
        // Sanity: the current params reflect half_extents=[10,10,10].
        let before = k.get_feature_call_params(id).unwrap();
        let before_val: serde_json::Value = serde_json::from_str(&before).unwrap();
        let arr = before_val["half_extents"].as_array().unwrap();
        assert!((arr[0].as_f64().unwrap() - 10.0).abs() < 1e-5);

        // Replace with a wider box.
        let new_params = serde_json::json!({
            "half_extents": [25.0, 15.0, 5.0],
            "center": [0.0, 0.0, 0.0],
            "translate": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": 1.0,
            "op": "union",
        });
        k.replace_feature_call_params(id, &new_params.to_string())
            .unwrap();

        // The spine feature must carry the new extents, and the
        // feature id stays stable.
        assert_eq!(k.document.features.len(), 1);
        let feat = &k.document.features[0];
        assert_eq!(feat.id.as_u32_if_flat(), Some(id));
        match &feat.primitive {
            Primitive::Box { half_extents, .. } => {
                assert!((half_extents[0] - 25.0).abs() < 1e-5);
                assert!((half_extents[1] - 15.0).abs() < 1e-5);
                assert!((half_extents[2] - 5.0).abs() < 1e-5);
            }
            other => panic!("expected Primitive::Box, got {other:?}"),
        }

        // And the wire form round-trips the new values.
        let after = k.get_feature_call_params(id).unwrap();
        let after_val: serde_json::Value = serde_json::from_str(&after).unwrap();
        let after_arr = after_val["half_extents"].as_array().unwrap();
        assert!((after_arr[0].as_f64().unwrap() - 25.0).abs() < 1e-5);
    }

    // ─── primitive mutations + undo/redo ─────────────────────────────

    #[test]
    fn add_primitive_appends_and_returns_new_id() {
        let mut k = Kernel::new();
        let id1 = k
            .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0)
            .unwrap();
        let id2 = k
            .add_primitive(
                Primitive::KIND_SPHERE,
                BoolOp::TAG_DIFFERENCE,
                10.0,
                0.0,
                0.0,
                4.0,
                0.0,
                0.0,
            )
            .unwrap();
        assert_ne!(id1, id2);
        assert_eq!(k.feature_count(), 2);
    }

    #[test]
    fn remove_feature_returns_true_only_for_existing_ids() {
        let mut k = Kernel::new();
        let id = k
            .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
            .unwrap();
        assert!(k.remove_feature(id));
        assert!(!k.remove_feature(id));
        assert_eq!(k.feature_count(), 0);
    }

    #[test]
    fn undo_then_redo_restores_state() {
        let mut k = Kernel::new();
        k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
            .unwrap();
        k.add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_DIFFERENCE,
            0.0,
            0.0,
            0.0,
            0.5,
            0.0,
            0.0,
        )
        .unwrap();
        assert_eq!(k.feature_count(), 2);

        assert!(k.undo());
        assert_eq!(k.feature_count(), 1);
        assert!(k.undo());
        assert_eq!(k.feature_count(), 0);
        assert!(!k.undo()); // history exhausted

        assert!(k.redo());
        assert_eq!(k.feature_count(), 1);
        assert!(k.redo());
        assert_eq!(k.feature_count(), 2);
        assert!(!k.redo());
    }

    #[test]
    fn mutation_after_undo_clears_redo_stack() {
        let mut k = Kernel::new();
        k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
            .unwrap();
        k.add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_UNION,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
        )
        .unwrap();
        k.undo();
        assert!(k.can_redo());
        k.add_primitive(
            Primitive::KIND_CYLINDER,
            BoolOp::TAG_UNION,
            0.0,
            0.0,
            0.0,
            1.0,
            2.0,
            0.0,
        )
        .unwrap();
        // The fresh edit invalidates redo.
        assert!(!k.can_redo());
    }

    #[test]
    fn set_feature_op_updates_packed_buffer() {
        let mut k = Kernel::new();
        let id = k
            .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
            .unwrap();
        assert_eq!(k.packed[7], BoolOp::TAG_UNION as f32);
        k.set_feature_op(id, BoolOp::TAG_DIFFERENCE).unwrap();
        assert_eq!(k.packed[7], BoolOp::TAG_DIFFERENCE as f32);
    }

    #[test]
    fn reorder_moves_feature() {
        let mut k = Kernel::new();
        let a = k
            .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
            .unwrap();
        let b = k
            .add_primitive(
                Primitive::KIND_SPHERE,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                1.0,
                0.0,
                0.0,
            )
            .unwrap();
        // Move sphere (index 1) to index 0.
        assert!(k.reorder_feature(b, 0));
        let json = k.list_features().unwrap();
        assert!(json.find("\"id\":1").unwrap() > json.find("\"id\":2").unwrap(), "{json}");
        let _ = a;
    }

    #[test]
    fn list_features_serializes_summary() {
        let mut k = Kernel::new();
        k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
            .unwrap();
        let json = k.list_features().unwrap();
        assert!(json.contains("\"kind\":\"box\""));
        assert!(json.contains("\"op\":\"union\""));
    }

    #[test]
    fn pick_returns_miss_when_document_empty() {
        let k = Kernel::new();
        let json = k.pick(0.0, 100.0, 0.0, 0.0, -1.0, 0.0);
        assert!(json.contains("\"hit\":false"), "{json}");
    }

    #[test]
    fn pick_returns_face_with_single_contributor_for_box_top() {
        let mut k = Kernel::new();
        let id = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                10.0,
                10.0,
                10.0,
            )
            .unwrap();
        let json = k.pick(0.0, 100.0, 0.0, 0.0, -1.0, 0.0);
        assert!(json.contains("\"hit\":true"), "{json}");
        assert!(json.contains("\"element\":\"face\""), "{json}");
        assert!(
            json.contains(&format!("\"contributors\":[{id}]")),
            "{json}"
        );
    }

    #[test]
    fn new_kernel_has_empty_mesh_at_default_resolution() {
        let k = Kernel::new();
        assert_eq!(k.mesh_resolution(), DEFAULT_MESH_RESOLUTION);
        assert_eq!(k.mesh_vertex_count(), 0);
        assert_eq!(k.mesh_index_count(), 0);
        // Version starts at 1 because `new` seeds the mesh once.
        assert_eq!(k.mesh_version(), 1);
    }

    #[test]
    fn adding_a_primitive_rebuilds_the_mesh_and_bumps_version() {
        let mut k = Kernel::new();
        let v0 = k.mesh_version();
        k.add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_UNION,
            0.0,
            0.0,
            0.0,
            25.0,
            25.0,
            25.0,
        )
        .unwrap();
        assert!(k.mesh_vertex_count() > 0);
        assert!(k.mesh_index_count() > 0);
        assert_eq!(k.mesh_index_count() % 3, 0);
        assert!(k.mesh_version() > v0);
    }

    #[test]
    fn empty_document_has_zero_bodies() {
        let k = Kernel::new();
        assert_eq!(k.body_count(), 0);
    }

    #[test]
    fn two_disjoint_boxes_produce_two_bodies() {
        let mut k = Kernel::new();
        let id_a = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                10.0,
                10.0,
                10.0,
            )
            .unwrap();
        let id_b = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                // 500 mm away — completely disjoint.
                500.0,
                0.0,
                0.0,
                10.0,
                10.0,
                10.0,
            )
            .unwrap();
        assert_eq!(k.body_count(), 2);
        // Bodies are ordered by first appearance in the spine, so
        // body 0 is always the origin box.
        assert_eq!(k.body_id(0), id_a);
        assert_eq!(k.body_id(1), id_b);
        assert!(k.body_vertex_count(0) > 0);
        assert!(k.body_vertex_count(1) > 0);
        // Aggregate mesh concatenates both bodies.
        let aggregate = k.mesh_vertex_count();
        assert_eq!(aggregate, k.body_vertex_count(0) + k.body_vertex_count(1));
    }

    #[test]
    fn mutating_one_body_leaves_the_other_bodys_version_unchanged() {
        let mut k = Kernel::new();
        let id_a = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                10.0,
                10.0,
                10.0,
            )
            .unwrap();
        let id_b = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                500.0,
                0.0,
                0.0,
                10.0,
                10.0,
                10.0,
            )
            .unwrap();

        // Snapshot the per-body versions.
        let version_a_before = k.body_version(0);
        let version_b_before = k.body_version(1);
        assert_eq!(k.body_id(0), id_a);
        assert_eq!(k.body_id(1), id_b);

        // Nudge body B — its payload hash changes, A's doesn't.
        assert!(k.nudge_feature(id_b, 5.0, 0.0, 0.0));

        // A is untouched, B got re-meshed.
        assert_eq!(k.body_version(0), version_a_before, "body A must not remesh");
        assert!(
            k.body_version(1) > version_b_before,
            "body B must remesh on its own nudge"
        );
    }

    #[test]
    fn body_id_is_stable_across_an_unrelated_mutation() {
        let mut k = Kernel::new();
        let id_a = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                10.0,
                10.0,
                10.0,
            )
            .unwrap();
        let id_b = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                500.0,
                0.0,
                0.0,
                10.0,
                10.0,
                10.0,
            )
            .unwrap();
        assert_eq!(k.body_id(0), id_a);
        assert_eq!(k.body_id(1), id_b);

        // Mutate B. A's id and position in the bodies list must
        // survive unchanged.
        assert!(k.nudge_feature(id_b, 5.0, 0.0, 0.0));
        assert_eq!(k.body_id(0), id_a);
        assert_eq!(k.body_id(1), id_b);
    }

    #[test]
    fn bridging_union_merges_two_bodies_into_one() {
        let mut k = Kernel::new();
        let id_a = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                10.0,
                10.0,
                10.0,
            )
            .unwrap();
        let _id_b = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                60.0,
                0.0,
                0.0,
                10.0,
                10.0,
                10.0,
            )
            .unwrap();
        assert_eq!(k.body_count(), 2);
        // Add a bridge that overlaps both — they merge into one.
        let _id_bridge = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                30.0,
                0.0,
                0.0,
                40.0,
                10.0,
                10.0,
            )
            .unwrap();
        assert_eq!(k.body_count(), 1);
        // Merged body keeps the lowest seed id.
        assert_eq!(k.body_id(0), id_a);
    }

    #[test]
    fn lod_is_off_by_default_every_body_uses_active_resolution() {
        let mut k = Kernel::new();
        assert_eq!(k.active_body_id(), 0);
        let _ = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                25.0,
                25.0,
                25.0,
            )
            .unwrap();
        let _ = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                500.0,
                0.0,
                0.0,
                25.0,
                25.0,
                25.0,
            )
            .unwrap();
        // Two same-size boxes under identical resolution land in the
        // same order of magnitude — their exact vertex counts diverge
        // slightly because the voxel grid is anchored in world space
        // and f32 rounding treats `x ≈ 25` differently from `x ≈ 525`,
        // but they should be within 1% of each other.
        let a = k.body_vertex_count(0) as f32;
        let b = k.body_vertex_count(1) as f32;
        let ratio = (a - b).abs() / a.max(b);
        assert!(
            ratio < 0.02,
            "vertex counts drifted more than 2% ({a} vs {b})",
        );
    }

    #[test]
    fn setting_active_body_drops_other_bodies_to_context_resolution() {
        let mut k = Kernel::new();
        // Context res is deliberately much smaller than active res
        // so the effect is clearly visible in vertex counts.
        assert!(k.set_context_mesh_resolution(8));
        let id_a = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                25.0,
                25.0,
                25.0,
            )
            .unwrap();
        let id_b = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                500.0,
                0.0,
                0.0,
                25.0,
                25.0,
                25.0,
            )
            .unwrap();

        // Baseline: no active body → every body at active (48) res.
        let baseline_a = k.body_vertex_count(0);
        let baseline_b = k.body_vertex_count(1);

        // Set A as active → B should drop to context (8) resolution
        // and lose vertices. A stays near the baseline (bitwise
        // identical, since its LOD didn't change).
        assert!(k.set_active_body_id(id_a));
        assert_eq!(k.active_body_id(), id_a);
        assert_eq!(k.body_vertex_count(0), baseline_a, "active body unchanged");
        // Same-topology box at res 8 has ≈ 8×–12× fewer vertices.
        assert!(
            k.body_vertex_count(1) * 4 < baseline_b,
            "context res should be meaningfully coarser ({} vs baseline {})",
            k.body_vertex_count(1),
            baseline_b,
        );

        // Setting the same active body again is a no-op.
        assert!(!k.set_active_body_id(id_a));

        // Clearing the active body restores both to active res.
        assert!(k.clear_active_body_id());
        assert_eq!(k.body_vertex_count(0), baseline_a);
        assert_eq!(k.body_vertex_count(1), baseline_b);
        let _ = id_b;
    }

    #[test]
    fn switching_active_body_remeshes_only_the_two_that_changed_lod() {
        let mut k = Kernel::new();
        assert!(k.set_context_mesh_resolution(8));
        let id_a = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                25.0,
                25.0,
                25.0,
            )
            .unwrap();
        let id_b = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                500.0,
                0.0,
                0.0,
                25.0,
                25.0,
                25.0,
            )
            .unwrap();
        let id_c = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                1000.0,
                0.0,
                0.0,
                25.0,
                25.0,
                25.0,
            )
            .unwrap();
        // A is the active body; B and C fall to context LOD.
        assert!(k.set_active_body_id(id_a));
        let v_a = k.body_version(0);
        let v_b = k.body_version(1);
        let v_c = k.body_version(2);

        // Switch active from A to B. A drops to context, B gets
        // promoted to active. C's LOD is unchanged — still context.
        assert!(k.set_active_body_id(id_b));
        assert!(k.body_version(0) > v_a, "A re-meshed at context res");
        assert!(k.body_version(1) > v_b, "B re-meshed at active res");
        assert_eq!(k.body_version(2), v_c, "C untouched");
        let _ = id_c;
    }

    #[test]
    fn sync_meshing_off_keeps_bodies_partitioned_but_meshes_empty() {
        // K3 part 5: after the frontend disables sync meshing, the
        // main-thread kernel should still maintain the body list
        // (it's needed for setActiveBodyByFeature and the CSG tree
        // used by pick), but every body's mesh buffer must be
        // empty. Flipping sync meshing back on re-meshes everything.
        let mut k = Kernel::new();
        let id_a = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                25.0,
                25.0,
                25.0,
            )
            .unwrap();
        let id_b = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                500.0,
                0.0,
                0.0,
                25.0,
                25.0,
                25.0,
            )
            .unwrap();
        assert!(k.sync_meshing());
        assert_eq!(k.body_count(), 2);
        let populated_a = k.body_vertex_count(0);
        let populated_b = k.body_vertex_count(1);
        assert!(populated_a > 0 && populated_b > 0);

        // Flip off — body list survives, meshes vanish.
        assert!(k.set_sync_meshing(false));
        assert!(!k.sync_meshing());
        assert_eq!(k.body_count(), 2);
        assert_eq!(k.body_vertex_count(0), 0, "body A mesh cleared");
        assert_eq!(k.body_vertex_count(1), 0, "body B mesh cleared");
        // Setting to the same value again is a no-op.
        assert!(!k.set_sync_meshing(false));

        // Picking still works because it reads the feature list
        // directly, not the mesh — raycast from above hits the box.
        let json = k.pick(0.0, 100.0, 0.0, 0.0, -1.0, 0.0);
        assert!(json.contains("\"hit\":true"), "{json}");
        assert!(
            json.contains(&format!("\"contributors\":[{id_a}]")),
            "{json}"
        );

        // Mutations go through without meshing.
        assert!(k.nudge_feature(id_b, 5.0, 0.0, 0.0));
        assert_eq!(k.body_vertex_count(1), 0, "still empty after mutation");

        // Flip back on — bodies get re-meshed from scratch.
        assert!(k.set_sync_meshing(true));
        assert!(k.body_vertex_count(0) > 0);
        assert!(k.body_vertex_count(1) > 0);
    }

    #[test]
    fn set_active_body_by_feature_finds_the_owning_body() {
        let mut k = Kernel::new();
        assert!(k.set_context_mesh_resolution(8));
        let _id_a = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                25.0,
                25.0,
                25.0,
            )
            .unwrap();
        let id_b = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                500.0,
                0.0,
                0.0,
                25.0,
                25.0,
                25.0,
            )
            .unwrap();
        assert!(k.set_active_body_by_feature(id_b));
        assert_eq!(k.active_body_id(), id_b);
    }

    #[test]
    fn set_mesh_resolution_rebuilds_and_skips_noop() {
        let mut k = Kernel::new();
        k.add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_UNION,
            0.0,
            0.0,
            0.0,
            25.0,
            25.0,
            25.0,
        )
        .unwrap();
        let before = k.mesh_version();
        assert!(k.set_mesh_resolution(32));
        assert!(k.mesh_version() > before);
        // Same value → no-op, version unchanged.
        let after = k.mesh_version();
        assert!(!k.set_mesh_resolution(32));
        assert_eq!(k.mesh_version(), after);
    }

    // ─── K4.5.4c: Document.feature_calls dual-write invariant ─

    /// Helper: assert that `document.feature_calls` is a
    /// byte-identical projection of `document.features` via
    /// `FeatureCall::from_spine_feature`. The dual-write hook in
    /// `rebuild_packed` is supposed to hold this invariant after
    /// every mutation.
    fn assert_feature_calls_match_spine(k: &Kernel) {
        let doc = &k.document;
        assert_eq!(
            doc.feature_calls.len(),
            doc.features.len(),
            "feature_calls length diverged from features",
        );
        for (i, f) in doc.features.iter().enumerate() {
            let expected = fielddraft_protocol::FeatureCall::from_spine_feature(f);
            assert_eq!(
                doc.feature_calls[i], expected,
                "feature_calls[{i}] drifted from its spine feature",
            );
        }
    }

    #[test]
    fn add_primitive_populates_feature_calls() {
        let mut k = Kernel::new();
        assert!(k.document.feature_calls.is_empty());
        let _ = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                5.0,
                5.0,
                5.0,
            )
            .unwrap();
        assert_eq!(k.document.feature_calls.len(), 1);
        assert_eq!(k.document.feature_calls[0].type_name, "box");
        assert_feature_calls_match_spine(&k);
    }

    #[test]
    fn every_mutation_keeps_feature_calls_in_sync() {
        let mut k = Kernel::new();
        let id_box = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                5.0,
                5.0,
                5.0,
            )
            .unwrap();
        let id_sphere = k
            .add_primitive(
                Primitive::KIND_SPHERE,
                BoolOp::TAG_UNION,
                100.0,
                0.0,
                0.0,
                3.0,
                0.0,
                0.0,
            )
            .unwrap();
        let id_cyl = k
            .add_primitive(
                Primitive::KIND_CYLINDER,
                BoolOp::TAG_UNION,
                0.0,
                200.0,
                0.0,
                2.0,
                4.0,
                0.0,
            )
            .unwrap();
        assert_feature_calls_match_spine(&k);

        // Mutation 1: translate the box — feature_calls[0].params
        // must pick up the new translate.
        assert!(k.set_feature_translate(id_box, 10.0, 20.0, 30.0));
        assert_feature_calls_match_spine(&k);
        assert_eq!(
            k.document.feature_calls[0].params["translate"],
            serde_json::json!([10.0, 20.0, 30.0])
        );

        // Mutation 2: nudge the sphere.
        assert!(k.nudge_feature(id_sphere, 1.0, 2.0, 3.0));
        assert_feature_calls_match_spine(&k);

        // Mutation 3: change the cylinder's op.
        k.set_feature_op(id_cyl, BoolOp::TAG_DIFFERENCE).unwrap();
        assert_feature_calls_match_spine(&k);
        let cyl_call_idx = k
            .document
            .features
            .iter()
            .position(|f| f.id == FeatureId::from_u32(id_cyl))
            .unwrap();
        assert_eq!(
            k.document.feature_calls[cyl_call_idx].params["op"],
            serde_json::json!("difference")
        );

        // Mutation 4: reorder the cylinder to the front.
        assert!(k.reorder_feature(id_cyl, 0));
        assert_feature_calls_match_spine(&k);
        assert_eq!(k.document.features[0].id, FeatureId::from_u32(id_cyl));

        // Mutation 5: remove the box.
        assert!(k.remove_feature(id_box));
        assert_feature_calls_match_spine(&k);
        assert_eq!(k.document.features.len(), 2);

        // Mutation 6: full transform write on the sphere.
        assert!(k.set_feature_transform(
            id_sphere, 7.0, 8.0, 9.0, 0.0, 0.0, 0.0, 1.0, 2.0
        ));
        assert_feature_calls_match_spine(&k);
    }

    #[test]
    fn undo_and_redo_restore_feature_calls_in_sync() {
        // Undo/redo swaps the whole document snapshot, so
        // `feature_calls` travels with it automatically — no extra
        // instrumentation needed. This test locks that behaviour.
        let mut k = Kernel::new();
        k.add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_UNION,
            0.0,
            0.0,
            0.0,
            1.0,
            1.0,
            1.0,
        )
        .unwrap();
        assert_feature_calls_match_spine(&k);

        k.add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_UNION,
            0.0,
            0.0,
            0.0,
            2.0,
            0.0,
            0.0,
        )
        .unwrap();
        assert_eq!(k.document.feature_calls.len(), 2);
        assert_feature_calls_match_spine(&k);

        assert!(k.undo());
        assert_eq!(k.document.feature_calls.len(), 1);
        assert_feature_calls_match_spine(&k);

        assert!(k.redo());
        assert_eq!(k.document.feature_calls.len(), 2);
        assert_feature_calls_match_spine(&k);

        // A mutation after an undo/redo cycle also stays in sync.
        assert!(k.nudge_feature(1, 1.0, 0.0, 0.0));
        assert_feature_calls_match_spine(&k);
    }

    #[test]
    fn load_json_rebuilds_feature_calls_from_features() {
        // Loading a legacy K1 JSON (no feature_calls field) goes
        // through `rebuild_packed`, which calls
        // `sync_feature_calls`, so the mirror is populated before
        // the loaded document is exposed to any caller.
        let mut k = Kernel::new();
        let legacy = r#"{
            "schema_version": 2,
            "name": "legacy",
            "features": [{
                "id": 1,
                "name": "Base",
                "op": "union",
                "primitive": {
                    "kind": "box",
                    "center": [0, 0, 0],
                    "half_extents": [5, 5, 5]
                },
                "transform": {
                    "translate": [0, 0, 0],
                    "rotation": [0, 0, 0, 1],
                    "scale": 1
                }
            }],
            "next_feature_id": 2
        }"#;
        k.load_json(legacy).unwrap();
        assert_eq!(k.document.features.len(), 1);
        assert_eq!(k.document.feature_calls.len(), 1);
        assert_feature_calls_match_spine(&k);
    }

    // ─── K4.5.4d: bootstrap from feature_calls ─────────────────

    #[test]
    fn load_json_bootstraps_features_from_feature_calls() {
        // Fabricated step-4d wire form: `feature_calls` populated,
        // `features` empty. The loader detects the heuristic,
        // walks the registry, and populates `features` before
        // `rebuild_packed` runs.
        let mut k = Kernel::new();
        let json = r#"{
            "schema_version": 2,
            "name": "step4d",
            "features": [],
            "feature_calls": [
                {
                    "type": "box",
                    "id": 1,
                    "params": {
                        "half_extents": [5, 5, 5],
                        "center": [0, 0, 0],
                        "translate": [0, 0, 0],
                        "rotation": [0, 0, 0, 1],
                        "scale": 1.0,
                        "op": "union",
                        "name": "Base"
                    }
                },
                {
                    "type": "sphere",
                    "id": 2,
                    "params": {
                        "radius": 3.0,
                        "center": [0, 0, 0],
                        "translate": [20, 0, 0],
                        "rotation": [0, 0, 0, 1],
                        "scale": 1.0,
                        "op": "difference",
                        "name": "Hole"
                    }
                }
            ],
            "next_feature_id": 3
        }"#;

        k.load_json(json).unwrap();
        assert_eq!(k.document.features.len(), 2);

        let box_f = &k.document.features[0];
        assert_eq!(box_f.name, "Base");
        assert!(matches!(
            box_f.primitive,
            Primitive::Box { half_extents, .. } if half_extents == [5.0, 5.0, 5.0]
        ));

        let sphere_f = &k.document.features[1];
        assert_eq!(sphere_f.name, "Hole");
        assert!(matches!(
            sphere_f.primitive,
            Primitive::Sphere { radius, .. } if radius == 3.0
        ));
        assert_eq!(sphere_f.transform.translate, [20.0, 0.0, 0.0]);
        assert_eq!(sphere_f.op, BoolOp::Difference);

        // feature_calls should still be populated after rebuild_packed
        // — the dual-write hook rebuilds them from the freshly
        // regenerated features, which is a round-trip no-op thanks
        // to the lossless primitive execute impls.
        assert_feature_calls_match_spine(&k);
    }

    #[test]
    fn bootstrap_preserves_rotation_and_scale_for_box() {
        // Build a document with a non-identity transform on a box,
        // capture its feature_calls, then load them fresh via the
        // bootstrap path (with `features` cleared) and verify
        // rotation and scale survive the round-trip.
        let mut k1 = Kernel::new();
        let id = k1
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                10.0,
                20.0,
                30.0,
                5.0,
                5.0,
                5.0,
            )
            .unwrap();
        // Rotation for 45° about Z: (0, 0, sin(22.5°), cos(22.5°))
        let s = (std::f32::consts::FRAC_PI_4 / 2.0).sin();
        let c = (std::f32::consts::FRAC_PI_4 / 2.0).cos();
        assert!(k1.set_feature_transform(
            id, 100.0, 200.0, 300.0, 0.0, 0.0, s, c, 2.5,
        ));

        // Capture the JSON, zero out `features` to simulate a
        // step-4d wire form, and reload.
        let json = k1.to_json().unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["features"] = serde_json::json!([]);
        let bootstrap_json = serde_json::to_string(&value).unwrap();

        let mut k2 = Kernel::new();
        k2.load_json(&bootstrap_json).unwrap();
        assert_eq!(k2.document.features.len(), 1);

        let loaded = &k2.document.features[0];
        assert_eq!(loaded.transform.translate, [100.0, 200.0, 300.0]);
        assert!((loaded.transform.rotation[0]).abs() < 1e-6);
        assert!((loaded.transform.rotation[1]).abs() < 1e-6);
        assert!((loaded.transform.rotation[2] - s).abs() < 1e-6);
        assert!((loaded.transform.rotation[3] - c).abs() < 1e-6);
        assert!((loaded.transform.scale - 2.5).abs() < 1e-6);
        assert_eq!(loaded.op, BoolOp::Union);
    }

    #[test]
    fn bootstrap_preserves_mixed_scene_across_primitive_types() {
        // One box, one sphere, one cylinder, each with a unique
        // translate and a custom op. Round-trip via the bootstrap
        // path and check every field.
        let mut k1 = Kernel::new();
        k1.add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_UNION,
            10.0,
            0.0,
            0.0,
            5.0,
            5.0,
            5.0,
        )
        .unwrap();
        let id_sphere = k1
            .add_primitive(
                Primitive::KIND_SPHERE,
                BoolOp::TAG_DIFFERENCE,
                20.0,
                0.0,
                0.0,
                3.0,
                0.0,
                0.0,
            )
            .unwrap();
        let _ = id_sphere;
        k1.add_primitive(
            Primitive::KIND_CYLINDER,
            BoolOp::TAG_INTERSECT,
            0.0,
            30.0,
            0.0,
            2.0,
            4.0,
            0.0,
        )
        .unwrap();
        let original_features = k1.document.features.clone();

        // Zero out `features` and reload.
        let json = k1.to_json().unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["features"] = serde_json::json!([]);
        let bootstrap_json = serde_json::to_string(&value).unwrap();

        let mut k2 = Kernel::new();
        k2.load_json(&bootstrap_json).unwrap();
        assert_eq!(k2.document.features.len(), 3);

        // Every reloaded feature should match its pre-bootstrap
        // counterpart byte for byte.
        for (original, loaded) in
            original_features.iter().zip(k2.document.features.iter())
        {
            assert_eq!(loaded.id, original.id);
            assert_eq!(loaded.name, original.name);
            assert_eq!(loaded.op, original.op);
            assert_eq!(loaded.primitive, original.primitive);
            assert_eq!(loaded.transform.translate, original.transform.translate);
            assert_eq!(loaded.transform.rotation, original.transform.rotation);
            assert_eq!(loaded.transform.scale, original.transform.scale);
        }
        assert_feature_calls_match_spine(&k2);
    }

    #[test]
    fn load_json_clears_history() {
        let mut k = Kernel::new();
        k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
            .unwrap();
        assert!(k.can_undo());
        let json = k.to_json().unwrap();
        k.load_json(&json).unwrap();
        assert!(!k.can_undo());
        assert!(!k.can_redo());
    }

    // ─── K4.5.4e: features-only wire form ──────────────────────

    #[test]
    fn to_json_does_not_emit_features_field() {
        // K4.5.4e: the Kernel's save path must emit a document
        // whose JSON contains `feature_calls` but no top-level
        // `features` key. The spine lives in-memory only.
        let mut k = Kernel::new();
        k.add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_UNION,
            1.0,
            2.0,
            3.0,
            4.0,
            5.0,
            6.0,
        )
        .unwrap();
        k.add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_DIFFERENCE,
            7.0,
            8.0,
            9.0,
            1.5,
            0.0,
            0.0,
        )
        .unwrap();

        let json = k.to_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = value.as_object().expect("document is a JSON object");
        assert!(
            !obj.contains_key("features"),
            "K4.5.4e wire form must omit `features`: {json}"
        );
        let calls = obj
            .get("feature_calls")
            .and_then(|v| v.as_array())
            .expect("feature_calls must be present");
        assert_eq!(calls.len(), 2);
    }

    #[test]
    fn kernel_save_load_roundtrips_via_feature_calls_only() {
        // Build a non-trivial scene, save, load into a fresh
        // kernel, and verify the spine is fully rehydrated from
        // the feature_calls wire form alone (no `features` field
        // in the JSON).
        let mut k1 = Kernel::new();
        let box_id = k1
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0,
                0.0,
                0.0,
                10.0,
                10.0,
                10.0,
            )
            .unwrap();
        k1.add_primitive(
            Primitive::KIND_CYLINDER,
            BoolOp::TAG_DIFFERENCE,
            5.0,
            0.0,
            0.0,
            2.0,
            12.0,
            0.0,
        )
        .unwrap();
        // Give the box a non-identity transform so we exercise
        // the lossless rotation/scale bootstrap path.
        let s = (std::f32::consts::FRAC_PI_4 / 2.0).sin();
        let c = (std::f32::consts::FRAC_PI_4 / 2.0).cos();
        assert!(k1.set_feature_transform(
            box_id, 1.0, 2.0, 3.0, 0.0, 0.0, s, c, 1.75,
        ));
        let original_features = k1.document.features.clone();

        // Save through the kernel and reality-check that the
        // wire form doesn't carry the spine.
        let json = k1.to_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(!value.as_object().unwrap().contains_key("features"));

        // Load into a fresh kernel and verify the spine.
        let mut k2 = Kernel::new();
        k2.load_json(&json).unwrap();
        assert_eq!(k2.document.features.len(), 2);
        for (original, loaded) in
            original_features.iter().zip(k2.document.features.iter())
        {
            assert_eq!(loaded.id, original.id);
            assert_eq!(loaded.name, original.name);
            assert_eq!(loaded.op, original.op);
            assert_eq!(loaded.primitive, original.primitive);
            assert_eq!(loaded.transform.translate, original.transform.translate);
            assert_eq!(loaded.transform.rotation, original.transform.rotation);
            assert_eq!(loaded.transform.scale, original.transform.scale);
        }
        assert_feature_calls_match_spine(&k2);
        // And the meshing pipeline woke up — rebuild_packed ran
        // on load, so the mesh has real geometry.
        assert!(k2.mesh_vertex_count() > 0);
    }
}
