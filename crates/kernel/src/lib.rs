//! FieldDraft geometry kernel.
//!
//! At M2 the kernel owns the canonical `Document`, evaluates the
//! composite SDF over its feature spine (with boolean ops), and exposes
//! a packed buffer for the renderer plus a mutation/undo API for the UI.

use fielddraft_protocol::{BoolOp, Document, Feature, FeatureId, Primitive};
use wasm_bindgen::prelude::*;

/// Packed representation of one feature for the zero-copy bridge.
///
/// Layout (8 × f32, 32 bytes — fits two `vec4f`s in a shader):
///   [0]     primitive kind tag (0 = box, 1 = sphere, 2 = cylinder)
///   [1..=3] center xyz (mm, world space)
///   [4..=6] shape parameters
///             box      → half-extents xyz
///             sphere   → (radius, 0, 0)
///             cylinder → (radius, half_height, 0)
///   [7]     boolean op tag (0 = union, 1 = difference, 2 = intersect)
pub const FEATURE_STRIDE: usize = 8;

/// Maximum undo / redo depth. The whole document is snapshotted per
/// edit, so depth × document-size ≈ memory cost. For M2 (small linear
/// feature lists) 64 is plenty without being painful.
const HISTORY_DEPTH: usize = 64;

// ─── Pure-Rust SDF evaluation (used by tests and the future CPU mesher) ──

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

/// Composite SDF of an entire feature spine.
pub fn sdf_features(features: &[Feature], p: [f32; 3]) -> f32 {
    let mut acc = f32::INFINITY;
    for feature in features {
        let di = sdf_primitive(&feature.primitive, p);
        acc = apply_op(acc, di, feature.op);
    }
    acc
}

/// Pack one feature into an 8-float slot of the renderer buffer.
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
            })
            .collect();
        serde_json::to_string(&summaries).map_err(|e| JsError::new(&e.to_string()))
    }

    // ── mutations ─────────────────────────────────────────────────────

    /// Append a primitive feature with the given kind, op, center and
    /// shape parameters. Returns the new feature's id.
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
        cx: f32,
        cy: f32,
        cz: f32,
        p0: f32,
        p1: f32,
        p2: f32,
    ) -> Result<FeatureId, JsError> {
        let primitive = match kind {
            Primitive::KIND_BOX => Primitive::Box {
                center: [cx, cy, cz],
                half_extents: [p0, p1, p2],
            },
            Primitive::KIND_SPHERE => Primitive::Sphere {
                center: [cx, cy, cz],
                radius: p0,
            },
            Primitive::KIND_CYLINDER => Primitive::Cylinder {
                center: [cx, cy, cz],
                radius: p0,
                half_height: p1,
            },
            other => return Err(JsError::new(&format!("invalid primitive kind: {other}"))),
        };
        let bool_op = Self::parse_op(op)?;

        self.snapshot();
        let id = self.document.allocate_id();
        let name = format!("{} {}", primitive.default_name(), id);
        self.document.features.push(Feature { id, name, op: bool_op, primitive });
        self.rebuild_packed();
        Ok(id)
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
        }
    }

    fn sphere_feature(id: FeatureId, op: BoolOp, center: [f32; 3], r: f32) -> Feature {
        Feature {
            id,
            name: format!("Sphere {id}"),
            op,
            primitive: Primitive::Sphere { center, radius: r },
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
