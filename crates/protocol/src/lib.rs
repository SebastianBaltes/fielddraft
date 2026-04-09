//! FieldDraft wire protocol — shared types for kernel ↔ frontend.
//!
//! This crate stays `no_std`-friendly-ish and avoids any wasm-bindgen deps,
//! so the same types can be consumed from the kernel crate and (later) a
//! JSON-schema export for the TypeScript side.

use serde::{Deserialize, Serialize};

/// Current on-disk / on-wire schema version for the project document.
pub const SCHEMA_VERSION: u32 = 2;

/// Stable identifier for a feature within a document.
///
/// Ids are assigned monotonically and never reused, so the outliner can
/// rely on them for selection state and React reconciliation across
/// reorders, undo, and redo.
pub type FeatureId = u32;

/// A single SDF primitive shape. The primitive itself carries no
/// boolean operation — that lives on its enclosing `Feature`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Primitive {
    Box { center: [f32; 3], half_extents: [f32; 3] },
    Sphere { center: [f32; 3], radius: f32 },
    Cylinder { center: [f32; 3], radius: f32, half_height: f32 },
}

impl Primitive {
    /// Numeric kind tag matching the packed-buffer / shader contract.
    pub const KIND_BOX: u32 = 0;
    pub const KIND_SPHERE: u32 = 1;
    pub const KIND_CYLINDER: u32 = 2;

    pub fn kind_tag(&self) -> u32 {
        match self {
            Primitive::Box { .. } => Self::KIND_BOX,
            Primitive::Sphere { .. } => Self::KIND_SPHERE,
            Primitive::Cylinder { .. } => Self::KIND_CYLINDER,
        }
    }

    /// Human-friendly default name (used when adding a feature).
    pub fn default_name(&self) -> &'static str {
        match self {
            Primitive::Box { .. } => "Box",
            Primitive::Sphere { .. } => "Sphere",
            Primitive::Cylinder { .. } => "Cylinder",
        }
    }
}

/// Boolean operation that combines a feature with the running result.
///
/// Evaluation order is the feature list itself: each feature folds into
/// the accumulator using its op. The very first `Union` feature seeds the
/// accumulator from `+∞` (empty space), so a list of `[Union(box)]`
/// renders that single box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoolOp {
    Union,
    Difference,
    Intersect,
}

impl BoolOp {
    /// Numeric tag for the packed buffer / shader contract.
    pub const TAG_UNION: u32 = 0;
    pub const TAG_DIFFERENCE: u32 = 1;
    pub const TAG_INTERSECT: u32 = 2;

    pub fn tag(self) -> u32 {
        match self {
            BoolOp::Union => Self::TAG_UNION,
            BoolOp::Difference => Self::TAG_DIFFERENCE,
            BoolOp::Intersect => Self::TAG_INTERSECT,
        }
    }
}

/// One node in the feature spine. At M2 the spine is linear; later
/// milestones (Pattern, Group, Mirror) will introduce nested children
/// that share the same `Feature` envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub id: FeatureId,
    pub name: String,
    pub op: BoolOp,
    pub primitive: Primitive,
}

/// Project document root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub schema_version: u32,
    pub name: String,
    /// Linear feature list, evaluated top-to-bottom.
    pub features: Vec<Feature>,
    /// Monotonic id allocator. Persisted so reloads keep stable ids.
    pub next_feature_id: FeatureId,
}

impl Default for Document {
    fn default() -> Self {
        // The empty default has no features at all so the user starts
        // with a clean slate. Tests and the smoke E2E both rely on this.
        Self {
            schema_version: SCHEMA_VERSION,
            name: "Untitled".to_string(),
            features: Vec::new(),
            next_feature_id: 1,
        }
    }
}

impl Document {
    /// Allocate the next feature id and bump the counter.
    pub fn allocate_id(&mut self) -> FeatureId {
        let id = self.next_feature_id;
        self.next_feature_id = self.next_feature_id.saturating_add(1);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_document_is_empty() {
        let doc = Document::default();
        assert_eq!(doc.schema_version, SCHEMA_VERSION);
        assert!(doc.features.is_empty());
        assert_eq!(doc.next_feature_id, 1);
    }

    #[test]
    fn document_roundtrips_through_json() {
        let mut doc = Document::default();
        let id = doc.allocate_id();
        doc.features.push(Feature {
            id,
            name: "Base".into(),
            op: BoolOp::Union,
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [10.0, 10.0, 10.0],
            },
        });

        let json = serde_json::to_string(&doc).unwrap();
        let parsed: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(doc, parsed);
    }

    #[test]
    fn allocate_id_is_monotonic() {
        let mut doc = Document::default();
        let a = doc.allocate_id();
        let b = doc.allocate_id();
        let c = doc.allocate_id();
        assert!(a < b && b < c);
        assert_eq!(doc.next_feature_id, c + 1);
    }

    #[test]
    fn primitive_kind_tags_match_constants() {
        assert_eq!(
            Primitive::Box { center: [0.0; 3], half_extents: [1.0; 3] }.kind_tag(),
            Primitive::KIND_BOX
        );
        assert_eq!(
            Primitive::Sphere { center: [0.0; 3], radius: 1.0 }.kind_tag(),
            Primitive::KIND_SPHERE
        );
        assert_eq!(
            Primitive::Cylinder { center: [0.0; 3], radius: 1.0, half_height: 1.0 }.kind_tag(),
            Primitive::KIND_CYLINDER
        );
    }

    #[test]
    fn bool_op_serializes_snake_case() {
        let json = serde_json::to_string(&BoolOp::Difference).unwrap();
        assert_eq!(json, "\"difference\"");
        let parsed: BoolOp = serde_json::from_str("\"intersect\"").unwrap();
        assert_eq!(parsed, BoolOp::Intersect);
    }

    #[test]
    fn bool_op_tags_match_constants() {
        assert_eq!(BoolOp::Union.tag(), BoolOp::TAG_UNION);
        assert_eq!(BoolOp::Difference.tag(), BoolOp::TAG_DIFFERENCE);
        assert_eq!(BoolOp::Intersect.tag(), BoolOp::TAG_INTERSECT);
    }
}
