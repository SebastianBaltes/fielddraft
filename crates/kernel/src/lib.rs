//! FieldDraft geometry kernel.
//!
//! At M2 the kernel owns the canonical `Document`, evaluates the
//! composite SDF over its feature spine (with boolean ops), and exposes
//! a packed buffer for the renderer plus a mutation/undo API for the UI.

use fielddraft_protocol::{BoolOp, Document, Feature, FeatureId, Primitive, Transform};
use wasm_bindgen::prelude::*;

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
        }
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
        Self { document, packed, undo: Vec::new(), redo: Vec::new() }
    }

    // ── document I/O ──────────────────────────────────────────────────

    #[wasm_bindgen(js_name = loadJson)]
    pub fn load_json(&mut self, json: &str) -> Result<usize, JsError> {
        let document: Document =
            serde_json::from_str(json).map_err(|e| JsError::new(&e.to_string()))?;
        self.document = document;
        self.rebuild_packed();
        // Loading is a fresh start: history is irrelevant.
        self.undo.clear();
        self.redo.clear();
        Ok(self.document.features.len())
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

    /// Return a JSON array of feature summaries (id, name, op, kind).
    #[wasm_bindgen(js_name = listFeatures)]
    pub fn list_features(&self) -> Result<String, JsError> {
        let summaries: Vec<FeatureSummary> = self
            .document
            .features
            .iter()
            .map(|f| FeatureSummary {
                id: f.id,
                name: f.name.clone(),
                op: Self::op_label(f.op),
                kind: Self::kind_label(&f.primitive),
                translate: f.transform.translate,
                scale: f.transform.scale,
            })
            .collect();
        serde_json::to_string(&summaries).map_err(|e| JsError::new(&e.to_string()))
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
    ) -> Result<FeatureId, JsError> {
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
        self.document.features.push(Feature {
            id,
            name,
            op: bool_op,
            primitive,
            transform,
        });
        self.rebuild_packed();
        Ok(id)
    }

    /// Replace the translate component of a feature's transform.
    #[wasm_bindgen(js_name = setFeatureTranslate)]
    pub fn set_feature_translate(
        &mut self,
        id: FeatureId,
        tx: f32,
        ty: f32,
        tz: f32,
    ) -> bool {
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
    pub fn nudge_feature(&mut self, id: FeatureId, dx: f32, dy: f32, dz: f32) -> bool {
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
        id: FeatureId,
        tx: f32,
        ty: f32,
        tz: f32,
        qx: f32,
        qy: f32,
        qz: f32,
        qw: f32,
        scale: f32,
    ) -> bool {
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
    pub fn remove_feature(&mut self, id: FeatureId) -> bool {
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
    pub fn set_feature_op(&mut self, id: FeatureId, op: u32) -> Result<bool, JsError> {
        let bool_op = Self::parse_op(op)?;
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
    pub fn reorder_feature(&mut self, id: FeatureId, target_index: usize) -> bool {
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

    #[wasm_bindgen(js_name = canRedo)]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
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

    fn box_feature(id: FeatureId, op: BoolOp, center: [f32; 3], half: [f32; 3]) -> Feature {
        Feature {
            id,
            name: format!("Box {id}"),
            op,
            primitive: Primitive::Box { center, half_extents: half },
            transform: Transform::IDENTITY,
        }
    }

    fn sphere_feature(id: FeatureId, op: BoolOp, center: [f32; 3], r: f32) -> Feature {
        Feature {
            id,
            name: format!("Sphere {id}"),
            op,
            primitive: Primitive::Sphere { center, radius: r },
            transform: Transform::IDENTITY,
        }
    }

    fn transformed_sphere(id: FeatureId, translate: [f32; 3], r: f32) -> Feature {
        Feature {
            id,
            name: format!("Sphere {id}"),
            op: BoolOp::Union,
            primitive: Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: r },
            transform: Transform {
                translate,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: 1.0,
            },
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
            id: 1,
            name: "S".into(),
            op: BoolOp::Union,
            primitive: Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 1.0 },
            transform: Transform {
                translate: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: 4.0,
            },
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
            id: 1,
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
}
