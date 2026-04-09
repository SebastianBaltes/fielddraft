//! FieldDraft geometry kernel.
//!
//! At M1 the kernel's job is minimal: hold a `Document`, evaluate SDF
//! primitives (Box / Sphere / Cylinder), and expose a packed `Float32Array`
//! view that the raymarching shader on the JS side can read without copies.

use fielddraft_protocol::{Document, Primitive};
use wasm_bindgen::prelude::*;

/// Packed representation of a single primitive for the zero-copy bridge.
///
/// Layout (8 × f32, 32 bytes, aligned for `Float32Array` views):
///   [0]     kind tag (0 = box, 1 = sphere, 2 = cylinder)
///   [1..=3] center xyz (world space, mm)
///   [4..=6] shape parameters:
///             box      → half-extents xyz
///             sphere   → (radius, 0, 0)
///             cylinder → (radius, half_height, 0)
///   [7]     reserved / padding for 32-byte stride
pub const PRIMITIVE_STRIDE: usize = 8;

pub const KIND_BOX: f32 = 0.0;
pub const KIND_SPHERE: f32 = 1.0;
pub const KIND_CYLINDER: f32 = 2.0;

/// Pure-Rust SDF evaluation — used by tests and (eventually) the CPU mesher.
pub fn sdf(primitive: &Primitive, p: [f32; 3]) -> f32 {
    match primitive {
        Primitive::Box { center, half_extents } => {
            let q = [
                (p[0] - center[0]).abs() - half_extents[0],
                (p[1] - center[1]).abs() - half_extents[1],
                (p[2] - center[2]).abs() - half_extents[2],
            ];
            let outside = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2) + q[2].max(0.0).powi(2)).sqrt();
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

/// Field union — the composite SDF is a straight `min` of the primitives.
pub fn sdf_union(primitives: &[Primitive], p: [f32; 3]) -> f32 {
    primitives
        .iter()
        .map(|prim| sdf(prim, p))
        .fold(f32::INFINITY, f32::min)
}

/// Pack one primitive into an 8-float slot.
pub fn pack_primitive(prim: &Primitive, out: &mut [f32; PRIMITIVE_STRIDE]) {
    *out = [0.0; PRIMITIVE_STRIDE];
    match prim {
        Primitive::Box { center, half_extents } => {
            out[0] = KIND_BOX;
            out[1..4].copy_from_slice(center);
            out[4..7].copy_from_slice(half_extents);
        }
        Primitive::Sphere { center, radius } => {
            out[0] = KIND_SPHERE;
            out[1..4].copy_from_slice(center);
            out[4] = *radius;
        }
        Primitive::Cylinder { center, radius, half_height } => {
            out[0] = KIND_CYLINDER;
            out[1..4].copy_from_slice(center);
            out[4] = *radius;
            out[5] = *half_height;
        }
    }
}

pub fn pack_document(doc: &Document) -> Vec<f32> {
    let mut buf = vec![0.0_f32; doc.primitives.len() * PRIMITIVE_STRIDE];
    for (i, prim) in doc.primitives.iter().enumerate() {
        let slot: &mut [f32; PRIMITIVE_STRIDE] = (&mut buf
            [i * PRIMITIVE_STRIDE..(i + 1) * PRIMITIVE_STRIDE])
            .try_into()
            .unwrap();
        pack_primitive(prim, slot);
    }
    buf
}

// ─── wasm-bindgen surface ────────────────────────────────────────────────────

/// Handle held by the JS side. Owns the current document and the packed
/// primitive buffer; the buffer pointer stays stable until the document
/// is mutated (which may reallocate).
#[wasm_bindgen]
pub struct Kernel {
    document: Document,
    packed: Vec<f32>,
}

#[wasm_bindgen]
impl Kernel {
    /// Create a new kernel with the default document (one box at the origin).
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        console_error_panic_hook::set_once();

        let document = Document::default();
        let packed = pack_document(&document);
        Self { document, packed }
    }

    /// Load a document from a JSON string. Returns the new primitive count.
    #[wasm_bindgen(js_name = loadJson)]
    pub fn load_json(&mut self, json: &str) -> Result<usize, JsError> {
        let document: Document =
            serde_json::from_str(json).map_err(|e| JsError::new(&e.to_string()))?;
        self.packed = pack_document(&document);
        self.document = document;
        Ok(self.document.primitives.len())
    }

    /// Serialize the current document to JSON (UTF-8).
    #[wasm_bindgen(js_name = toJson)]
    pub fn to_json(&self) -> Result<String, JsError> {
        serde_json::to_string(&self.document).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Pointer into the WASM linear memory for the packed primitive buffer.
    /// JS wraps this in a `Float32Array` view — zero copy.
    #[wasm_bindgen(js_name = primitivesPtr)]
    pub fn primitives_ptr(&self) -> *const f32 {
        self.packed.as_ptr()
    }

    /// Number of primitives currently held.
    #[wasm_bindgen(js_name = primitiveCount)]
    pub fn primitive_count(&self) -> usize {
        self.document.primitives.len()
    }

    /// Number of f32s per primitive in the packed buffer.
    #[wasm_bindgen(js_name = primitiveStride)]
    pub fn primitive_stride() -> usize {
        PRIMITIVE_STRIDE
    }

    /// Evaluate the composite SDF at a world-space point. Primarily for tests
    /// and debugging — the viewport uses the packed buffer + shader instead.
    #[wasm_bindgen(js_name = evalSdf)]
    pub fn eval_sdf(&self, x: f32, y: f32, z: f32) -> f32 {
        sdf_union(&self.document.primitives, [x, y, z])
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

    #[test]
    fn box_sdf_zero_on_face() {
        let b = Primitive::Box {
            center: [0.0, 0.0, 0.0],
            half_extents: [1.0, 1.0, 1.0],
        };
        assert!((sdf(&b, [1.0, 0.0, 0.0])).abs() < 1e-5);
        assert!(sdf(&b, [0.0, 0.0, 0.0]) < 0.0); // inside
        assert!(sdf(&b, [2.0, 0.0, 0.0]) > 0.0); // outside
    }

    #[test]
    fn sphere_sdf_matches_radius() {
        let s = Primitive::Sphere {
            center: [0.0, 0.0, 0.0],
            radius: 5.0,
        };
        assert!((sdf(&s, [5.0, 0.0, 0.0])).abs() < 1e-5);
        assert!((sdf(&s, [0.0, 10.0, 0.0]) - 5.0).abs() < 1e-5);
        assert!((sdf(&s, [0.0, 0.0, 0.0]) + 5.0).abs() < 1e-5);
    }

    #[test]
    fn cylinder_sdf_radial_and_axial() {
        let c = Primitive::Cylinder {
            center: [0.0, 0.0, 0.0],
            radius: 2.0,
            half_height: 3.0,
        };
        assert!((sdf(&c, [2.0, 0.0, 0.0])).abs() < 1e-5);
        assert!((sdf(&c, [0.0, 3.0, 0.0])).abs() < 1e-5);
        assert!(sdf(&c, [0.0, 0.0, 0.0]) < 0.0);
    }

    #[test]
    fn union_returns_minimum() {
        let prims = vec![
            Primitive::Sphere { center: [0.0, 0.0, 0.0], radius: 1.0 },
            Primitive::Sphere { center: [10.0, 0.0, 0.0], radius: 1.0 },
        ];
        assert!((sdf_union(&prims, [10.0, 0.0, 0.0]) + 1.0).abs() < 1e-5);
        assert!((sdf_union(&prims, [0.0, 0.0, 0.0]) + 1.0).abs() < 1e-5);
    }

    #[test]
    fn pack_layout_matches_stride() {
        let doc = Document::default();
        let buf = pack_document(&doc);
        assert_eq!(buf.len(), doc.primitives.len() * PRIMITIVE_STRIDE);
        assert_eq!(buf[0], KIND_BOX);
    }

    #[test]
    fn pack_sphere_encodes_radius() {
        let mut slot = [0.0_f32; PRIMITIVE_STRIDE];
        pack_primitive(
            &Primitive::Sphere { center: [1.0, 2.0, 3.0], radius: 4.0 },
            &mut slot,
        );
        assert_eq!(slot[0], KIND_SPHERE);
        assert_eq!(&slot[1..4], &[1.0, 2.0, 3.0]);
        assert_eq!(slot[4], 4.0);
    }
}
