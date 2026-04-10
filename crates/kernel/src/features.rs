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

use serde_json::Value;

use fielddraft_protocol::{
    BoolOp, Document, Feature as SpineFeature, FeatureId, Primitive, Transform,
};

/// Derive macro that generates `TYPE_NAME`, `type_name()`,
/// `schema_json()` and `parse()` helpers on a feature definition
/// struct. See `fielddraft_features_macros` for the attribute
/// vocabulary — K4.5 step 3 supports `#[feature(type = "...")]`
/// at the struct level and auto-detects field types, every field
/// is implicitly required.
pub use fielddraft_features_macros::FeatureSchema;

/// Re-export of the canonical on-disk feature-call shape. K4.5.4b
/// promoted `FeatureCall` from a kernel-internal helper to a
/// first-class wire type inside [`fielddraft_protocol`] so the
/// Document can eventually store `Vec<FeatureCall>` as its
/// authoritative serialisation form (K4.5.4c+). Existing
/// `crate::features::FeatureCall` imports keep working through
/// this re-export — the move is purely structural.
pub use fielddraft_protocol::FeatureCall;

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
        r.register(Box::new(ExtrudeFeature));
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
//
// These are the reference `FeatureKind` impls for the three K1
// primitives. Their parameter parsing is now driven by the
// `FeatureSchema` derive from `fielddraft-features-macros`
// (K4.5.3), so each `execute` body shrinks to a two-step recipe:
// parse the definition JSON into the typed `{Box,Sphere,Cylinder}
// Def` struct, then assemble a `SpineFeature` record and push it
// onto the document.
//
// The display name still has to be assembled at runtime because
// it depends on the `FeatureId` that's passed into `execute` —
// the derive's `default` attribute can only quote Rust rvalues,
// it can't capture closures over the id. That one extra line at
// the top of each execute function is the only hand-written
// parameter logic that remains in these three built-ins.

/// `box` — K1 axis-aligned box primitive.
#[derive(FeatureSchema)]
#[schema(type = "box")]
pub struct BoxDef {
    pub half_extents: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(name = "Op", default = ::fielddraft_protocol::BoolOp::Union)]
    pub op: BoolOp,
}

pub struct BoxPrimitive;

impl FeatureKind for BoxPrimitive {
    fn type_name(&self) -> &'static str {
        BoxDef::TYPE_NAME
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = BoxDef::parse(def)?;
        let name = optional_display_name(def, || format!("Box {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op: d.op,
            primitive: Primitive::Box {
                center: d.center,
                half_extents: d.half_extents,
            },
            transform: Transform {
                translate: d.translate,
                rotation,
                scale,
            },
        });
        Ok(())
    }
}

/// `sphere` — K1 sphere primitive.
#[derive(FeatureSchema)]
#[schema(type = "sphere")]
pub struct SphereDef {
    pub radius: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::Union)]
    pub op: BoolOp,
}

pub struct SpherePrimitive;

impl FeatureKind for SpherePrimitive {
    fn type_name(&self) -> &'static str {
        SphereDef::TYPE_NAME
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SphereDef::parse(def)?;
        positive_f32(d.radius, SphereDef::TYPE_NAME, "radius")?;
        let name = optional_display_name(def, || format!("Sphere {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op: d.op,
            primitive: Primitive::Sphere {
                center: d.center,
                radius: d.radius,
            },
            transform: Transform {
                translate: d.translate,
                rotation,
                scale,
            },
        });
        Ok(())
    }
}

/// `cylinder` — K1 Y-axis-aligned cylinder primitive.
#[derive(FeatureSchema)]
#[schema(type = "cylinder")]
pub struct CylinderDef {
    pub radius: f32,
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::Union)]
    pub op: BoolOp,
}

pub struct CylinderPrimitive;

impl FeatureKind for CylinderPrimitive {
    fn type_name(&self) -> &'static str {
        CylinderDef::TYPE_NAME
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = CylinderDef::parse(def)?;
        positive_f32(d.radius, CylinderDef::TYPE_NAME, "radius")?;
        positive_f32(d.half_height, CylinderDef::TYPE_NAME, "half_height")?;
        let name = optional_display_name(def, || format!("Cylinder {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op: d.op,
            primitive: Primitive::Cylinder {
                center: d.center,
                radius: d.radius,
                half_height: d.half_height,
            },
            transform: Transform {
                translate: d.translate,
                rotation,
                scale,
            },
        });
        Ok(())
    }
}

// ─── ExtrudeDef (K4.5.8: Rust-native reference implementation) ──
//
// ExtrudeDef is the first K4.5 reference feature that exercises
// the full parameter vocabulary the K4.5.4f derive learned —
// `Query`, `Length`, `BoolOp`, `f32` and `[f32; 3]` all appear in
// one definition struct, so the §4.6 promise that "the derive
// covers the kernel runtime types" is demonstrated end-to-end
// rather than in unit-test fixtures only.
//
// ## Execute contract
//
// The protocol's `Primitive` enum doesn't carry an `Extrude`
// variant yet — adding one would ripple through the mesher, the
// DC pass, the BVH and the wire form, which is well out of
// scope for one slice. Instead, K4.5.8's executor **lowers** an
// ExtrudeDef into a plain `Primitive::Box`: the rectangular
// profile (half-width / half-height in X and Y) supplies the
// first two half-extents, and the extrude depth converted to
// millimetres via `Length::to_mm` supplies the Z half-extent
// (depth / 2, so the solid is centred on the extrude plane).
// The result is a real mesh-able feature that K3's dual-
// contouring pipeline handles exactly like any hand-added box
// — the difference is that the spine entry was produced by a
// FeatureCall routed through the registry, not by a direct
// `Kernel::add_primitive` call.
//
// ## What's intentionally simplified vs §4.6
//
// * **`profiles: Query` is accepted but not resolved.** K4.5.7
//   ships the query parser and a version cache, but there's no
//   sketch-producing feature upstream whose profile a query
//   could point at yet. The executor stores the query string
//   for round-trip fidelity and moves on. A later slice will
//   resolve the query into a real `Sketch2d` once sketch
//   features exist, at which point the `profile_half_x` /
//   `profile_half_y` fields can be removed in favour of the
//   sketch's own extent.
// * **Only the Z extrude direction is supported.** The §4.6
//   example includes `second_direction: bool` and
//   `depth2: Option<Length>`; `Option<T>` isn't a derive-
//   supported field type yet, and the two-sided extrude has no
//   semantic value while the mesh-level representation stays a
//   simple centred box. Both fields are deferred.
// * **Operation is `BoolOp`, not the full §4.6 "new / add /
//   remove / intersect" operation vocabulary.** The extra
//   "new" case (create a separate body) needs body-id
//   plumbing that's not yet wired through the registry path.

/// `extrude` — K4.5.8 Rust-native reference feature.
#[derive(FeatureSchema)]
#[schema(type = "extrude")]
pub struct ExtrudeDef {
    /// Sketch profile this extrude consumes. Evaluated lazily
    /// against the current document so upstream edits don't
    /// break downstream extrudes. K4.5.8 parses and stores the
    /// query but doesn't resolve it — see the module docstring.
    pub profiles: ::fielddraft_protocol::Query,

    /// Extrude depth along the local Z axis. Any supported
    /// length unit works; the executor converts to millimetres
    /// via [`Length::to_mm`] before lowering to a box.
    pub depth: ::fielddraft_protocol::Length,

    /// Half-width of the rectangular profile placeholder in
    /// millimetres. Will be derived from the resolved sketch
    /// once [`profiles`] actually routes through the sketch
    /// feature graph.
    pub profile_half_x: f32,

    /// Half-height of the rectangular profile placeholder in
    /// millimetres. Same story as [`profile_half_x`].
    pub profile_half_y: f32,

    /// Boolean op used to fold the extruded body into the
    /// running feature spine. Defaults to [`BoolOp::Union`],
    /// matching the §4.6 convention that a freshly extruded
    /// body adds material unless the author says otherwise.
    #[param(default = ::fielddraft_protocol::BoolOp::Union)]
    pub operation: BoolOp,

    /// Origin of the extruded box in world space. Z is the
    /// extrude plane; the resulting box spans
    /// `[z - depth_mm/2, z + depth_mm/2]`.
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
}

pub struct ExtrudeFeature;

impl FeatureKind for ExtrudeFeature {
    fn type_name(&self) -> &'static str {
        ExtrudeDef::TYPE_NAME
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ExtrudeDef::parse(def)?;
        positive_f32(d.profile_half_x, ExtrudeDef::TYPE_NAME, "profile_half_x")?;
        positive_f32(d.profile_half_y, ExtrudeDef::TYPE_NAME, "profile_half_y")?;

        // Length → mm at the model-unit boundary. The kernel's
        // native distance scale is millimetres, so every feature
        // that consumes a typed runtime length is responsible for
        // converting once, at the edge.
        let depth_mm = d.depth.to_mm();
        if depth_mm <= 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: ExtrudeDef::TYPE_NAME,
                name: "depth",
                message: format!("must be > 0 mm, got {depth_mm}"),
            });
        }

        let name = optional_display_name(def, || format!("Extrude {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);

        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op: d.operation,
            primitive: Primitive::Box {
                center: d.center,
                half_extents: [d.profile_half_x, d.profile_half_y, depth_mm / 2.0],
            },
            transform: Transform {
                translate: [0.0, 0.0, 0.0],
                rotation,
                scale,
            },
        });
        Ok(())
    }
}

// ─── parameter helpers for the K1 primitives ─────────────────────

/// Extract an optional `"name"` string parameter from the
/// definition JSON, falling back to a runtime-derived label that
/// depends on the `FeatureId` being passed into `execute`. The
/// derive can't express this fallback because its `default`
/// attribute quotes Rust rvalues statically — the id isn't in
/// scope at attribute-parse time.
fn optional_display_name(def: &Value, fallback: impl FnOnce() -> String) -> String {
    def.get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(fallback)
}

/// Extract an optional `"rotation"` quaternion (`[x, y, z, w]`) from
/// the definition JSON, falling back to the identity quaternion
/// when missing or malformed. Used by the K1 primitives'
/// `execute` so that a `FeatureCall → Feature` regeneration pass
/// preserves any rotation the user set via `setFeatureTransform`.
/// Without this helper, K4.5.4d's bootstrap would silently reset
/// every rotation to identity.
fn optional_rotation(def: &Value) -> [f32; 4] {
    def.get("rotation")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            if arr.len() != 4 {
                return None;
            }
            let mut out = [0.0_f32; 4];
            for (i, item) in arr.iter().enumerate() {
                out[i] = item.as_f64()? as f32;
            }
            Some(out)
        })
        .unwrap_or([0.0, 0.0, 0.0, 1.0])
}

/// Extract an optional `"scale"` value from the definition JSON,
/// falling back to the identity scale (`1.0`) when missing or
/// malformed. Mirrors [`optional_rotation`] — both close the
/// lossy-round-trip gap in the K4.5.4d bootstrap path.
fn optional_scale(def: &Value) -> f32 {
    def.get("scale")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(1.0)
}

/// Post-parse guard for numeric fields that must be strictly
/// positive. The derive's `f32` slot parser only checks "is it a
/// number"; domain constraints like "radius > 0" are re-checked
/// here so the K1 primitives still reject zero and negative
/// values the same way the K4.5.2 hand-rolled versions did.
fn positive_f32(
    value: f32,
    feature: &'static str,
    name: &'static str,
) -> Result<(), FeatureError> {
    if value > 0.0 {
        Ok(())
    } else {
        Err(FeatureError::InvalidParameter {
            feature,
            name,
            message: format!("must be > 0, got {value}"),
        })
    }
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
    fn with_builtins_registers_the_k1_primitives_and_k4_5_8_extrude() {
        let registry = FeatureRegistry::with_builtins();
        assert_eq!(registry.len(), 4);
        assert!(registry.get("box").is_some());
        assert!(registry.get("sphere").is_some());
        assert!(registry.get("cylinder").is_some());
        // K4.5.8 lands ExtrudeDef as the fourth built-in.
        assert!(registry.get("extrude").is_some());
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

    // ─── K4.5.3: FeatureSchema derive ──────────────────────────

    /// Tiny fixture that exercises every field type the K4.5.3
    /// derive supports. When a future step grows the derive to
    /// cover `Length`, `Angle`, `Query` and friends, those can
    /// slot in here alongside the existing four.
    #[derive(FeatureSchema, Debug, PartialEq)]
    #[schema(type = "fixture")]
    struct FixtureDef {
        depth: f32,
        origin: [f32; 3],
        enabled: bool,
        label: String,
    }

    #[test]
    fn derive_generates_type_name_constant_and_method() {
        assert_eq!(FixtureDef::TYPE_NAME, "fixture");
        assert_eq!(FixtureDef::type_name(), "fixture");
    }

    #[test]
    fn derive_emits_schema_with_every_field() {
        let schema = FixtureDef::schema_json();
        assert_eq!(schema["type"], json!("fixture"));
        let params = schema["params"]
            .as_array()
            .expect("params must be an array");
        assert_eq!(params.len(), 4);

        let depth = &params[0];
        assert_eq!(depth["key"], json!("depth"));
        assert_eq!(depth["name"], json!("Depth"));
        assert_eq!(depth["type"], json!("f32"));
        assert_eq!(depth["required"], json!(true));

        let origin = &params[1];
        assert_eq!(origin["key"], json!("origin"));
        assert_eq!(origin["type"], json!("vec3"));

        let enabled = &params[2];
        assert_eq!(enabled["key"], json!("enabled"));
        assert_eq!(enabled["type"], json!("bool"));

        let label = &params[3];
        assert_eq!(label["key"], json!("label"));
        assert_eq!(label["type"], json!("string"));
    }

    #[test]
    fn derive_prettifies_multi_word_identifiers() {
        // `half_extents` → `Half extents`: separate words with a
        // space and capitalise only the first word. This is what
        // the FeatureDialog UI will display (K6a).
        #[derive(FeatureSchema)]
        #[schema(type = "multi_word")]
        #[allow(dead_code)]
        struct MultiWordDef {
            half_extents: [f32; 3],
        }
        let schema = MultiWordDef::schema_json();
        let params = schema["params"].as_array().unwrap();
        assert_eq!(params[0]["name"], json!("Half extents"));
    }

    #[test]
    fn derive_parse_accepts_well_formed_json() {
        let def = json!({
            "depth": 10.5,
            "origin": [1.0, 2.0, 3.0],
            "enabled": true,
            "label": "greeting"
        });
        let parsed = FixtureDef::parse(&def).unwrap();
        assert_eq!(
            parsed,
            FixtureDef {
                depth: 10.5,
                origin: [1.0, 2.0, 3.0],
                enabled: true,
                label: "greeting".to_string(),
            }
        );
    }

    #[test]
    fn derive_parse_reports_missing_fields() {
        let def = json!({
            "depth": 10.5,
            "origin": [1.0, 2.0, 3.0],
            "enabled": true
            // missing `label`
        });
        let err = FixtureDef::parse(&def).unwrap_err();
        assert_eq!(
            err,
            FeatureError::MissingParameter {
                feature: "fixture",
                name: "label"
            }
        );
    }

    #[test]
    fn derive_parse_reports_wrong_types() {
        // number where string is expected
        let def = json!({
            "depth": 10.5,
            "origin": [1.0, 2.0, 3.0],
            "enabled": true,
            "label": 42
        });
        let err = FixtureDef::parse(&def).unwrap_err();
        match err {
            FeatureError::InvalidParameter {
                feature: "fixture",
                name: "label",
                ..
            } => {}
            other => panic!("expected InvalidParameter on `label`, got {other:?}"),
        }
    }

    #[test]
    fn derive_parse_reports_wrong_vec3_shape() {
        let def = json!({
            "depth": 10.5,
            "origin": [1.0, 2.0], // only two elements
            "enabled": true,
            "label": "greeting"
        });
        let err = FixtureDef::parse(&def).unwrap_err();
        match err {
            FeatureError::InvalidParameter {
                feature: "fixture",
                name: "origin",
                ..
            } => {}
            other => panic!("expected InvalidParameter on `origin`, got {other:?}"),
        }
    }

    #[test]
    fn derive_parse_rejects_non_number_vec3_element() {
        let def = json!({
            "depth": 10.5,
            "origin": [1.0, "oops", 3.0],
            "enabled": true,
            "label": "greeting"
        });
        let err = FixtureDef::parse(&def).unwrap_err();
        match err {
            FeatureError::InvalidParameter {
                feature: "fixture",
                name: "origin",
                ..
            } => {}
            other => panic!("expected InvalidParameter on `origin`, got {other:?}"),
        }
    }

    // ─── K4.5.4a: #[param] attribute ──────────────────────────

    /// Every field carries a default or a name override, so the
    /// K4.5.4a attribute vocabulary gets exercised end-to-end.
    #[derive(FeatureSchema, Debug, PartialEq)]
    #[schema(type = "param_fixture")]
    #[allow(dead_code)]
    struct ParamFixture {
        #[param(name = "Pretty Depth", default = 5.0)]
        depth: f32,
        #[param(default = [1.0, 2.0, 3.0])]
        origin: [f32; 3],
        #[param(default = ::fielddraft_protocol::BoolOp::Difference)]
        op: BoolOp,
    }

    #[test]
    fn param_name_attribute_overrides_display_name() {
        let schema = ParamFixture::schema_json();
        let params = schema["params"].as_array().unwrap();
        let depth = &params[0];
        assert_eq!(depth["key"], json!("depth"));
        assert_eq!(depth["name"], json!("Pretty Depth"));
        // Other fields keep the auto-prettified snake_case name.
        assert_eq!(params[1]["name"], json!("Origin"));
        assert_eq!(params[2]["name"], json!("Op"));
    }

    #[test]
    fn param_default_marks_field_as_optional_in_schema() {
        let schema = ParamFixture::schema_json();
        let params = schema["params"].as_array().unwrap();
        // Every field has a default → required: false.
        for p in params {
            assert_eq!(p["required"], json!(false), "param {:?}", p);
        }
    }

    #[test]
    fn param_default_falls_back_on_missing_keys() {
        // Empty JSON → every field takes its default.
        let parsed = ParamFixture::parse(&json!({})).unwrap();
        assert_eq!(
            parsed,
            ParamFixture {
                depth: 5.0,
                origin: [1.0, 2.0, 3.0],
                op: BoolOp::Difference,
            }
        );
    }

    #[test]
    fn param_default_is_ignored_when_key_is_present() {
        let parsed = ParamFixture::parse(&json!({
            "depth": 42.0,
            "origin": [9.0, 9.0, 9.0],
            "op": "intersect"
        }))
        .unwrap();
        assert_eq!(
            parsed,
            ParamFixture {
                depth: 42.0,
                origin: [9.0, 9.0, 9.0],
                op: BoolOp::Intersect,
            }
        );
    }

    #[test]
    fn param_default_fields_still_validate_wrong_types() {
        // Explicit wrong type must still error — the default only
        // kicks in for missing / null slots.
        let err = ParamFixture::parse(&json!({ "depth": "nope" })).unwrap_err();
        match err {
            FeatureError::InvalidParameter {
                feature: "param_fixture",
                name: "depth",
                ..
            } => {}
            other => panic!("expected InvalidParameter on `depth`, got {other:?}"),
        }
    }

    #[test]
    fn param_default_null_slot_falls_back_to_default() {
        // `null` must behave the same as a missing key.
        let parsed = ParamFixture::parse(&json!({ "depth": null })).unwrap();
        assert_eq!(parsed.depth, 5.0);
    }

    #[test]
    fn param_boolop_accepts_all_three_operator_strings() {
        for (input, expected) in [
            ("union", BoolOp::Union),
            ("difference", BoolOp::Difference),
            ("intersect", BoolOp::Intersect),
        ] {
            let def = json!({ "op": input });
            let parsed = ParamFixture::parse(&def).unwrap();
            assert_eq!(parsed.op, expected);
        }
    }

    #[test]
    fn param_boolop_rejects_unknown_operator() {
        let def = json!({ "op": "totally_bogus" });
        let err = ParamFixture::parse(&def).unwrap_err();
        match err {
            FeatureError::InvalidOp {
                feature: "param_fixture",
                value,
            } => assert_eq!(value, "totally_bogus"),
            other => panic!("expected InvalidOp, got {other:?}"),
        }
    }

    #[test]
    fn box_def_schema_matches_the_k1_parameter_surface() {
        // Regression: the derive-generated schema for the box
        // primitive should match the K4.5.2 hand-rolled surface
        // exactly — four params, half_extents required, the
        // other three optional via defaults.
        let schema = BoxDef::schema_json();
        assert_eq!(schema["type"], json!("box"));
        let params = schema["params"].as_array().unwrap();
        assert_eq!(params.len(), 4);

        assert_eq!(params[0]["key"], json!("half_extents"));
        assert_eq!(params[0]["required"], json!(true));

        assert_eq!(params[1]["key"], json!("center"));
        assert_eq!(params[1]["required"], json!(false));

        assert_eq!(params[2]["key"], json!("translate"));
        assert_eq!(params[2]["required"], json!(false));

        assert_eq!(params[3]["key"], json!("op"));
        assert_eq!(params[3]["name"], json!("Op"));
        assert_eq!(params[3]["required"], json!(false));
    }

    #[test]
    fn derive_parse_reports_missing_f32_field() {
        let def = json!({
            "origin": [1.0, 2.0, 3.0],
            "enabled": true,
            "label": "greeting"
        });
        let err = FixtureDef::parse(&def).unwrap_err();
        assert_eq!(
            err,
            FeatureError::MissingParameter {
                feature: "fixture",
                name: "depth"
            }
        );
    }

    // ─── legacy equivalence (K4.5.2) ───────────────────────────

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

    // ─── K4.5.4f: Length / Angle / Query / generic enum ───────

    /// K4.5.4f fixture that exercises every new param type the
    /// derive learned in this slice: `Length`, `Angle`, `Query`,
    /// plus a user-defined enum that falls through to the generic
    /// `serde_json::from_value` path — standing in for the §4.6
    /// "Enum" parameter slot.
    #[derive(
        Debug,
        PartialEq,
        ::serde::Serialize,
        ::serde::Deserialize,
    )]
    #[serde(rename_all = "snake_case")]
    enum Finish {
        Anodised,
        Polished,
        Brushed,
    }

    #[derive(FeatureSchema, Debug, PartialEq)]
    #[schema(type = "runtime_fixture")]
    #[allow(dead_code)]
    struct RuntimeFixture {
        depth: ::fielddraft_protocol::Length,
        sweep: ::fielddraft_protocol::Angle,
        seed: ::fielddraft_protocol::Query,
        finish: Finish,
    }

    #[test]
    fn runtime_fixture_schema_reports_typed_param_names() {
        let schema = RuntimeFixture::schema_json();
        let params = schema["params"].as_array().unwrap();
        assert_eq!(params.len(), 4);
        assert_eq!(params[0]["key"], json!("depth"));
        assert_eq!(params[0]["type"], json!("length"));
        assert_eq!(params[1]["key"], json!("sweep"));
        assert_eq!(params[1]["type"], json!("angle"));
        assert_eq!(params[2]["key"], json!("seed"));
        assert_eq!(params[2]["type"], json!("query"));
        // The user-defined enum lands in the generic fallback
        // slot; the schema hint is `"generic"`.
        assert_eq!(params[3]["key"], json!("finish"));
        assert_eq!(params[3]["type"], json!("generic"));
    }

    #[test]
    fn runtime_fixture_round_trips_all_new_types() {
        let def = json!({
            "depth": {"value": 25.0, "unit": "mm"},
            "sweep": {"value": 90.0, "unit": "deg"},
            "seed": "qCreatedBy(box_1)",
            "finish": "polished"
        });
        let parsed = RuntimeFixture::parse(&def).unwrap();
        assert_eq!(parsed.depth, ::fielddraft_protocol::Length::mm(25.0));
        assert_eq!(parsed.sweep, ::fielddraft_protocol::Angle::deg(90.0));
        assert_eq!(parsed.seed.as_str(), "qCreatedBy(box_1)");
        assert_eq!(parsed.finish, Finish::Polished);
    }

    #[test]
    fn length_param_rejects_missing_unit_field() {
        // serde's `from_value` errors out when the Length struct
        // is missing a required field — the derive maps that to
        // InvalidParameter with the offending field name.
        let def = json!({
            "depth": {"value": 10.0},
            "sweep": {"value": 0.0, "unit": "rad"},
            "seed": "q",
            "finish": "brushed"
        });
        let err = RuntimeFixture::parse(&def).unwrap_err();
        match err {
            FeatureError::InvalidParameter {
                feature: "runtime_fixture",
                name: "depth",
                ..
            } => {}
            other => panic!("expected InvalidParameter on `depth`, got {other:?}"),
        }
    }

    #[test]
    fn angle_param_rejects_unknown_unit() {
        let def = json!({
            "depth": {"value": 1.0, "unit": "mm"},
            "sweep": {"value": 100.0, "unit": "gradian"},
            "seed": "q",
            "finish": "brushed"
        });
        let err = RuntimeFixture::parse(&def).unwrap_err();
        match err {
            FeatureError::InvalidParameter {
                feature: "runtime_fixture",
                name: "sweep",
                ..
            } => {}
            other => panic!("expected InvalidParameter on `sweep`, got {other:?}"),
        }
    }

    #[test]
    fn query_param_rejects_non_string() {
        let def = json!({
            "depth": {"value": 1.0, "unit": "mm"},
            "sweep": {"value": 0.0, "unit": "rad"},
            "seed": 42,
            "finish": "brushed"
        });
        let err = RuntimeFixture::parse(&def).unwrap_err();
        match err {
            FeatureError::InvalidParameter {
                feature: "runtime_fixture",
                name: "seed",
                ..
            } => {}
            other => panic!("expected InvalidParameter on `seed`, got {other:?}"),
        }
    }

    #[test]
    fn generic_enum_fallback_rejects_unknown_variant() {
        let def = json!({
            "depth": {"value": 1.0, "unit": "mm"},
            "sweep": {"value": 0.0, "unit": "rad"},
            "seed": "q",
            "finish": "chromed"
        });
        let err = RuntimeFixture::parse(&def).unwrap_err();
        match err {
            FeatureError::InvalidParameter {
                feature: "runtime_fixture",
                name: "finish",
                ..
            } => {}
            other => panic!("expected InvalidParameter on `finish`, got {other:?}"),
        }
    }

    /// A second fixture that proves `#[param(default = ...)]`
    /// still works when the field type is a runtime typed param
    /// (K4.5.4f × K4.5.4a interaction).
    #[derive(FeatureSchema, Debug, PartialEq)]
    #[schema(type = "runtime_defaults")]
    #[allow(dead_code)]
    struct RuntimeDefaults {
        #[param(default = ::fielddraft_protocol::Length::mm(5.0))]
        depth: ::fielddraft_protocol::Length,
        #[param(default = ::fielddraft_protocol::Angle::deg(0.0))]
        sweep: ::fielddraft_protocol::Angle,
    }

    #[test]
    fn runtime_defaults_apply_on_missing_keys() {
        let parsed = RuntimeDefaults::parse(&json!({})).unwrap();
        assert_eq!(parsed.depth, ::fielddraft_protocol::Length::mm(5.0));
        assert_eq!(parsed.sweep, ::fielddraft_protocol::Angle::deg(0.0));
    }

    #[test]
    fn runtime_defaults_marked_optional_in_schema() {
        let schema = RuntimeDefaults::schema_json();
        let params = schema["params"].as_array().unwrap();
        for p in params {
            assert_eq!(p["required"], json!(false), "param {p:?}");
        }
    }

    // ─── K4.5.8: ExtrudeDef reference implementation ──────────

    /// Convenience: build an extrude definition JSON with the
    /// K4.5.8 schema. The `depth` is expressed as a
    /// `{value, unit}` object so the `Length` parser path runs.
    fn extrude_def(
        profile_query: &str,
        depth_mm: f32,
        half_x: f32,
        half_y: f32,
    ) -> serde_json::Value {
        json!({
            "profiles": profile_query,
            "depth": { "value": depth_mm, "unit": "mm" },
            "profile_half_x": half_x,
            "profile_half_y": half_y,
        })
    }

    #[test]
    fn extrude_def_schema_has_every_runtime_type() {
        let schema = ExtrudeDef::schema_json();
        assert_eq!(schema["type"], json!("extrude"));
        let params = schema["params"].as_array().unwrap();
        assert_eq!(params.len(), 6);

        // Each key / type pair documents the K4.5.4f vocabulary
        // demonstrated in a single definition.
        let map: std::collections::HashMap<&str, &serde_json::Value> =
            params.iter().map(|p| (p["key"].as_str().unwrap(), p)).collect();
        assert_eq!(map["profiles"]["type"], json!("query"));
        assert_eq!(map["profiles"]["required"], json!(true));
        assert_eq!(map["depth"]["type"], json!("length"));
        assert_eq!(map["depth"]["required"], json!(true));
        assert_eq!(map["profile_half_x"]["type"], json!("f32"));
        assert_eq!(map["profile_half_y"]["type"], json!("f32"));
        assert_eq!(map["operation"]["type"], json!("bool_op"));
        assert_eq!(map["operation"]["required"], json!(false));
        assert_eq!(map["center"]["type"], json!("vec3"));
        assert_eq!(map["center"]["required"], json!(false));
    }

    #[test]
    fn extrude_execute_lowers_to_a_box_with_depth_as_z_half_extent() {
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let feature = ExtrudeFeature;
        feature
            .execute(
                &mut ctx,
                &id(1),
                &extrude_def("qCreatedBy(sketch_1)", 40.0, 5.0, 7.0),
            )
            .unwrap();

        assert_eq!(doc.features.len(), 1);
        let f = &doc.features[0];
        assert_eq!(f.id, id(1));
        assert_eq!(f.op, BoolOp::Union);
        match &f.primitive {
            Primitive::Box { center, half_extents } => {
                assert_eq!(*center, [0.0, 0.0, 0.0]);
                // profile_half_x / profile_half_y carry through
                // unchanged; depth (40 mm) lowers to a Z
                // half-extent of 20 mm so the box is centred on
                // the extrude plane.
                assert_eq!(*half_extents, [5.0, 7.0, 20.0]);
            }
            other => panic!("expected Primitive::Box, got {other:?}"),
        }
    }

    #[test]
    fn extrude_converts_non_mm_depth_to_model_units() {
        // 5 cm should land as z half-extent = 25 mm.
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let feature = ExtrudeFeature;
        let def = json!({
            "profiles": "qCreatedBy(s)",
            "depth": { "value": 5.0, "unit": "cm" },
            "profile_half_x": 1.0,
            "profile_half_y": 1.0,
        });
        feature.execute(&mut ctx, &id(1), &def).unwrap();
        let f = &doc.features[0];
        match &f.primitive {
            Primitive::Box { half_extents, .. } => {
                assert!((half_extents[2] - 25.0).abs() < 1e-5);
            }
            other => panic!("expected Primitive::Box, got {other:?}"),
        }
    }

    #[test]
    fn extrude_honours_center_and_operation_parameters() {
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let feature = ExtrudeFeature;
        let def = json!({
            "profiles": "qCreatedBy(s)",
            "depth": { "value": 10.0, "unit": "mm" },
            "profile_half_x": 2.0,
            "profile_half_y": 3.0,
            "operation": "difference",
            "center": [100.0, 50.0, 0.0],
        });
        feature.execute(&mut ctx, &id(7), &def).unwrap();
        let f = &doc.features[0];
        assert_eq!(f.op, BoolOp::Difference);
        match &f.primitive {
            Primitive::Box { center, half_extents } => {
                assert_eq!(*center, [100.0, 50.0, 0.0]);
                assert_eq!(half_extents[2], 5.0);
            }
            other => panic!("expected Primitive::Box, got {other:?}"),
        }
    }

    #[test]
    fn extrude_rejects_non_positive_profile_and_depth() {
        let feature = ExtrudeFeature;
        let mut doc = empty_ctx();

        // Zero half_x
        {
            let mut ctx = Context::new(&mut doc);
            let def = extrude_def("q", 10.0, 0.0, 1.0);
            let err = feature.execute(&mut ctx, &id(1), &def).unwrap_err();
            assert!(matches!(
                err,
                FeatureError::InvalidParameter {
                    feature: "extrude",
                    name: "profile_half_x",
                    ..
                }
            ));
        }
        // Negative depth (encoded via a negative value field)
        {
            let mut ctx = Context::new(&mut doc);
            let def = json!({
                "profiles": "q",
                "depth": { "value": -5.0, "unit": "mm" },
                "profile_half_x": 1.0,
                "profile_half_y": 1.0,
            });
            let err = feature.execute(&mut ctx, &id(1), &def).unwrap_err();
            assert!(matches!(
                err,
                FeatureError::InvalidParameter {
                    feature: "extrude",
                    name: "depth",
                    ..
                }
            ));
        }
    }

    #[test]
    fn extrude_is_reachable_via_the_builtin_registry() {
        // End-to-end: look up "extrude" on the default registry
        // and regenerate a single call. This is the path any
        // document load or worker regeneration would take.
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let registry = FeatureRegistry::with_builtins();
        let calls = vec![FeatureCall::new(
            "extrude",
            id(1),
            extrude_def("qCreatedBy(sketch_1)", 12.0, 3.0, 4.0),
        )];
        registry.regenerate(&mut ctx, &calls).unwrap();
        assert_eq!(doc.features.len(), 1);
        match &doc.features[0].primitive {
            Primitive::Box { half_extents, .. } => {
                assert_eq!(*half_extents, [3.0, 4.0, 6.0]);
            }
            other => panic!("expected Primitive::Box, got {other:?}"),
        }
    }

    /// Rust-level stand-in for the §4.6 `test_extrude_chain` JS
    /// example. The JS body hangs on `fd.registerFeature`,
    /// `fd.sketch`, and `fd.extrude` — none of which are wired
    /// through the worker yet. This Rust test mirrors the
    /// behaviour that `test_extrude_chain` will eventually drive
    /// end-to-end: build `count` extrude calls in a loop, each
    /// with a depth that grows linearly, regenerate the whole
    /// chain through the registry, and verify the resulting
    /// spine. Once the JS-side `fd.registerFeature` path lands,
    /// an equivalent Playwright-layer test will run alongside
    /// this one and compare outputs.
    #[test]
    fn extrude_chain_regenerates_five_features_with_growing_depths() {
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let registry = FeatureRegistry::with_builtins();

        const COUNT: u32 = 5;
        const SPACING_MM: f32 = 20.0;
        let calls: Vec<FeatureCall> = (0..COUNT)
            .map(|i| {
                let depth_mm = ((i + 1) as f32) * 5.0;
                FeatureCall::new(
                    "extrude",
                    id(i + 1),
                    json!({
                        "profiles": format!("qCreatedBy(sketch_{i})"),
                        "depth": { "value": depth_mm, "unit": "mm" },
                        "profile_half_x": 5.0,
                        "profile_half_y": 5.0,
                        "center": [(i as f32) * SPACING_MM, 0.0, 0.0],
                        "operation": if i == 0 { "union" } else { "union" },
                    }),
                )
            })
            .collect();

        registry.regenerate(&mut ctx, &calls).unwrap();

        assert_eq!(doc.features.len(), COUNT as usize);
        for (i, f) in doc.features.iter().enumerate() {
            let expected_depth = ((i as u32 + 1) as f32) * 5.0;
            let expected_center_x = (i as f32) * SPACING_MM;
            match &f.primitive {
                Primitive::Box { center, half_extents } => {
                    assert_eq!(*center, [expected_center_x, 0.0, 0.0]);
                    assert!(
                        (half_extents[2] - expected_depth / 2.0).abs() < 1e-5,
                        "step {i}: half-z expected {}, got {}",
                        expected_depth / 2.0,
                        half_extents[2]
                    );
                }
                other => panic!("expected Primitive::Box, got {other:?}"),
            }
        }
    }

    #[test]
    fn extrude_query_field_round_trips_through_the_parser() {
        // The profiles field is accepted as an opaque Query
        // string in K4.5.8. Verify that well-formed query
        // expressions from K4.5.7's vocabulary parse into the
        // definition without rejection.
        let feature = ExtrudeFeature;
        for q in [
            "qCreatedBy(sketch_1)",
            "qFaceOf(42)",
            "qEdgeBetween(1, 2)",
        ] {
            let mut doc = empty_ctx();
            let mut ctx = Context::new(&mut doc);
            feature
                .execute(&mut ctx, &id(1), &extrude_def(q, 10.0, 1.0, 1.0))
                .unwrap_or_else(|e| panic!("query `{q}` failed: {e}"));
        }
    }
}
