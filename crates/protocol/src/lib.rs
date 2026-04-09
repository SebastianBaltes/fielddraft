//! FieldDraft wire protocol — shared types for kernel ↔ frontend.
//!
//! This crate stays `no_std`-friendly-ish and avoids any wasm-bindgen deps,
//! so the same types can be consumed from the kernel crate and (later) a
//! JSON-schema export for the TypeScript side.

use serde::{Deserialize, Serialize};

/// Current on-disk / on-wire schema version for the project document.
pub const SCHEMA_VERSION: u32 = 1;

/// A single SDF primitive in the scene. M1 ships with three shapes; M2+ will
/// replace this with the full feature-tree representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Primitive {
    Box { center: [f32; 3], half_extents: [f32; 3] },
    Sphere { center: [f32; 3], radius: f32 },
    Cylinder { center: [f32; 3], radius: f32, half_height: f32 },
}

/// Project document root. At M1 this is just a flat list of primitives
/// plus metadata; the real feature-tree lands in M2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub schema_version: u32,
    pub name: String,
    pub primitives: Vec<Primitive>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            name: "Untitled".to_string(),
            primitives: vec![
                Primitive::Box {
                    center: [0.0, 0.0, 0.0],
                    half_extents: [10.0, 10.0, 10.0],
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_document_roundtrips_through_json() {
        let doc = Document::default();
        let json = serde_json::to_string(&doc).unwrap();
        let parsed: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(doc, parsed);
    }

    #[test]
    fn schema_version_is_pinned() {
        assert_eq!(SCHEMA_VERSION, 1);
        assert_eq!(Document::default().schema_version, 1);
    }

    #[test]
    fn primitive_variants_serialize_with_snake_case_tag() {
        let p = Primitive::Sphere {
            center: [1.0, 2.0, 3.0],
            radius: 4.5,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"kind\":\"sphere\""));
    }
}
