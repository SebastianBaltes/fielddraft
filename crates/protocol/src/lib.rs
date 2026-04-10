//! FieldDraft wire protocol — shared types for kernel ↔ frontend.
//!
//! This crate stays `no_std`-friendly-ish and avoids any wasm-bindgen deps,
//! so the same types can be consumed from the kernel crate and (later) a
//! JSON-schema export for the TypeScript side.

use std::fmt;
use std::num::ParseIntError;
use std::str::FromStr;

use serde::de::{self, Deserializer, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Current on-disk / on-wire schema version for the project document.
pub const SCHEMA_VERSION: u32 = 2;

/// Hierarchical, typed identifier for a feature or one of its sub-nodes.
///
/// The K4.5 "Feature-as-Function" refactor replaces the flat `u32` ids
/// from K1/K2/K4 with a tree-structured id so sub-operations inside a
/// feature body can carve out their own stable namespace. Concretely:
///
/// * `parent` is an optional ancestor id — `None` for top-level
///   feature entries in the document, `Some` for sub-nodes created
///   inside a feature body.
/// * `local` is the short per-level name stored as [`SmolStr`] so
///   the common flat-numeric form (`"1"`, `"42"`) stays inline-
///   allocated without hitting the heap.
///
/// Pretty-printed as a dotted path (`extrude_3.op`, `fillet_7.mask_0`)
/// via the [`fmt::Display`] impl. Parseable back via [`FromStr`] with
/// the mirror rules.
///
/// ## Backward compatibility with K1 documents
///
/// Old `fielddraft` JSON documents serialise feature ids as bare
/// integers (`"id": 3`). The custom [`Serialize`] / [`Deserialize`]
/// impls below keep that shape for flat-numeric ids (those with no
/// parent whose `local` parses as `u32`), and reject nothing on the
/// deserialiser side — bare numbers, dotted strings, and bare strings
/// are all accepted. The K4.5.1 slice never creates hierarchical
/// ids, so every document in the wild still round-trips to the
/// exact same bytes it had under K1.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Default)]
pub struct FeatureId {
    pub parent: Option<Box<FeatureId>>,
    pub local: SmolStr,
}

impl FeatureId {
    /// Build a top-level id from a textual local name.
    pub fn root(local: impl Into<SmolStr>) -> Self {
        Self {
            parent: None,
            local: local.into(),
        }
    }

    /// Build a top-level id from a flat numeric counter — the
    /// classic K1-style id, just typed. Used by
    /// [`Document::allocate_id`] to keep the existing
    /// monotonic-counter allocation scheme alive inside the new
    /// type.
    pub fn from_u32(n: u32) -> Self {
        Self::root(n.to_string())
    }

    /// Build a child id underneath this one. The new id keeps the
    /// whole ancestor chain so `Display` can emit the full path.
    pub fn child(&self, local: impl Into<SmolStr>) -> Self {
        Self {
            parent: Some(Box::new(self.clone())),
            local: local.into(),
        }
    }

    /// Return the parent id, if any.
    pub fn parent(&self) -> Option<&FeatureId> {
        self.parent.as_deref()
    }

    /// `true` when this id has no parent and the local part parses
    /// cleanly as a `u32`. This is the "compat-shim" predicate:
    /// [`Self::as_u32_if_flat`] uses it to decide whether the id
    /// can still go through the K1-era `u32` wasm-bindgen surface.
    pub fn is_flat_numeric(&self) -> bool {
        self.parent.is_none() && self.local.parse::<u32>().is_ok()
    }

    /// Collapse the id back to a `u32`, assuming it's flat-numeric.
    /// Returns `None` for hierarchical or non-numeric ids. The
    /// wasm-bindgen surface in `crates/kernel/src/lib.rs` uses this
    /// to keep the existing `u32` signatures working during K4.5.1
    /// — every id issued in this slice is still flat-numeric, so
    /// the `.expect` paths are unreachable for well-behaved inputs.
    pub fn as_u32_if_flat(&self) -> Option<u32> {
        if self.parent.is_some() {
            return None;
        }
        self.local.parse::<u32>().ok()
    }
}

impl fmt::Display for FeatureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(parent) = &self.parent {
            write!(f, "{parent}.{}", self.local)
        } else {
            f.write_str(&self.local)
        }
    }
}

/// Parse error for [`FeatureId`]. A future hierarchical slice may
/// grow this with richer variants (empty segments, forbidden chars).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureIdParseError {
    pub message: &'static str,
}

impl fmt::Display for FeatureIdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for FeatureIdParseError {}

impl From<ParseIntError> for FeatureIdParseError {
    fn from(_: ParseIntError) -> Self {
        Self {
            message: "invalid numeric feature id",
        }
    }
}

impl FromStr for FeatureId {
    type Err = FeatureIdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(FeatureIdParseError {
                message: "empty feature id",
            });
        }
        // Split at the *last* dot so parent chains parse left-
        // to-right but build right-to-left, matching the
        // [`Display`] shape `"grandparent.parent.local"`.
        match s.rsplit_once('.') {
            None => Ok(FeatureId::root(s)),
            Some((head, tail)) => {
                if tail.is_empty() {
                    return Err(FeatureIdParseError {
                        message: "empty local segment in feature id",
                    });
                }
                let parent = FeatureId::from_str(head)?;
                Ok(FeatureId {
                    parent: Some(Box::new(parent)),
                    local: SmolStr::from(tail),
                })
            }
        }
    }
}

impl Serialize for FeatureId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Keep byte-identical shape with K1 documents: a flat
        // numeric id serialises as a bare integer, anything else
        // as the dotted-path string form.
        if let Some(n) = self.as_u32_if_flat() {
            serializer.serialize_u32(n)
        } else {
            serializer.collect_str(self)
        }
    }
}

impl<'de> Deserialize<'de> for FeatureId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FeatureIdVisitor;

        impl<'de> Visitor<'de> for FeatureIdVisitor {
            type Value = FeatureId;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a feature id: integer or dotted string")
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<FeatureId, E> {
                Ok(FeatureId::from_u32(v as u32))
            }

            fn visit_u32<E: de::Error>(self, v: u32) -> Result<FeatureId, E> {
                Ok(FeatureId::from_u32(v))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<FeatureId, E> {
                if v < 0 {
                    return Err(E::custom("negative feature id"));
                }
                Ok(FeatureId::from_u32(v as u32))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<FeatureId, E> {
                FeatureId::from_str(v).map_err(E::custom)
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<FeatureId, E> {
                FeatureId::from_str(&v).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(FeatureIdVisitor)
    }
}

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

/// Affine placement of a feature in world space.
///
/// Stored as **translate + unit-quaternion rotation + uniform scale**.
/// Per-axis scale would break the SDF distance property (the field
/// stops being a true Euclidean distance), so we keep scale uniform.
/// Quaternion order is `(x, y, z, w)`; identity is `(0, 0, 0, 1)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub translate: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: f32,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translate: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: 1.0,
    };

    pub fn is_identity(&self) -> bool {
        self.translate == [0.0, 0.0, 0.0]
            && self.rotation == [0.0, 0.0, 0.0, 1.0]
            && self.scale == 1.0
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
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
    /// Affine placement of the primitive in world space.
    /// `#[serde(default)]` lets us roll forward older documents that
    /// predate transforms — they load with identity placement.
    #[serde(default)]
    pub transform: Transform,
}

/// One entry in the K4.5 Feature-as-Function feature tree.
///
/// The canonical §4.6 JSON wire format for every mutation a user
/// makes to a document: a `{ type, id, params }` triple where the
/// type name picks a [`FeatureKind`] (in the kernel registry) and
/// the `params` blob carries the user's typed parameter values as
/// raw JSON until a `FeatureSchema` derive validates them.
///
/// K4.5.4b moved this struct from `kernel::features` to the
/// protocol crate so it can eventually live inside [`Document`]
/// as the authoritative serialisation form (K4.5.4c+). The kernel
/// re-exports `FeatureCall` from `crate::features::FeatureCall`
/// so existing imports keep working — the move is purely
/// structural, no behaviour changed in this slice.
///
/// [`FeatureKind`]: ../fielddraft_kernel/features/trait.FeatureKind.html
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureCall {
    /// Registry key for the feature type. `#[serde(rename = "type")]`
    /// emits the §4.6 canonical field name on the wire.
    #[serde(rename = "type")]
    pub type_name: String,
    /// Stable id of this call within the feature tree. Step 1
    /// (K4.5.1) introduced the hierarchical [`FeatureId`] that
    /// sub-operations inside a feature body can extend with
    /// child names.
    pub id: FeatureId,
    /// Typed parameter values. Held as raw `serde_json::Value` so
    /// the wire shape is schema-less until the feature's
    /// `FeatureKind::execute` parses it.
    #[serde(default)]
    pub params: serde_json::Value,
}

impl FeatureCall {
    /// Build a feature call from its three canonical fields.
    pub fn new(
        type_name: impl Into<String>,
        id: FeatureId,
        params: serde_json::Value,
    ) -> Self {
        Self {
            type_name: type_name.into(),
            id,
            params,
        }
    }
}

/// Project document root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub schema_version: u32,
    pub name: String,
    /// Linear feature list, evaluated top-to-bottom.
    pub features: Vec<Feature>,
    /// Monotonic counter that seeds the next flat-numeric
    /// [`FeatureId`]. Persisted so reloads keep stable ids. The
    /// field name stays as `next_feature_id` for JSON backward
    /// compatibility with K1 documents that wrote it as a bare
    /// integer; the type is a plain counter because K4.5.1 still
    /// only issues flat-numeric ids. A future K4.5 slice will
    /// grow a proper hierarchical allocator on top.
    pub next_feature_id: u32,
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
    /// Allocate the next flat-numeric [`FeatureId`] and bump the
    /// counter. K4.5.1 still uses the K1 allocation scheme (`"1"`,
    /// `"2"`, `"3"` in the new typed form). Hierarchical allocation
    /// via the Feature Registry will come in a later slice.
    pub fn allocate_id(&mut self) -> FeatureId {
        let n = self.next_feature_id;
        self.next_feature_id = self.next_feature_id.saturating_add(1);
        FeatureId::from_u32(n)
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
            transform: Transform::IDENTITY,
        });

        let json = serde_json::to_string(&doc).unwrap();
        let parsed: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(doc, parsed);
    }

    #[test]
    fn transform_default_is_identity() {
        let t = Transform::default();
        assert!(t.is_identity());
        assert_eq!(t.translate, [0.0, 0.0, 0.0]);
        assert_eq!(t.rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(t.scale, 1.0);
    }

    #[test]
    fn legacy_feature_without_transform_loads_with_identity() {
        // A document that predates the transform field should still parse,
        // gaining `Transform::IDENTITY` via `#[serde(default)]`.
        let json = r#"{
            "schema_version": 2,
            "name": "legacy",
            "features": [{
                "id": 1,
                "name": "Base",
                "op": "union",
                "primitive": {"kind":"box","center":[0,0,0],"half_extents":[1,1,1]}
            }],
            "next_feature_id": 2
        }"#;
        let parsed: Document = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.features.len(), 1);
        assert!(parsed.features[0].transform.is_identity());
    }

    #[test]
    fn allocate_id_is_monotonic() {
        let mut doc = Document::default();
        let a = doc.allocate_id();
        let b = doc.allocate_id();
        let c = doc.allocate_id();
        // K4.5 still issues flat-numeric ids, so we can round-trip
        // them through u32 for the ordering assertion.
        let an = a.as_u32_if_flat().unwrap();
        let bn = b.as_u32_if_flat().unwrap();
        let cn = c.as_u32_if_flat().unwrap();
        assert!(an < bn && bn < cn);
        assert_eq!(doc.next_feature_id, cn + 1);
    }

    #[test]
    fn feature_id_display_and_parse_round_trip() {
        let flat = FeatureId::from_u32(42);
        assert_eq!(flat.to_string(), "42");
        assert_eq!(FeatureId::from_str("42").unwrap(), flat);
        assert!(flat.is_flat_numeric());

        let child = flat.child("op");
        assert_eq!(child.to_string(), "42.op");
        assert_eq!(FeatureId::from_str("42.op").unwrap(), child);
        assert!(!child.is_flat_numeric());
        assert!(child.as_u32_if_flat().is_none());

        let grand = child.child("mask_0");
        assert_eq!(grand.to_string(), "42.op.mask_0");
        assert_eq!(FeatureId::from_str("42.op.mask_0").unwrap(), grand);
    }

    #[test]
    fn feature_id_serializes_flat_numeric_as_bare_integer() {
        // Backward-compat with K1 JSON documents that stored ids
        // as bare integers.
        let id = FeatureId::from_u32(7);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "7");
        let parsed: FeatureId = serde_json::from_str("7").unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn feature_id_serializes_hierarchical_as_string() {
        let id = FeatureId::from_u32(3).child("op");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"3.op\"");
        let parsed: FeatureId = serde_json::from_str("\"3.op\"").unwrap();
        assert_eq!(parsed, id);
    }

    // ─── FeatureCall (K4.5.4b) ──────────────────────────────────

    #[test]
    fn feature_call_builds_from_fields() {
        let call = FeatureCall::new(
            "box",
            FeatureId::from_u32(7),
            serde_json::json!({ "half_extents": [1.0, 2.0, 3.0] }),
        );
        assert_eq!(call.type_name, "box");
        assert_eq!(call.id, FeatureId::from_u32(7));
        assert_eq!(
            call.params,
            serde_json::json!({ "half_extents": [1.0, 2.0, 3.0] })
        );
    }

    #[test]
    fn feature_call_serde_shape_uses_type_field_not_type_name() {
        // §4.6 canonical wire shape: `{ "type": "...", "id": ...,
        // "params": {...} }`. The Rust field is called `type_name`
        // because `type` is a reserved keyword; the
        // `#[serde(rename)]` glue emits `type` on the wire.
        let call = FeatureCall::new(
            "sphere",
            FeatureId::from_u32(3),
            serde_json::json!({ "radius": 5.0 }),
        );
        let wire = serde_json::to_value(&call).unwrap();
        assert_eq!(wire.get("type"), Some(&serde_json::json!("sphere")));
        assert!(wire.get("type_name").is_none());
        assert_eq!(wire.get("id"), Some(&serde_json::json!(3)));
        assert_eq!(
            wire.get("params"),
            Some(&serde_json::json!({ "radius": 5.0 }))
        );
    }

    #[test]
    fn feature_call_round_trips_through_json() {
        let call = FeatureCall::new(
            "cylinder",
            FeatureId::from_u32(1).child("cap"),
            serde_json::json!({
                "radius": 4.0,
                "half_height": 10.0,
                "op": "difference"
            }),
        );
        let wire = serde_json::to_string(&call).unwrap();
        let parsed: FeatureCall = serde_json::from_str(&wire).unwrap();
        assert_eq!(parsed, call);
    }

    #[test]
    fn feature_call_params_default_to_null_when_missing() {
        // `#[serde(default)]` lets a call without a params field
        // still deserialise — handy for the K4.5 step-5 JS shell
        // that sometimes emits parameter-less calls during drag
        // previews.
        let wire = r#"{"type":"box","id":2}"#;
        let parsed: FeatureCall = serde_json::from_str(wire).unwrap();
        assert_eq!(parsed.type_name, "box");
        assert_eq!(parsed.id, FeatureId::from_u32(2));
        assert_eq!(parsed.params, serde_json::Value::Null);
    }

    #[test]
    fn feature_id_deserializer_accepts_both_shapes() {
        let from_int: FeatureId = serde_json::from_str("5").unwrap();
        assert_eq!(from_int, FeatureId::from_u32(5));
        let from_str: FeatureId = serde_json::from_str("\"5\"").unwrap();
        assert_eq!(from_str, FeatureId::from_u32(5));
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
