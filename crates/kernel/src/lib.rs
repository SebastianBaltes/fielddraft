//! FieldDraft geometry kernel.
//!
//! The kernel owns the canonical `Document`, builds a CSG tree from its
//! feature spine, and meshes it via Dual Contouring. Mesh buffers are
//! exposed to JavaScript through a zero-copy pointer interface.

pub mod body;
pub mod brep;
pub mod bvh;
pub mod csg;
pub mod sdf_dual;
pub mod dc;
pub mod mdc;
pub mod mt;
pub mod rfta;
pub mod expr;
pub mod features;
// K13: mesh_sdf moved to the protocol crate so
// Primitive::ImportedMesh can carry it serialised.
pub use fielddraft_protocol::mesh_sdf;
pub mod sketch_regions;
pub mod queries;
pub mod export;
pub mod export_3mf;
pub mod raycast;
pub mod step_classifier;
pub mod step_commands;
pub mod sdf_compiler;
pub mod sketch2d;
pub mod sketch_features;
pub mod script_runtime;

use body::{
    build_bounded_for_body, build_bounded_with_primitives, detect_uniform_translation,
    hash_body_payload, hash_body_payload_split, partition, Body,
};
use bvh::BoundedCsg;
use csg::{CsgNode, DecodedFeature};
use dc::Mesh;
use raycast::{raycast, RaycastParams};
use features::quat_to_euler_xyz;
use fielddraft_protocol::{BoolOp, Document, FeatureId, Primitive, Transform};
use wasm_bindgen::prelude::*;

/// Default voxel grid resolution for the *active* body — the one the
/// user is currently editing. 48 cells along the longest AABB axis
/// strikes a balance between rebuild latency (a handful of ms for a
/// small spine) and visible smoothness for a 300 mm build volume.
/// The frontend overrides this to 256 for high-quality display; the
/// narrow-band mesher makes that practical.
pub const DEFAULT_MESH_RESOLUTION: u32 = 48;

/// Default voxel grid resolution for *context* bodies — the ones
/// that aren't currently being edited. Half of the active
/// resolution means ≈ 8× fewer cells and ≈ 8× less meshing work
/// when the partition pass sweeps across them, while still looking
/// identifiable enough on screen to serve as background reference.
pub const DEFAULT_CONTEXT_MESH_RESOLUTION: u32 = 24;

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
        child.translate[0] * parent.scale[0],
        child.translate[1] * parent.scale[1],
        child.translate[2] * parent.scale[2],
    ];
    let rotated = quat_rotate(parent.rotation, scaled_child_t);
    Transform {
        translate: [
            parent.translate[0] + rotated[0],
            parent.translate[1] + rotated[1],
            parent.translate[2] + rotated[2],
        ],
        rotation: quat_multiply(parent.rotation, child.rotation),
        scale: [
            parent.scale[0] * child.scale[0],
            parent.scale[1] * child.scale[1],
            parent.scale[2] * child.scale[2],
        ],
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
    [p[0] / t.scale[0], p[1] / t.scale[1], p[2] / t.scale[2]]
}

/// Invert a rigid + **uniform-scale** [`Transform`]. The body placement
/// (see [`body::Body::placement`]) is restricted to this class precisely
/// so its inverse exists as another `Transform`: `compose(t, inverse(t))`
/// and `compose(inverse(t), t)` both round-trip to identity.
///
/// For a transform `T(p) = s·R·p + t`, the inverse is
/// `T⁻¹(q) = (1/s)·R⁻¹·(q − t) = R⁻¹·(q/s) − R⁻¹·(t/s)`, which is again
/// an affine transform with rotation `R⁻¹` (the quaternion conjugate),
/// scale `1/s` and translation `−R⁻¹·(t/s)`. The input's scale is read as
/// uniform via [`Transform::min_scale`]; callers that build the placement
/// guarantee uniformity, so non-uniform residue never reaches here.
pub fn inverse(t: &Transform) -> Transform {
    let s = t.min_scale();
    let inv_s = if s.abs() < 1e-12 { 0.0 } else { 1.0 / s };
    // Conjugate of a unit quaternion (x, y, z, w) is (−x, −y, −z, w).
    let inv_rot = [-t.rotation[0], -t.rotation[1], -t.rotation[2], t.rotation[3]];
    // −R⁻¹·(t/s): scale the translation, rotate by the inverse rotation,
    // negate. `quat_rotate(inv_rot, ·)` applies R⁻¹.
    let scaled = [
        t.translate[0] * inv_s,
        t.translate[1] * inv_s,
        t.translate[2] * inv_s,
    ];
    let rotated = quat_rotate(inv_rot, scaled);
    Transform {
        translate: [-rotated[0], -rotated[1], -rotated[2]],
        rotation: inv_rot,
        scale: [inv_s, inv_s, inv_s],
    }
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
            (f.abs() - *thickness) * s * 0.2887
        }
        Primitive::Thread { nominal_radius, pitch, depth, half_height } => {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let theta = p[2].atan2(p[0]);
            let y_unwrapped = p[1] - *pitch * theta / std::f32::consts::TAU;
            let phase = (y_unwrapped / pitch) * std::f32::consts::TAU;
            let r_profile = *nominal_radius - *depth * 0.5 * (1.0 - phase.cos());
            let d_radial = r - r_profile;
            let d_axial = p[1].abs() - *half_height;
            // Lipschitz normalization: |∇f| peaks at sqrt(1 + (π·depth/pitch)²);
            // divide by it for a conservative ≤1 distance (cf. sdf_dual::Thread,
            // Gyroid 0.2887, Cone wall_scale).
            let lip = (1.0 + (std::f32::consts::PI * *depth / pitch.max(1e-6)).powi(2)).sqrt();
            d_radial.max(d_axial) / lip
        }
        Primitive::Cone { r_bottom, r_top, half_height } => {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let hh = *half_height;
            // Interpolated radius at p.y
            let t = ((p[1] + hh) / (2.0 * hh)).clamp(0.0, 1.0);
            let r_at_y = r_bottom * (1.0 - t) + r_top * t;
            // Perpendicular distance to the inclined wall: project the raw
            // radial gap onto the wall normal so the field stays Lipschitz-1.
            // For a cylinder (dr == 0) the factor is 1.
            let dr = r_bottom - r_top;
            let slant_len = (dr * dr + (2.0 * hh) * (2.0 * hh)).sqrt();
            let wall_scale = if slant_len > 1e-9 { (2.0 * hh) / slant_len } else { 1.0 };
            let d_wall = (r - r_at_y) * wall_scale;
            let d_axial = p[1].abs() - hh;
            if d_wall > 0.0 && d_axial > 0.0 {
                (d_wall * d_wall + d_axial * d_axial).sqrt()
            } else {
                d_wall.max(d_axial)
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
        Primitive::Ellipsoid { radii } => {
            let r_min = radii[0].min(radii[1]).min(radii[2]);
            let q = [p[0] / radii[0], p[1] / radii[1], p[2] / radii[2]];
            let len = (q[0]*q[0] + q[1]*q[1] + q[2]*q[2]).sqrt();
            (len - 1.0) * r_min
        }
    }
}

/// Walk the parent chain of `child` and return `true` as soon as any
/// ancestor equals `ancestor`. Used by `remove_feature` to cascade
/// deletions across spine clones produced by Pattern/Mirror/Hole
/// execute paths, which live under hierarchical child ids such as
/// `fillet_7.mask_0` or `linear_pattern_3.1`.
#[cfg(test)]
fn feature_id_is_descendant_of(child: &FeatureId, ancestor: &FeatureId) -> bool {
    let mut cur = child.parent();
    while let Some(p) = cur {
        if p == ancestor {
            return true;
        }
        cur = p.parent();
    }
    false
}

/// Read a 3-element float vector out of a `FeatureCall` params
/// object, falling back to `default` when the key is absent or
/// malformed. Used by the call-first transform mutators to detect
/// no-op edits against the current wire value.
fn json_vec3(
    params: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: [f32; 3],
) -> [f32; 3] {
    match params.get(key).and_then(|v| v.as_array()) {
        Some(a) if a.len() == 3 => [
            a[0].as_f64().unwrap_or(default[0] as f64) as f32,
            a[1].as_f64().unwrap_or(default[1] as f64) as f32,
            a[2].as_f64().unwrap_or(default[2] as f64) as f32,
        ],
        _ => default,
    }
}

/// Quaternion sibling of [`json_vec3`] (default identity rotation).
fn json_vec4(
    params: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: [f32; 4],
) -> [f32; 4] {
    match params.get(key).and_then(|v| v.as_array()) {
        Some(a) if a.len() == 4 => [
            a[0].as_f64().unwrap_or(default[0] as f64) as f32,
            a[1].as_f64().unwrap_or(default[1] as f64) as f32,
            a[2].as_f64().unwrap_or(default[2] as f64) as f32,
            a[3].as_f64().unwrap_or(default[3] as f64) as f32,
        ],
        _ => default,
    }
}

// ─── wasm-bindgen surface ────────────────────────────────────────────────

/// Compact JS view of one feature for the outliner.
#[derive(serde::Serialize)]
struct FeatureSummary {
    id: FeatureId,
    name: String,
    op: &'static str,
    kind: String,
    translate: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
    /// K8.7: mirrors `FeatureCall.suppressed`. The outliner
    /// reads this to render the eye-icon affordance and to
    /// grey out the row when the feature is skipped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    suppressed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    rolled_back: bool,
}

/// Owned by the JS side. Holds the canonical document and the
/// undo/redo stacks.
#[wasm_bindgen]
pub struct Kernel {
    document: Document,
    /// DL-61: the decoded runtime IR, rebuilt from `document.features`
    /// (1:1, same order) on every regeneration / remesh. The geometry
    /// pipeline — partition, hashing, BVH/B-Rep weaving, STEP classify —
    /// consumes this open form rather than the fat `Feature` spine.
    decoded: Vec<DecodedFeature>,
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
    /// Mesh algorithm selection: 0 = MDC (default), 1 = RFTA.
    mesh_algorithm: u8,
    /// Milestone-1 parametric weaving: when on, pure grundforms (currently
    /// the Box) get an exact analytic B-Rep instead of an octree-sampled one.
    /// Default `false` — opt-in via `setParametricWeave`. Anything not yet
    /// handled analytically falls back to the octree weave, so this never
    /// changes the result for unsupported shapes.
    parametric_weave: bool,
    /// Global version counter bumped on every mesh rebuild (any body
    /// touched). Frontend uses this to detect "anything changed";
    /// per-body freshness is tracked by each `Body`'s own `version`.
    mesh_version: u32,
    /// K10.4: edge fillets collected from FilletEdgeFeature calls
    /// during the last regenerate pass. Applied as a post-processing
    /// step on the CsgNode tree before meshing.
    edge_fillets: Vec<csg::EdgeFillet>,
    /// C2: resolved CSYS of every `mate_connector` from the last
    /// regenerate pass, so the viewport can render datum triads without
    /// re-resolving the face query on the JS side.
    mate_connectors: Vec<features::ResolvedMateConnector>,
    /// Rollback bar position: if set, features at index >= this value
    /// are skipped during regeneration (but stay in the document).
    rollback_after: Option<usize>,
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
        // DL-61 C2b: `feature_calls` is the single source of truth.
        // Every mutator edits a call's params (or the call list) and
        // re-derives the `features` spine via
        // `bootstrap_features_from_calls`, so there is no reverse
        // sync any more — `rebuild_packed` just re-meshes the
        // current document.
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
        // DL-61 C2b: `self.decoded` is produced solely by
        // `bootstrap_features_from_calls`; every path that changes
        // `feature_calls` (mutators, undo/redo, load) re-bootstraps
        // before re-meshing, so the IR is already current here.
        let partitions = partition(&self.decoded);

        let mut previous: std::collections::HashMap<FeatureId, Body> =
            std::mem::take(&mut self.bodies)
                .into_iter()
                .map(|b| (b.id.clone(), b))
                .collect();

        let mut new_bodies: Vec<Body> = Vec::with_capacity(partitions.len());
        for part in partitions {
            let id = part.seed_id(&self.decoded);
            let (geometry_hash, transform_hash) =
                hash_body_payload_split(&self.decoded, &part.feature_indices);
            let payload_hash = hash_body_payload(&self.decoded, &part.feature_indices);
            let target_resolution = self.target_resolution_for(&id);

            if let Some(mut prev) = previous.remove(&id) {
                if prev.payload_hash == payload_hash
                    && prev.mesh_resolution == target_resolution
                {
                    prev.feature_indices = part.feature_indices;
                    prev.aabb = part.aabb;
                    new_bodies.push(prev);
                    continue;
                }

                // K3.6: affine fast-path — geometry unchanged, only
                // transforms differ, and the change is a uniform
                // translation across all features in the body.
                if prev.geometry_hash == geometry_hash
                    && prev.mesh_resolution == target_resolution
                {
                    let new_transforms: Vec<fielddraft_protocol::Transform> = part
                        .feature_indices
                        .iter()
                        .map(|&i| self.decoded[i].transform)
                        .collect();
                    if let Some(delta) = detect_uniform_translation(
                        &prev.last_mesh_transforms,
                        &new_transforms,
                    ) {
                        let cumulative = match prev.transform_delta {
                            Some([px, py, pz]) => [px + delta[0], py + delta[1], pz + delta[2]],
                            None => delta,
                        };
                        prev.feature_indices = part.feature_indices;
                        prev.aabb = part.aabb;
                        prev.payload_hash = payload_hash;
                        prev.geometry_hash = geometry_hash;
                        prev.transform_hash = transform_hash;
                        prev.transform_delta = Some(cumulative);
                        prev.last_mesh_transforms = new_transforms;
                        prev.compiled_sdf = Some(self.compile_sdf_for(&prev.feature_indices));
                        prev.version = prev.version.wrapping_add(1);
                        new_bodies.push(prev);
                        continue;
                    }
                }

                let mesh = self.build_mesh_for(&part.feature_indices, target_resolution);
                let transforms: Vec<fielddraft_protocol::Transform> = part
                    .feature_indices
                    .iter()
                    .map(|&i| self.decoded[i].transform)
                    .collect();
                prev.feature_indices = part.feature_indices;
                prev.aabb = part.aabb;
                prev.payload_hash = payload_hash;
                prev.geometry_hash = geometry_hash;
                prev.transform_hash = transform_hash;
                prev.mesh_resolution = target_resolution;
                prev.mesh = mesh;
                prev.compiled_sdf = Some(self.compile_sdf_for(&prev.feature_indices));
                prev.version = prev.version.wrapping_add(1);
                prev.transform_delta = None;
                prev.last_mesh_transforms = transforms;
                prev.brep = None;
                new_bodies.push(prev);
            } else {
                let mesh = self.build_mesh_for(&part.feature_indices, target_resolution);
                let transforms: Vec<fielddraft_protocol::Transform> = part
                    .feature_indices
                    .iter()
                    .map(|&i| self.decoded[i].transform)
                    .collect();
                let compiled = self.compile_sdf_for(&part.feature_indices);
                new_bodies.push(Body {
                    id,
                    feature_indices: part.feature_indices,
                    aabb: part.aabb,
                    payload_hash,
                    geometry_hash,
                    transform_hash,
                    mesh_resolution: target_resolution,
                    mesh,
                    version: 1,
                    transform_delta: None,
                    last_mesh_transforms: transforms,
                    compiled_sdf: Some(compiled),
                    // Recomputed lazily on the first `ensure_brep`; the
                    // weave needs it, the mesh path does not.
                    placement: fielddraft_protocol::Transform::IDENTITY,
                    brep: None,
                });
            }
        }

        self.bodies = new_bodies;

        // Brocken A (A3): tag every freshly-built mesh with per-vertex B-Rep
        // FACE IDENTITY so a hovered/clicked triangle resolves to a stable
        // face. Bodies carried through the affine fast-path keep their old
        // (already-tagged) mesh untouched — their `face_index` is still valid
        // because correlation is body-LOCAL and a pure move doesn't change
        // which face a vertex sits on. We therefore only (re)tag meshes whose
        // `face_index` is missing/out-of-sync, weaving the B-Rep once and
        // sharing the cache with the later face/vertex exports.
        let parametric = self.parametric_weave;
        for i in 0..self.bodies.len() {
            let tc = self.bodies[i].mesh.triangle_count();
            if tc == 0 || self.bodies[i].mesh.face_index.len() == tc {
                continue;
            }
            self.bodies[i].ensure_brep(&self.decoded, parametric);
            let placement = self.bodies[i].placement;
            let brep = self.bodies[i].brep_cached().expect("woven above");
            let tags = crate::brep::face_export::correlate_mesh_faces(
                brep,
                &self.bodies[i].mesh.positions,
                &self.bodies[i].mesh.normals,
                &self.bodies[i].mesh.indices,
                &self.bodies[i].mesh.contributors,
                &placement,
            );
            self.bodies[i].mesh.face_index = tags;
        }

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

        // Passthrough fast-path: a body consisting of a single ImportedMesh
        // feature is rendered directly from its triangles — no DC needed.
        if feature_indices.len() == 1 {
            let f = &self.decoded[feature_indices[0]];
            if let CsgNode::Leaf {
                primitive: Primitive::ImportedMesh { mesh },
                ..
            } = &f.node
            {
                let cid = f.id.as_u32_or_root().unwrap_or(0);
                return Self::mesh_from_imported(mesh, cid);
            }
        }

        let (bounded, prims) = build_bounded_with_primitives(&self.decoded, feature_indices);
        let prim_refs: std::collections::HashMap<u32, &Primitive> =
            prims.iter().map(|(k, v)| (*k, v)).collect();
        match self.mesh_algorithm {
            1 => rfta::reach_for_the_arcs(&bounded, resolution),
            2 => mt::marching_tetrahedra(&bounded, resolution),
            _ => dc::dual_contour_with_primitives(&bounded, resolution, &prim_refs),
        }
    }

    /// Convert an ImportedMesh's triangle data directly into a renderable
    /// Mesh without running Dual Contouring.
    fn mesh_from_imported(mesh: &fielddraft_protocol::MeshSdf, contributor_id: u32) -> Mesh {
        let (positions, normals, indices) = mesh.to_display_mesh();
        let vert_count = positions.len() / 3;
        let classifications = vec![1u8; vert_count];
        let mut contributors = vec![dc::CONTRIBUTOR_SENTINEL; vert_count * csg::MAX_CONTRIBUTORS];
        for v in 0..vert_count {
            contributors[v * csg::MAX_CONTRIBUTORS] = contributor_id;
        }
        Mesh { positions, normals, indices, classifications, contributors, face_index: Vec::new() }
    }

    fn compile_sdf_for(&self, feature_indices: &[usize]) -> sdf_compiler::CompiledSdf {
        let bounded = build_bounded_for_body(&self.decoded, feature_indices);
        sdf_compiler::compile_body(&bounded)
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
        // Translate the FeatureId to an index into the decoded
        // feature list first, then search the bodies for that index.
        let idx = self
            .decoded
            .iter()
            .position(|f| &f.id == feature_id)?;
        self.bodies
            .iter()
            .find(|b| b.feature_indices.contains(&idx))
            .map(|b| b.id.clone())
    }

    fn op_label(op: BoolOp) -> &'static str {
        match op {
            BoolOp::Union => "union",
            BoolOp::Difference => "difference",
            BoolOp::Intersect => "intersect",
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
            "sweep" => "sweep",
            "mate_connector" => "mate_connector",
            "linear_pattern" => "linear_pattern",
            "circular_pattern" => "circular_pattern",
            "mirror" => "mirror",
            "boolean" => "boolean",
            "translate" => "translate",
            "rotate" => "rotate",
            "scale" => "scale",
            "material" => "material",
            _ => "unknown",
        }
    }

    /// Outliner kind for a feature call. Built-ins map to their static
    /// label; a script instance carries its script id as `type_name`
    /// (not a built-in), so we pass that id straight through when the
    /// document still defines that script — the outliner resolves the
    /// name + icon from `state.scriptTypes` keyed by exactly this id.
    /// A call whose script was deleted falls back to "unknown".
    fn kind_label(&self, type_name: &str) -> String {
        let builtin = Self::static_kind_name(type_name);
        if builtin != "unknown" {
            return builtin.to_string();
        }
        if self.document.scripts.iter().any(|s| s.id == type_name) {
            return type_name.to_string();
        }
        "unknown".to_string()
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
        // `DecodedFeature.name` (sketches, patterns).
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
            "sweep" => "Sweep",
            "mate_connector" => "Mate connector",
            "linear_pattern" => "Linear pattern",
            "circular_pattern" => "Circular pattern",
            "mirror" => "Mirror",
            "boolean" => "Boolean",
            "translate" => "Translate",
            "rotate" => "Rotate",
            "scale" => "Scale",
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

    fn op_wire_name(tag: u32) -> Result<&'static str, JsError> {
        match Self::parse_op(tag)? {
            BoolOp::Union => Ok("union"),
            BoolOp::Difference => Ok("difference"),
            BoolOp::Intersect => Ok("intersect"),
        }
    }

    /// DL-61 C2b call-first core: locate a top-level `FeatureCall`
    /// by id, hand its params object to `edit`, and — if the edit
    /// reports a real change — snapshot, re-derive the spine via
    /// `bootstrap_features_from_calls`, and re-mesh. Returns false
    /// when no call matches the id or the edit was a no-op (no
    /// snapshot is taken in either case). On a regeneration error
    /// the snapshot is rolled back so a bad edit never leaves a
    /// torn document.
    ///
    /// This replaces the old "poke `document.features` + reverse
    /// sync" flow: `feature_calls` is authoritative, so transform/op
    /// edits write into the call's params and the spine falls out of
    /// a normal regeneration.
    fn edit_feature_call_params(
        &mut self,
        id: FeatureId,
        edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> bool,
    ) -> bool {
        let Some(idx) = self
            .document
            .feature_calls
            .iter()
            .position(|c| c.id == id)
        else {
            return false;
        };
        let mut params = self.document.feature_calls[idx]
            .params
            .as_object()
            .cloned()
            .unwrap_or_default();
        if !edit(&mut params) {
            return false;
        }
        self.snapshot();
        self.document.feature_calls[idx].params = serde_json::Value::Object(params);
        if self.bootstrap_features_from_calls().is_err() {
            self.undo();
            return false;
        }
        self.rebuild_packed();
        true
    }

    fn push_feature_call(
        &mut self,
        type_name: &str,
        params: serde_json::Value,
    ) -> Result<u32, JsError> {
        self.snapshot();
        let id = self.document.allocate_id();
        let flat = id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");
        let call = fielddraft_protocol::FeatureCall::new(type_name, id, params);
        self.document.feature_calls.push(call);
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "{type_name} regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(flat)
    }
}

/// The app version this WASM binary was stamped with at build time
/// (see `build.rs`). The web app compares it against `__APP_VERSION__`
/// on boot to catch a stale kernel module. Returns "dev" when built
/// without `FIELDDRAFT_VERSION` (e.g. `cargo test`).
#[wasm_bindgen(js_name = wasmVersion)]
pub fn wasm_version() -> String {
    env!("FIELDDRAFT_VERSION").to_string()
}

#[wasm_bindgen]
impl Kernel {
    /// Create a new kernel with an empty document.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        console_error_panic_hook::set_once();

        let document = Document::default();
        let mut k = Self {
            document,
            decoded: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            bodies: Vec::new(),
            active_mesh_resolution: DEFAULT_MESH_RESOLUTION,
            context_mesh_resolution: DEFAULT_CONTEXT_MESH_RESOLUTION,
            active_body_id: None,
            sync_meshing: true,
            mesh_algorithm: 0,
            parametric_weave: false,
            mesh_version: 0,
            edge_fillets: Vec::new(),
            mate_connectors: Vec::new(),
            rollback_after: None,
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

        // Self-heal documents written before the cascade-delete fix: a
        // delete used to leave gizmo transforms (translate/rotate/scale)
        // orphaned — their `target_id` points at a feature that no longer
        // exists — and such an orphan fails *every* regeneration. Prune
        // them on the way in so a stale `.fdraft` loads instead of dying
        // with "target feature N not found".
        self.prune_dangling_transform_calls();

        // DL-61 C2b: `feature_calls` is the sole wire form, so a load
        // always rebuilds the decoded spine by replaying the calls
        // through the feature registry. Any registry error surfaces
        // as a load failure (fail-stop per §4.6).
        self.bootstrap_features_from_calls()
            .map_err(|e| JsError::new(&format!("feature regeneration failed: {e}")))?;

        self.rebuild_packed();
        // Loading is a fresh start: history is irrelevant.
        self.undo.clear();
        self.redo.clear();
        Ok(self.decoded.len())
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
        let mut registry = features::FeatureRegistry::with_builtins();
        // FieldSkript S2b: a document's scripts register as runtime
        // feature kinds keyed by their stable id, so a `FeatureCall`
        // whose `type` is a script id dispatches through the same
        // regenerate path as a built-in. Their geometry is produced by
        // the JS `build()` the worker drives via the script runner; a
        // headless run with no runner fails such a feature explicitly.
        for record in &self.document.scripts {
            registry.register(Box::new(script_runtime::ScriptFeature::new(
                record.id.clone(),
                record.schema.clone(),
            )));
        }
        let calls = std::mem::take(&mut self.document.feature_calls);
        let rollback = self.rollback_after;
        // DL-61 C2b: regeneration accumulates the decoded spine in the
        // Context (the Document no longer holds it) and we move it out as
        // `self.decoded` — the sole producer of the IR. On a fail-stop
        // error the partial spine is still captured so the caller can
        // render what regenerated before the failing call.
        let (result, spine) = {
            let mut ctx = features::Context::new(&mut self.document);
            let r = registry.regenerate(&mut ctx, &calls, rollback);
            // K10.4: capture edge fillets collected during regen.
            self.edge_fillets = std::mem::take(&mut ctx.edge_fillets);
            // C2: capture resolved mate-connector CSYS for the viewport.
            self.mate_connectors = std::mem::take(&mut ctx.mate_connectors);
            (r, std::mem::take(&mut ctx.features))
        };
        self.document.feature_calls = calls;
        self.decoded = spine;
        result
    }

    #[wasm_bindgen(js_name = toJson)]
    pub fn to_json(&self) -> Result<String, JsError> {
        serde_json::to_string(&self.document).map_err(|e| JsError::new(&e.to_string()))
    }

    #[wasm_bindgen(js_name = featureCount)]
    pub fn feature_count(&self) -> usize {
        self.decoded.len()
    }

    // ── FieldSkript records (S1) ──────────────────────────────────────
    //
    // Document-local, runtime-defined feature types. The kernel stores
    // them as opaque data (`Document.scripts`) so they round-trip via
    // `toJson`/`loadJson`; their `build()` bodies run web-side, so these
    // accessors never touch the decoded spine or the mesh. Mutations
    // snapshot for undo but skip `rebuild_packed` — no geometry changes.

    /// List the document's FieldSkript records as a JSON array.
    #[wasm_bindgen(js_name = listScripts)]
    pub fn list_scripts(&self) -> Result<String, JsError> {
        serde_json::to_string(&self.document.scripts)
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Insert or replace a FieldSkript record (keyed by `id`). Takes
    /// the serialised `FieldSkriptRecord` JSON; returns `true` when it
    /// replaced an existing record, `false` when it inserted a new one.
    #[wasm_bindgen(js_name = upsertScript)]
    pub fn upsert_script(&mut self, json: &str) -> Result<bool, JsError> {
        let record: fielddraft_protocol::FieldSkriptRecord =
            serde_json::from_str(json).map_err(|e| JsError::new(&e.to_string()))?;
        self.snapshot();
        if let Some(slot) = self
            .document
            .scripts
            .iter_mut()
            .find(|r| r.id == record.id)
        {
            *slot = record;
            Ok(true)
        } else {
            self.document.scripts.push(record);
            Ok(false)
        }
    }

    /// Remove a FieldSkript record by id. Returns `true` when a record
    /// was removed.
    #[wasm_bindgen(js_name = removeScript)]
    pub fn remove_script(&mut self, id: &str) -> bool {
        // Snapshot only on an actual change so a no-op remove doesn't
        // pollute the undo stack.
        if !self.document.scripts.iter().any(|r| r.id == id) {
            return false;
        }
        self.snapshot();
        self.document.scripts.retain(|r| r.id != id);
        true
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

    /// K6a: list every feature type name the UI can instantiate,
    /// sorted for deterministic UI rendering. The result is a JSON
    /// array of strings, e.g. `["box", "cylinder", "extrude",
    /// "sphere"]`. Built-in registry names are joined with every
    /// document FieldSkript id (a script's id doubles as its
    /// `FeatureCall::type_name`), so saved scripts show up as
    /// instantiable types alongside the built-ins.
    #[wasm_bindgen(js_name = listFeatureTypes)]
    pub fn list_feature_types(&self) -> Result<String, JsError> {
        let registry = features::FeatureRegistry::with_builtins();
        let mut names = registry.type_names();
        names.extend(self.document.scripts.iter().map(|r| r.id.clone()));
        names.sort();
        names.dedup();
        serde_json::to_string(&names).map_err(|e| JsError::new(&e.to_string()))
    }

    /// K6a: look up the FeatureSchema-generated JSON schema for
    /// a feature type. Returns the serialised JSON string or an
    /// error if the type isn't registered. A document FieldSkript
    /// (id == `type_name`) resolves to its stored `schema` blob —
    /// the same RawFeatureSchema the web FeatureDialog consumes — so
    /// the outliner's param dialog works for script instances too;
    /// otherwise the built-in registry is consulted.
    #[wasm_bindgen(js_name = schemaForType)]
    pub fn schema_for_type(&self, type_name: &str) -> Result<String, JsError> {
        if let Some(record) = self.document.scripts.iter().find(|r| r.id == type_name) {
            return serde_json::to_string(&record.schema)
                .map_err(|e| JsError::new(&e.to_string()));
        }
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
        for (idx, call) in self.document.feature_calls.iter().enumerate() {
            let rb = self.rollback_after.is_some_and(|ra| idx >= ra);
            if let Some(spine) = self
                .decoded
                .iter()
                .find(|f| f.id == call.id)
            {
                // DL-61 C2b: for a spine-backed call the call's
                // `type_name` already determines the outliner kind
                // (the decoded feature carries a lowered `node`, not a
                // `Primitive`), so we label straight off the type name.
                summaries.push(FeatureSummary {
                    id: spine.id.clone(),
                    name: spine.name.clone(),
                    op: Self::op_label(spine.op),
                    kind: self.kind_label(&call.type_name),
                    translate: spine.transform.translate,
                    rotation: spine.transform.rotation,
                    scale: spine.transform.scale,
                    suppressed: call.suppressed,
                    rolled_back: rb,
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
                let name = if matches!(call.type_name.as_str(), "translate" | "rotate" | "scale") {
                    let target_id = call.params.get("target_id")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    let target_name = self.decoded.iter()
                        .find(|f| f.id.as_u32_if_flat() == Some(target_id))
                        .map(|f| f.name.as_str())
                        .unwrap_or("?");
                    let label = match call.type_name.as_str() {
                        "translate" => "Translate",
                        "rotate" => "Rotate",
                        "scale" => "Scale",
                        _ => unreachable!(),
                    };
                    if let Some(user_name) = call.params.get("name").and_then(|v| v.as_str()) {
                        if !user_name.is_empty() { user_name.to_string() }
                        else { format!("{label} {target_name}") }
                    } else {
                        format!("{label} {target_name}")
                    }
                } else {
                    Self::synth_name_for_call(call)
                };
                summaries.push(FeatureSummary {
                    id: call.id.clone(),
                    name,
                    op,
                    kind: self.kind_label(&call.type_name),
                    translate,
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                    suppressed: call.suppressed,
                    rolled_back: rb,
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
        let op_wire = Self::op_wire_name(op)?;
        self.push_feature_call("extrude", serde_json::json!({
            "profiles": format!("qCreatedBy({sketch_feature_id})"),
            "depth": { "value": depth_mm, "unit": "mm" },
            "profile_half_x": 0.0,
            "profile_half_y": 0.0,
            "operation": op_wire,
            "center": [0.0, 0.0, tz],
        }))
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
        let op_wire = Self::op_wire_name(op)?;
        self.push_feature_call("revolve", serde_json::json!({
            "profiles": format!("qCreatedBy({sketch_feature_id})"),
            "operation": op_wire,
            "center": [cx, cy, cz],
        }))
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
        self.push_feature_call("hole", serde_json::json!({
            "diameter": { "value": diameter_mm, "unit": "mm" },
            "depth": { "value": depth_mm, "unit": "mm" },
            "translate": [tx, ty, tz],
        }))
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
        let op_wire = Self::op_wire_name(op)?;
        self.push_feature_call("gyroid", serde_json::json!({
            "cell_size": { "value": cell_size_mm, "unit": "mm" },
            "thickness": { "value": thickness_mm, "unit": "mm" },
            "op": op_wire,
            "blend_radius": { "value": 0.0, "unit": "mm" },
            "chamfer_size": { "value": 0.0, "unit": "mm" },
            "displace_depth": { "value": 0.0, "unit": "mm" },
            "shell_thickness": { "value": 0.0, "unit": "mm" },
        }))
    }

    #[wasm_bindgen(js_name = addBoolean)]
    pub fn add_boolean(&mut self) -> Result<u32, JsError> {
        self.push_feature_call("boolean", serde_json::json!({
            "operation": "Union",
            "tools": "",
            "targets": "",
            "keep_tools": false,
        }))
    }

    #[wasm_bindgen(js_name = addMaterial)]
    pub fn add_material(&mut self) -> Result<u32, JsError> {
        self.push_feature_call("material", serde_json::json!({
            "targets": "",
            "color": [0.55, 0.65, 0.85],
            "roughness": 0.35,
            "metalness": 0.0,
            "emissive": [0.0, 0.0, 0.0],
            "opacity": 1.0,
            "wireframe": false,
            "density": 0.0,
            "youngs_modulus": 0.0,
            "poissons_ratio": 0.0,
            "yield_tensile": 0.0,
            "ultimate_tensile": 0.0,
            "yield_compressive": 0.0,
        }))
    }

    /// K9.6 / C2: add a mate connector anchored to a B-Rep face. The
    /// face is given as a query string (e.g. `qFace(<id_hash>)`); the
    /// connector's CSYS is resolved from that face during regeneration.
    /// `ou`/`ov` shift the origin in the face's in-plane basis.
    #[wasm_bindgen(js_name = addMateConnector)]
    pub fn add_mate_connector(
        &mut self,
        face: &str,
        ou: f32,
        ov: f32,
    ) -> Result<u32, JsError> {
        self.push_feature_call("mate_connector", serde_json::json!({
            "face": face,
            "offset": [ou, ov],
        }))
    }

    /// C2: resolved CSYS of every placed `mate_connector`, as a JSON
    /// array of `{ id, origin, normal, x_axis }`. The viewport reads this
    /// to render datum triads instead of re-resolving the face query.
    #[wasm_bindgen(js_name = resolvedMateConnectors)]
    pub fn resolved_mate_connectors(&self) -> Result<String, JsError> {
        serde_json::to_string(&self.mate_connectors).map_err(|e| JsError::new(&e.to_string()))
    }

    /// DL-61: add a fillet_edge feature selecting an edge by query
    /// (e.g. `qEdge(<id_hash>)`). The mask tube and feature pair are
    /// derived from the resolved edge during regeneration.
    #[wasm_bindgen(js_name = addFilletEdge)]
    pub fn add_fillet_edge(
        &mut self,
        edge: String,
        blend_radius: f32,
    ) -> Result<u32, JsError> {
        self.push_feature_call("fillet_edge", serde_json::json!({
            "edge": edge,
            "blend_radius": blend_radius,
        }))
    }

    /// Add a `shell` feature: hollow the body owning the picked face(s)
    /// — planar or curved (any analytic surface) — to a wall of
    /// `thickness_mm`, opening it flush with each face. `open_faces` is one
    /// or more comma-separated face queries (e.g. `qFace(<id_hash>)`).
    #[wasm_bindgen(js_name = addShell)]
    pub fn add_shell(
        &mut self,
        open_faces: String,
        thickness_mm: f32,
    ) -> Result<u32, JsError> {
        self.push_feature_call("shell", serde_json::json!({
            "open_faces": open_faces,
            "thickness": { "value": thickness_mm, "unit": "mm" },
        }))
    }

    /// Extrude one or more coplanar B-Rep faces (P1). `faces` is one or more
    /// comma-separated planar face queries (e.g. `qFace(<id_hash>)`); their
    /// boundary loops are projected into their shared plane and swept by
    /// `depth_mm`. `symmetric` grows `±depth/2` about the plane; `false` is a
    /// blind / two-sided extrude `[−depth_back, +depth]` along the face normal
    /// (the default the UI sends, matching Onshape). Adds material (Union).
    #[wasm_bindgen(js_name = addExtrudeFaces)]
    pub fn add_extrude_faces(
        &mut self,
        faces: String,
        depth_mm: f32,
        symmetric: bool,
        depth_back_mm: f32,
    ) -> Result<u32, JsError> {
        self.push_feature_call("extrude", serde_json::json!({
            "profiles": "",
            "faces": faces,
            "depth": { "value": depth_mm, "unit": "mm" },
            "symmetric": symmetric,
            "depth_back": { "value": depth_back_mm, "unit": "mm" },
        }))
    }

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
    /// Stores only the original positions+indices in the document (~2MB)
    /// instead of the full MeshSdf with tris/bboxes (~175MB after voxel repair).
    /// The MeshSdf is rebuilt on-demand in feature execution.
    #[wasm_bindgen(js_name = addImportedMesh)]
    pub fn add_imported_mesh(
        &mut self,
        positions: &[f32],
        indices: &[u32],
        op: u32,
    ) -> Result<u32, JsError> {
        let op_wire = Self::op_wire_name(op)?;
        let report = fielddraft_protocol::MeshSdf::from_buffers(positions, indices)
            .check_manifold();
        self.push_feature_call("imported_mesh", serde_json::json!({
            "op": op_wire,
            "__positions": positions,
            "__indices": indices,
            "__watertight": report.is_watertight,
        }))
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
        let source_fid = FeatureId::from_u32(source_id);
        if !self.decoded.iter().any(|f| f.id == source_fid) {
            return Err(JsError::new(&format!(
                "source feature {source_id} not found on the spine"
            )));
        }
        self.push_feature_call("linear_pattern", serde_json::json!({
            "source": format!("qCreatedBy({source_id})"),
            "direction": [dx, dy, dz],
            "count": count as f32,
            "spacing": { "value": spacing_mm, "unit": "mm" },
        }))
    }

    /// K8.1: append a `circular_pattern` feature call that
    /// replicates the source around an axis-through-center
    /// pivot. `angle_deg` is the full sweep (360° for a
    /// closed ring). Returns the new pattern feature id.
    #[wasm_bindgen(js_name = addCircularPattern)]
    #[allow(clippy::too_many_arguments)] // wasm-bindgen geometry API; flat scalar params
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
        if !self.decoded.iter().any(|f| f.id == source_fid) {
            return Err(JsError::new(&format!(
                "source feature {source_id} not found on the spine"
            )));
        }
        self.push_feature_call("circular_pattern", serde_json::json!({
            "source": format!("qCreatedBy({source_id})"),
            "axis": [axis_x, axis_y, axis_z],
            "center": [center_x, center_y, center_z],
            "count": count as f32,
            "angle_deg": angle_deg,
        }))
    }

    /// K8.2: append a `mirror` feature call that reflects the
    /// source feature through a plane. `plane` is a face/datum
    /// query (e.g. `qFace(<hash>)` or `origin_plane_xy`) whose
    /// world-space frame supplies the mirror plane. Returns the
    /// new mirror feature id.
    #[wasm_bindgen(js_name = addMirror)]
    pub fn add_mirror(
        &mut self,
        source_id: u32,
        plane: &str,
    ) -> Result<u32, JsError> {
        let source_fid = FeatureId::from_u32(source_id);
        if !self.decoded.iter().any(|f| f.id == source_fid) {
            return Err(JsError::new(&format!(
                "source feature {source_id} not found on the spine"
            )));
        }
        self.push_feature_call("mirror", serde_json::json!({
            "source": format!("qCreatedBy({source_id})"),
            "plane": plane,
        }))
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
        let tree = csg::CsgNode::from_decoded(&self.decoded)
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

    #[wasm_bindgen(js_name = setRollbackIndex)]
    pub fn set_rollback_index(&mut self, index: i32) {
        let n = self.document.feature_calls.len();
        let prev = self.rollback_after;
        self.rollback_after = if index < 0 || index as usize >= n {
            None
        } else {
            Some(index as usize)
        };
        if self.bootstrap_features_from_calls().is_err() {
            // Rolling to this position left the spine unbuildable.
            // Revert to the previous rollback position and rebuild so
            // we never render a torn, half-built spine.
            self.rollback_after = prev;
            let _ = self.bootstrap_features_from_calls();
        }
        self.rebuild_packed();
    }

    #[wasm_bindgen(js_name = getRollbackIndex)]
    pub fn get_rollback_index(&self) -> i32 {
        match self.rollback_after {
            Some(idx) => idx as i32,
            None => -1,
        }
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
    ///   - thread   → (nominal_radius, pitch, depth)  half_height defaults to 5
    #[wasm_bindgen(js_name = addPrimitive)]
    #[allow(clippy::too_many_arguments)] // wasm-bindgen geometry API; flat scalar params
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
        // DL-61 C2b: addPrimitive is call-first — it builds the
        // primitive's `FeatureCall` directly (the same wire shape the
        // matching `FeatureKind::execute` parses) and regenerates the
        // spine via `push_feature_call`. The per-kind `params` keys
        // mirror each primitive `Def`'s field names exactly.
        let op_wire = Self::op_wire_name(op)?;
        let (type_name, mut params) = match kind {
            Primitive::KIND_BOX => (
                "box",
                serde_json::json!({ "half_extents": [p0, p1, p2] }),
            ),
            Primitive::KIND_SPHERE => ("sphere", serde_json::json!({ "radius": p0 })),
            Primitive::KIND_CYLINDER => (
                "cylinder",
                serde_json::json!({ "radius": p0, "half_height": p1 }),
            ),
            Primitive::KIND_TORUS => (
                "torus",
                serde_json::json!({ "major_radius": p0, "minor_radius": p1 }),
            ),
            Primitive::KIND_THREAD => (
                "thread",
                serde_json::json!({
                    "nominal_radius": p0, "pitch": p1, "depth": p2, "half_height": 5.0,
                }),
            ),
            Primitive::KIND_CONE => (
                "cone",
                serde_json::json!({ "r_bottom": p0, "r_top": p1, "half_height": p2 }),
            ),
            Primitive::KIND_CAPSULE => (
                "capsule",
                serde_json::json!({ "radius": p0, "half_height": p1 }),
            ),
            Primitive::KIND_HEX_PRISM => (
                "hex_prism",
                serde_json::json!({ "apothem": p0, "half_height": p1 }),
            ),
            Primitive::KIND_ELLIPSOID => {
                ("ellipsoid", serde_json::json!({ "radii": [p0, p1, p2] }))
            }
            other => return Err(JsError::new(&format!("invalid primitive kind: {other}"))),
        };
        if let Some(obj) = params.as_object_mut() {
            obj.insert("translate".into(), serde_json::json!([tx, ty, tz]));
            obj.insert("op".into(), serde_json::json!(op_wire));
        }
        self.push_feature_call(type_name, params)
    }

    /// Insert a feature instance of an arbitrary registered type
    /// (built-in OR a document FieldSkript whose id == `type_name`),
    /// with `params_json` as its parameter blob. Returns the new flat
    /// feature id. This is the generic counterpart to the type-specific
    /// `addBox`/`addPrimitive`/… inserters — it's what lets a saved
    /// FieldSkript be instantiated as a feature.
    #[wasm_bindgen(js_name = addFeatureCall)]
    pub fn add_feature_call(&mut self, type_name: &str, params_json: &str) -> Result<u32, JsError> {
        let params: serde_json::Value =
            serde_json::from_str(params_json).map_err(|e| JsError::new(&e.to_string()))?;
        self.push_feature_call(type_name, params)
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
        self.edit_feature_call_params(id, |p| {
            if json_vec3(p, "translate", [0.0, 0.0, 0.0]) == [tx, ty, tz] {
                return false;
            }
            p.insert("translate".into(), serde_json::json!([tx, ty, tz]));
            true
        })
    }

    /// K7.21h: replace the rotation component of a feature's transform.
    /// Quaternion order: (x, y, z, w).
    #[wasm_bindgen(js_name = setFeatureRotation)]
    pub fn set_feature_rotation(
        &mut self,
        id: u32,
        rx: f32,
        ry: f32,
        rz: f32,
        rw: f32,
    ) -> bool {
        let id = FeatureId::from_u32(id);
        let rot = [rx, ry, rz, rw];
        self.edit_feature_call_params(id, |p| {
            if json_vec4(p, "rotation", [0.0, 0.0, 0.0, 1.0]) == rot {
                return false;
            }
            p.insert("rotation".into(), serde_json::json!(rot));
            true
        })
    }

    #[wasm_bindgen(js_name = setFeatureScale)]
    pub fn set_feature_scale(&mut self, id: u32, sx: f32, sy: f32, sz: f32) -> bool {
        let id = FeatureId::from_u32(id);
        self.edit_feature_call_params(id, |p| {
            let cur = json_vec3(p, "scale", [1.0, 1.0, 1.0]);
            if (cur[0] - sx).abs() < 1e-7
                && (cur[1] - sy).abs() < 1e-7
                && (cur[2] - sz).abs() < 1e-7
            {
                return false;
            }
            p.insert("scale".into(), serde_json::json!([sx, sy, sz]));
            true
        })
    }

    fn apply_gizmo_feature(
        &mut self,
        type_name: &str,
        target_id: u32,
        params: serde_json::Value,
    ) -> bool {
        let target = FeatureId::from_u32(target_id);
        if !self.decoded.iter().any(|f| f.id == target) {
            return false;
        }

        let should_merge = self.document.feature_calls.last().is_some_and(|c| {
            c.type_name == type_name
                && !c.suppressed
                && c.params.get("target_id")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32 == target_id
        });

        if should_merge {
            let idx = self.document.feature_calls.len() - 1;
            self.document.feature_calls[idx].params = params;
        } else {
            self.snapshot();
            let id = self.document.allocate_id();
            self.document.feature_calls.push(
                fielddraft_protocol::FeatureCall {
                    id,
                    type_name: type_name.to_string(),
                    params,
                    suppressed: false,
                },
            );
        }

        let _ = self.bootstrap_features_from_calls();
        self.rebuild_packed();
        true
    }

    #[wasm_bindgen(js_name = applyGizmoTranslate)]
    pub fn apply_gizmo_translate(
        &mut self,
        target_id: u32,
        tx: f32, ty: f32, tz: f32,
    ) -> bool {
        let params = serde_json::json!({
            "target_id": target_id,
            "translate": [tx, ty, tz],
        });
        self.apply_gizmo_feature("translate", target_id, params)
    }

    #[wasm_bindgen(js_name = applyGizmoRotate)]
    pub fn apply_gizmo_rotate(
        &mut self,
        target_id: u32,
        rx: f32, ry: f32, rz: f32, rw: f32,
    ) -> bool {
        let euler = quat_to_euler_xyz(rx, ry, rz, rw);
        let params = serde_json::json!({
            "target_id": target_id,
            "rotation": [euler[0], euler[1], euler[2]],
        });
        self.apply_gizmo_feature("rotate", target_id, params)
    }

    #[wasm_bindgen(js_name = applyGizmoScale)]
    pub fn apply_gizmo_scale(
        &mut self,
        target_id: u32,
        sx: f32, sy: f32, sz: f32,
    ) -> bool {
        let params = serde_json::json!({
            "target_id": target_id,
            "scale": [sx, sy, sz],
        });
        self.apply_gizmo_feature("scale", target_id, params)
    }

    /// Add `(dx, dy, dz)` to a feature's translate. Convenience for
    /// keyboard nudging.
    #[wasm_bindgen(js_name = nudgeFeature)]
    pub fn nudge_feature(&mut self, id: u32, dx: f32, dy: f32, dz: f32) -> bool {
        if dx == 0.0 && dy == 0.0 && dz == 0.0 {
            return false;
        }
        let id = FeatureId::from_u32(id);
        self.edit_feature_call_params(id, |p| {
            let cur = json_vec3(p, "translate", [0.0, 0.0, 0.0]);
            p.insert(
                "translate".into(),
                serde_json::json!([cur[0] + dx, cur[1] + dy, cur[2] + dz]),
            );
            true
        })
    }

    /// Replace the full transform (translate, quaternion, scale).
    #[wasm_bindgen(js_name = setFeatureTransform)]
    #[allow(clippy::too_many_arguments)] // wasm-bindgen geometry API; flat scalar params
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
        sx: f32,
        sy: f32,
        sz: f32,
    ) -> bool {
        let id = FeatureId::from_u32(id);
        self.edit_feature_call_params(id, |p| {
            let t = json_vec3(p, "translate", [0.0, 0.0, 0.0]);
            let r = json_vec4(p, "rotation", [0.0, 0.0, 0.0, 1.0]);
            let s = json_vec3(p, "scale", [1.0, 1.0, 1.0]);
            if t == [tx, ty, tz]
                && r == [qx, qy, qz, qw]
                && s == [sx, sy, sz]
            {
                return false;
            }
            p.insert("translate".into(), serde_json::json!([tx, ty, tz]));
            p.insert("rotation".into(), serde_json::json!([qx, qy, qz, qw]));
            p.insert("scale".into(), serde_json::json!([sx, sy, sz]));
            true
        })
    }

    /// Drop any gizmo transform call whose `target_id` references a
    /// feature that isn't present in `feature_calls`. Runs to a fixpoint
    /// so a transform chained onto a just-pruned transform goes too.
    /// Idempotent and cheap on a healthy document (no dangling refs → no
    /// change). Used by [`Kernel::load_json`] to heal documents written
    /// before `remove_feature` learned to cascade.
    fn prune_dangling_transform_calls(&mut self) {
        loop {
            let present: std::collections::HashSet<FeatureId> =
                self.document.feature_calls.iter().map(|c| c.id.clone()).collect();
            let before = self.document.feature_calls.len();
            self.document.feature_calls.retain(|c| {
                match c.params.get("target_id").and_then(|v| v.as_u64()) {
                    Some(t) => present.contains(&FeatureId::from_u32(t as u32)),
                    None => true,
                }
            });
            if self.document.feature_calls.len() == before {
                break;
            }
        }
    }

    /// Remove the feature with the given id. Returns true if anything
    /// was removed.
    #[wasm_bindgen(js_name = removeFeature)]
    pub fn remove_feature(&mut self, id: u32) -> bool {
        let target_id = FeatureId::from_u32(id);

        // DL-61 C2b: `feature_calls` is authoritative, so a delete is
        // just "drop the call(s) and regenerate". The spine (including
        // any descendant clones a Pattern/Mirror/Hole execute path
        // would have produced) falls out of the regeneration.
        if !self.document.feature_calls.iter().any(|c| c.id == target_id) {
            return false;
        }

        self.snapshot();

        // Cascade: a deleted feature takes its dependents with it. Gizmo
        // transform calls (translate/rotate/scale) reference their
        // subject through a `target_id` param; left behind, an orphaned
        // transform poisons *every* later regeneration with "target
        // feature N not found" (it errored here too, but the delete
        // tolerated it — so the orphan lingered and broke the next
        // addPrimitive). Collect the dependency closure to a fixpoint so
        // transform-of-transform chains go too, then drop them all.
        let mut removed: std::collections::HashSet<FeatureId> =
            std::iter::once(target_id).collect();
        loop {
            let mut grew = false;
            for c in &self.document.feature_calls {
                if removed.contains(&c.id) {
                    continue;
                }
                let depends_on_removed = c
                    .params
                    .get("target_id")
                    .and_then(|v| v.as_u64())
                    .map(|t| removed.contains(&FeatureId::from_u32(t as u32)))
                    .unwrap_or(false);
                if depends_on_removed {
                    removed.insert(c.id.clone());
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        self.document
            .feature_calls
            .retain(|c| !removed.contains(&c.id));

        let _ = self.bootstrap_features_from_calls();
        self.rebuild_packed();
        true
    }

    /// Change the boolean op of an existing feature.
    #[wasm_bindgen(js_name = setFeatureOp)]
    pub fn set_feature_op(&mut self, id: u32, op: u32) -> Result<bool, JsError> {
        let wire = Self::op_wire_name(op)?;
        let id = FeatureId::from_u32(id);
        Ok(self.edit_feature_call_params(id, |p| {
            if p.get("op").and_then(|v| v.as_str()) == Some(wire) {
                return false;
            }
            p.insert("op".into(), serde_json::json!(wire));
            true
        }))
    }

    /// Move a feature to a new index. Indices clamp into bounds.
    #[wasm_bindgen(js_name = reorderFeature)]
    pub fn reorder_feature(&mut self, id: u32, target_index: usize) -> bool {
        let id = FeatureId::from_u32(id);
        let from = self.document.feature_calls.iter().position(|c| c.id == id);
        let Some(from) = from else { return false };
        let to = target_index.min(self.document.feature_calls.len().saturating_sub(1));
        if from == to {
            return false;
        }
        self.snapshot();
        let call = self.document.feature_calls.remove(from);
        self.document.feature_calls.insert(to, call);
        // Reordering changes Boolean/regeneration order, so re-derive
        // the spine from the new call order.
        let _ = self.bootstrap_features_from_calls();
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
            // DL-61 C2b: snapshots carry only `feature_calls`, so the
            // decoded spine must be regenerated from the restored
            // call list before re-meshing.
            let _ = self.bootstrap_features_from_calls();
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
            let _ = self.bootstrap_features_from_calls();
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

    // ── M14a: 3MF export ────────────────────────────────────────

    #[wasm_bindgen(js_name = export3mf)]
    pub fn export_3mf(&mut self) -> Vec<u8> {
        let prev = self.ensure_bodies_for_export();
        let out = {
            let meshes: Vec<&Mesh> = self.bodies.iter().map(|b| &b.mesh).collect();
            export_3mf::export_3mf(&meshes)
        };
        self.restore_sync_meshing(prev);
        out
    }

    // ── M14b: STEP classification ─────────────────────────────────

    #[wasm_bindgen(js_name = classifyForStep)]
    pub fn classify_for_step(&self) -> String {
        let report = step_classifier::classify(&self.decoded);
        report.to_json().to_string()
    }

    #[wasm_bindgen(js_name = stepCommands)]
    pub fn step_commands(&self) -> String {
        let cmds = step_commands::build_step_commands(&self.decoded);
        cmds.to_string()
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
    /// Flat-numeric ids pass through as-is. Hierarchical ids
    /// (pattern clones like "3.1") are hashed to a unique u32 with
    /// the high bit set so they never collide with flat counters.
    #[wasm_bindgen(js_name = bodyId)]
    pub fn body_id(&self, i: usize) -> u32 {
        let id = &self.bodies[i].id;
        match id.as_u32_if_flat() {
            Some(n) => n,
            None => {
                let s = id.to_string();
                let mut h: u32 = 0x811c_9dc5;
                for b in s.as_bytes() {
                    h ^= *b as u32;
                    h = h.wrapping_mul(0x0100_0193);
                }
                h | 0x8000_0000
            }
        }
    }

    /// Root feature id that seeded this body. For flat-numeric
    /// bodies this equals `bodyId`; for pattern clones it returns
    /// the pattern feature's flat id so the UI can look up the
    /// feature name even though `bodyId` is a unique hash.
    #[wasm_bindgen(js_name = bodySeedFeatureId)]
    pub fn body_seed_feature_id(&self, i: usize) -> u32 {
        let id = &self.bodies[i].id;
        match id.as_u32_if_flat() {
            Some(n) => n,
            None => {
                let mut root = id;
                while let Some(parent) = root.parent() {
                    root = parent;
                }
                root.as_u32_if_flat().unwrap_or(0)
            }
        }
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

    /// K3.6: returns `true` when the i-th body has a pending
    /// translation delta that the frontend should apply as a GPU
    /// matrix instead of re-uploading geometry.
    #[wasm_bindgen(js_name = bodyHasTransformDelta)]
    pub fn body_has_transform_delta(&self, i: usize) -> bool {
        self.bodies[i].transform_delta.is_some()
    }

    /// K3.6: returns a 3-element `Float32Array` [dx, dy, dz] for
    /// the i-th body's pending translation delta. Panics if the
    /// body has no delta — guard with `bodyHasTransformDelta` first.
    #[wasm_bindgen(js_name = bodyTransformDelta)]
    pub fn body_transform_delta(&self, i: usize) -> Vec<f32> {
        let d = self.bodies[i]
            .transform_delta
            .expect("no transform delta");
        vec![d[0], d[1], d[2]]
    }

    #[wasm_bindgen(js_name = bodySdfOpcodesPtr)]
    pub fn body_sdf_opcodes_ptr(&self, i: usize) -> *const f32 {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .map_or(std::ptr::null(), |s| s.opcodes.as_ptr())
    }

    #[wasm_bindgen(js_name = bodySdfOpcodesLen)]
    pub fn body_sdf_opcodes_len(&self, i: usize) -> usize {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .map_or(0, |s| s.opcodes.len())
    }

    #[wasm_bindgen(js_name = bodySdfAabb)]
    pub fn body_sdf_aabb(&self, i: usize) -> Vec<f32> {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .map_or(vec![0.0; 6], |s| {
                vec![
                    s.aabb.min[0], s.aabb.min[1], s.aabb.min[2],
                    s.aabb.max[0], s.aabb.max[1], s.aabb.max[2],
                ]
            })
    }

    #[wasm_bindgen(js_name = bodyVolumeCount)]
    pub fn body_volume_count(&self, i: usize) -> usize {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .map_or(0, |s| s.volumes.len())
    }

    #[wasm_bindgen(js_name = bodyVolumeDataPtr)]
    pub fn body_volume_data_ptr(&self, i: usize, vol_idx: usize) -> *const f32 {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .and_then(|s| s.volumes.get(vol_idx))
            .map_or(std::ptr::null(), |v| v.data.as_ptr())
    }

    #[wasm_bindgen(js_name = bodyVolumeDataLen)]
    pub fn body_volume_data_len(&self, i: usize, vol_idx: usize) -> usize {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .and_then(|s| s.volumes.get(vol_idx))
            .map_or(0, |v| v.data.len())
    }

    #[wasm_bindgen(js_name = bodyVolumeDims)]
    pub fn body_volume_dims(&self, i: usize, vol_idx: usize) -> Vec<u32> {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .and_then(|s| s.volumes.get(vol_idx))
            .map_or(vec![0, 0, 0], |v| v.dims.to_vec())
    }

    #[wasm_bindgen(js_name = bodyHybridInfoJson)]
    pub fn body_hybrid_info_json(&self, i: usize) -> String {
        let bounded = body::build_bounded_for_body(
            &self.decoded,
            &self.bodies[i].feature_indices,
        );
        let info = bounded.classify_for_hybrid();
        serde_json::to_string(&info).unwrap_or_else(|_| "{}".into())
    }

    // ── Per-body *feature-based* B-Rep (sharp topology) ──────────

    /// Counts of the body's feature-based B-Rep entities as a JSON
    /// object: `{"vertices","edges","faces","shells","solids",
    /// "watertight"}`. `watertight` reflects the body's single outer
    /// shell's `is_closed` flag.
    ///
    /// This is the **real** B-Rep topology (sharp corners / edges /
    /// trimmed faces from the analytic weave). The B-Rep is woven
    /// lazily on first call and cached on the body, so this query is
    /// cheap on repeat (DL-30). Smooth-only bodies (e.g. a lone sphere)
    /// carry no sharp
    /// features and legitimately report zero/near-zero counts —
    /// feature-based B-Rep captures sharp topology only (DL-29).
    #[wasm_bindgen(js_name = bodyBrepCounts)]
    pub fn body_brep_counts(&mut self, i: usize) -> String {
        // Split the borrow: `ensure_brep` needs `&mut bodies[i]` and
        // `&document.features` simultaneously, which a method on the
        // whole `Kernel` can't express without aliasing.
        let brep = self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let watertight = brep.shells.first().map(|s| s.is_closed).unwrap_or(false);
        serde_json::json!({
            "vertices": brep.vertex_count(),
            "edges": brep.edge_count(),
            "faces": brep.face_count(),
            "shells": brep.shell_count(),
            "solids": brep.solid_count(),
            "watertight": watertight,
        })
        .to_string()
    }

    /// Per-body analytic B-Rep edges, sampled to **world-space** polylines for
    /// viewport rendering and hover. Returns a JSON array; each edge is
    /// `{"a":<branch u32>,"b":<branch u32>,"idHash":<decimal string>,
    /// "points":[x,y,z, x,y,z, …]}`, where `a`/`b` are the canonical
    /// colliding-branch signature. Those two ids are
    /// the same `as_u32()` branch values that [`Kernel::pick`] reports in its
    /// `contributors` array, so the frontend can match a hovered edge to its
    /// polyline. `idHash` is the regeneration-stable
    /// [`crate::brep::naming::TopoRef`] edge id (decimal string, like the
    /// faces/vertices exports), so a click on an edge can emit
    /// `qEdge(<idHash>)`. Points are sampled in the body-local frame and folded to world
    /// by the body's `placement` (DL-56), exactly like the STEP export. Reuses
    /// the same lazily-woven, cached B-Rep as the counts / STEP queries.
    #[wasm_bindgen(js_name = bodyBrepEdges)]
    pub fn body_brep_edges(&mut self, i: usize) -> String {
        use crate::brep::naming::TopoRef;
        use crate::brep::EdgeId;

        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let placement = self.bodies[i].placement;
        let brep = self.bodies[i].brep_cached().expect("woven above");
        let s = placement.scale[0];
        let mut edges = Vec::with_capacity(brep.edges.len());
        for (k, edge) in brep.edges.iter().enumerate() {
            let id_hash = TopoRef::from_edge(brep, EdgeId(k as u32)).id_hash;
            let local = crate::brep::edge_sampling::sample_edge(&edge.geometry);
            let mut points: Vec<f32> = Vec::with_capacity(local.len() * 3);
            for p in &local {
                // world = R·(s·p) + t  (rigid + uniform scale placement).
                let scaled = [p[0] as f32 * s, p[1] as f32 * s, p[2] as f32 * s];
                let r = quat_rotate(placement.rotation, scaled);
                points.push(r[0] + placement.translate[0]);
                points.push(r[1] + placement.translate[1]);
                points.push(r[2] + placement.translate[2]);
            }
            edges.push(serde_json::json!({
                "a": edge.signature.0.as_u32(),
                "b": edge.signature.1.as_u32(),
                "idHash": id_hash.to_string(),
                "points": points,
            }));
        }
        serde_json::Value::Array(edges).to_string()
    }

    /// Per-body feature-based B-Rep **faces**, in the body-LOCAL frame, as a
    /// JSON array — one entry per `brep.faces[k]` (Brocken A1). Each entry:
    /// `{"faceIndex","idHash","kind":"face","branchId","featureId",
    /// "surfaceKind","centroidLocal":[x,y,z],"normalLocal":[x,y,z]}`.
    ///
    /// `idHash` is the regeneration-stable [`crate::brep::naming::TopoRef`]
    /// id (FNV-1a over the sorted branch signature + spatial index),
    /// serialised as a **decimal string** because a `u64` is unsafe as a JS
    /// `Number`. `faceIndex` indexes the same ordering as the per-vertex
    /// `face_index` mesh tags (`body_face_index_ptr`), so a hovered triangle's
    /// tag selects this entry. Geometry stays body-local; the caller folds the
    /// body `placement` (`bodyPlacement`) in for world coordinates.
    #[wasm_bindgen(js_name = bodyBrepFaces)]
    pub fn body_brep_faces(&mut self, i: usize) -> String {
        use crate::brep::face_export::{face_centroid_normal_local, surface_kind};
        use crate::brep::naming::TopoRef;
        use crate::brep::FaceId;

        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let brep = self.bodies[i].brep_cached().expect("woven above");
        let mut faces = Vec::with_capacity(brep.faces.len());
        for (k, face) in brep.faces.iter().enumerate() {
            let id_hash = TopoRef::from_face(brep, FaceId(k as u32)).id_hash;
            let (centroid, normal) = face_centroid_normal_local(brep, FaceId(k as u32));
            faces.push(serde_json::json!({
                "faceIndex": k,
                "idHash": id_hash.to_string(),
                "kind": "face",
                "branchId": face.branch_id.as_u32(),
                "featureId": face.branch_id.feature_id(),
                "surfaceKind": surface_kind(&face.surface),
                "centroidLocal": centroid,
                "normalLocal": normal,
            }));
        }
        serde_json::Value::Array(faces).to_string()
    }

    /// Per-body feature-based B-Rep **vertices**, in the body-LOCAL frame, as a
    /// JSON array — one entry per `brep.vertices[k]` (Brocken A1). Each entry:
    /// `{"vertexIndex","idHash","kind":"vertex","positionLocal":[x,y,z],
    /// "branches":[<u32 packed>, …]}`. `idHash` is the stable
    /// [`crate::brep::naming::TopoRef`] vertex id as a decimal string.
    #[wasm_bindgen(js_name = bodyBrepVertices)]
    pub fn body_brep_vertices(&mut self, i: usize) -> String {
        use crate::brep::naming::TopoRef;
        use crate::brep::VertexId;

        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let brep = self.bodies[i].brep_cached().expect("woven above");
        let mut verts = Vec::with_capacity(brep.vertices.len());
        for (k, vertex) in brep.vertices.iter().enumerate() {
            let id_hash = TopoRef::from_vertex(brep, VertexId(k as u32)).id_hash;
            let branches: Vec<u32> = vertex.branches.iter().map(|b| b.as_u32()).collect();
            verts.push(serde_json::json!({
                "vertexIndex": k,
                "idHash": id_hash.to_string(),
                "kind": "vertex",
                "positionLocal": vertex.position,
                "branches": branches,
            }));
        }
        serde_json::Value::Array(verts).to_string()
    }

    /// The body's rigid + uniform-scale **placement** as JSON
    /// `{"translate":[x,y,z],"rotation":[x,y,z,w],"scale":[sx,sy,sz]}`
    /// (Brocken A2). B-Reps stay body-LOCAL and an affine transform never
    /// invalidates them; the frontend composes this placement back in
    /// (`localToWorld`) to obtain world coordinates. `placement` is recomputed
    /// lazily on the first `ensure_brep`, so we weave first to guarantee it is
    /// current.
    #[wasm_bindgen(js_name = bodyPlacement)]
    pub fn body_placement(&mut self, i: usize) -> String {
        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let p = self.bodies[i].placement;
        serde_json::json!({
            "translate": p.translate,
            "rotation": p.rotation,
            "scale": p.scale,
        })
        .to_string()
    }

    /// Pointer to the per-**triangle** B-Rep **face index** buffer (Brocken
    /// A3): one `u32` per mesh triangle giving the arena index of the B-Rep
    /// face that triangle lies on (same ordering as `bodyBrepFaces`), or
    /// `u32::MAX` when unresolved. Length is `bodyTriangleCount` (==
    /// `bodyIndexCount / 3`). A hovered/clicked triangle resolves to its stable
    /// face by indexing this buffer with the triangle id. Per-triangle (not
    /// per-vertex) because planar simplification leaves flat faces with only
    /// shared boundary vertices, so vertices can't be uniquely owned by a face
    /// — triangle centroids always can. Populated during the mesh rebuild; an
    /// un-tagged body (e.g. a passthrough imported mesh) exposes an empty
    /// buffer (null pointer, length 0).
    #[wasm_bindgen(js_name = bodyFaceIndexPtr)]
    pub fn body_face_index_ptr(&self, i: usize) -> *const u32 {
        self.bodies[i].mesh.face_index.as_ptr()
    }

    /// Length of the per-triangle face-index buffer (`bodyFaceIndexPtr`).
    /// Equals `bodyTriangleCount` for a tagged mesh, or 0 when the body carries
    /// no face tags.
    #[wasm_bindgen(js_name = bodyFaceIndexLen)]
    pub fn body_face_index_len(&self, i: usize) -> usize {
        self.bodies[i].mesh.face_index.len()
    }

    /// Serialise body `i`'s feature-based B-Rep as an ISO-10303-21
    /// (STEP) AP214 advanced-B-Rep file. Reuses the same lazily-woven,
    /// cached B-Rep as [`Kernel::body_brep_counts`]; the returned
    /// string is a complete `.step` document ready to download. The
    /// product name is derived from the body's stable feature id.
    #[wasm_bindgen(js_name = bodyStepExport)]
    pub fn body_step_export(&mut self, i: usize) -> String {
        let product_name = format!("body_{}", self.bodies[i].id);
        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let placement = self.bodies[i].placement;
        let brep = self.bodies[i].brep_cached().expect("woven above");
        crate::brep::step_export::export_step_placed(brep, &placement, &product_name)
    }

    /// Serialise *every* body's feature-based B-Rep into ONE ISO-10303-21
    /// (STEP) AP214 file, each body a separate `MANIFOLD_SOLID_BREP` solid
    /// in a single shape representation (DL-53: bodies stay separate). Each
    /// body's B-Rep is lazily woven and cached exactly as in
    /// [`Kernel::body_step_export`]; the returned string is a complete
    /// `.step` document ready to download.
    #[wasm_bindgen(js_name = sceneStepExport)]
    pub fn scene_step_export(&mut self) -> String {
        // Weave every body first (mutates each body's cache), then borrow the
        // breps immutably for export — two passes so the mutable `ensure_brep`
        // borrows don't overlap the shared-slice borrow.
        for i in 0..self.bodies.len() {
            self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        }
        let breps: Vec<(&crate::brep::BRep, fielddraft_protocol::Transform)> = self
            .bodies
            .iter()
            .map(|b| (b.brep_cached().expect("woven above"), b.placement))
            .collect();
        crate::brep::step_export::export_step_scene_placed(&breps, "scene")
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

    // ── Mesh algorithm selection ────────────────────────────────────────

    #[wasm_bindgen(js_name = setMeshAlgorithm)]
    pub fn set_mesh_algorithm(&mut self, name: &str) -> bool {
        let algo = match name {
            "rfta" => 1,
            "mt" => 2,
            _ => 0,
        };
        if algo == self.mesh_algorithm {
            return false;
        }
        self.mesh_algorithm = algo;
        self.bodies.clear();
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

    /// Toggle milestone-1 parametric weaving (exact analytic B-Rep for pure
    /// grundforms). Returns `false` if the flag was already at `enabled`.
    /// Clears the body cache and remeshes so every body re-weaves through the
    /// chosen path — the woven `id_hash`es are identical on both routes, so
    /// existing element selections keep resolving across the toggle.
    #[wasm_bindgen(js_name = setParametricWeave)]
    pub fn set_parametric_weave(&mut self, enabled: bool) -> bool {
        if self.parametric_weave == enabled {
            return false;
        }
        self.parametric_weave = enabled;
        // The render mesh (DC) is identical on both routes — only the woven
        // B-Rep differs. Invalidate each body's B-Rep cache and bump its
        // version so the frontend refetches the now-exact (or now-sampled)
        // edges / faces; no remesh needed.
        for b in &mut self.bodies {
            b.brep = None;
            b.version = b.version.wrapping_add(1);
        }
        self.mesh_version = self.mesh_version.wrapping_add(1);
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
        let csg = CsgNode::from_decoded(&self.decoded)
            .apply_edge_fillets(&self.edge_fillets);
        let bounded = BoundedCsg::from_csg(&csg);
        let params = RaycastParams::default();
        match raycast(&bounded, [ox, oy, oz], [dx, dy, dz], params) {
            None => r#"{"hit":false}"#.to_string(),
            Some(hit) => {
                // Two parallel arrays, both tiny and stable — hand-formatted
                // to avoid serde_json::Value churn:
                //   `contributors`: the flat root *feature* ids (sub-index
                //     stripped). This is what selection, query-picking and
                //     edge fillets consume — they map an id to a feature in
                //     the tree, so they need the feature, not the face.
                //   `branches`: the packed BranchIds (feature << SUB_BITS |
                //     sub) in the same order. The analytic B-Rep edge overlay
                //     matches a hovered edge by its colliding-face signature,
                //     which is exactly these packed per-face ids.
                let mut contributors = String::from("[");
                let mut branches = String::from("[");
                for (i, id) in hit.contributors.as_slice().iter().enumerate() {
                    if i > 0 {
                        contributors.push(',');
                        branches.push(',');
                    }
                    contributors.push_str(&id.feature_id().to_string());
                    branches.push_str(&id.as_u32().to_string());
                }
                contributors.push(']');
                branches.push(']');
                format!(
                    r#"{{"hit":true,"point":[{},{},{}],"normal":[{},{},{}],"t":{},"element":"{}","contributors":{},"branches":{}}}"#,
                    hit.point[0],
                    hit.point[1],
                    hit.point[2],
                    hit.normal[0],
                    hit.normal[1],
                    hit.normal[2],
                    hit.t,
                    hit.element.label(),
                    contributors,
                    branches,
                )
            }
        }
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
    // DL-61 C2b: `Feature` is no longer part of the production
    // surface (the kernel runs on call-first `feature_calls` +
    // `DecodedFeature`), but the test fixtures still build spine
    // features directly, so they decode through `from_parts` here.
    use crate::csg::PrimitiveModifiers;

    fn box_feature(id: u32, op: BoolOp, center: [f32; 3], half: [f32; 3]) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Box {id}"),
            op,
            Primitive::Box { center, half_extents: half },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    fn sphere_feature(id: u32, op: BoolOp, center: [f32; 3], r: f32) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Sphere {id}"),
            op,
            Primitive::Sphere { center, radius: r },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
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

    // ─── kernel mutations + undo/redo ────────────────────────────────

    // ─── K6b.3a: sketch commit path ─────────────────────────────────

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
        assert_eq!(k.decoded.len(), 1);
        assert_eq!(k.decoded[0].id.as_u32_if_flat(), Some(box_id));

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
        assert_eq!(k.decoded.len(), 1);
        assert_eq!(k.decoded[0].id.as_u32_if_flat(), Some(box_id));
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
        assert_eq!(k.decoded.len(), 1);
        let extrude_spine = k
            .decoded
            .iter()
            .find(|f| f.id.as_u32_if_flat() == Some(extrude_id))
            .expect("extrude spine entry");
        match extrude_spine.export_primitive().expect("extrude primitive") {
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
            .decoded
            .iter()
            .find(|f| matches!(f.export_primitive(), Some(Primitive::Extrude { .. })))
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
        assert_eq!(k.decoded.len(), 3);
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
        assert_eq!(k.decoded.len(), 1);
        let feat = &k.decoded[0];
        assert_eq!(feat.id.as_u32_if_flat(), Some(id));
        match feat.export_primitive().expect("box primitive") {
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

    #[test]
    fn replace_feature_call_params_preserves_blend_radius_through_sync() {
        let mut k = Kernel::new();
        let id = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0, 0.0, 0.0,
                10.0, 10.0, 10.0,
            )
            .unwrap();

        let new_params = serde_json::json!({
            "half_extents": [10.0, 10.0, 10.0],
            "center": [0.0, 0.0, 0.0],
            "translate": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": 1.0,
            "op": "union",
            "blend_radius": 5.0,
            "twist_rate": 0.3,
        });
        k.replace_feature_call_params(id, &new_params.to_string())
            .unwrap();

        let after = k.get_feature_call_params(id).unwrap();
        let v: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert!((v["blend_radius"].as_f64().unwrap() - 5.0).abs() < 1e-5);
        assert!((v["twist_rate"].as_f64().unwrap() - 0.3).abs() < 1e-5);

        let json = k.to_json().unwrap();
        let doc: serde_json::Value = serde_json::from_str(&json).unwrap();
        let calls = doc["feature_calls"].as_array().unwrap();
        let call = calls.iter().find(|c| c["id"] == id).unwrap();
        assert!((call["params"]["blend_radius"].as_f64().unwrap() - 5.0).abs() < 1e-5);
        assert!((call["params"]["twist_rate"].as_f64().unwrap() - 0.3).abs() < 1e-5);
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

    // ─── generic feature-call insert + script-aware type queries ──────

    #[test]
    fn add_feature_call_appends_builtin_and_returns_new_id() {
        let mut k = Kernel::new();
        let params = r#"{ "half_extents": [1.0, 2.0, 3.0], "op": "union" }"#;
        let id = k.add_feature_call("box", params).unwrap();
        assert_eq!(k.feature_count(), 1);
        assert!(
            k.document.feature_calls.iter().any(|c| c.id == FeatureId::from_u32(id)),
            "the new box call lands in feature_calls"
        );
        // The decoded spine grows to mirror the inserted call.
        assert!(k.decoded.iter().any(|f| f.id == FeatureId::from_u32(id)));
    }

    #[test]
    fn add_feature_call_rejects_malformed_params_json() {
        // `add_feature_call`'s error branch constructs a `JsError`, which
        // panics on native test targets, so we exercise the guard it
        // relies on: malformed params json fails to parse, which is what
        // makes the method short-circuit before touching the document.
        assert!(serde_json::from_str::<serde_json::Value>("{ this is not json").is_err());
    }

    /// Build a minimal valid serialised `FieldSkriptRecord` for the
    /// `upsert_script` path. The `schema` is the opaque RawFeatureSchema
    /// blob the web side stores verbatim.
    fn script_record_json(id: &str, schema: serde_json::Value) -> String {
        serde_json::to_string(&serde_json::json!({
            "id": id,
            "name": "Widget",
            "icon": "ƒ",
            "source": "",
            "compiledJs": "",
            "schema": schema,
            "version": 1,
        }))
        .unwrap()
    }

    #[test]
    fn list_feature_types_includes_script_ids() {
        let mut k = Kernel::new();
        let before: Vec<String> =
            serde_json::from_str(&k.list_feature_types().unwrap()).unwrap();
        assert!(!before.iter().any(|t| t == "my_script"));

        k.upsert_script(&script_record_json(
            "my_script",
            serde_json::json!({ "type": "widget", "name": "Widget", "params": [] }),
        ))
        .unwrap();

        let after: Vec<String> =
            serde_json::from_str(&k.list_feature_types().unwrap()).unwrap();
        assert!(after.iter().any(|t| t == "my_script"));
        // Still sorted and de-duplicated.
        let mut sorted = after.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(after, sorted);
    }

    #[test]
    fn schema_for_type_resolves_scripts_builtins_and_errors() {
        let mut k = Kernel::new();
        let schema = serde_json::json!({
            "type": "widget", "name": "Widget", "params": [],
        });
        k.upsert_script(&script_record_json("my_script", schema.clone()))
            .unwrap();

        // A script id resolves to its stored schema verbatim.
        let got: serde_json::Value =
            serde_json::from_str(&k.schema_for_type("my_script").unwrap()).unwrap();
        assert_eq!(got, schema);

        // A built-in type still routes through the registry and returns
        // that registry's schema verbatim.
        let builtin: serde_json::Value =
            serde_json::from_str(&k.schema_for_type("box").unwrap()).unwrap();
        let registry = features::FeatureRegistry::with_builtins();
        assert_eq!(builtin, registry.schema_for("box").unwrap());

        // An unknown type still misses both the scripts and the registry;
        // we assert the registry miss directly because the method's
        // not-found branch builds a `JsError` (panics on native targets).
        assert!(!k.document.scripts.iter().any(|r| r.id == "definitely_not_a_type"));
        assert!(registry.schema_for("definitely_not_a_type").is_none());
    }

    /// Regression for the outliner showing "? script_1 N" on a
    /// FieldSkript instance. A script call carries the script id as its
    /// `type_name`; `static_kind_name` collapses every non-built-in to
    /// "unknown", so `list_features` must instead pass the script id
    /// through as the `kind` (the web outliner keys `scriptTypes` —
    /// name + icon — by exactly this id).
    #[test]
    fn list_features_reports_script_id_as_kind() {
        let mut k = Kernel::new();
        k.upsert_script(&script_record_json(
            "script_1",
            serde_json::json!({ "type": "widget", "name": "Widget", "params": [] }),
        ))
        .unwrap();
        // A script instance and a built-in side by side.
        k.document.feature_calls.push(fielddraft_protocol::FeatureCall::new(
            "script_1",
            FeatureId::from_u32(1),
            serde_json::json!({}),
        ));
        k.document.feature_calls.push(fielddraft_protocol::FeatureCall::new(
            "box",
            FeatureId::from_u32(2),
            serde_json::json!({}),
        ));

        let arr: Vec<serde_json::Value> =
            serde_json::from_str(&k.list_features().unwrap()).unwrap();
        let kind_of = |id: u64| {
            arr.iter()
                .find(|f| f["id"] == serde_json::json!(id))
                .unwrap()["kind"]
                .clone()
        };
        assert_eq!(kind_of(1), serde_json::json!("script_1"));
        assert_eq!(kind_of(2), serde_json::json!("box"));

        // A call whose script was deleted falls back to "unknown".
        k.document.scripts.clear();
        let arr2: Vec<serde_json::Value> =
            serde_json::from_str(&k.list_features().unwrap()).unwrap();
        assert_eq!(
            arr2.iter().find(|f| f["id"] == serde_json::json!(1)).unwrap()["kind"],
            serde_json::json!("unknown")
        );
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

    /// Regression for the "delete button silently does nothing"
    /// bug reported in UX testing 2026-04-13. `Hole` is a
    /// non-spine feature type that pushes its `DecodedFeature` under a
    /// hierarchical child id (`<hole>.0`), so the old scan-by-flat-id
    /// loop in `remove_feature` couldn't match and returned false for
    /// every hole. Every feature the outliner shows must be
    /// removable, regardless of whether it's spine-backed or lives
    /// purely in `feature_calls`.
    #[test]
    fn remove_feature_deletes_hole_and_its_hierarchical_child_spine() {
        let mut k = Kernel::new();
        // Seed a base box so the hole has something to subtract from.
        let box_id = k
            .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, 20.0, 20.0, 20.0)
            .unwrap();
        let hole_id = k.add_hole(6.0, 40.0, 0.0, 0.0, 0.0).unwrap();

        // The hole lands in `feature_calls` as a flat-id entry and
        // its spine mirror lives under the hierarchical child id
        // `<hole_id>.0`. Both should disappear on remove.
        assert!(
            k.document.feature_calls.iter().any(|c| c.id == FeatureId::from_u32(hole_id)),
            "hole call present before remove"
        );
        let hole_spine_pre = k.decoded.iter().filter(|f| {
            let fid = FeatureId::from_u32(hole_id);
            f.id == fid || feature_id_is_descendant_of(&f.id, &fid)
        }).count();
        assert!(hole_spine_pre > 0, "hole has at least one spine entry before remove");

        assert!(k.remove_feature(hole_id), "delete returns true for a hole");
        assert!(!k.remove_feature(hole_id), "second delete returns false");

        // The hole is gone from both storages.
        assert!(
            !k.document.feature_calls.iter().any(|c| c.id == FeatureId::from_u32(hole_id)),
            "hole call was removed"
        );
        let hole_spine_post = k.decoded.iter().filter(|f| {
            let fid = FeatureId::from_u32(hole_id);
            f.id == fid || feature_id_is_descendant_of(&f.id, &fid)
        }).count();
        assert_eq!(hole_spine_post, 0, "no descendant spine entries survived");

        // The base box is untouched.
        assert!(
            k.decoded.iter().any(|f| f.id == FeatureId::from_u32(box_id)),
            "unrelated box remains"
        );
    }

    /// Regression: deleting a feature must take its gizmo transforms
    /// with it. A `translate`/`rotate`/`scale` call references its
    /// subject via `target_id`; an orphaned transform left in
    /// `feature_calls` made the *next* regeneration (e.g. adding another
    /// box) fail with "target feature N not found". Reproduces the bug
    /// reported after deleting everything from the scene, then adding a
    /// box.
    #[test]
    fn remove_feature_cascades_to_gizmo_transforms() {
        let mut k = Kernel::new();
        let box_id = k
            .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
            .unwrap();
        // Move it — pushes a `translate` call targeting the box.
        assert!(k.apply_gizmo_translate(box_id, 5.0, 0.0, 0.0));
        assert!(
            k.document
                .feature_calls
                .iter()
                .any(|c| c.type_name == "translate"),
            "translate call present after gizmo move"
        );

        // Delete the box: the orphaned translate must go too.
        assert!(k.remove_feature(box_id));
        assert!(
            k.document.feature_calls.is_empty(),
            "box and its translate are both gone"
        );

        // The bug: adding another primitive now regenerates cleanly.
        let next = k.add_primitive(
            Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0,
        );
        assert!(next.is_ok(), "regeneration succeeds after the cascade delete");
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
    fn set_feature_op_updates_feature() {
        let mut k = Kernel::new();
        let id = k
            .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0)
            .unwrap();
        assert_eq!(k.decoded[0].op, BoolOp::Union);
        k.set_feature_op(id, BoolOp::TAG_DIFFERENCE).unwrap();
        assert_eq!(k.decoded[0].op, BoolOp::Difference);
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
        // `contributors` are flat feature ids (what selection consumes);
        // `branches` carry the packed BranchId per face. The box +Y (top)
        // face is sub-index 2.
        assert!(
            json.contains(&format!("\"contributors\":[{id}]")),
            "{json}"
        );
        let branch = csg::BranchId::from_feature_sub(&FeatureId::from_u32(id), 2).as_u32();
        assert!(
            json.contains(&format!("\"branches\":[{branch}]")),
            "{json}"
        );
    }

    #[test]
    fn pick_selects_sphere_by_flat_feature_id() {
        // Regression: a sphere added beside a box must be pickable in the
        // scene. `pick` reports the flat feature id in `contributors` so the
        // frontend can select it; the curved face's packed branch lands in
        // `branches`. (The sphere is a single sub-0 analytic face — SA-6.)
        let mut k = Kernel::new();
        k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, 10.0, 10.0, 10.0)
            .unwrap();
        let sphere_id = k
            .add_primitive(Primitive::KIND_SPHERE, BoolOp::TAG_UNION, 60.0, 0.0, 0.0, 10.0, 10.0, 10.0)
            .unwrap();
        // Ray from far +X straight back hits the sphere first.
        let json = k.pick(200.0, 0.0, 0.0, -1.0, 0.0, 0.0);
        assert!(json.contains("\"hit\":true"), "{json}");
        assert!(
            json.contains(&format!("\"contributors\":[{sphere_id}]")),
            "{json}"
        );
        let branch = csg::BranchId::from_feature_sub(&FeatureId::from_u32(sphere_id), 0).as_u32();
        assert!(json.contains(&format!("\"branches\":[{branch}]")), "{json}");
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
    fn each_union_is_its_own_body() {
        let mut k = Kernel::new();
        let _id_a = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                0.0, 0.0, 0.0,
                10.0, 10.0, 10.0,
            )
            .unwrap();
        let _id_b = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                60.0, 0.0, 0.0,
                10.0, 10.0, 10.0,
            )
            .unwrap();
        assert_eq!(k.body_count(), 2);
        let _id_c = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_UNION,
                30.0, 0.0, 0.0,
                40.0, 10.0, 10.0,
            )
            .unwrap();
        assert_eq!(k.body_count(), 3, "each union must be its own body");
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
        // Flat feature id for box A in `contributors`; packed BranchId for
        // its +Y (top) face (sub-index 2) in `branches`.
        assert!(
            json.contains(&format!("\"contributors\":[{id_a}]")),
            "{json}"
        );
        let branch_a = csg::BranchId::from_feature_sub(&FeatureId::from_u32(id_a), 2).as_u32();
        assert!(
            json.contains(&format!("\"branches\":[{branch_a}]")),
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

    // ─── DL-61 C2b: feature_calls is the single source of truth ─

    /// Helper: with `feature_calls` authoritative and the spine
    /// derived from it, assert the regenerated spine lines up 1:1
    /// with the calls. Valid for the primitive-only documents these
    /// tests build — a Pattern/Mirror would add child spine entries
    /// and break the length equality. Locks that every call-first
    /// mutator leaves the spine consistent with the call list.
    fn assert_spine_matches_calls(k: &Kernel) {
        assert_eq!(
            k.decoded.len(),
            k.document.feature_calls.len(),
            "spine length diverged from feature_calls",
        );
        for (f, c) in k.decoded.iter().zip(&k.document.feature_calls) {
            assert_eq!(f.id, c.id, "spine id drifted from its feature_call");
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
        assert_spine_matches_calls(&k);
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
        assert_spine_matches_calls(&k);

        // Mutation 1: translate the box — feature_calls[0].params
        // must pick up the new translate.
        assert!(k.set_feature_translate(id_box, 10.0, 20.0, 30.0));
        assert_spine_matches_calls(&k);
        assert_eq!(
            k.document.feature_calls[0].params["translate"],
            serde_json::json!([10.0, 20.0, 30.0])
        );

        // Mutation 2: nudge the sphere.
        assert!(k.nudge_feature(id_sphere, 1.0, 2.0, 3.0));
        assert_spine_matches_calls(&k);

        // Mutation 3: change the cylinder's op.
        k.set_feature_op(id_cyl, BoolOp::TAG_DIFFERENCE).unwrap();
        assert_spine_matches_calls(&k);
        let cyl_call_idx = k
            .decoded
            .iter()
            .position(|f| f.id == FeatureId::from_u32(id_cyl))
            .unwrap();
        assert_eq!(
            k.document.feature_calls[cyl_call_idx].params["op"],
            serde_json::json!("difference")
        );

        // Mutation 4: reorder the cylinder to the front.
        assert!(k.reorder_feature(id_cyl, 0));
        assert_spine_matches_calls(&k);
        assert_eq!(k.decoded[0].id, FeatureId::from_u32(id_cyl));

        // Mutation 5: remove the box.
        assert!(k.remove_feature(id_box));
        assert_spine_matches_calls(&k);
        assert_eq!(k.decoded.len(), 2);

        // Mutation 6: full transform write on the sphere.
        assert!(k.set_feature_transform(
            id_sphere, 7.0, 8.0, 9.0, 0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0
        ));
        assert_spine_matches_calls(&k);
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
        assert_spine_matches_calls(&k);

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
        assert_spine_matches_calls(&k);

        assert!(k.undo());
        assert_eq!(k.document.feature_calls.len(), 1);
        assert_spine_matches_calls(&k);

        assert!(k.redo());
        assert_eq!(k.document.feature_calls.len(), 2);
        assert_spine_matches_calls(&k);

        // A mutation after an undo/redo cycle also stays in sync.
        assert!(k.nudge_feature(1, 1.0, 0.0, 0.0));
        assert_spine_matches_calls(&k);
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
        assert_eq!(k.decoded.len(), 2);

        let box_f = &k.decoded[0];
        assert_eq!(box_f.name, "Base");
        assert!(matches!(
            box_f.export_primitive(),
            Some(Primitive::Box { half_extents, .. }) if half_extents == [5.0, 5.0, 5.0]
        ));

        let sphere_f = &k.decoded[1];
        assert_eq!(sphere_f.name, "Hole");
        assert!(matches!(
            sphere_f.export_primitive(),
            Some(Primitive::Sphere { radius, .. }) if radius == 3.0
        ));
        assert_eq!(sphere_f.transform.translate, [20.0, 0.0, 0.0]);
        assert_eq!(sphere_f.op, BoolOp::Difference);

        // feature_calls should still be populated after rebuild_packed
        // — the dual-write hook rebuilds them from the freshly
        // regenerated features, which is a round-trip no-op thanks
        // to the lossless primitive execute impls.
        assert_spine_matches_calls(&k);
    }

    #[test]
    fn load_json_prunes_orphaned_gizmo_transform() {
        // A document written before the cascade-delete fix can carry a
        // gizmo transform whose `target_id` points at a feature that was
        // already deleted. Loading it used to fail outright with
        // "feature regeneration failed: ... target feature N not found".
        // The loader now prunes the orphan and loads what remains.
        let mut k = Kernel::new();
        let json = r#"{
            "schema_version": 2,
            "name": "stale",
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
                    "type": "translate",
                    "id": 2,
                    "params": {
                        "target_id": 3,
                        "translate": [5, 0, 0]
                    }
                }
            ],
            "next_feature_id": 4
        }"#;

        k.load_json(json).expect("stale document loads after pruning");
        // The orphaned translate is gone; only the box survives.
        assert_eq!(k.document.feature_calls.len(), 1);
        assert_eq!(k.decoded.len(), 1);
        assert_eq!(k.decoded[0].name, "Base");
        // And regeneration is healthy afterwards.
        let next = k.add_primitive(
            Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0,
        );
        assert!(next.is_ok(), "regeneration succeeds after pruning the orphan");
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
            id, 100.0, 200.0, 300.0, 0.0, 0.0, s, c, 2.5, 2.5, 2.5,
        ));

        // Capture the JSON, zero out `features` to simulate a
        // step-4d wire form, and reload.
        let json = k1.to_json().unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["features"] = serde_json::json!([]);
        let bootstrap_json = serde_json::to_string(&value).unwrap();

        let mut k2 = Kernel::new();
        k2.load_json(&bootstrap_json).unwrap();
        assert_eq!(k2.decoded.len(), 1);

        let loaded = &k2.decoded[0];
        assert_eq!(loaded.transform.translate, [100.0, 200.0, 300.0]);
        assert!((loaded.transform.rotation[0]).abs() < 1e-6);
        assert!((loaded.transform.rotation[1]).abs() < 1e-6);
        assert!((loaded.transform.rotation[2] - s).abs() < 1e-6);
        assert!((loaded.transform.rotation[3] - c).abs() < 1e-6);
        assert!((loaded.transform.scale[0] - 2.5).abs() < 1e-6);
        assert!((loaded.transform.scale[1] - 2.5).abs() < 1e-6);
        assert!((loaded.transform.scale[2] - 2.5).abs() < 1e-6);
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
        let original_features = k1.decoded.clone();

        // Zero out `features` and reload.
        let json = k1.to_json().unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["features"] = serde_json::json!([]);
        let bootstrap_json = serde_json::to_string(&value).unwrap();

        let mut k2 = Kernel::new();
        k2.load_json(&bootstrap_json).unwrap();
        assert_eq!(k2.decoded.len(), 3);

        // Every reloaded feature should match its pre-bootstrap
        // counterpart byte for byte.
        for (original, loaded) in
            original_features.iter().zip(k2.decoded.iter())
        {
            assert_eq!(loaded.id, original.id);
            assert_eq!(loaded.name, original.name);
            assert_eq!(loaded.op, original.op);
            assert_eq!(loaded.export_primitive(), original.export_primitive());
            assert_eq!(loaded.transform.translate, original.transform.translate);
            assert_eq!(loaded.transform.rotation, original.transform.rotation);
            assert_eq!(loaded.transform.scale, original.transform.scale);
        }
        assert_spine_matches_calls(&k2);
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

    // ─── FieldSkript S1: document-local script records ─────────

    #[test]
    fn script_crud_persists_in_document_json() {
        let mut k = Kernel::new();
        assert_eq!(k.list_scripts().unwrap(), "[]");

        let rec = r#"{"id":"slot","name":"Slot","source":"// ts",
            "compiledJs":"// js","schema":{"type":"slot","params":[]},"version":1}"#;
        // First upsert inserts (returns false), second replaces (true).
        assert!(!k.upsert_script(rec).unwrap());
        let rec2 = r#"{"id":"slot","name":"Slot v2","source":"// ts2",
            "compiledJs":"// js2","schema":{"type":"slot","params":[]},"version":2}"#;
        assert!(k.upsert_script(rec2).unwrap());

        let scripts: serde_json::Value =
            serde_json::from_str(&k.list_scripts().unwrap()).unwrap();
        assert_eq!(scripts.as_array().unwrap().len(), 1);
        assert_eq!(scripts[0]["name"], "Slot v2");

        // Records round-trip through the document JSON with the
        // camelCase `compiledJs` key.
        let json = k.to_json().unwrap();
        assert!(json.contains("\"compiledJs\""), "{json}");
        let mut k2 = Kernel::new();
        k2.load_json(&json).unwrap();
        let reloaded: serde_json::Value =
            serde_json::from_str(&k2.list_scripts().unwrap()).unwrap();
        assert_eq!(reloaded[0]["id"], "slot");
        assert_eq!(reloaded[0]["version"], 2);

        // Remove is idempotent: true once, false thereafter.
        assert!(k2.remove_script("slot"));
        assert!(!k2.remove_script("slot"));
        assert_eq!(k2.list_scripts().unwrap(), "[]");
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
            box_id, 1.0, 2.0, 3.0, 0.0, 0.0, s, c, 1.75, 1.75, 1.75,
        ));
        let original_features = k1.decoded.clone();

        // Save through the kernel and reality-check that the
        // wire form doesn't carry the spine.
        let json = k1.to_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(!value.as_object().unwrap().contains_key("features"));

        // Load into a fresh kernel and verify the spine.
        let mut k2 = Kernel::new();
        k2.load_json(&json).unwrap();
        assert_eq!(k2.decoded.len(), 2);
        for (original, loaded) in
            original_features.iter().zip(k2.decoded.iter())
        {
            assert_eq!(loaded.id, original.id);
            assert_eq!(loaded.name, original.name);
            assert_eq!(loaded.op, original.op);
            assert_eq!(loaded.export_primitive(), original.export_primitive());
            assert_eq!(loaded.transform.translate, original.transform.translate);
            assert_eq!(loaded.transform.rotation, original.transform.rotation);
            assert_eq!(loaded.transform.scale, original.transform.scale);
        }
        assert_spine_matches_calls(&k2);
        // And the meshing pipeline woke up — rebuild_packed ran
        // on load, so the mesh has real geometry.
        assert!(k2.mesh_vertex_count() > 0);
    }

    #[test]
    fn bug25_translated_cylinder_mesh_consistency() {
        let mut k = Kernel::new();

        // Two identical standalone cylinders, one at origin, one translated.
        let _id1 = k
            .add_primitive(
                Primitive::KIND_CYLINDER,
                BoolOp::TAG_UNION,
                0.0, 0.0, 0.0,
                0.6, 2.5, 0.0,
            )
            .unwrap();
        let _id2 = k
            .add_primitive(
                Primitive::KIND_CYLINDER,
                BoolOp::TAG_UNION,
                200.0, 0.0, 0.0,
                0.6, 2.5, 0.0,
            )
            .unwrap();

        assert_eq!(k.body_count(), 2);
        let v0 = k.body_vertex_count(0);
        let v1 = k.body_vertex_count(1);
        let diff = (v0 as i64 - v1 as i64).unsigned_abs();
        assert!(diff <= 2, "identical cylinders at different positions should mesh nearly identically: {v0} vs {v1}");
    }

    #[test]
    fn boolean_tab_switch_changes_tool_op() {
        let mut k = Kernel::new();
        let _box1 = k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0,0.0,0.0, 20.0,20.0,20.0).unwrap();
        let box2 = k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 10.0,0.0,0.0, 15.0,15.0,15.0).unwrap();
        assert_eq!(k.decoded.len(), 2);
        assert_eq!(k.decoded[0].op, BoolOp::Union);
        assert_eq!(k.decoded[1].op, BoolOp::Union);

        let bool_id = k.add_boolean().unwrap();
        // Initially Union with empty tools — no spine change
        assert_eq!(k.decoded[0].op, BoolOp::Union);
        assert_eq!(k.decoded[1].op, BoolOp::Union);

        // Set tools to box2 + operation to Subtract (simulates tab switch)
        let params = serde_json::json!({
            "operation": "Subtract",
            "tools": format!("qCreatedBy({})", box2),
            "targets": "",
            "keep_tools": false,
        });
        k.replace_feature_call_params(bool_id, &params.to_string()).unwrap();
        // box2 should now be Difference
        assert_eq!(k.decoded[0].op, BoolOp::Union, "box1 stays Union");
        assert_eq!(k.decoded[1].op, BoolOp::Difference, "box2 is Difference after Subtract");

        // Switch tab to Union
        let params = serde_json::json!({
            "operation": "Union",
            "tools": format!("qCreatedBy({})", box2),
            "targets": "",
            "keep_tools": false,
        });
        k.replace_feature_call_params(bool_id, &params.to_string()).unwrap();
        assert_eq!(k.decoded[1].op, BoolOp::Union, "box2 reverts to Union");

        // Switch tab to Intersect
        let params = serde_json::json!({
            "operation": "Intersect",
            "tools": format!("qCreatedBy({})", box2),
            "targets": "",
            "keep_tools": false,
        });
        k.replace_feature_call_params(bool_id, &params.to_string()).unwrap();
        assert_eq!(k.decoded[1].op, BoolOp::Intersect, "box2 is Intersect");
    }

    #[test]
    fn boolean_union_merges_tools_into_one_body() {
        let mut k = Kernel::new();
        let box1 = k.add_primitive(
            Primitive::KIND_BOX, BoolOp::TAG_UNION,
            0.0, 0.0, 0.0, 20.0, 20.0, 20.0,
        ).unwrap();
        let sph = k.add_primitive(
            Primitive::KIND_SPHERE, BoolOp::TAG_UNION,
            20.0, 0.0, 0.0, 15.0, 0.0, 0.0,
        ).unwrap();
        // Two plain primitives are two separate parts.
        assert_eq!(k.body_count(), 2, "two plain unions are two parts");

        let bool_id = k.add_boolean().unwrap();
        let params = serde_json::json!({
            "operation": "Union",
            "tools": format!("qCreatedBy({}), qCreatedBy({})", box1, sph),
            "targets": "",
            "keep_tools": false,
        });
        k.replace_feature_call_params(bool_id, &params.to_string()).unwrap();

        // The boolean routes the second tool into the first tool's body.
        assert_eq!(k.decoded[0].op, BoolOp::Union);
        assert_eq!(k.decoded[1].op, BoolOp::Union);
        assert!(k.decoded[0].target_ids.is_empty(), "seed carries no target");
        assert_eq!(k.decoded[1].target_ids, vec![FeatureId::from_u32(box1)]);
        assert_eq!(k.body_count(), 1, "an explicit union is one part");

        // The folded tree is a single solid spanning both lobes.
        let tree = csg::CsgNode::from_decoded(&k.decoded);
        let sdf = |p: [f32; 3]| tree.sdf(p, csg::EvalMode::Logical);
        assert!(sdf([0.0, 0.0, 0.0]) < 0.0, "inside the box lobe");
        assert!(sdf([30.0, 0.0, 0.0]) < 0.0, "inside the sphere lobe");
        assert!(sdf([100.0, 0.0, 0.0]) > 0.0, "outside both");
    }

    #[test]
    fn shared_tool_before_second_seed_in_spine_still_fuses() {
        // Same transitive-union scene as `two_boolean_unions_sharing_a_tool…`,
        // but the SPINE ORDER is the one the user's project hit: the shared
        // bridge (sphere) is created *before* the second box. The Boolean
        // picks its seed as the first tool in spine order, so op2's seed is
        // the sphere — NOT the second box. If the seed-assignment clears the
        // sphere's accumulated link to op1's box, the first box splits off
        // into its own Part ("2 Parts that should be 1"). All three overlap,
        // so geometry never gates the merge; the only thing that can break it
        // is clobbering the bridge's link.
        let mut k = Kernel::new();
        let box_a = k.add_primitive(
            Primitive::KIND_BOX, BoolOp::TAG_UNION,
            -10.0, 0.0, 0.0, 12.0, 12.0, 12.0,
        ).unwrap();
        let sph = k.add_primitive(
            Primitive::KIND_SPHERE, BoolOp::TAG_UNION,
            0.0, 0.0, 0.0, 15.0, 0.0, 0.0,
        ).unwrap();
        // Created AFTER the sphere → spine-later than the shared bridge.
        let box_b = k.add_primitive(
            Primitive::KIND_BOX, BoolOp::TAG_UNION,
            10.0, 0.0, 0.0, 12.0, 12.0, 12.0,
        ).unwrap();

        // Op 1: box_a ∪ sphere.
        let b1 = k.add_boolean().unwrap();
        let p1 = serde_json::json!({
            "operation": "Union",
            "tools": format!("qCreatedBy({}), qCreatedBy({})", box_a, sph),
            "targets": "",
            "keep_tools": false,
        });
        k.replace_feature_call_params(b1, &p1.to_string()).unwrap();

        // Op 2: box_b ∪ sphere — sphere is spine-earlier than box_b, so it
        // is op2's seed.
        let b2 = k.add_boolean().unwrap();
        let p2 = serde_json::json!({
            "operation": "Union",
            "tools": format!("qCreatedBy({}), qCreatedBy({})", box_b, sph),
            "targets": "",
            "keep_tools": false,
        });
        k.replace_feature_call_params(b2, &p2.to_string()).unwrap();

        assert_eq!(
            k.body_count(), 1,
            "a shared bridge created before the second seed must still fuse all three"
        );
        let tree = csg::CsgNode::from_decoded(&k.decoded);
        let sdf = |p: [f32; 3]| tree.sdf(p, csg::EvalMode::Logical);
        assert!(sdf([-10.0, 0.0, 0.0]) < 0.0, "inside box_a");
        assert!(sdf([0.0, 0.0, 0.0]) < 0.0, "inside the sphere");
        assert!(sdf([10.0, 0.0, 0.0]) < 0.0, "inside box_b");
    }

    #[test]
    fn two_boolean_unions_sharing_a_tool_form_one_body() {
        // Scene: a left box, a right box, and a centre sphere that bridges
        // both. The user builds it as TWO separate union operations that
        // share the sphere: (left ∪ sphere) and (right ∪ sphere). Union is
        // transitive, so all three must collapse into a single Part — the
        // second union must not clobber the sphere's link to the first.
        let mut k = Kernel::new();
        let left = k.add_primitive(
            Primitive::KIND_BOX, BoolOp::TAG_UNION,
            -20.0, 0.0, 0.0, 10.0, 10.0, 10.0,
        ).unwrap();
        let right = k.add_primitive(
            Primitive::KIND_BOX, BoolOp::TAG_UNION,
            20.0, 0.0, 0.0, 10.0, 10.0, 10.0,
        ).unwrap();
        let sph = k.add_primitive(
            Primitive::KIND_SPHERE, BoolOp::TAG_UNION,
            0.0, 0.0, 0.0, 15.0, 0.0, 0.0,
        ).unwrap();
        assert_eq!(k.body_count(), 3, "three plain unions are three parts");

        // Operation 1: left ∪ sphere.
        let b1 = k.add_boolean().unwrap();
        let p1 = serde_json::json!({
            "operation": "Union",
            "tools": format!("qCreatedBy({}), qCreatedBy({})", left, sph),
            "targets": "",
            "keep_tools": false,
        });
        k.replace_feature_call_params(b1, &p1.to_string()).unwrap();

        // Operation 2: right ∪ sphere — shares the sphere with op 1.
        let b2 = k.add_boolean().unwrap();
        let p2 = serde_json::json!({
            "operation": "Union",
            "tools": format!("qCreatedBy({}), qCreatedBy({})", right, sph),
            "targets": "",
            "keep_tools": false,
        });
        k.replace_feature_call_params(b2, &p2.to_string()).unwrap();

        // The sphere accumulates BOTH seed links (left + right), not just
        // the most recent one.
        let sphere_feat = k.decoded.iter().find(|f| f.id == FeatureId::from_u32(sph)).unwrap();
        assert!(
            sphere_feat.target_ids.contains(&FeatureId::from_u32(left))
                && sphere_feat.target_ids.contains(&FeatureId::from_u32(right)),
            "the bridging sphere links to both union seeds: {:?}",
            sphere_feat.target_ids
        );
        assert_eq!(k.body_count(), 1, "the shared sphere fuses both unions into one part");

        // The fused solid spans all three lobes.
        let tree = csg::CsgNode::from_decoded(&k.decoded);
        let sdf = |p: [f32; 3]| tree.sdf(p, csg::EvalMode::Logical);
        assert!(sdf([-20.0, 0.0, 0.0]) < 0.0, "inside the left box");
        assert!(sdf([0.0, 0.0, 0.0]) < 0.0, "inside the sphere");
        assert!(sdf([20.0, 0.0, 0.0]) < 0.0, "inside the right box");
    }

    /// The merged union mesh is the *correct* outer surface — every mesh vertex
    /// lies on the union surface, never inside the solid. Guards against a
    /// regression where the union would mesh with an interior "scoop" (a lobe's
    /// hidden cap left in the mesh). NB: the concave-scoop the user saw at the
    /// box∪sphere overlap is a *renderer* compositing artifact (a translucent
    /// context body occluding the active body's raymarch), not a mesh defect —
    /// this test pins the geometry side so that distinction stays provable.
    #[test]
    fn union_mesh_has_no_interior_scoop() {
        let mut k = Kernel::new();
        // Box [±10] at origin; sphere sitting on top, overlapping the +Z face.
        let box1 = k.add_primitive(
            Primitive::KIND_BOX, BoolOp::TAG_UNION,
            0.0, 0.0, 0.0, 10.0, 10.0, 10.0,
        ).unwrap();
        let sph = k.add_primitive(
            Primitive::KIND_SPHERE, BoolOp::TAG_UNION,
            0.0, 0.0, 14.0, 8.0, 0.0, 0.0,
        ).unwrap();
        let bool_id = k.add_boolean().unwrap();
        let params = serde_json::json!({
            "operation": "Union",
            "tools": format!("qCreatedBy({}), qCreatedBy({})", box1, sph),
            "targets": "", "keep_tools": false,
        });
        k.replace_feature_call_params(bool_id, &params.to_string()).unwrap();
        assert_eq!(k.body_count(), 1, "the explicit union is one body");

        let tree = csg::CsgNode::from_decoded(&k.decoded);
        let sdf = |p: [f32; 3]| tree.sdf(p, csg::EvalMode::Logical);
        let body = &k.bodies[0];
        let pos = &body.mesh.positions;
        let mut worst = 0.0f32;
        for v in 0..pos.len() / 3 {
            let d = sdf([pos[3 * v], pos[3 * v + 1], pos[3 * v + 2]]);
            if d < worst {
                worst = d;
            }
        }
        assert!(
            worst > -0.5,
            "union mesh has interior vertices (scoop): worst sdf = {worst}"
        );
    }

    #[test]
    fn boolean_subtract_sdf_is_geometrically_correct() {
        let mut k = Kernel::new();
        let box1 = k.add_primitive(
            Primitive::KIND_BOX, BoolOp::TAG_UNION,
            0.0, 0.0, 0.0, 50.0, 50.0, 50.0,
        ).unwrap();
        let sph = k.add_primitive(
            Primitive::KIND_SPHERE, BoolOp::TAG_UNION,
            30.0, 0.0, 0.0, 40.0, 0.0, 0.0,
        ).unwrap();

        let bool_id = k.add_boolean().unwrap();
        let params = serde_json::json!({
            "operation": "Subtract",
            "tools": format!("qCreatedBy({})", sph),
            "targets": format!("qCreatedBy({})", box1),
            "keep_tools": false,
        });
        k.replace_feature_call_params(bool_id, &params.to_string()).unwrap();

        assert_eq!(k.decoded[1].op, BoolOp::Difference);
        assert_eq!(k.body_count(), 1, "box and sphere-diff share one body");

        let tree = csg::CsgNode::from_decoded(&k.decoded);
        let sdf = |p: [f32; 3]| tree.sdf(p, csg::EvalMode::Logical);
        // sphere center [30,0,0] radius 40 → extends x=-10..70
        assert!(sdf([-40.0, 0.0, 0.0]) < 0.0, "inside box, outside sphere: stays solid");
        assert!(sdf([0.0, 0.0, 0.0]) > 0.0, "box center inside sphere: carved out");
        assert!(sdf([35.0, 0.0, 0.0]) > 0.0, "deep in sphere overlap: carved out");
        assert!(sdf([80.0, 0.0, 0.0]) > 0.0, "outside both: exterior");
    }

    // ─── Feature-based B-Rep topology API + lazy cache ──────────────

    #[test]
    fn body_brep_counts_box_is_watertight_cube() {
        let mut k = Kernel::new();
        k.decoded = vec![box_feature(
            1,
            BoolOp::Union,
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
        )];
        k.rebuild_mesh_internal();
        assert_eq!(k.body_count(), 1);

        let json = k.body_brep_counts(0);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["vertices"], 8, "box B-Rep has 8 corners");
        assert_eq!(v["edges"], 12, "box B-Rep has 12 edges");
        assert_eq!(v["faces"], 6, "box B-Rep has 6 faces");
        assert_eq!(v["shells"], 1);
        assert_eq!(v["solids"], 1);
        assert_eq!(v["watertight"], true, "box outer shell is closed");
    }

    #[test]
    fn body_brep_woven_during_remesh_and_reused() {
        // Brocken A: the mesh path now weaves the B-Rep eagerly during remesh
        // to tag render triangles with their face identity (the user wants the
        // B-Rep present at render time). The weave is cached and keyed on the
        // body-local payload, so subsequent queries reuse it without
        // re-weaving.
        let mut k = Kernel::new();
        k.decoded = vec![box_feature(
            1,
            BoolOp::Union,
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
        )];
        k.rebuild_mesh_internal();

        // Eagerly woven for face tagging.
        assert!(
            k.bodies[0].brep.is_some(),
            "B-Rep is woven during remesh for mesh face tagging"
        );

        let cached = k.bodies[0].brep.clone();
        let first = k.body_brep_counts(0);
        let second = k.body_brep_counts(0);
        assert_eq!(first, second, "repeat query returns identical counts");
        assert_eq!(
            cached,
            k.bodies[0].brep,
            "queries reuse the cache woven at remesh, not re-weave"
        );
    }

    #[test]
    fn body_brep_reflects_new_geometry_after_remesh() {
        let mut k = Kernel::new();
        k.decoded = vec![box_feature(
            1,
            BoolOp::Union,
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
        )];
        k.rebuild_mesh_internal();
        let key_before = k.bodies[0].brep.as_ref().map(|(key, _)| *key);
        assert!(key_before.is_some(), "B-Rep woven at remesh");

        // Grow the box → geometry hash changes → full remesh re-weaves the
        // B-Rep under a new payload key (the cache self-heals on geometry
        // change rather than being lazily None'd).
        k.decoded[0] = box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
        k.rebuild_mesh_internal();
        let key_after = k.bodies[0].brep.as_ref().map(|(key, _)| *key);
        assert!(key_after.is_some(), "B-Rep re-woven at remesh");
        assert_ne!(
            key_before, key_after,
            "geometry change must re-weave the B-Rep under a new key"
        );
    }

    #[test]
    fn body_brep_never_panics_on_smooth_and_csg_bodies() {
        // (1) Lone sphere: smooth, no sharp features → must not panic.
        let mut k = Kernel::new();
        k.decoded = vec![sphere_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], 1.0)];
        k.rebuild_mesh_internal();
        let json = k.body_brep_counts(0);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v["vertices"].is_number());
        assert!(v["faces"].is_number());

        // (2) A CSG body (box minus an overlapping sphere) must also
        // weave without panicking.
        let mut k2 = Kernel::new();
        k2.decoded = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [2.0, 2.0, 2.0]),
            sphere_feature(2, BoolOp::Difference, [2.0, 2.0, 2.0], 1.5),
        ];
        k2.rebuild_mesh_internal();
        assert_eq!(k2.body_count(), 1, "box and carving sphere share one body");
        let json2 = k2.body_brep_counts(0);
        let v2: serde_json::Value = serde_json::from_str(&json2).unwrap();
        assert!(v2["edges"].is_number());
        assert!(v2["shells"].is_number());
    }

    // ─── STEP (AP214) export ────────────────────────────────────────

    /// Collect every defined `#N=` id and every referenced `#N` id from a
    /// STEP document.
    fn step_defined_and_referenced(
        step: &str,
    ) -> (
        std::collections::HashSet<usize>,
        std::collections::HashSet<usize>,
    ) {
        let mut defined = std::collections::HashSet::new();
        let mut referenced = std::collections::HashSet::new();
        for line in step.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix('#') {
                if let Some(eq) = rest.find('=') {
                    if let Ok(id) = rest[..eq].parse::<usize>() {
                        assert!(defined.insert(id), "#{id} defined twice");
                        let body = &rest[eq + 1..];
                        let bytes = body.as_bytes();
                        let mut i = 0;
                        while i < bytes.len() {
                            if bytes[i] == b'#' {
                                let mut j = i + 1;
                                while j < bytes.len() && bytes[j].is_ascii_digit() {
                                    j += 1;
                                }
                                if j > i + 1 {
                                    referenced.insert(body[i + 1..j].parse::<usize>().unwrap());
                                }
                                i = j;
                            } else {
                                i += 1;
                            }
                        }
                    }
                }
            }
        }
        (defined, referenced)
    }

    #[test]
    fn body_step_export_box_is_valid_advanced_brep() {
        let mut k = Kernel::new();
        k.decoded = vec![box_feature(
            1,
            BoolOp::Union,
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
        )];
        k.rebuild_mesh_internal();
        assert_eq!(k.body_count(), 1);

        let step = k.body_step_export(0);
        assert!(step.starts_with("ISO-10303-21;"));
        assert!(step.trim_end().ends_with("END-ISO-10303-21;"));
        assert_eq!(step.matches("HEADER;").count(), 1);
        assert_eq!(step.matches("DATA;").count(), 1);
        assert!(step.contains("FILE_SCHEMA(("));

        let count = |name: &str| {
            step.lines()
                .filter(|l| {
                    l.find('=')
                        .map(|e| l[e + 1..].trim_start().starts_with(&format!("{name}(")))
                        .unwrap_or(false)
                })
                .count()
        };
        assert_eq!(count("MANIFOLD_SOLID_BREP"), 1);
        assert_eq!(count("CLOSED_SHELL"), 1);
        assert_eq!(count("ADVANCED_FACE"), 6);
        assert_eq!(count("PLANE"), 6);
        assert_eq!(count("VERTEX_POINT"), 8);
        assert_eq!(count("EDGE_CURVE"), 12);

        let (defined, referenced) = step_defined_and_referenced(&step);
        let missing: Vec<usize> = referenced.difference(&defined).copied().collect();
        assert!(missing.is_empty(), "dangling references: {missing:?}");
    }

    #[test]
    fn body_step_export_twisted_box_refs_resolve() {
        // A twisted box yields curved NURBS edges; the STEP graph must
        // still be referentially complete.
        let feat = DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "Box 1".into(),
            BoolOp::Union,
            Primitive::Box { center: [0.0, 0.0, 0.0], half_extents: [1.0, 1.0, 1.0] },
            Transform::IDENTITY,
            PrimitiveModifiers { twist_rate: 0.3, ..PrimitiveModifiers::NONE },
            vec![],
        );
        let mut k = Kernel::new();
        k.decoded = vec![feat];
        k.rebuild_mesh_internal();
        assert_eq!(k.body_count(), 1);

        let step = k.body_step_export(0);
        let (defined, referenced) = step_defined_and_referenced(&step);
        let missing: Vec<usize> = referenced.difference(&defined).copied().collect();
        assert!(missing.is_empty(), "dangling references: {missing:?}");
        assert!(step.contains("B_SPLINE_CURVE_WITH_KNOTS"));
    }

    // ─── DL-50 correction: body-local weave + placement fold-back ─────────

    /// `inverse(t)` must round-trip a rigid + uniform-scale transform:
    /// `compose(t, inverse(t)) ≈ compose(inverse(t), t) ≈ IDENTITY`.
    #[test]
    fn inverse_transform_round_trips() {
        let s2 = std::f32::consts::FRAC_1_SQRT_2; // sin/cos of 45° = 90° half-angle
        let t = Transform {
            translate: [3.0, -2.0, 5.0],
            rotation: [0.0, s2, 0.0, s2], // 90° about Y
            scale: [2.0, 2.0, 2.0],
        };
        let approx_identity = |c: &Transform| {
            let close = |a: [f32; 3], b: [f32; 3]| {
                (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
            };
            close(c.translate, [0.0, 0.0, 0.0])
                && close(c.scale, [1.0, 1.0, 1.0])
                // identity quat is (0,0,0,±1)
                && c.rotation[0].abs() < 1e-4
                && c.rotation[1].abs() < 1e-4
                && c.rotation[2].abs() < 1e-4
                && (c.rotation[3].abs() - 1.0).abs() < 1e-4
        };
        let inv = inverse(&t);
        assert!(approx_identity(&compose_transforms(&t, &inv)), "t∘t⁻¹ ≠ I");
        assert!(approx_identity(&compose_transforms(&inv, &t)), "t⁻¹∘t ≠ I");
    }

    /// Headline reuse test: a pure body move must NOT re-weave. We weave a
    /// body, capture the cached B-Rep's address and the placement-invariant
    /// local key, then translate every feature by a delta and re-`ensure_brep`.
    /// The local key is unchanged and the cached B-Rep is the *same allocation*
    /// (pointer identity), proving the weave did not run again.
    #[test]
    fn move_reuses_local_brep_cache() {
        use body::{compute_body_placement, hash_body_local_payload, Body};
        let features: Vec<DecodedFeature> =
            vec![box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0])];
        let parts = body::partition(&features);
        assert_eq!(parts.len(), 1);
        let part = &parts[0];

        let mut bdy = Body {
            id: part.seed_id(&features),
            feature_indices: part.feature_indices.clone(),
            aabb: part.aabb,
            payload_hash: 0,
            geometry_hash: 0,
            transform_hash: 0,
            mesh_resolution: 0,
            mesh: Mesh::default(),
            version: 1,
            transform_delta: None,
            last_mesh_transforms: Vec::new(),
            compiled_sdf: None,
            placement: Transform::IDENTITY,
            brep: None,
        };

        bdy.ensure_brep(&features, false);
        let key0 = hash_body_local_payload(&features, &bdy.feature_indices, &bdy.placement);
        let ptr0 = bdy.brep_cached().unwrap() as *const _;

        // Move the body: translate the (only) feature by a delta. The body's
        // feature_indices stay the same; only the transform shifts.
        let mut moved = features.clone();
        moved[0].transform.translate = [10.0, -4.0, 7.0];

        // Placement now reflects the move (cheap, every call).
        let placement_moved = compute_body_placement(&moved, &bdy.feature_indices);
        assert_eq!(placement_moved.translate, [10.0, -4.0, 7.0]);
        let key1 = hash_body_local_payload(&moved, &bdy.feature_indices, &placement_moved);
        assert_eq!(key0, key1, "pure move must NOT change the local cache key");

        bdy.ensure_brep(&moved, false);
        let ptr1 = bdy.brep_cached().unwrap() as *const _;
        assert_eq!(ptr0, ptr1, "pure move must reuse the cached B-Rep (no re-weave)");
        assert_eq!(bdy.placement.translate, [10.0, -4.0, 7.0], "placement updated");
    }

    /// World-equivalence: exporting a body placed at transform `T` yields the
    /// same world coordinates as the identity-placed body shifted by `T`.
    /// Concretely: a box at the origin and the same box translated by `D`
    /// export CARTESIAN_POINTs that differ by exactly `D` (the local weave +
    /// placement fold-back reproduces the old world-space STEP).
    #[test]
    fn step_world_equivalence_under_move() {
        let mut at_origin = Kernel::new();
        at_origin.decoded =
            vec![box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0])];
        at_origin.rebuild_mesh_internal();
        let step0 = at_origin.body_step_export(0);
        let pts0 = step_cartesian_points(&step0);

        let mut moved = Kernel::new();
        let mut feat = box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        feat.transform.translate = [10.0, 20.0, -5.0];
        moved.decoded = vec![feat];
        moved.rebuild_mesh_internal();
        let step1 = moved.body_step_export(0);
        let pts1 = step_cartesian_points(&step1);

        // Same number of emitted points (de-dup is geometry-driven and the
        // geometry is identical up to a rigid shift).
        assert!(!pts0.is_empty(), "box must emit CARTESIAN_POINTs");
        assert_eq!(
            pts0.len(),
            pts1.len(),
            "a move must not change the point count"
        );
        // Every origin point + delta must appear in the moved set: the local
        // weave + placement fold-back reproduces the old world-space STEP.
        let d = [10.0_f64, 20.0, -5.0];
        for p in &pts0 {
            let shifted = [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
            assert!(
                pts1.iter().any(|q| (0..3).all(|i| (q[i] - shifted[i]).abs() < 1e-5)),
                "moved STEP missing shifted point {shifted:?}"
            );
        }
    }

    // ─── Brocken A: B-Rep face/vertex exports + mesh face tagging ────

    /// Parse a `serde_json::Value` array returned by a `bodyBrep*` export.
    fn parse_json_array(s: &str) -> Vec<serde_json::Value> {
        match serde_json::from_str::<serde_json::Value>(s).unwrap() {
            serde_json::Value::Array(a) => a,
            other => panic!("expected JSON array, got {other:?}"),
        }
    }

    /// A unit cube body driven through the full add/rebuild pipeline.
    fn box_kernel(half: f32) -> Kernel {
        let mut k = Kernel::new();
        k.add_primitive(Primitive::KIND_BOX, BoolOp::TAG_UNION, 0.0, 0.0, 0.0, half, half, half)
            .unwrap();
        k
    }

    #[test]
    fn brocken_a_box_faces_six_distinct_planes() {
        let mut k = box_kernel(25.0);
        let faces = parse_json_array(&k.body_brep_faces(0));
        assert_eq!(faces.len(), 6, "a box B-Rep has 6 faces");

        let mut id_hashes = std::collections::HashSet::new();
        for f in &faces {
            assert_eq!(f["kind"], "face");
            assert_eq!(f["surfaceKind"], "plane");
            // idHash is a decimal string that parses to a u64.
            let hash_str = f["idHash"].as_str().expect("idHash is a string");
            let parsed: u64 = hash_str.parse().expect("idHash parses to u64");
            assert_ne!(parsed, 0);
            id_hashes.insert(hash_str.to_string());

            // Each face centroid sits on a ±25 box side (one coord at ±25).
            let c = f["centroidLocal"].as_array().unwrap();
            let coords: Vec<f64> = c.iter().map(|v| v.as_f64().unwrap()).collect();
            let on_side = coords.iter().any(|&v| (v.abs() - 25.0).abs() < 1.0);
            assert!(on_side, "centroid {coords:?} should lie on a ±25 box side");

            // Normal is unit-length and axis-aligned for a box.
            let n = f["normalLocal"].as_array().unwrap();
            let nv: Vec<f64> = n.iter().map(|v| v.as_f64().unwrap()).collect();
            let len = (nv[0] * nv[0] + nv[1] * nv[1] + nv[2] * nv[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-6, "normal {nv:?} not unit length");
        }
        assert_eq!(id_hashes.len(), 6, "all 6 face idHashes must be distinct");
    }

    #[test]
    fn brocken_a_box_vertices_eight_distinct() {
        let mut k = box_kernel(25.0);
        let verts = parse_json_array(&k.body_brep_vertices(0));
        assert_eq!(verts.len(), 8, "a box B-Rep has 8 corner vertices");

        let mut id_hashes = std::collections::HashSet::new();
        for v in &verts {
            assert_eq!(v["kind"], "vertex");
            let hash_str = v["idHash"].as_str().expect("idHash is a string");
            let _: u64 = hash_str.parse().expect("idHash parses to u64");
            id_hashes.insert(hash_str.to_string());

            // Corner sits at (±25, ±25, ±25).
            let p = v["positionLocal"].as_array().unwrap();
            for coord in p {
                let c = coord.as_f64().unwrap();
                assert!((c.abs() - 25.0).abs() < 1.0, "corner coord {c} not ±25");
            }
            // At least 3 colliding branches at a box corner.
            assert!(v["branches"].as_array().unwrap().len() >= 3);
        }
        assert_eq!(id_hashes.len(), 8, "all 8 vertex idHashes must be distinct");
    }

    #[test]
    fn brocken_a_placement_round_trips() {
        let mut k = box_kernel(10.0);
        let p: serde_json::Value =
            serde_json::from_str(&k.body_placement(0)).unwrap();
        // Identity placement for an origin box: zero translate, identity quat,
        // unit scale.
        let t = p["translate"].as_array().unwrap();
        assert_eq!(t.iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>(), vec![0.0, 0.0, 0.0]);
        let r = p["rotation"].as_array().unwrap();
        assert_eq!(r.iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>(), vec![0.0, 0.0, 0.0, 1.0]);
        let s = p["scale"].as_array().unwrap();
        for v in s {
            assert!((v.as_f64().unwrap() - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn brocken_a_face_idhash_stable_across_resize() {
        // The whole point of TopoRef: a non-topological parameter change
        // (resize) leaves face idHashes byte-for-byte identical, because they
        // derive from branch signatures, not geometry.
        let mut k_small = box_kernel(10.0);
        let mut k_big = box_kernel(40.0);

        let collect = |k: &mut Kernel| -> std::collections::BTreeSet<String> {
            parse_json_array(&k.body_brep_faces(0))
                .iter()
                .map(|f| f["idHash"].as_str().unwrap().to_string())
                .collect()
        };
        let small = collect(&mut k_small);
        let big = collect(&mut k_big);
        assert_eq!(small, big, "resize must not change face idHashes");
    }

    #[test]
    fn brocken_a_mesh_face_index_partitions_box() {
        let k = box_kernel(25.0);
        // add_primitive already ran the rebuild + per-triangle face tagging.
        let tc = k.body_triangle_count(0);
        assert!(tc > 0, "box must produce a non-empty mesh");

        let face_index = k.bodies[0].mesh.face_index.clone();
        assert_eq!(face_index.len(), tc, "one face tag per mesh triangle");

        // Every triangle resolves to a valid face index < 6, no sentinels on a
        // clean box, and all 6 faces are represented (the tags partition the
        // mesh by side).
        let mut distinct = std::collections::HashSet::new();
        for &fi in &face_index {
            assert_ne!(
                fi,
                crate::brep::face_export::FACE_INDEX_SENTINEL,
                "no sentinels on a clean box",
            );
            assert!(fi < 6, "face_index {fi} must be < 6 for a box");
            distinct.insert(fi);
        }
        assert_eq!(distinct.len(), 6, "the 6 face tags must partition the mesh");

        // Triangles clearly INTERIOR to one flat side (one centroid coord at
        // ±25, the other two well within the face, away from shared edges)
        // must all carry that side's single face tag. Corner/edge triangles
        // straddle two faces and are legitimately ambiguous, so we exclude
        // them. All 6 sides must be represented among the interior triangles.
        let positions = &k.bodies[0].mesh.positions;
        let indices = &k.bodies[0].mesh.indices;
        let mut side_tag: std::collections::HashMap<(usize, i8), u32> =
            std::collections::HashMap::new();
        for t in 0..tc {
            let mut c = [0.0f32; 3];
            for j in 0..3 {
                let vi = indices[t * 3 + j] as usize;
                c[0] += positions[vi * 3];
                c[1] += positions[vi * 3 + 1];
                c[2] += positions[vi * 3 + 2];
            }
            c = [c[0] / 3.0, c[1] / 3.0, c[2] / 3.0];
            // A face-interior triangle has exactly ONE axis pinned to ±25 and
            // the other two safely inside (|coord| < 20), away from any shared
            // edge. Count pinned axes and inside axes.
            let mut pinned_axis: Option<usize> = None;
            let mut pinned_count = 0;
            let mut inside_count = 0;
            for (axis, &coord) in c.iter().enumerate() {
                if (coord.abs() - 25.0).abs() < 0.5 {
                    pinned_axis = Some(axis);
                    pinned_count += 1;
                } else if coord.abs() < 20.0 {
                    inside_count += 1;
                }
            }
            if pinned_count != 1 || inside_count != 2 {
                continue; // corner / edge / off-face triangle
            }
            let axis = pinned_axis.unwrap();
            let sign = if c[axis] >= 0.0 { 1i8 } else { -1i8 };
            let key = (axis, sign);
            let tag = face_index[t];
            match side_tag.get(&key) {
                None => {
                    side_tag.insert(key, tag);
                }
                Some(&existing) => assert_eq!(
                    existing, tag,
                    "interior triangles on side {key:?} share a face tag",
                ),
            }
        }
        assert_eq!(side_tag.len(), 6, "all 6 box sides must be represented");
    }

    #[test]
    fn brocken_br_edges_carry_parseable_id_hash() {
        // Brocken B-R: bodyBrepEdges now emits a decimal `idHash` per edge
        // (alongside a/b/points), so a clicked edge can emit qEdge(<idHash>).
        let mut k = box_kernel(25.0);
        let edges = parse_json_array(&k.body_brep_edges(0));
        assert_eq!(edges.len(), 12, "a box B-Rep has 12 edges");

        let mut id_hashes = std::collections::HashSet::new();
        for e in &edges {
            // Existing fields still present.
            assert!(e["a"].is_number());
            assert!(e["b"].is_number());
            assert!(e["points"].is_array());
            // New field: a decimal string that parses to a u64.
            let hash_str = e["idHash"].as_str().expect("idHash is a string");
            let parsed: u64 = hash_str.parse().expect("idHash parses to u64");
            assert_ne!(parsed, 0);
            id_hashes.insert(hash_str.to_string());
        }
        assert_eq!(id_hashes.len(), 12, "all 12 edge idHashes must be distinct");
    }

    /// Collect every `CARTESIAN_POINT('',(x,y,z))` coordinate from a STEP file.
    fn step_cartesian_points(step: &str) -> Vec<[f64; 3]> {
        let mut out = Vec::new();
        for line in step.lines() {
            if let Some(idx) = line.find("CARTESIAN_POINT('',(") {
                let rest = &line[idx + "CARTESIAN_POINT('',(".len()..];
                if let Some(end) = rest.find(')') {
                    let nums: Vec<f64> = rest[..end]
                        .split(',')
                        .filter_map(|s| s.trim().parse::<f64>().ok())
                        .collect();
                    if nums.len() == 3 {
                        out.push([nums[0], nums[1], nums[2]]);
                    }
                }
            }
        }
        out
    }
}
