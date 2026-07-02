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
pub mod path_provider;
pub mod sweep_path;
pub mod sweep_gate;
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

/// Profile-frame anchor of a [`Primitive::Helix`]: `(y_anchor, r_inner)`
/// — the sketch's axial centre (per-turn instance reference) and its
/// inner radius (clamps the near-axis Lipschitz blow-up).
pub fn helix_profile_anchor(sketch: &fielddraft_protocol::Sketch2d) -> (f32, f32) {
    let bb = sketch.aabb();
    if bb.is_empty() {
        (0.0, 0.0)
    } else {
        ((bb.min[1] + bb.max[1]) * 0.5, bb.min[0].max(0.0))
    }
}

/// Map a helix-LOCAL point into the swept profile's 2D `(r, w)` frame
/// and return `(q, ell, scale)`: the profile point, the signed
/// end-overshoot coordinate, and the conservative Lipschitz scale of
/// the unwrap. The helix winds about the local Y axis.
///
/// The finite sweep runs over the CONTINUOUS parameter `t ∈ [0,
/// 2π·turns]` (fractional turns end mid-winding). The nearest pass for
/// the query's angular position is found by rounding, then its
/// parameter is clamped to the sweep span — clamping `t` (not an
/// integer winding index) keeps the field continuous across the
/// `atan2` branch seam at the helix ends (ticket vtyh: the per-winding
/// clamp jumped `w` by a full pitch there, flipping signs → phantom
/// ribbon + checkerboard marching artifacts).
///
/// `ell` is the angular distance to the sweep span mapped to a chord
/// length at the clamped radius (≤ 0 inside the span, > 0 beyond an
/// end, halved as a safety margin — the zero crossing, i.e. the end-cap
/// surface, is unaffected). Callers combine it with the profile
/// distance via [`helix_compose`].
///
/// `taper` shears the radial coordinate along +y. The scale bounds the
/// composed map: `|∇w|² ≤ 1 + (pitch/(2πr))²` (tangential unwrap),
/// `|∇r'|² ≤ 1 + taper²`, `|∇ell|² ≤ 1.25` — the constant 2.25 covers
/// the `ell` component within the square root so the composed field
/// stays ≤ true distance (sphere-march safe), the same
/// self-normalisation idea as `Primitive::Thread` / Gyroid. `r` is
/// clamped below by `r_inner` (and a pitch-derived floor) so the
/// near-axis bound does not collapse the field to zero inside the bore.
/// Mirrored in the web GPU interpreter (`OP_WARP_HELIX`/`OP_HELIX_SCALE`).
/// Returns `(profile_point, ell, scale, cap_side)`. `cap_side` names which open
/// end the query overshoots — `-1` past the START (t < 0), `+1` past the END
/// (t > t_end), `0` within the swept span — so the provenance eval can tag the
/// two end caps with distinct sub-branches (pickable faces, ticket exus).
pub fn helix_profile_point(
    p: [f32; 3],
    pitch: f32,
    turns: f32,
    left_handed: bool,
    taper: f32,
    y_anchor: f32,
    r_inner: f32,
    start_angle: f32,
) -> ([f32; 2], f32, f32, f32) {
    use std::f32::consts::{PI, TAU};
    let pitch = pitch.max(1e-3);
    let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
    let theta = p[2].atan2(p[0]);
    let hand = if left_handed { -1.0 } else { 1.0 };
    // Handed sweep angle of the query relative to the start angle, and
    // the sweep parameter of the nearest helix pass at that position.
    let th = hand * theta - start_angle;
    let n = ((p[1] - y_anchor) / pitch - th / TAU).round();
    let t_n = th + TAU * n;
    let t_end = TAU * turns.max(0.0);
    let t_c = t_n.clamp(0.0, t_end);
    // Profile frame at the (possibly end-clamped) pass.
    let w = p[1] - pitch * t_c / TAU;
    let rp = r - taper * (p[1] - y_anchor);
    // Per-feature constant clamp radius: dropping the per-point `r`
    // only ever increases `q`, i.e. damps more — still conservative,
    // and it makes the Lipschitz scale a constant the opcode stream
    // can bake in at emission time (exact CPU/GPU parity).
    let rr = r_inner.max(pitch / TAU * 0.25).max(1e-3);
    // Signed angular distance to the sweep span, as a chord at the
    // query's radius (capped at the clamp radius, halved for safety).
    let l_t = (t_n - t_end * 0.5).abs() - t_end * 0.5;
    let ell = l_t.signum() * r.min(rr) * (l_t.abs().min(PI) * 0.5).sin();
    let q = pitch / (TAU * rr);
    let scale = 1.0 / (2.25 + q * q + taper * taper).sqrt();
    // Which half of the sweep the pass falls in: −1 = start side, +1 = end
    // side. Only consulted by the provenance eval when the cap term `ell` ties
    // (so an interior wall point ignores it); naming the half — not a strict
    // overshoot — keeps the rim itself (exactly at the cap) tagged.
    let cap_side = if t_n < t_end * 0.5 { -1.0 } else { 1.0 };
    ([rp, w], ell, scale, cap_side)
}

/// Combine a helix profile distance with the signed end-overshoot
/// coordinate from [`helix_profile_point`] — IQ's extrude composition
/// (`(d2, ell)` as a 2D box SDF), so the finite sweep ends with clean
/// cap cross-sections instead of a discontinuity.
pub fn helix_compose(d2: f32, ell: f32) -> f32 {
    let outside = (d2.max(0.0)).hypot(ell.max(0.0));
    let inside = d2.max(ell).min(0.0);
    outside + inside
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
        // Helical sweep: unwrap into the profile's (r, w) frame and
        // evaluate the 2D sketch SDF, Lipschitz-normalised. Mirrors
        // `CsgNode::Helix`'s tree eval for the non-tree (picking) path.
        Primitive::Helix {
            sketch, center, pitch, turns, left_handed, taper, start_angle,
        } => {
            let (y_anchor, r_inner) = helix_profile_anchor(sketch);
            let local = [p[0] - center[0], p[1] - center[1], p[2] - center[2]];
            let (q, ell, scale, _) = helix_profile_point(
                local,
                *pitch,
                *turns,
                *left_handed,
                *taper,
                y_anchor,
                r_inner,
                *start_angle,
            );
            helix_compose(sketch.sdf(q), ell) * scale
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
    /// Regen error message of a failing feature (ticket sx5a) — the
    /// outliner paints the row red and exposes this as the tooltip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Non-fatal merge-scope warning — the outliner paints the row in the
    /// warning colour and exposes this as the tooltip; the body renders as a
    /// red translucent preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
}

/// Collect every `qCreatedBy(<feature_id>)` reference embedded in a feature
/// call's params, walking nested arrays/objects and comma-separated
/// MultiQuery strings. Datum references (`qCreatedBy(origin_plane_xy)`) and
/// other non-numeric targets don't parse to a [`FeatureId`] and are skipped.
/// Used by [`Kernel::reorder_feature`] to keep the tree a valid program
/// (every feature references only earlier features).
fn created_by_references(params: &serde_json::Value) -> Vec<FeatureId> {
    fn walk(v: &serde_json::Value, out: &mut Vec<FeatureId>) {
        match v {
            serde_json::Value::String(s) => {
                for part in s.split(',') {
                    if let Some(inner) = part
                        .trim()
                        .strip_prefix("qCreatedBy(")
                        .and_then(|x| x.strip_suffix(')'))
                    {
                        if let Ok(fid) = inner.parse::<FeatureId>() {
                            out.push(fid);
                        }
                    }
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            serde_json::Value::Object(o) => o.values().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(params, &mut out);
    out
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
    /// Milestone-1 parametric weaving: when on, pure grundforms get an exact
    /// analytic B-Rep (CSG branch-pair seeds) instead of an octree-sampled one.
    /// Default `true` — the seed weave resolves sub-cell features (e.g. grazing
    /// near-tangent intersections) the octree cannot, and any body it cannot
    /// resolve analytically falls back to the octree weave per-body, so the
    /// result is never worse than the octree route. Toggle via `setParametricWeave`.
    parametric_weave: bool,
    /// While `true` (web: gizmo drag with "B-Rep rebuild while dragging"
    /// off, and permanently in the mesher worker), the kernel never weaves:
    /// the regen face-tagging pass is skipped and the per-body B-Rep exports
    /// serve only a still-current cache — a body whose geometry changed
    /// reports empty payloads. `setBrepSuppressed(false)` catches up with
    /// ONE weave per stale body and bumps their versions so the frontend
    /// refetches. Runtime-only, never serialised.
    brep_suppressed: bool,
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
    /// Per-feature regen errors from the last `bootstrap_features_from_calls`
    /// (ticket sx5a, Onshape semantics): a failing feature stays in the
    /// document, contributes no geometry, and carries its message here —
    /// `listFeatures` exports it so the UI can paint the row red.
    feature_errors: std::collections::HashMap<FeatureId, String>,
    /// Per-feature NON-fatal merge-scope warnings from the last regen: a
    /// feature-implicit boolean (Add/Remove/Intersect) with no resolved merge
    /// scope. Geometry still regenerates (the body shows as a red translucent
    /// preview); `listFeatures` exports the message so the UI paints the row
    /// in the warning colour with this tooltip.
    feature_warnings: std::collections::HashMap<FeatureId, String>,
    /// Rollback bar position: if set, features at index >= this value
    /// are skipped during regeneration (but stay in the document).
    rollback_after: Option<usize>,
    /// Regen-loop weave cache carried ACROSS regenerations. Installed into
    /// each regenerate `Context` and taken back afterwards, so a feature
    /// that resolves element queries mid-loop (shell's `qFace` open faces)
    /// only pays the octree weave when the upstream features actually
    /// changed — not on every regen/load. Keyed by content hash, so a stale
    /// hit is impossible; without it, loading a weave-heavy document
    /// (spline-revolve + shell) re-wove from scratch every time.
    regen_brep_cache: body::BrepCache,
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
                // Keep the woven B-Rep across geometry-preserving remeshes
                // (LOD switch, transform change): `ensure_brep` revalidates
                // against its own placement-invariant content key anyway,
                // and re-weaving a weave-heavy body on every active-body
                // switch froze the UI for the whole octree pass.
                if prev.geometry_hash != geometry_hash {
                    prev.brep = None;
                }
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

        // Brocken A (A3): face-identity tagging. Skipped while B-Rep output
        // is suppressed (gizmo drag) — `setBrepSuppressed(false)` runs the
        // catch-up pass once at drag end.
        if !self.brep_suppressed {
            self.retag_mesh_faces();
        }

        self.mesh_version = self.mesh_version.wrapping_add(1);
    }

    /// Brocken A (A3): tag every freshly-built mesh with per-vertex B-Rep
    /// FACE IDENTITY so a hovered/clicked triangle resolves to a stable
    /// face. Bodies carried through the affine fast-path keep their old
    /// (already-tagged) mesh untouched — their `face_index` is still valid
    /// because correlation is body-LOCAL and a pure move doesn't change
    /// which face a vertex sits on. We therefore only (re)tag meshes whose
    /// `face_index` is missing/out-of-sync, weaving the B-Rep once and
    /// sharing the cache with the later face/vertex exports.
    fn retag_mesh_faces(&mut self) {
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
            BoolOp::Add => "add",
            BoolOp::New => "new",
            BoolOp::Remove => "remove",
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
            "helix" => "helix",
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
            "shell" => "shell",
            "thicken" => "thicken",
            "torus" => "torus",
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
            "helix" => "Helix",
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
            "shell" => "Shell",
            "thicken" => "Thicken",
            "translate" => "Translate",
            "rotate" => "Rotate",
            "scale" => "Scale",
            other => other,
        };
        format!("{label} {}", call.id)
    }

    fn parse_op(tag: u32) -> Result<BoolOp, JsError> {
        match tag {
            BoolOp::TAG_ADD => Ok(BoolOp::Add),
            BoolOp::TAG_NEW => Ok(BoolOp::New),
            BoolOp::TAG_REMOVE => Ok(BoolOp::Remove),
            BoolOp::TAG_INTERSECT => Ok(BoolOp::Intersect),
            other => Err(JsError::new(&format!("invalid op tag: {other}"))),
        }
    }

    fn op_wire_name(tag: u32) -> Result<&'static str, JsError> {
        match Self::parse_op(tag)? {
            BoolOp::Add => Ok("add"),
            BoolOp::New => Ok("new"),
            BoolOp::Remove => Ok("remove"),
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
        self.push_feature_call_with(type_name, params, false)
    }

    /// `suppressed: true` creates the call as a PENDING ghost in one undo
    /// step (ticket x7af): the regen loop skips it, so the body keeps its
    /// pre-feature shape until the UI confirms via `setFeatureSuppressed`.
    fn push_feature_call_with(
        &mut self,
        type_name: &str,
        params: serde_json::Value,
        suppressed: bool,
    ) -> Result<u32, JsError> {
        self.snapshot();
        let id = self.document.allocate_id();
        let flat = id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");
        let mut call = fielddraft_protocol::FeatureCall::new(type_name, id, params);
        call.suppressed = suppressed;
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
            parametric_weave: true,
            brep_suppressed: false,
            mesh_version: 0,
            edge_fillets: Vec::new(),
            mate_connectors: Vec::new(),
            feature_errors: std::collections::HashMap::new(),
            feature_warnings: std::collections::HashMap::new(),
            rollback_after: None,
            regen_brep_cache: body::BrepCache::new(),
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
    /// Always returns `Ok(())` since the regen loop became error-tolerant
    /// (ticket sx5a): a failing feature is recorded in `feature_errors`
    /// and skipped — it contributes no geometry — instead of fail-stopping
    /// the document. The `Result` signature stays so the many defensive
    /// call sites keep compiling; their revert branches are dead defense.
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
        let (errors, spine) = {
            let mut ctx = features::Context::new(&mut self.document);
            // Install the cross-regen weave cache so mid-loop element
            // queries (shell qFace) reuse prior weaves instead of paying
            // the octree on every regenerate; taken back below.
            ctx.brep_cache = std::mem::take(&mut self.regen_brep_cache);
            let errs = registry.regenerate(&mut ctx, &calls, rollback);
            self.regen_brep_cache = std::mem::take(&mut ctx.brep_cache);
            // K10.4: capture edge fillets collected during regen.
            self.edge_fillets = std::mem::take(&mut ctx.edge_fillets);
            // C2: capture resolved mate-connector CSYS for the viewport.
            self.mate_connectors = std::mem::take(&mut ctx.mate_connectors);
            // Merge-scope warnings collected during regen (non-fatal).
            self.feature_warnings = std::mem::take(&mut ctx.feature_warnings)
                .into_iter()
                .map(|(id, w)| (id.root_ancestor().clone(), w))
                .collect();
            (errs, std::mem::take(&mut ctx.features))
        };
        self.document.feature_calls = calls;
        self.decoded = spine;
        self.feature_errors = errors
            .0
            .into_iter()
            .map(|(id, e)| (id, e.to_string()))
            .collect();
        Ok(())
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
                    error: self.feature_errors.get(&call.id).cloned(),
                    warning: self.feature_warnings.get(&call.id).cloned(),
                });
            } else {
                // K8.9: hole pushes its spine entry under a
                // hierarchical child id (`<hole>/0`), so the
                // lookup above misses it. Surface the op that
                // matches the kind's known hard-coded semantics
                // here instead of defaulting to "add".
                let op = match call.type_name.as_str() {
                    "hole" => "remove",
                    "gyroid" => "intersect",
                    _ => "add",
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
                    error: self.feature_errors.get(&call.id).cloned(),
                    warning: self.feature_warnings.get(&call.id).cloned(),
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
    /// the world Z axis. `suppressed` creates the call as a
    /// PENDING ghost (ticket x7af, same flow as
    /// `addExtrudeFaces`): the body keeps its pre-extrude shape
    /// until the detail dialog confirms.
    #[wasm_bindgen(js_name = addExtrude)]
    pub fn add_extrude(
        &mut self,
        sketch_id: u32,
        depth_mm: f32,
        op: u32,
        tz: f32,
        suppressed: bool,
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
        self.push_feature_call_with(
            "extrude",
            serde_json::json!({
                "profiles": format!("qCreatedBy({sketch_feature_id})"),
                "depth": { "value": depth_mm, "unit": "mm" },
                "profile_half_x": 0.0,
                "profile_half_y": 0.0,
                "operation": op_wire,
                "center": [0.0, 0.0, tz],
            }),
            suppressed,
        )
    }

    /// K8.4: append a `revolve` feature call that consumes the
    /// sketch with id `sketch_id` and sweeps its profile around
    /// the local Y axis through `(cx, cy, cz)`. `op` uses the
    /// same `BoolOp::TAG_*` tags as `add_primitive`. Returns
    /// the new feature id.
    #[wasm_bindgen(js_name = addRevolve)]
    pub fn add_revolve(&mut self, sketch_id: u32, op: u32) -> Result<u32, JsError> {
        let op_wire = Self::op_wire_name(op)?;
        // `sketch_id == 0` creates an EMPTY revolve (ticket 1rax): the
        // dialog's query picker fills `profiles` + `axis` afterwards. Until
        // BOTH are resolved the revolve is invalid — it produces NO body and
        // reports a warning (label warn-colour + tooltip) instead of a garbage
        // solid (ticket ni56). The `axis` field is REQUIRED; there is no
        // implicit local-Y fallback.
        if sketch_id == 0 {
            return self.push_feature_call("revolve", serde_json::json!({
                "profiles": "",
                "operation": op_wire,
            }));
        }
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
        self.push_feature_call("revolve", serde_json::json!({
            "profiles": format!("qCreatedBy({sketch_feature_id})"),
            "operation": op_wire,
        }))
    }

    /// Append a `helix` feature call sweeping the sketch's profile
    /// along a helical path around the sketch-local Y axis (real
    /// screw threads — see `HelixDef`). `pitch_mm` is the axial
    /// advance per revolution, `turns` the revolution count. As with
    /// `addRevolve`, `sketch_id == 0` creates an EMPTY helix whose
    /// `profiles` the dialog's query picker fills afterwards.
    #[wasm_bindgen(js_name = addHelix)]
    pub fn add_helix(
        &mut self,
        sketch_id: u32,
        op: u32,
        pitch_mm: f32,
        turns: f32,
    ) -> Result<u32, JsError> {
        let op_wire = Self::op_wire_name(op)?;
        if sketch_id == 0 {
            return self.push_feature_call("helix", serde_json::json!({
                "profiles": "",
                "operation": op_wire,
                "pitch": { "value": pitch_mm, "unit": "mm" },
                "turns": turns,
            }));
        }
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
        self.push_feature_call("helix", serde_json::json!({
            "profiles": format!("qCreatedBy({sketch_feature_id})"),
            "operation": op_wire,
            "pitch": { "value": pitch_mm, "unit": "mm" },
            "turns": turns,
        }))
    }

    /// Append a `sweep` feature call: a profile sketch swept along a path
    /// sketch (Sketch2d path source). Both ids must be existing sketches.
    #[wasm_bindgen(js_name = addSweep)]
    pub fn add_sweep(
        &mut self,
        profile_sketch_id: u32,
        path_sketch_id: u32,
        op: u32,
    ) -> Result<u32, JsError> {
        let op_wire = Self::op_wire_name(op)?;
        for (sid, label) in [(profile_sketch_id, "profile"), (path_sketch_id, "path")] {
            if sid == 0 {
                return Err(JsError::new(&format!("sweep needs a {label} sketch")));
            }
            let fid = FeatureId::from_u32(sid);
            let exists = self
                .document
                .feature_calls
                .iter()
                .any(|c| c.id == fid && c.type_name == "sketch");
            if !exists {
                return Err(JsError::new(&format!("feature {sid} is not a sketch")));
            }
        }
        let profile_fid = FeatureId::from_u32(profile_sketch_id);
        let path_fid = FeatureId::from_u32(path_sketch_id);
        self.push_feature_call(
            "sweep",
            serde_json::json!({
                "profiles": format!("qCreatedBy({profile_fid})"),
                "path": { "kind": "sketch2d", "sketch": format!("qCreatedBy({path_fid})") },
                "operation": op_wire,
            }),
        )
    }

    /// Append a `sweep` along a default 3D control-point path (Gizmo3d source).
    /// The user then routes the spine by editing the control points. The
    /// default is a gentle S so the sweep is visible immediately.
    #[wasm_bindgen(js_name = addSweepGizmo)]
    pub fn add_sweep_gizmo(
        &mut self,
        profile_sketch_id: u32,
        op: u32,
    ) -> Result<u32, JsError> {
        let op_wire = Self::op_wire_name(op)?;
        if profile_sketch_id == 0 {
            return Err(JsError::new("sweep needs a profile sketch"));
        }
        let fid = FeatureId::from_u32(profile_sketch_id);
        let exists = self
            .document
            .feature_calls
            .iter()
            .any(|c| c.id == fid && c.type_name == "sketch");
        if !exists {
            return Err(JsError::new(&format!("feature {profile_sketch_id} is not a sketch")));
        }
        self.push_feature_call(
            "sweep",
            serde_json::json!({
                "profiles": format!("qCreatedBy({fid})"),
                "path": {
                    "kind": "gizmo3d",
                    "nodes": [[0.0, 0.0, 0.0], [40.0, 25.0, 0.0], [80.0, 25.0, 20.0], [120.0, 0.0, 20.0]],
                    "closed": false
                },
                "operation": op_wire,
            }),
        )
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
    /// Add a `mate_connector` anchored to a B-Rep element. `anchor` is a
    /// `qFace`/`qEdge`/`qVertex` query; the anchor defines +Z (edge direction /
    /// face normal). `t` positions an edge anchor (0.5 = midpoint, ignored
    /// otherwise). `secondary` is a reference face query that orients the
    /// secondary axes (edge) or defines +Z (vertex); empty → auto-picked.
    /// `reorient` rotates the secondary axes in 90° steps about +Z (0..3).
    /// `flip` reverses +Z.
    #[wasm_bindgen(js_name = addMateConnector)]
    pub fn add_mate_connector(
        &mut self,
        anchor: &str,
        t: f32,
        secondary: &str,
        reorient: u32,
        flip: bool,
    ) -> Result<u32, JsError> {
        self.push_feature_call("mate_connector", serde_json::json!({
            "anchor": anchor,
            "t": t,
            "secondary": secondary,
            "reorient": reorient,
            "flip": flip,
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
    /// `suppressed` creates the call as a pending ghost (ticket x7af): the
    /// body keeps its pre-extrude shape until the UI confirms the feature.
    #[wasm_bindgen(js_name = addExtrudeFaces)]
    pub fn add_extrude_faces(
        &mut self,
        faces: String,
        depth_mm: f32,
        symmetric: bool,
        depth_back_mm: f32,
        suppressed: bool,
    ) -> Result<u32, JsError> {
        // Ticket k280: faces ride on the unified `profiles` field now.
        self.push_feature_call_with("extrude", serde_json::json!({
            "profiles": faces,
            "depth": { "value": depth_mm, "unit": "mm" },
            "symmetric": symmetric,
            "depth_back": { "value": depth_back_mm, "unit": "mm" },
        }), suppressed)
    }

    /// Thicken one or more **curved** analytic faces into a thin solid wall by
    /// offsetting their surfaces along the normal (sphere / cylinder / torus).
    /// `faces` is a comma-separated list of `qFace(<id_hash>)` selections;
    /// `thickness_mm` grows the wall outward, `thickness_back_mm` inward (only
    /// when `symmetric` is false), `symmetric` splits the wall about the surface.
    /// `up_to_face` is an optional `qFace(<id_hash>)` target: when non-empty the
    /// wall grows up to that coaxial face and the thickness params are ignored.
    #[wasm_bindgen(js_name = addThicken)]
    pub fn add_thicken(
        &mut self,
        faces: String,
        thickness_mm: f32,
        symmetric: bool,
        thickness_back_mm: f32,
        up_to_face: String,
    ) -> Result<u32, JsError> {
        self.push_feature_call("thicken", serde_json::json!({
            "faces": faces,
            "thickness": { "value": thickness_mm, "unit": "mm" },
            "symmetric": symmetric,
            "thickness_back": { "value": thickness_back_mm, "unit": "mm" },
            "up_to_face": up_to_face,
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

    /// Ticket dhjn: the `qRegion(sketch_id, i)` query string for the closed
    /// region of `sketch_id` containing the sketch-space point `(u, v)`, or an
    /// empty string when the point is outside every region. The viewport calls
    /// this when a Revolve/Extrude profile picker is armed and the user clicks
    /// a single filled region — so the field stores just that region instead of
    /// the whole sketch. Region index order matches `listSketchRegions`.
    #[wasm_bindgen(js_name = regionQueryAtPoint)]
    pub fn region_query_at_point(
        &self,
        sketch_id: u32,
        u: f32,
        v: f32,
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
        let res = crate::sketch_features::PROFILE_REGION_RESOLUTION;
        match sketch_regions::region_index_at_point(&sketch2d, [u, v], res) {
            Some(i) => Ok(format!("qRegion({sketch_id}, {i})")),
            None => Ok(String::new()),
        }
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
    ///
    /// `coalesce` skips the undo snapshot: a throttled drag
    /// (live extrude) replaces the same call dozens of times
    /// per second — the caller snapshots once via the FIRST,
    /// non-coalesced replace and the whole drag stays a single
    /// undo step (mirrors `apply_gizmo_feature`'s merge). On
    /// regeneration failure a coalesced replace restores the
    /// previous params in place instead of popping undo.
    #[wasm_bindgen(js_name = replaceFeatureCallParams)]
    pub fn replace_feature_call_params(
        &mut self,
        id: u32,
        params_json: &str,
        coalesce: bool,
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

        if !coalesce {
            self.snapshot();
        }
        // K8.7: preserve the call's suppressed flag across a
        // params replace — otherwise toggling suppression off by
        // renaming or editing any other field would resurrect it.
        let old = &self.document.feature_calls[idx];
        let type_name = old.type_name.clone();
        let suppressed = old.suppressed;
        let old_params = old.params.clone();
        self.document.feature_calls[idx] = fielddraft_protocol::FeatureCall {
            type_name,
            id: target,
            params: new_params,
            suppressed,
        };
        if let Err(err) = self.bootstrap_features_from_calls() {
            if coalesce {
                self.document.feature_calls[idx].params = old_params;
                let _ = self.bootstrap_features_from_calls();
                self.rebuild_packed();
            } else {
                self.undo();
            }
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
    pub fn set_variable(
        &mut self,
        name: &str,
        value: f32,
        kind: &str,
        formula: Option<String>,
    ) -> Result<(), JsError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(JsError::new("variable name must not be empty"));
        }
        if !value.is_finite() {
            return Err(JsError::new(&format!(
                "variable `{trimmed}` must be a finite number"
            )));
        }
        // An unknown / empty kind keeps the existing kind (when upserting) or
        // defaults to Number — so callers that don't care can pass "".
        let kind = match kind {
            "length" => fielddraft_protocol::VarKind::Length,
            "angle" => fielddraft_protocol::VarKind::Angle,
            "number" => fielddraft_protocol::VarKind::Number,
            _ => self
                .document
                .variables
                .get(trimmed)
                .map(|v| v.kind)
                .unwrap_or_default(),
        };
        // An empty formula string means "plain literal" (no source text to
        // remember) — collapse it to None so the wire form stays clean.
        let formula = formula.filter(|s| !s.trim().is_empty());
        self.snapshot();
        self.document.variables.insert(
            trimmed.to_string(),
            fielddraft_protocol::Variable::with_formula(value, kind, formula),
        );
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

    /// Ticket 8ar9: rename a document-level variable, keeping its
    /// value. Errors on an empty new name or a collision with an
    /// existing variable; a no-op rename (`old == new`) succeeds.
    /// Re-bootstraps so any formula that referenced the old name
    /// surfaces as a regen error (which rolls the rename back) —
    /// future formula support will rewrite references here instead.
    #[wasm_bindgen(js_name = renameVariable)]
    pub fn rename_variable(&mut self, old: &str, new: &str) -> Result<(), JsError> {
        let new_name = new.trim();
        if new_name.is_empty() {
            return Err(JsError::new("variable name must not be empty"));
        }
        if old == new_name {
            return Ok(());
        }
        let Some(var) = self.document.variables.get(old).cloned() else {
            return Err(JsError::new(&format!("no variable named `{old}`")));
        };
        if self.document.variables.contains_key(new_name) {
            return Err(JsError::new(&format!(
                "a variable named `{new_name}` already exists"
            )));
        }
        self.snapshot();
        self.document.variables.remove(old);
        self.document
            .variables
            .insert(new_name.to_string(), var);
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "variable rename failed to regenerate: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(())
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
            kind: fielddraft_protocol::VarKind,
            #[serde(skip_serializing_if = "Option::is_none")]
            formula: Option<&'a str>,
        }
        let out: Vec<Entry> = self
            .document
            .variables
            .iter()
            .map(|(name, var)| Entry {
                name,
                value: var.value,
                kind: var.kind,
                formula: var.formula.as_deref(),
            })
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
        // Snapshot semantics: the feature tree is a program — every feature
        // may only reference features BEFORE it. Reject a reorder that would
        // push a feature ahead of a feature it references (or pull a
        // referenced feature behind its consumer); the UI keeps the original
        // order and surfaces the rejection.
        let mut proposed: Vec<_> = self.document.feature_calls.iter().collect();
        let moved = proposed.remove(from);
        proposed.insert(to, moved);
        let pos: std::collections::HashMap<FeatureId, usize> =
            proposed.iter().enumerate().map(|(i, c)| (c.id.clone(), i)).collect();
        for (i, c) in proposed.iter().enumerate() {
            for r in created_by_references(&c.params) {
                if let Some(&ri) = pos.get(&r) {
                    if ri >= i {
                        return false; // forward reference — illegal order
                    }
                }
            }
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

    /// Evaluate the body's compiled opcode stream on the CPU mirror
    /// interpreter at a world point. This is the parity REFERENCE for the
    /// GPU probe (`window.__fdSdfVmProbe` runs the live TSL interpreter on
    /// the same stream): both sides interpret identical bytes, so any
    /// disagreement is a GPU-side interpreter bug.
    #[wasm_bindgen(js_name = bodySdfEval)]
    pub fn body_sdf_eval(&self, i: usize, x: f32, y: f32, z: f32) -> f32 {
        self.bodies[i].compiled_sdf.as_ref().map_or(f32::MAX, |s| {
            sdf_compiler::eval_opcodes_with_volumes(&s.opcodes, [x, y, z], &s.volumes)
        })
    }

    /// Conservative raymarch step factor (≤1) for this body — the renderer
    /// multiplies its march step by this so a thin-walled revolve/shell isn't
    /// overshot at grazing angles (Ticket 1wnm). 1.0 = full step.
    #[wasm_bindgen(js_name = bodySdfMarchRelax)]
    pub fn body_sdf_march_relax(&self, i: usize) -> f32 {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .map_or(1.0, |s| s.march_relax)
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
        // The JS side queries by index from a meshed-body snapshot that can
        // lag the live kernel by a frame (e.g. right after "New" empties the
        // bodies). An out-of-range index must yield an empty info, never panic.
        let Some(body) = self.bodies.get(i) else {
            return serde_json::to_string(&bvh::HybridInfo::default())
                .unwrap_or_else(|_| "{}".into());
        };
        let bounded =
            body::build_bounded_for_body(&self.decoded, &body.feature_indices);
        let info = bounded.classify_for_hybrid();
        serde_json::to_string(&info).unwrap_or_else(|_| "{}".into())
    }

    // ── Per-body *feature-based* B-Rep (sharp topology) ──────────

    /// Gate for the per-body B-Rep exports while weaving is suppressed
    /// (`setBrepSuppressed`): `true` when serving body `i` costs no weave —
    /// either suppression is off, or the body's cached B-Rep is still
    /// current (e.g. after a pure move, whose placement-invariant key is
    /// unchanged). `false` means "emit an empty payload"; as a side effect
    /// the body's `placement` has been refreshed.
    fn brep_export_allowed(&mut self, i: usize) -> bool {
        !self.brep_suppressed || self.bodies[i].brep_is_current(&self.decoded)
    }

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
        if !self.brep_export_allowed(i) {
            return serde_json::json!({
                "vertices": 0, "edges": 0, "faces": 0,
                "shells": 0, "solids": 0, "watertight": false,
            })
            .to_string();
        }
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

    /// Stable ids (see [`Kernel::body_id`]) of bodies whose woven B-Rep is
    /// NOT current — i.e. an export would either weave synchronously (no
    /// suppression) or serve an empty payload (suppressed). The async
    /// weaver pipeline polls this after every commit and farms the listed
    /// bodies out to the weaver worker. Refreshes placements as a side
    /// effect; never weaves.
    #[wasm_bindgen(js_name = bodyBrepStaleIds)]
    pub fn body_brep_stale_ids(&mut self) -> String {
        let mut stale: Vec<u32> = Vec::new();
        for i in 0..self.bodies.len() {
            if !self.bodies[i].brep_is_current(&self.decoded) {
                stale.push(self.body_id(i));
            }
        }
        serde_json::to_string(&stale).unwrap_or_else(|_| "[]".into())
    }

    /// WEAVER WORKER: weave body `body_id`'s B-Rep against the current
    /// document state and return it serialized as MessagePack
    /// `(placement-invariant key, BRep)` (named mode — the B-Rep's
    /// internally-tagged enums need a self-describing format), ready for
    /// [`Kernel::import_body_brep`] on the main thread. Returns an empty
    /// buffer when the id is unknown (the document changed under the
    /// request) or serialization fails — the importer treats both as a
    /// miss and the next scheduled weave supersedes.
    #[wasm_bindgen(js_name = weaveBodyBrep)]
    pub fn weave_body_brep(&mut self, body_id: u32) -> Vec<u8> {
        let Some(i) = (0..self.bodies.len()).find(|&i| self.body_id(i) == body_id)
        else {
            return Vec::new();
        };
        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let Some((key, brep)) = self.bodies[i].brep.as_ref() else {
            return Vec::new();
        };
        let mut buf = Vec::new();
        let mut ser = rmp_serde::Serializer::new(&mut buf)
            .with_struct_map()
            .with_human_readable();
        use serde::Serialize as _;
        if (*key, brep).serialize(&mut ser).is_err() {
            return Vec::new();
        }
        buf
    }

    /// MAIN THREAD: install a worker-woven B-Rep (payload from
    /// [`Kernel::weave_body_brep`]) into the body's cache. The serialized
    /// placement-invariant key is validated against the CURRENT document
    /// state — a result that raced a newer edit simply misses, and the
    /// next scheduled weave supersedes it. On success the body version
    /// bumps and the mesh faces re-tag (a no-op in worker-meshing mode,
    /// where main-thread meshes are empty) so overlays and picking
    /// refetch. Returns whether anything was installed.
    #[wasm_bindgen(js_name = importBodyBrep)]
    pub fn import_body_brep(&mut self, body_id: u32, bytes: &[u8]) -> bool {
        use crate::brep::BRep;
        let Some(i) = (0..self.bodies.len()).find(|&i| self.body_id(i) == body_id)
        else {
            return false;
        };
        let mut de = rmp_serde::Deserializer::new(bytes).with_human_readable();
        use serde::Deserialize as _;
        let Ok((key, brep)) = <(u64, BRep)>::deserialize(&mut de) else {
            return false;
        };
        self.bodies[i].refresh_placement(&self.decoded);
        let current = crate::body::hash_body_local_payload(
            &self.decoded,
            &self.bodies[i].feature_indices,
            &self.bodies[i].placement,
        );
        if current != key {
            return false; // raced a newer edit — superseded
        }
        if matches!(&self.bodies[i].brep, Some((k, _)) if *k == key) {
            return false; // already current — avoid version churn
        }
        self.bodies[i].brep = Some((key, brep));
        self.bodies[i].version = self.bodies[i].version.wrapping_add(1);
        self.retag_mesh_faces();
        self.mesh_version = self.mesh_version.wrapping_add(1);
        true
    }

    /// Per-body analytic B-Rep edges, sampled to **world-space** polylines for
    /// viewport rendering and hover. Returns a JSON array; each edge is
    /// `{"a":<branch u32>,"b":<branch u32>,"idHash":<decimal string>,
    /// "points":[x,y,z, …],"controlPoints":[x,y,z, …],"degree":<usize>}`,
    /// where `points` is the dense tessellation (for the wireframe) and
    /// `controlPoints`/`degree` are the sparse structural points for the debug
    /// points overlay (on-curve nodes sit at every `degree`-th control point).
    /// `a`/`b` are the canonical
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

        if !self.brep_export_allowed(i) {
            return "[]".into();
        }
        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let placement = self.bodies[i].placement;
        let brep = self.bodies[i].brep_cached().expect("woven above");
        let s = placement.scale[0];
        let mut edges = Vec::with_capacity(brep.edges.len());
        // Fold a body-local point chain to world: world = R·(s·p) + t
        // (rigid + uniform-scale placement), flattened to `[x,y,z, …]`.
        let to_world = |pts: &[[f64; 3]]| -> Vec<f32> {
            let mut out = Vec::with_capacity(pts.len() * 3);
            for p in pts {
                let scaled = [p[0] as f32 * s, p[1] as f32 * s, p[2] as f32 * s];
                let r = quat_rotate(placement.rotation, scaled);
                out.push(r[0] + placement.translate[0]);
                out.push(r[1] + placement.translate[1]);
                out.push(r[2] + placement.translate[2]);
            }
            out
        };
        // Edge → adjacent face ARENA indices (same ordering as
        // `bodyBrepFaces`). Selection display needs the face-precise
        // association: coplanar split faces share one branch signature
        // (the coincidence canonicalisation), so matching edges by
        // `a`/`b` alone lights up the whole coplanar family (ticket b91c).
        let mut edge_faces: Vec<Vec<u32>> = vec![Vec::new(); brep.edges.len()];
        for (fk, face) in brep.faces.iter().enumerate() {
            for lp in std::iter::once(&face.outer).chain(face.inner.iter()) {
                for (eid, _) in &lp.edges {
                    let v = &mut edge_faces[eid.0 as usize];
                    if !v.contains(&(fk as u32)) {
                        v.push(fk as u32);
                    }
                }
            }
        }
        for (k, edge) in brep.edges.iter().enumerate() {
            let id_hash = TopoRef::from_edge(brep, EdgeId(k as u32)).id_hash;
            let points = to_world(&crate::brep::edge_sampling::sample_edge(&edge.geometry));
            // Structural points for the debug overlay: the few control points
            // that define the curve, plus its degree (on-curve nodes sit at
            // every `degree`-th control point). Far sparser than `points`.
            let (cps_local, degree) =
                crate::brep::edge_sampling::edge_control_structure(&edge.geometry);
            let control_points = to_world(&cps_local);
            edges.push(serde_json::json!({
                "a": edge.signature.0.as_u32(),
                "b": edge.signature.1.as_u32(),
                "idHash": id_hash.to_string(),
                "points": points,
                "controlPoints": control_points,
                "degree": degree,
                "faces": edge_faces[k],
            }));
        }
        serde_json::Value::Array(edges).to_string()
    }

    /// **Debug only.** Per-body topology-octree leaf cells for the viewport's
    /// octree-cell overlay. Returns a JSON object
    /// `{"translate":[x,y,z],"rotation":[x,y,z,w],"scale":s,
    /// "cells":[{"min":[x,y,z],"max":[x,y,z],"class":<0|1|2>}, …]}`, where the
    /// cells are in the body-LOCAL frame and `translate`/`rotation`/`scale`
    /// are the body's rigid+uniform-scale placement (apply them as a single
    /// world transform on the frontend — a group matrix — to fold the cells to
    /// world, exactly like the analytic faces are local + placement).
    /// `class` is `0`=Face, `1`=Edge, `2`=Vertex. The octree is rebuilt on
    /// every call (not cached); this is a diagnostic path, not the hot path.
    #[wasm_bindgen(js_name = bodyOctreeCells)]
    pub fn body_octree_cells(&mut self, i: usize) -> String {
        if !self.brep_export_allowed(i) {
            let p = self.bodies[i].placement; // refreshed by the gate
            return serde_json::json!({
                "translate": p.translate,
                "rotation": p.rotation,
                "scale": p.scale[0],
                "cells": [],
            })
            .to_string();
        }
        // Refresh placement off the (cached) weave, then rebuild the octree.
        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let placement = self.bodies[i].placement;
        let cells = self.bodies[i].debug_octree_cells(&self.decoded);
        let cells_json: Vec<serde_json::Value> = cells
            .iter()
            .map(|(min, max, class)| {
                serde_json::json!({ "min": min, "max": max, "class": class })
            })
            .collect();
        serde_json::json!({
            "translate": placement.translate,
            "rotation": placement.rotation,
            "scale": placement.scale[0],
            "cells": cells_json,
        })
        .to_string()
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
        use crate::brep::face_export::{face_centroid_normal_local, surface_kind, surface_params};
        use crate::brep::naming::TopoRef;
        use crate::brep::FaceId;

        if !self.brep_export_allowed(i) {
            return "[]".into();
        }
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
                "surfaceParams": surface_params(&face.surface),
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

        if !self.brep_export_allowed(i) {
            return "[]".into();
        }
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
    /// (`localToWorld`) to obtain world coordinates. Placement is derived
    /// straight from the features — it never depends on the weave, so this
    /// refreshes it directly without ever triggering a B-Rep build.
    #[wasm_bindgen(js_name = bodyPlacement)]
    pub fn body_placement(&mut self, i: usize) -> String {
        self.bodies[i].refresh_placement(&self.decoded);
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

    /// Suppress every B-Rep weave while a gizmo drag streams throttled
    /// parameter commits (web setting "B-Rep rebuild while dragging",
    /// default suppressed). While on, regen skips the eager face-tagging
    /// weave and the B-Rep exports (`bodyBrepEdges`/`Faces`/`Vertices`/
    /// `Counts`) serve only a still-current cache — a body whose geometry
    /// changed under the drag reports empty payloads until the drag ends.
    /// Turning the flag off bumps the version of every body whose B-Rep
    /// went stale (the frontend's refetch signal) and re-runs the
    /// face-tagging pass, weaving each stale body exactly once. Returns
    /// `false` if the flag was already at `on`.
    #[wasm_bindgen(js_name = setBrepSuppressed)]
    pub fn set_brep_suppressed(&mut self, on: bool) -> bool {
        if self.brep_suppressed == on {
            return false;
        }
        self.brep_suppressed = on;
        if !on {
            let mut any_stale = false;
            for i in 0..self.bodies.len() {
                if !self.bodies[i].brep_is_current(&self.decoded) {
                    self.bodies[i].version = self.bodies[i].version.wrapping_add(1);
                    any_stale = true;
                }
            }
            self.retag_mesh_faces();
            if any_stale {
                self.mesh_version = self.mesh_version.wrapping_add(1);
            }
        }
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
#[path = "lib_tests.rs"]
mod tests;
