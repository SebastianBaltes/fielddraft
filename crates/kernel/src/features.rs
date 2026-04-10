//! Feature registry and the Feature-as-Function execution model.
//!
//! This is step 2 of the K4.5 refactor (README §4.6, §10 K4.5 bullet).
//! The goal of this step is narrow: stand up a [`FeatureKind`] trait
//! and a [`FeatureRegistry`] that can walk a `Vec<FeatureCall>`, look
//! up each entry by its type name, validate the parameter JSON against
//! the feature's expectations, and call the feature's `execute` on a
//! mutable [`Context`]. The first three built-in features — box,
//! sphere, cylinder — are the K1 primitives re-packaged into the new
//! model so we can see the regeneration pipeline end-to-end.
//!
//! ## What this module deliberately does *not* do yet
//!
//! * **No proc-macro.** The derive-based schema generator is step 3.
//!   Built-in features register manually.
//! * **No JSON schema export.** Also step 3 once the derive lands.
//! * **No Regeneration Worker integration.** The worker's global
//!   freeze + `globalThis.fd = {…}` injection is step 5.
//! * **No queries.** `qCreatedBy` / `qFaceOf` / `qEdgeBetween` are
//!   step 7.
//! * **The Kernel struct is unchanged.** `crates/kernel/src/lib.rs`
//!   still drives mutations through `add_primitive` and friends.
//!   Wiring the registry into the main kernel path is a later
//!   session — this module is exercised exclusively by its own
//!   tests for now.
//! * **No hierarchical ids are issued yet.** Step 1 (K4.5.1) gave us
//!   the typed [`FeatureId`] shape; step 2 still only uses flat
//!   numeric ids via `FeatureId::from_u32`.
//!
//! ## Naming note
//!
//! The trait is called `FeatureKind`, not `Feature`, to dodge the
//! collision with [`fielddraft_protocol::Feature`] — the protocol
//! struct still represents one entry in the spine and will morph
//! into `FeatureCall` in step 4 (`Vec<FeatureCall>` as the canonical
//! serialisation form). When that rename lands the trait can be
//! renamed to `Feature` in the same commit.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use fielddraft_protocol::{
    BoolOp, Document, Feature as SpineFeature, FeatureId, Primitive, Transform,
};

/// Errors that can happen while looking up a feature in the registry
/// or while a feature's `execute` validates its parameter JSON.
#[derive(Debug, Clone, PartialEq)]
pub enum FeatureError {
    /// The feature call's `type` field didn't match any registered
    /// feature kind.
    UnknownFeatureType(String),
    /// A required parameter was missing from the definition JSON.
    MissingParameter {
        feature: &'static str,
        name: &'static str,
    },
    /// A parameter was present but the wrong shape (e.g. a number
    /// where a `[f32; 3]` was expected, or a negative length).
    InvalidParameter {
        feature: &'static str,
        name: &'static str,
        message: String,
    },
    /// The `op` field referenced an unknown boolean operation.
    InvalidOp { feature: &'static str, value: String },
}

impl fmt::Display for FeatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFeatureType(name) => {
                write!(f, "unknown feature type `{name}`")
            }
            Self::MissingParameter { feature, name } => {
                write!(f, "feature `{feature}`: missing parameter `{name}`")
            }
            Self::InvalidParameter {
                feature,
                name,
                message,
            } => write!(
                f,
                "feature `{feature}`: parameter `{name}` invalid: {message}",
            ),
            Self::InvalidOp { feature, value } => {
                write!(f, "feature `{feature}`: unknown op `{value}`")
            }
        }
    }
}

impl std::error::Error for FeatureError {}

/// Mutable world state passed to every feature's `execute` call.
///
/// K4.5 step 2 keeps this intentionally thin: the only thing a
/// feature can touch is the [`Document`] whose feature spine backs
/// the existing K1/K2/K4 mesher pipeline. Later slices will grow the
/// context to own the body cache, provenance tables and query
/// results so features can read each other's outputs — but those
/// pieces are K4.5 step 5+ scope.
///
/// The lifetime `'a` ties the context to the caller's borrow of the
/// document, so a feature can't smuggle a reference out of its
/// execution.
pub struct Context<'a> {
    pub document: &'a mut Document,
}

impl<'a> Context<'a> {
    pub fn new(document: &'a mut Document) -> Self {
        Self { document }
    }
}

/// The behaviour of one feature type.
///
/// A `FeatureKind` is a stateless function table: the registry holds
/// one boxed trait object per type name, and every [`FeatureCall`]
/// with that type name in a document's feature tree dispatches to
/// the same `execute` function. Per-call parameters live in the
/// call's `params` JSON blob, not in the trait object.
///
/// Implementors are `Send + Sync` so a future K4.5 step can hand the
/// registry over to the Regeneration Worker without wrapping every
/// feature in a lock.
pub trait FeatureKind: Send + Sync {
    /// Registry key — stable, snake_case, matches the `type` field
    /// in the serialised feature tree.
    fn type_name(&self) -> &'static str;

    /// Validate the definition JSON and mutate the context. On
    /// success every side effect is already baked into the context;
    /// on error the side effects accumulated so far stay, but the
    /// registry's `regenerate` pass will halt and refuse to run any
    /// further calls (fail-stop / Rollback semantics per §4.6).
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError>;
}

/// One entry in a serialisable feature tree. The canonical on-disk
/// form of the K4.5 Feature-as-Function model: every mutation a user
/// made in the tree is one of these, ordered top-to-bottom.
///
/// For K4.5 step 2 the `params` blob is a raw `serde_json::Value`;
/// step 3's derive-macro will give each feature a strongly-typed
/// Rust struct and emit a JSON Schema alongside it, but the runtime
/// format stays JSON so the JavaScript side of the dual API shell
/// (step 6) can consume it with zero glue code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureCall {
    #[serde(rename = "type")]
    pub type_name: String,
    pub id: FeatureId,
    #[serde(default)]
    pub params: Value,
}

impl FeatureCall {
    pub fn new(
        type_name: impl Into<String>,
        id: FeatureId,
        params: Value,
    ) -> Self {
        Self {
            type_name: type_name.into(),
            id,
            params,
        }
    }
}

/// Central registry of [`FeatureKind`] impls, keyed by type name.
pub struct FeatureRegistry {
    features: HashMap<String, Box<dyn FeatureKind>>,
}

impl Default for FeatureRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl FeatureRegistry {
    /// Empty registry. Use [`FeatureRegistry::register`] to add
    /// feature kinds before calling [`FeatureRegistry::regenerate`].
    pub fn new() -> Self {
        Self {
            features: HashMap::new(),
        }
    }

    /// Empty registry pre-populated with the three K1 primitives —
    /// `box`, `sphere`, `cylinder`. This is the intended entry
    /// point for all non-test callers in K4.5 step 2 and for the
    /// tests that want to exercise the full built-in surface.
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        r.register(Box::new(BoxPrimitive));
        r.register(Box::new(SpherePrimitive));
        r.register(Box::new(CylinderPrimitive));
        r
    }

    /// Register a feature kind. If one with the same type name is
    /// already registered it's replaced silently — the last
    /// registration wins, which lets tests stub a built-in without
    /// building a fresh registry from scratch.
    pub fn register(&mut self, feature: Box<dyn FeatureKind>) {
        self.features
            .insert(feature.type_name().to_string(), feature);
    }

    /// Look up a feature kind by type name.
    pub fn get(&self, type_name: &str) -> Option<&dyn FeatureKind> {
        self.features.get(type_name).map(|b| b.as_ref())
    }

    /// How many feature kinds are registered.
    pub fn len(&self) -> usize {
        self.features.len()
    }

    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// Walk a feature-call list in order and execute each entry
    /// against the context. Fails fast on the first error: nothing
    /// after the failing call runs, and the caller can inspect the
    /// partially-mutated context to show the user which feature
    /// went red.
    pub fn regenerate(
        &self,
        ctx: &mut Context<'_>,
        calls: &[FeatureCall],
    ) -> Result<(), FeatureError> {
        for call in calls {
            let feature = self
                .features
                .get(&call.type_name)
                .ok_or_else(|| FeatureError::UnknownFeatureType(call.type_name.clone()))?;
            feature.execute(ctx, &call.id, &call.params)?;
        }
        Ok(())
    }
}

// ─── built-in K1 primitives ──────────────────────────────────────

/// `box` — K1 axis-aligned box primitive.
///
/// Parameters:
/// * `half_extents` — required `[f32; 3]`
/// * `center`       — optional `[f32; 3]`, default `[0, 0, 0]`
/// * `translate`    — optional `[f32; 3]`, default `[0, 0, 0]`
/// * `op`           — optional `"union" | "difference" | "intersect"`,
///                    default `"union"`
/// * `name`         — optional display name; falls back to
///                    `"Box <id>"`
pub struct BoxPrimitive;

impl FeatureKind for BoxPrimitive {
    fn type_name(&self) -> &'static str {
        "box"
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let half_extents = read_vec3_required(def, "box", "half_extents")?;
        let center = read_vec3_or(def, "box", "center", [0.0; 3])?;
        let translate = read_vec3_or(def, "box", "translate", [0.0; 3])?;
        let op = read_op_or(def, "box", BoolOp::Union)?;
        let name = read_string_or(def, "name", format!("Box {id}"));

        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op,
            primitive: Primitive::Box {
                center,
                half_extents,
            },
            transform: Transform {
                translate,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: 1.0,
            },
        });
        Ok(())
    }
}

/// `sphere` — K1 sphere primitive.
///
/// Parameters:
/// * `radius`    — required `f32`, must be `> 0`
/// * `center`    — optional `[f32; 3]`, default `[0, 0, 0]`
/// * `translate` — optional `[f32; 3]`, default `[0, 0, 0]`
/// * `op`        — optional, default `"union"`
/// * `name`      — optional, default `"Sphere <id>"`
pub struct SpherePrimitive;

impl FeatureKind for SpherePrimitive {
    fn type_name(&self) -> &'static str {
        "sphere"
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let radius = read_positive_f32_required(def, "sphere", "radius")?;
        let center = read_vec3_or(def, "sphere", "center", [0.0; 3])?;
        let translate = read_vec3_or(def, "sphere", "translate", [0.0; 3])?;
        let op = read_op_or(def, "sphere", BoolOp::Union)?;
        let name = read_string_or(def, "name", format!("Sphere {id}"));

        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op,
            primitive: Primitive::Sphere { center, radius },
            transform: Transform {
                translate,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: 1.0,
            },
        });
        Ok(())
    }
}

/// `cylinder` — K1 Y-axis-aligned cylinder primitive.
///
/// Parameters:
/// * `radius`      — required `f32`, must be `> 0`
/// * `half_height` — required `f32`, must be `> 0`
/// * `center`      — optional `[f32; 3]`, default `[0, 0, 0]`
/// * `translate`   — optional `[f32; 3]`, default `[0, 0, 0]`
/// * `op`          — optional, default `"union"`
/// * `name`        — optional, default `"Cylinder <id>"`
pub struct CylinderPrimitive;

impl FeatureKind for CylinderPrimitive {
    fn type_name(&self) -> &'static str {
        "cylinder"
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let radius = read_positive_f32_required(def, "cylinder", "radius")?;
        let half_height =
            read_positive_f32_required(def, "cylinder", "half_height")?;
        let center = read_vec3_or(def, "cylinder", "center", [0.0; 3])?;
        let translate = read_vec3_or(def, "cylinder", "translate", [0.0; 3])?;
        let op = read_op_or(def, "cylinder", BoolOp::Union)?;
        let name = read_string_or(def, "name", format!("Cylinder {id}"));

        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op,
            primitive: Primitive::Cylinder {
                center,
                radius,
                half_height,
            },
            transform: Transform {
                translate,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: 1.0,
            },
        });
        Ok(())
    }
}

// ─── parameter helpers ───────────────────────────────────────────

fn read_vec3_required(
    def: &Value,
    feature: &'static str,
    name: &'static str,
) -> Result<[f32; 3], FeatureError> {
    let slot = def
        .get(name)
        .ok_or(FeatureError::MissingParameter { feature, name })?;
    parse_vec3(slot, feature, name)
}

fn read_vec3_or(
    def: &Value,
    feature: &'static str,
    name: &'static str,
    fallback: [f32; 3],
) -> Result<[f32; 3], FeatureError> {
    match def.get(name) {
        None => Ok(fallback),
        Some(Value::Null) => Ok(fallback),
        Some(slot) => parse_vec3(slot, feature, name),
    }
}

fn parse_vec3(
    value: &Value,
    feature: &'static str,
    name: &'static str,
) -> Result<[f32; 3], FeatureError> {
    let arr = value
        .as_array()
        .ok_or_else(|| FeatureError::InvalidParameter {
            feature,
            name,
            message: "expected a 3-element array".to_string(),
        })?;
    if arr.len() != 3 {
        return Err(FeatureError::InvalidParameter {
            feature,
            name,
            message: format!("expected 3 elements, got {}", arr.len()),
        });
    }
    let mut out = [0.0_f32; 3];
    for (i, item) in arr.iter().enumerate() {
        out[i] = item.as_f64().map(|v| v as f32).ok_or_else(|| {
            FeatureError::InvalidParameter {
                feature,
                name,
                message: format!("element {i} is not a number"),
            }
        })?;
    }
    Ok(out)
}

fn read_positive_f32_required(
    def: &Value,
    feature: &'static str,
    name: &'static str,
) -> Result<f32, FeatureError> {
    let slot = def
        .get(name)
        .ok_or(FeatureError::MissingParameter { feature, name })?;
    let value = slot
        .as_f64()
        .ok_or_else(|| FeatureError::InvalidParameter {
            feature,
            name,
            message: "expected a number".to_string(),
        })? as f32;
    if !(value > 0.0) {
        return Err(FeatureError::InvalidParameter {
            feature,
            name,
            message: format!("must be > 0, got {value}"),
        });
    }
    Ok(value)
}

fn read_op_or(
    def: &Value,
    feature: &'static str,
    fallback: BoolOp,
) -> Result<BoolOp, FeatureError> {
    let Some(slot) = def.get("op") else {
        return Ok(fallback);
    };
    if slot.is_null() {
        return Ok(fallback);
    }
    let s = slot
        .as_str()
        .ok_or_else(|| FeatureError::InvalidParameter {
            feature,
            name: "op",
            message: "expected a string".to_string(),
        })?;
    match s {
        "union" => Ok(BoolOp::Union),
        "difference" => Ok(BoolOp::Difference),
        "intersect" => Ok(BoolOp::Intersect),
        other => Err(FeatureError::InvalidOp {
            feature,
            value: other.to_string(),
        }),
    }
}

fn read_string_or(def: &Value, name: &str, fallback: String) -> String {
    def.get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or(fallback)
}

// ─── tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(n: u32) -> FeatureId {
        FeatureId::from_u32(n)
    }

    fn empty_ctx() -> Document {
        Document::default()
    }

    #[test]
    fn with_builtins_registers_the_three_k1_primitives() {
        let registry = FeatureRegistry::with_builtins();
        assert_eq!(registry.len(), 3);
        assert!(registry.get("box").is_some());
        assert!(registry.get("sphere").is_some());
        assert!(registry.get("cylinder").is_some());
        assert!(registry.get("torus").is_none());
    }

    #[test]
    fn register_is_last_wins() {
        // A test stub that succeeds unconditionally and returns the
        // type_name "box" — it overrides the built-in.
        struct StubBox;
        impl FeatureKind for StubBox {
            fn type_name(&self) -> &'static str {
                "box"
            }
            fn execute(
                &self,
                _ctx: &mut Context<'_>,
                _id: &FeatureId,
                _def: &Value,
            ) -> Result<(), FeatureError> {
                Err(FeatureError::InvalidParameter {
                    feature: "box",
                    name: "stub",
                    message: "stub rejects everything".into(),
                })
            }
        }
        let mut registry = FeatureRegistry::with_builtins();
        registry.register(Box::new(StubBox));
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let call = FeatureCall::new(
            "box",
            id(1),
            json!({ "half_extents": [1, 1, 1] }),
        );
        let err = registry.regenerate(&mut ctx, &[call]).unwrap_err();
        match err {
            FeatureError::InvalidParameter { name: "stub", .. } => {}
            other => panic!("expected stub error, got {other:?}"),
        }
    }

    #[test]
    fn box_primitive_appends_one_spine_feature() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);

        let call = FeatureCall::new(
            "box",
            id(1),
            json!({
                "half_extents": [10.0, 20.0, 30.0],
                "translate": [100.0, 0.0, 0.0],
                "op": "union"
            }),
        );
        registry.regenerate(&mut ctx, &[call]).unwrap();

        assert_eq!(doc.features.len(), 1);
        let f = &doc.features[0];
        assert_eq!(f.id, id(1));
        assert_eq!(f.op, BoolOp::Union);
        assert_eq!(f.transform.translate, [100.0, 0.0, 0.0]);
        match &f.primitive {
            Primitive::Box {
                center,
                half_extents,
            } => {
                assert_eq!(*center, [0.0, 0.0, 0.0]);
                assert_eq!(*half_extents, [10.0, 20.0, 30.0]);
            }
            other => panic!("expected Primitive::Box, got {other:?}"),
        }
    }

    #[test]
    fn sphere_and_cylinder_also_land_on_the_spine() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);

        let calls = vec![
            FeatureCall::new("sphere", id(1), json!({ "radius": 5.0 })),
            FeatureCall::new(
                "cylinder",
                id(2),
                json!({
                    "radius": 2.0,
                    "half_height": 10.0,
                    "op": "difference"
                }),
            ),
        ];
        registry.regenerate(&mut ctx, &calls).unwrap();

        assert_eq!(doc.features.len(), 2);
        assert!(matches!(
            doc.features[0].primitive,
            Primitive::Sphere { radius, .. } if radius == 5.0
        ));
        assert!(matches!(
            doc.features[1].primitive,
            Primitive::Cylinder {
                radius,
                half_height,
                ..
            } if radius == 2.0 && half_height == 10.0
        ));
        assert_eq!(doc.features[1].op, BoolOp::Difference);
    }

    #[test]
    fn unknown_feature_type_is_an_error() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let call = FeatureCall::new("not_a_feature", id(1), json!({}));
        let err = registry.regenerate(&mut ctx, &[call]).unwrap_err();
        assert_eq!(
            err,
            FeatureError::UnknownFeatureType("not_a_feature".into())
        );
        assert!(doc.features.is_empty());
    }

    #[test]
    fn missing_required_parameter_is_an_error() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        // Box without `half_extents`.
        let call = FeatureCall::new("box", id(1), json!({}));
        let err = registry.regenerate(&mut ctx, &[call]).unwrap_err();
        assert_eq!(
            err,
            FeatureError::MissingParameter {
                feature: "box",
                name: "half_extents"
            }
        );
        assert!(doc.features.is_empty());
    }

    #[test]
    fn invalid_vec3_shape_is_an_error() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let call = FeatureCall::new(
            "box",
            id(1),
            json!({ "half_extents": [1.0, 2.0] }),
        );
        let err = registry.regenerate(&mut ctx, &[call]).unwrap_err();
        match err {
            FeatureError::InvalidParameter {
                feature: "box",
                name: "half_extents",
                ..
            } => {}
            other => panic!("expected InvalidParameter, got {other:?}"),
        }
    }

    #[test]
    fn negative_radius_is_an_error() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let call =
            FeatureCall::new("sphere", id(1), json!({ "radius": -3.0 }));
        let err = registry.regenerate(&mut ctx, &[call]).unwrap_err();
        match err {
            FeatureError::InvalidParameter {
                feature: "sphere",
                name: "radius",
                ..
            } => {}
            other => panic!("expected InvalidParameter, got {other:?}"),
        }
    }

    #[test]
    fn regeneration_is_fail_stop() {
        // Build: one good sphere, one bad cylinder (missing half_height),
        // one good box. The sphere must land on the spine, the cylinder
        // must error, and the box must *not* run because of fail-stop.
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let calls = vec![
            FeatureCall::new("sphere", id(1), json!({ "radius": 5.0 })),
            FeatureCall::new("cylinder", id(2), json!({ "radius": 2.0 })),
            FeatureCall::new(
                "box",
                id(3),
                json!({ "half_extents": [1.0, 1.0, 1.0] }),
            ),
        ];
        let err = registry.regenerate(&mut ctx, &calls).unwrap_err();
        assert!(matches!(
            err,
            FeatureError::MissingParameter {
                feature: "cylinder",
                name: "half_height"
            }
        ));
        assert_eq!(
            doc.features.len(),
            1,
            "fail-stop: only the sphere should have landed"
        );
        assert!(matches!(
            doc.features[0].primitive,
            Primitive::Sphere { .. }
        ));
    }

    #[test]
    fn feature_call_round_trips_through_json() {
        let call = FeatureCall::new(
            "box",
            id(7),
            json!({
                "half_extents": [10, 20, 30],
                "translate": [100, 0, 0]
            }),
        );
        let wire = serde_json::to_string(&call).unwrap();
        let parsed: FeatureCall = serde_json::from_str(&wire).unwrap();
        assert_eq!(call, parsed);
    }

    #[test]
    fn feature_call_json_uses_type_not_type_name_field() {
        // The Serde rename `#[serde(rename = "type")]` is the
        // canonical §4.6 wire format; nothing else in the project
        // should ever emit `type_name` on the wire.
        let call =
            FeatureCall::new("box", id(1), json!({ "half_extents": [1, 1, 1] }));
        let wire = serde_json::to_value(&call).unwrap();
        assert_eq!(wire.get("type"), Some(&json!("box")));
        assert!(wire.get("type_name").is_none());
    }

    #[test]
    fn feature_tree_matches_legacy_add_primitive_path() {
        // The pivotal equivalence test: build the same geometry
        // two ways — through the K1-style `Document::allocate_id +
        // features.push` direct path, and through the K4.5 feature
        // registry — and verify the resulting documents are
        // structurally identical. That's the guarantee K4.5 is
        // built on: *the new execution layer is a faithful proxy
        // for the old one, with zero behaviour drift*.
        let mut legacy_doc = Document::default();
        for (kind, params) in [
            ("box", ([10.0, 10.0, 10.0], [0.0, 0.0, 0.0], BoolOp::Union)),
            (
                "box",
                ([5.0, 5.0, 5.0], [80.0, 0.0, 0.0], BoolOp::Difference),
            ),
        ] {
            let id = legacy_doc.allocate_id();
            let (half, translate, op) = params;
            legacy_doc.features.push(SpineFeature {
                id: id.clone(),
                name: format!("Box {id}"),
                op,
                primitive: Primitive::Box {
                    center: [0.0; 3],
                    half_extents: half,
                },
                transform: Transform {
                    translate,
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: 1.0,
                },
            });
            let _ = kind;
        }

        // Same scenario via the registry.
        let mut registry_doc = Document::default();
        let mut ctx = Context::new(&mut registry_doc);
        let registry = FeatureRegistry::with_builtins();
        let calls = vec![
            FeatureCall::new(
                "box",
                id(1),
                json!({
                    "half_extents": [10.0, 10.0, 10.0],
                    "translate": [0.0, 0.0, 0.0],
                    "op": "union"
                }),
            ),
            FeatureCall::new(
                "box",
                id(2),
                json!({
                    "half_extents": [5.0, 5.0, 5.0],
                    "translate": [80.0, 0.0, 0.0],
                    "op": "difference"
                }),
            ),
        ];
        registry.regenerate(&mut ctx, &calls).unwrap();

        // The `next_feature_id` counter differs (the registry path
        // never called `allocate_id`), so compare the feature lists
        // directly rather than the whole document.
        assert_eq!(legacy_doc.features, registry_doc.features);
    }
}
