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
    BoolOp, Document, Feature as SpineFeature, FeatureId, MultiQuery, Primitive, Transform,
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
    /// K6b.1: solved sketch state per sketch feature id.
    pub sketches: HashMap<FeatureId, crate::sketch_features::SketchData>,
    /// K10.4: edge fillets collected during regen, applied as a
    /// post-processing pass on the CsgNode tree.
    pub edge_fillets: Vec<crate::csg::EdgeFillet>,
}

impl<'a> Context<'a> {
    pub fn new(document: &'a mut Document) -> Self {
        Self {
            document,
            sketches: HashMap::new(),
            edge_fillets: Vec::new(),
        }
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

    /// K6a: the JSON schema shape the K4.5.4a `FeatureSchema`
    /// derive generates for the definition struct this feature
    /// kind consumes. Returned as `serde_json::Value` so the
    /// wasm-bindgen surface can serialise it straight through
    /// to the TypeScript FeatureDialog generator without the
    /// kernel needing to know about the widget layer.
    fn schema_json(&self) -> Value;

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
        r.register(Box::new(RevolveFeature));
        r.register(Box::new(HoleFeature));
        r.register(Box::new(TorusPrimitive));
        r.register(Box::new(CapsulePrimitive));
        r.register(Box::new(EllipsoidPrimitive));
        r.register(Box::new(HexPrismPrimitive));
        r.register(Box::new(ConePrimitive));
        r.register(Box::new(ThreadFeature));
        r.register(Box::new(MateConnectorFeature));
        r.register(Box::new(SweepFeature));
        r.register(Box::new(FilletEdgeFeature));
        r.register(Box::new(ImportedMeshFeature));
        r.register(Box::new(GyroidFeature));
        r.register(Box::new(LinearPatternFeature));
        r.register(Box::new(CircularPatternFeature));
        r.register(Box::new(MirrorFeature));
        r.register(Box::new(crate::sketch_features::SketchFeature::new()));
        r.register(Box::new(TransformFeature));
        r.register(Box::new(BooleanFeature));
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

    /// K6a: list every type name the registry knows about. The
    /// order is hash-map-defined and must not be relied on by
    /// callers that need a stable ordering (the FeatureDialog
    /// generator sorts the result before rendering).
    pub fn type_names(&self) -> Vec<String> {
        self.features.keys().cloned().collect()
    }

    /// K6a: look up a feature kind's schema by type name. Returns
    /// the raw [`Value`] the `FeatureSchema` derive emits —
    /// wasm-bindgen callers re-serialise it as a string before
    /// handing it across the JS boundary.
    pub fn schema_for(&self, type_name: &str) -> Option<Value> {
        self.features.get(type_name).map(|f| f.schema_json())
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
        // K8.11: snapshot the variables table once per
        // regenerate — execute paths don't mutate it, and
        // cloning here means the param resolver can borrow
        // without tangling with the `ctx.document` mutable
        // borrow.
        let variables = ctx.document.variables.clone();
        for call in calls {
            // K8.7: suppressed calls drop out of the tree —
            // their `execute` never runs, so no spine/body
            // contribution and no sketch/cache side effects.
            if call.suppressed {
                continue;
            }
            let feature = self
                .features
                .get(&call.type_name)
                .ok_or_else(|| FeatureError::UnknownFeatureType(call.type_name.clone()))?;
            // K8.11: resolve any formula-valued params (e.g.
            // `{value: "=width/2", unit: "mm"}`) against the
            // document variables before handing them off to
            // `execute`. Features see plain numeric values and
            // don't need to know the formula engine exists.
            let mut resolved = call.params.clone();
            crate::expr::resolve_params_with_variables(&mut resolved, &variables)
                .map_err(|e| FeatureError::InvalidParameter {
                    feature: "<formula>",
                    name: "params",
                    message: format!("{} (in {})", e, call.type_name),
                })?;
            feature.execute(ctx, &call.id, &resolved)?;
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
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct BoxPrimitive;

impl FeatureKind for BoxPrimitive {
    fn type_name(&self) -> &'static str {
        BoxDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        BoxDef::schema_json()
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
            blend_radius: d.blend_radius,
            chamfer_size: d.chamfer_size,
            twist_rate: d.twist_rate,
            bend_rate: d.bend_rate,
            displace_depth: d.displace_depth,
            displace_freq: d.displace_freq,
            shell_thickness: d.shell_thickness,
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
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct SpherePrimitive;

impl FeatureKind for SpherePrimitive {
    fn type_name(&self) -> &'static str {
        SphereDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SphereDef::schema_json()
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
            blend_radius: d.blend_radius,
            chamfer_size: d.chamfer_size,
            twist_rate: d.twist_rate,
            bend_rate: d.bend_rate,
            displace_depth: d.displace_depth,
            displace_freq: d.displace_freq,
            shell_thickness: d.shell_thickness,
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
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct CylinderPrimitive;

impl FeatureKind for CylinderPrimitive {
    fn type_name(&self) -> &'static str {
        CylinderDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        CylinderDef::schema_json()
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
            blend_radius: d.blend_radius,
            chamfer_size: d.chamfer_size,
            twist_rate: d.twist_rate,
            bend_rate: d.bend_rate,
            displace_depth: d.displace_depth,
            displace_freq: d.displace_freq,
            shell_thickness: d.shell_thickness,
        });
        Ok(())
    }
}

// ─── Cone (truncated cone / frustum) ─────────────────────────────

/// `cone` — truncated cone along the Y axis.
#[derive(FeatureSchema)]
#[schema(type = "cone")]
pub struct ConeDef {
    pub r_bottom: f32,
    pub r_top: f32,
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::Union)]
    pub op: BoolOp,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct ConePrimitive;

impl FeatureKind for ConePrimitive {
    fn type_name(&self) -> &'static str {
        ConeDef::TYPE_NAME
    }
    fn schema_json(&self) -> Value {
        ConeDef::schema_json()
    }
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ConeDef::parse(def)?;
        positive_f32(d.half_height, ConeDef::TYPE_NAME, "half_height")?;
        let name = optional_display_name(def, || format!("Cone {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op: d.op,
            primitive: Primitive::Cone {
                r_bottom: d.r_bottom,
                r_top: d.r_top,
                half_height: d.half_height,
            },
            transform: Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            blend_radius: d.blend_radius,
            chamfer_size: d.chamfer_size,
            twist_rate: d.twist_rate,
            bend_rate: d.bend_rate,
            displace_depth: d.displace_depth,
            displace_freq: d.displace_freq,
            shell_thickness: d.shell_thickness,
        });
        Ok(())
    }
}

// ─── K12: Thread ─────────────────────────────────────────────────

/// `thread` — metric-style screw thread via direct helix SDF.
#[derive(FeatureSchema)]
#[schema(type = "thread")]
pub struct ThreadDef {
    /// Nominal (outer) radius in mm.
    pub nominal_radius: f32,
    /// Pitch — distance per revolution in mm.
    pub pitch: f32,
    /// Thread depth in mm (groove amplitude).
    pub depth: f32,
    /// Half the total bolt length along Y.
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::Union)]
    pub op: BoolOp,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
}

pub struct ThreadFeature;

impl FeatureKind for ThreadFeature {
    fn type_name(&self) -> &'static str {
        ThreadDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        ThreadDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ThreadDef::parse(def)?;
        positive_f32(d.nominal_radius, ThreadDef::TYPE_NAME, "nominal_radius")?;
        positive_f32(d.pitch, ThreadDef::TYPE_NAME, "pitch")?;
        positive_f32(d.depth, ThreadDef::TYPE_NAME, "depth")?;
        positive_f32(d.half_height, ThreadDef::TYPE_NAME, "half_height")?;
        let name = optional_display_name(def, || format!("Thread {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op: d.op,
            primitive: Primitive::Thread {
                nominal_radius: d.nominal_radius,
                pitch: d.pitch,
                depth: d.depth,
                half_height: d.half_height,
            },
            transform: Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            blend_radius: d.blend_radius,
            chamfer_size: d.chamfer_size,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        });
        Ok(())
    }
}

// ─── Torus ───────────────────────────────────────────────────────

/// `torus` — Y-axis-aligned torus (donut).
#[derive(FeatureSchema)]
#[schema(type = "torus")]
pub struct TorusDef {
    /// Distance from the axis to the centre of the tube.
    pub major_radius: f32,
    /// Radius of the tube cross-section.
    pub minor_radius: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::Union)]
    pub op: BoolOp,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct TorusPrimitive;

impl FeatureKind for TorusPrimitive {
    fn type_name(&self) -> &'static str {
        TorusDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        TorusDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = TorusDef::parse(def)?;
        positive_f32(d.major_radius, TorusDef::TYPE_NAME, "major_radius")?;
        positive_f32(d.minor_radius, TorusDef::TYPE_NAME, "minor_radius")?;
        let name = optional_display_name(def, || format!("Torus {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op: d.op,
            primitive: Primitive::Torus {
                center: d.center,
                major_radius: d.major_radius,
                minor_radius: d.minor_radius,
            },
            transform: Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            blend_radius: d.blend_radius,
            chamfer_size: d.chamfer_size,
            twist_rate: d.twist_rate,
            bend_rate: d.bend_rate,
            displace_depth: d.displace_depth,
            displace_freq: d.displace_freq,
            shell_thickness: d.shell_thickness,
        });
        Ok(())
    }
}

// ─── Capsule ─────────────────────────────────────────────────────

/// `capsule` — Y-axis-aligned capsule (line-segment swept sphere).
/// `half_height` is the half-length of the inner segment;
/// total height = 2 * half_height + 2 * radius.
#[derive(FeatureSchema)]
#[schema(type = "capsule")]
pub struct CapsuleDef {
    pub radius: f32,
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::Union)]
    pub op: BoolOp,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct CapsulePrimitive;

impl FeatureKind for CapsulePrimitive {
    fn type_name(&self) -> &'static str {
        CapsuleDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        CapsuleDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = CapsuleDef::parse(def)?;
        positive_f32(d.radius, CapsuleDef::TYPE_NAME, "radius")?;
        positive_f32(d.half_height, CapsuleDef::TYPE_NAME, "half_height")?;
        let name = optional_display_name(def, || format!("Capsule {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op: d.op,
            primitive: Primitive::Capsule {
                radius: d.radius,
                half_height: d.half_height,
            },
            transform: Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            blend_radius: d.blend_radius,
            chamfer_size: d.chamfer_size,
            twist_rate: d.twist_rate,
            bend_rate: d.bend_rate,
            displace_depth: d.displace_depth,
            displace_freq: d.displace_freq,
            shell_thickness: d.shell_thickness,
        });
        Ok(())
    }
}

// ─── Ellipsoid ───────────────────────────────────────────────────

/// `ellipsoid` — axis-aligned ellipsoid with independent X/Y/Z radii.
/// SDF is approximate (coordinate scaling + sphere) but accurate enough
/// for DC meshing.
#[derive(FeatureSchema)]
#[schema(type = "ellipsoid")]
pub struct EllipsoidDef {
    #[param(default = [1.0, 1.0, 1.0])]
    pub radii: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::Union)]
    pub op: BoolOp,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct EllipsoidPrimitive;

impl FeatureKind for EllipsoidPrimitive {
    fn type_name(&self) -> &'static str {
        EllipsoidDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        EllipsoidDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = EllipsoidDef::parse(def)?;
        positive_f32(d.radii[0], EllipsoidDef::TYPE_NAME, "radii[0]")?;
        positive_f32(d.radii[1], EllipsoidDef::TYPE_NAME, "radii[1]")?;
        positive_f32(d.radii[2], EllipsoidDef::TYPE_NAME, "radii[2]")?;
        let name = optional_display_name(def, || format!("Ellipsoid {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op: d.op,
            primitive: Primitive::Ellipsoid {
                radii: d.radii,
            },
            transform: Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            blend_radius: d.blend_radius,
            chamfer_size: d.chamfer_size,
            twist_rate: d.twist_rate,
            bend_rate: d.bend_rate,
            displace_depth: d.displace_depth,
            displace_freq: d.displace_freq,
            shell_thickness: d.shell_thickness,
        });
        Ok(())
    }
}

// ─── HexPrism ────────────────────────────────────────────────────

/// `hex_prism` — Y-axis-aligned regular hexagonal prism.
/// `apothem` is the inradius (centre to flat face); circumradius ≈ apothem / 0.866.
#[derive(FeatureSchema)]
#[schema(type = "hex_prism")]
pub struct HexPrismDef {
    pub apothem: f32,
    pub half_height: f32,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::Union)]
    pub op: BoolOp,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct HexPrismPrimitive;

impl FeatureKind for HexPrismPrimitive {
    fn type_name(&self) -> &'static str {
        HexPrismDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        HexPrismDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = HexPrismDef::parse(def)?;
        positive_f32(d.apothem, HexPrismDef::TYPE_NAME, "apothem")?;
        positive_f32(d.half_height, HexPrismDef::TYPE_NAME, "half_height")?;
        let name = optional_display_name(def, || format!("HexPrism {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op: d.op,
            primitive: Primitive::HexPrism {
                apothem: d.apothem,
                half_height: d.half_height,
            },
            transform: Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            blend_radius: d.blend_radius,
            chamfer_size: d.chamfer_size,
            twist_rate: d.twist_rate,
            bend_rate: d.bend_rate,
            displace_depth: d.displace_depth,
            displace_freq: d.displace_freq,
            shell_thickness: d.shell_thickness,
        });
        Ok(())
    }
}

// ─── K13.2: Sweep Feature (straight-line v0) ────────────────────
//
// V0 sweep: extrudes a sketch profile along an arbitrary 3D
// direction vector. Internally lowers to Primitive::Extrude
// with a rotation that aligns the local Z axis to the sweep
// direction. Curved sweeps (arc/spline path) come in v1 via
// mesh_sdf lofting.

/// `sweep` — straight-line sweep along a direction vector.
#[derive(FeatureSchema)]
#[schema(type = "sweep")]
pub struct SweepDef {
    pub profiles: ::fielddraft_protocol::Query,
    pub direction: [f32; 3],
    pub length: ::fielddraft_protocol::Length,
    #[param(default = ::fielddraft_protocol::BoolOp::Union)]
    pub operation: BoolOp,
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
}

pub struct SweepFeature;

impl FeatureKind for SweepFeature {
    fn type_name(&self) -> &'static str {
        SweepDef::TYPE_NAME
    }
    fn schema_json(&self) -> Value {
        SweepDef::schema_json()
    }
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SweepDef::parse(def)?;
        let length_mm = d.length.to_mm();
        if length_mm <= 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: SweepDef::TYPE_NAME,
                name: "length",
                message: format!("must be > 0 mm, got {length_mm}"),
            });
        }

        // Resolve the sketch profile.
        let sketch = (|| -> Option<::fielddraft_protocol::Sketch2d> {
            let expr = crate::queries::parse_query(d.profiles.as_str()).ok()?;
            let crate::queries::QueryExpr::CreatedBy(sketch_id) = expr else {
                return None;
            };
            let sketch_data = ctx.sketches.get(&sketch_id)?;
            let s = crate::sketch_features::build_sketch2d_from_sub_calls(
                &sketch_data.sub_calls,
            );
            if matches!(s, ::fielddraft_protocol::Sketch2d::Empty) {
                None
            } else {
                Some(s)
            }
        })()
        .ok_or(FeatureError::MissingParameter {
            feature: SweepDef::TYPE_NAME,
            name: "profiles",
        })?;

        // Build rotation quaternion that maps Z → direction.
        let dx = d.direction[0];
        let dy = d.direction[1];
        let dz = d.direction[2];
        let dir_len = (dx * dx + dy * dy + dz * dz).sqrt();
        if dir_len < 1e-6 {
            return Err(FeatureError::InvalidParameter {
                feature: SweepDef::TYPE_NAME,
                name: "direction",
                message: "must be a non-zero vector".to_string(),
            });
        }
        let dir = [dx / dir_len, dy / dir_len, dz / dir_len];

        // Rotation from Z=(0,0,1) to dir via axis-angle.
        let z = [0.0_f32, 0.0, 1.0];
        let dot = z[2] * dir[2] + z[1] * dir[1] + z[0] * dir[0];
        let rotation = if dot > 0.9999 {
            [0.0, 0.0, 0.0, 1.0] // already aligned
        } else if dot < -0.9999 {
            [0.0, 1.0, 0.0, 0.0] // 180° around Y
        } else {
            // axis = cross(z, dir), angle = acos(dot)
            let ax = z[1] * dir[2] - z[2] * dir[1];
            let ay = z[2] * dir[0] - z[0] * dir[2];
            let az = z[0] * dir[1] - z[1] * dir[0];
            let axis_len = (ax * ax + ay * ay + az * az).sqrt();
            let half = (dot.clamp(-1.0, 1.0)).acos() * 0.5;
            let s = half.sin() / axis_len;
            [ax * s, ay * s, az * s, half.cos()]
        };

        let name = optional_display_name(def, || format!("Sweep {id}"));

        ctx.document.features.push(SpineFeature {
            id: id.child("0"),
            name,
            op: d.operation,
            primitive: Primitive::Extrude {
                sketch,
                half_depth_z: length_mm / 2.0,
                center: d.center,
            },
            transform: Transform {
                translate: [0.0, 0.0, 0.0],
                rotation,
                scale: [1.0, 1.0, 1.0],
            },
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        });
        Ok(())
    }
}

// ─── K9.6: Mate Connector ────────────────────────────────────────
//
// An attachable local coordinate system on the model surface.
// Stores an origin point + a normal (Z-axis of the CSYS).
// Doesn't produce geometry — it's a datum feature visible
// in the viewport as a small axis triad. Used by assemblies
// (P1) to define attachment points.

/// `mate_connector` — local coordinate system on a surface.
#[derive(FeatureSchema)]
#[schema(type = "mate_connector")]
pub struct MateConnectorDef {
    /// Origin point in world space.
    pub origin: [f32; 3],
    /// Surface normal (Z-axis of the CSYS).
    pub normal: [f32; 3],
}

pub struct MateConnectorFeature;

impl FeatureKind for MateConnectorFeature {
    fn type_name(&self) -> &'static str {
        MateConnectorDef::TYPE_NAME
    }
    fn schema_json(&self) -> Value {
        MateConnectorDef::schema_json()
    }
    fn execute(
        &self,
        _ctx: &mut Context<'_>,
        _id: &FeatureId,
        _def: &Value,
    ) -> Result<(), FeatureError> {
        // Mate connectors don't produce spine geometry.
        // They're datum features that live purely in the
        // feature_calls list for the outliner + viewport
        // to read and render.
        Ok(())
    }
}

// ─── K7.21j: Transform Feature ──────────────────────────────────
//
// Gizmo-driven parametric transform. Stores absolute world-space
// values and REPLACES the target's transform during regeneration.
// Consecutive transforms on the same target are merged by the
// kernel's `apply_gizmo_transform` method so the feature tree
// shows at most one Transform per uninterrupted editing sequence.

#[derive(Debug)]
pub(crate) struct TransformDef {
    target_id: u32,
    translate: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
}

impl Default for TransformDef {
    fn default() -> Self {
        Self {
            target_id: 0,
            translate: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }
    }
}

impl TransformDef {
    const TYPE_NAME: &'static str = "transform";

    fn schema_json() -> Value {
        serde_json::json!({
            "type": Self::TYPE_NAME,
            "params": [
                {"key": "translate", "name": "Translation", "type": "vec3", "required": false},
                {"key": "rotation",  "name": "Rotation",    "type": "vec3", "required": false},
                {"key": "scale",     "name": "Scale",       "type": "vec3", "required": false,
                 "default": [1.0, 1.0, 1.0]},
            ]
        })
    }

    fn read_vec3(params: &Value, key: &str, default: [f32; 3]) -> [f32; 3] {
        if let Some(arr) = params.get(key).and_then(|v| v.as_array()) {
            let c = |i: usize| arr.get(i).and_then(|v| v.as_f64()).unwrap_or(default[i] as f64) as f32;
            [c(0), c(1), c(2)]
        } else {
            default
        }
    }

    fn euler_xyz_to_quat(rx_deg: f32, ry_deg: f32, rz_deg: f32) -> [f32; 4] {
        let (s1, c1) = (rx_deg.to_radians() * 0.5).sin_cos();
        let (s2, c2) = (ry_deg.to_radians() * 0.5).sin_cos();
        let (s3, c3) = (rz_deg.to_radians() * 0.5).sin_cos();
        [
            s1 * c2 * c3 + c1 * s2 * s3,
            c1 * s2 * c3 - s1 * c2 * s3,
            c1 * c2 * s3 + s1 * s2 * c3,
            c1 * c2 * c3 - s1 * s2 * s3,
        ]
    }

    pub(crate) fn quat_to_euler_xyz(qx: f32, qy: f32, qz: f32, qw: f32) -> [f32; 3] {
        let m11 = 1.0 - 2.0 * (qy * qy + qz * qz);
        let m12 = 2.0 * (qx * qy - qw * qz);
        let m13 = 2.0 * (qx * qz + qw * qy);
        let m23 = 2.0 * (qy * qz - qw * qx);
        let m33 = 1.0 - 2.0 * (qx * qx + qy * qy);

        let ry = m13.clamp(-1.0, 1.0).asin();
        let (rx, rz) = if m13.abs() < 0.9999999 {
            ((-m23).atan2(m33), (-m12).atan2(m11))
        } else {
            let m32 = 2.0 * (qy * qz + qw * qx);
            let m22 = 1.0 - 2.0 * (qx * qx + qz * qz);
            (m32.atan2(m22), 0.0)
        };
        [rx.to_degrees(), ry.to_degrees(), rz.to_degrees()]
    }

    fn from_params(params: &Value) -> Self {
        let target_id = params.get("target_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let f = |key: &str, default: f64| -> f32 {
            params.get(key).and_then(|v| v.as_f64()).unwrap_or(default) as f32
        };

        let translate = if params.get("translate").and_then(|v| v.as_array()).is_some() {
            Self::read_vec3(params, "translate", [0.0, 0.0, 0.0])
        } else {
            [f("translate_x", 0.0), f("translate_y", 0.0), f("translate_z", 0.0)]
        };

        let rotation = if params.get("rotation").and_then(|v| v.as_array()).is_some() {
            let euler = Self::read_vec3(params, "rotation", [0.0, 0.0, 0.0]);
            Self::euler_xyz_to_quat(euler[0], euler[1], euler[2])
        } else {
            [f("rotation_x", 0.0), f("rotation_y", 0.0), f("rotation_z", 0.0), f("rotation_w", 1.0)]
        };

        let scale = if params.get("scale").and_then(|v| v.as_array()).is_some() {
            Self::read_vec3(params, "scale", [1.0, 1.0, 1.0])
        } else {
            [f("scale_x", 1.0), f("scale_y", 1.0), f("scale_z", 1.0)]
        };

        Self { target_id, translate, rotation, scale }
    }
}

pub struct TransformFeature;

impl FeatureKind for TransformFeature {
    fn type_name(&self) -> &'static str {
        TransformDef::TYPE_NAME
    }
    fn schema_json(&self) -> Value {
        TransformDef::schema_json()
    }
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let td = TransformDef::from_params(def);
        let target = FeatureId::from_u32(td.target_id);
        let pos = ctx.document.features.iter().position(|f| f.id == target);
        let idx = pos.ok_or_else(|| FeatureError::InvalidParameter {
            feature: "transform",
            name: "target_id",
            message: format!("target feature {} not found", td.target_id),
        })?;
        let t = &mut ctx.document.features[idx].transform;
        t.translate = td.translate;
        t.rotation = td.rotation;
        t.scale = td.scale;
        Ok(())
    }
}

// ─── K10.4: Fillet Edge Feature ──────────────────────────────────
//
// A feature that applies per-edge spatial-mask fillets to the
// CSG tree. The edge fillets are stored in the document and
// applied as a post-processing pass after from_features builds
// the base tree. The feature itself doesn't push a spine entry —
// it modifies the tree shape by replacing Union/Difference
// nodes with MaskedBlend where the edge specification matches.

/// `fillet_edge` — per-edge fillet with spatial mask.
#[derive(FeatureSchema)]
#[schema(type = "fillet_edge")]
pub struct FilletEdgeDef {
    /// Flat-numeric id of the first feature at the junction.
    pub feature_a: f32,
    /// Flat-numeric id of the second feature.
    pub feature_b: f32,
    /// Capsule start [x, y, z].
    pub mask_p0: [f32; 3],
    /// Capsule end [x, y, z].
    pub mask_p1: [f32; 3],
    /// Capsule radius.
    #[param(default = 10.0)]
    pub mask_radius: f32,
    /// Fillet blend radius.
    #[param(default = 5.0)]
    pub blend_radius: f32,
}

pub struct FilletEdgeFeature;

impl FeatureKind for FilletEdgeFeature {
    fn type_name(&self) -> &'static str {
        FilletEdgeDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        FilletEdgeDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = FilletEdgeDef::parse(def)?;
        if d.blend_radius < 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: FilletEdgeDef::TYPE_NAME,
                name: "blend_radius",
                message: format!("must be >= 0, got {}", d.blend_radius),
            });
        }
        if d.mask_radius < 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: FilletEdgeDef::TYPE_NAME,
                name: "mask_radius",
                message: format!("must be >= 0, got {}", d.mask_radius),
            });
        }
        ctx.edge_fillets.push(crate::csg::EdgeFillet {
            feature_a: FeatureId::from_u32(d.feature_a as u32),
            feature_b: FeatureId::from_u32(d.feature_b as u32),
            mask_p0: d.mask_p0,
            mask_p1: d.mask_p1,
            mask_radius: d.mask_radius,
            blend_radius: d.blend_radius,
            blend_mode: crate::csg::BlendMode::Fillet,
        });
        Ok(())
    }
}

// ─── K13.1: Imported Mesh as CSG Solid ───────────────────────────
//
// Wraps a MeshSdf (triangle buffer + per-tri AABBs + ray-parity
// sign test) as a full CSG leaf. The mesh data serialises inline
// in the FeatureCall params so it survives save/load via
// localStorage. Performance is O(N) per sample — acceptable for
// imports up to ~50K triangles at DC default resolution.

/// `imported_mesh` — an imported triangle mesh that
/// participates in CSG Booleans.
#[derive(FeatureSchema)]
#[schema(type = "imported_mesh")]
pub struct ImportedMeshDef {
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(name = "Scale", default = 1.0)]
    pub uniform_scale: f32,
    #[param(name = "Z-up", default = false)]
    pub swap_yz: bool,
    #[param(default = ::fielddraft_protocol::BoolOp::Union)]
    pub op: BoolOp,
}

pub struct ImportedMeshFeature;

impl FeatureKind for ImportedMeshFeature {
    fn type_name(&self) -> &'static str {
        ImportedMeshDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        ImportedMeshDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ImportedMeshDef::parse(def)?;

        // Compact format (v2): positions + indices stored separately.
        // Legacy format (v1): full MeshSdf under "__mesh".
        let mesh: fielddraft_protocol::MeshSdf = if let Some(pos_val) = def.get("__positions") {
            let positions: Vec<f32> = serde_json::from_value(pos_val.clone()).map_err(|e| {
                FeatureError::InvalidParameter {
                    feature: ImportedMeshDef::TYPE_NAME,
                    name: "__positions",
                    message: e.to_string(),
                }
            })?;
            let idx_val = def.get("__indices").ok_or(FeatureError::MissingParameter {
                feature: ImportedMeshDef::TYPE_NAME,
                name: "__indices",
            })?;
            let indices: Vec<u32> = serde_json::from_value(idx_val.clone()).map_err(|e| {
                FeatureError::InvalidParameter {
                    feature: ImportedMeshDef::TYPE_NAME,
                    name: "__indices",
                    message: e.to_string(),
                }
            })?;
            fielddraft_protocol::MeshSdf::from_buffers(&positions, &indices)
        } else {
            let mesh_val = def.get("__mesh").ok_or(FeatureError::MissingParameter {
                feature: ImportedMeshDef::TYPE_NAME,
                name: "__mesh",
            })?;
            serde_json::from_value(mesh_val.clone()).map_err(|e| {
                FeatureError::InvalidParameter {
                    feature: ImportedMeshDef::TYPE_NAME,
                    name: "__mesh",
                    message: e.to_string(),
                }
            })?
        };

        let name = optional_display_name(def, || format!("Imported mesh {id}"));
        let base_rotation = optional_rotation(def);
        let s = if d.uniform_scale > 0.0 { d.uniform_scale } else { 1.0 };
        let scale = [s, s, s];

        let rotation = if d.swap_yz {
            // -90° around X to convert Z-up → Y-up
            let swap: [f32; 4] = [-std::f32::consts::FRAC_1_SQRT_2, 0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2];
            crate::quat_multiply(swap, base_rotation)
        } else {
            base_rotation
        };

        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op: d.op,
            primitive: Primitive::ImportedMesh {
                mesh: Box::new(mesh),
            },
            transform: Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        });
        Ok(())
    }
}

// ─── K11.0: Gyroid TPMS Infill ───────────────────────────────────
//
// The first T2 "SDF superpower". A gyroid is an infinite
// triply-periodic minimal surface described purely by the
// formula `sin(x)cos(y) + sin(y)cos(z) + sin(z)cos(x)`.
// Thickening via `|f| - thickness` produces a solid lattice.
// Intersecting the lattice with a host body clips it to a
// finite region — the standard workflow is "add box, add
// gyroid with op=Intersect".
//
// The spine entry uses a hierarchical child id (same pattern
// as HoleFeature) to avoid a stray duplicate call from
// sync_feature_calls.

/// `gyroid` — TPMS lattice infill.
#[derive(FeatureSchema)]
#[schema(type = "gyroid")]
pub struct GyroidDef {
    #[param(name = "Bodies", default = ::fielddraft_protocol::MultiQuery::new(""))]
    pub targets: ::fielddraft_protocol::MultiQuery,
    /// Distance between repeating cells in mm.
    pub cell_size: ::fielddraft_protocol::Length,
    /// Wall half-thickness in mm. Larger = thicker walls.
    pub thickness: ::fielddraft_protocol::Length,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::Intersect)]
    pub op: BoolOp,
    #[param(name = "Blend radius", default = 0.0)]
    pub blend_radius: f32,
    #[param(name = "Chamfer size", default = 0.0)]
    pub chamfer_size: f32,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = 0.0)]
    pub shell_thickness: f32,
}

pub struct GyroidFeature;

impl FeatureKind for GyroidFeature {
    fn type_name(&self) -> &'static str {
        GyroidDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        GyroidDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = GyroidDef::parse(def)?;
        let cell_mm = d.cell_size.to_mm();
        let thick_mm = d.thickness.to_mm();
        positive_f32(cell_mm, GyroidDef::TYPE_NAME, "cell_size")?;
        positive_f32(thick_mm, GyroidDef::TYPE_NAME, "thickness")?;

        let name = optional_display_name(def, || format!("Gyroid {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        ctx.document.features.push(SpineFeature {
            id: id.child("0"),
            name,
            op: d.op,
            primitive: Primitive::Gyroid {
                cell_size: cell_mm,
                thickness: thick_mm,
            },
            transform: Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            blend_radius: d.blend_radius,
            chamfer_size: d.chamfer_size,
            twist_rate: d.twist_rate,
            bend_rate: d.bend_rate,
            displace_depth: d.displace_depth,
            displace_freq: d.displace_freq,
            shell_thickness: d.shell_thickness,
        });
        Ok(())
    }
}

// ─── K8.9: Hole ──────────────────────────────────────────────────
//
// HoleFeature is a parametric subtract-cylinder. It carries the
// same geometry as a `cylinder` call with op=difference, but the
// dialog-facing schema uses `Length`-typed `diameter` and `depth`
// fields (Onshape parity) so users don't have to mentally halve
// the diameter into a radius or double the depth into a
// half-height. The execute path lowers to Primitive::Cylinder
// with half_extent math done at the model-unit boundary, which
// keeps the mesher and BVH completely ignorant of the dedicated
// Hole kind.
//
// The `op` field is intentionally absent: a hole that adds
// material isn't a hole. If the user wants to turn it into a
// union or intersect they can delete it and add a regular
// cylinder feature instead.

/// `hole` — parametric cylinder subtract with diameter + depth.
#[derive(FeatureSchema)]
#[schema(type = "hole")]
pub struct HoleDef {
    /// Diameter at the mouth of the hole.
    pub diameter: ::fielddraft_protocol::Length,
    /// Total depth along the local Y axis.
    pub depth: ::fielddraft_protocol::Length,
    /// World-space position of the hole's centre.
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
}

pub struct HoleFeature;

impl FeatureKind for HoleFeature {
    fn type_name(&self) -> &'static str {
        HoleDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        HoleDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = HoleDef::parse(def)?;
        let diameter_mm = d.diameter.to_mm();
        let depth_mm = d.depth.to_mm();
        positive_f32(diameter_mm, HoleDef::TYPE_NAME, "diameter")?;
        positive_f32(depth_mm, HoleDef::TYPE_NAME, "depth")?;

        let name = optional_display_name(def, || format!("Hole {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        // Spine entry uses a hierarchical child id (`<hole>/0`)
        // so `Document::sync_feature_calls` drops it on the
        // next sync — the authoritative wire form is the
        // original `hole` FeatureCall, and we don't want a
        // stray `cylinder` FeatureCall to be synthesised from
        // the derived primitive. Same pattern the K8.0 pattern
        // features use for their spine clones.
        ctx.document.features.push(SpineFeature {
            id: id.child("0"),
            name,
            op: BoolOp::Difference,
            primitive: Primitive::Cylinder {
                center: [0.0, 0.0, 0.0],
                radius: diameter_mm / 2.0,
                half_height: depth_mm / 2.0,
            },
            transform: Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
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

/// `extrude` — Rust-native feature that consumes a sketch
/// profile and lowers it to a 3D body.
///
/// K7.1 turns the `profiles` query into a real resolution
/// path: the executor parses the query, finds the referenced
/// sketch in [`Context::sketches`], synthesises a
/// [`crate::sketch2d::Sketch2d`] via
/// `build_sketch2d_from_sub_calls` and reads the profile
/// extents from the resulting AABB. The `profile_half_x` /
/// `profile_half_y` fields become optional fallbacks for
/// tests and for documents that point at a sketch the kernel
/// can't resolve yet.
///
/// Known shape-fidelity limitation: the current spine pipeline
/// has no `Primitive::Extrude` variant, so the lowering still
/// produces a `Primitive::Box` whose half-extents match the
/// sketch's bounding rectangle. A rectangle sketch is
/// pixel-perfect; a circle becomes its bounding box. Fixing
/// that is a later K7 slice which adds the Primitive variant
/// and teaches the mesher/BVH about it.
#[derive(FeatureSchema)]
#[schema(type = "extrude")]
pub struct ExtrudeDef {
    /// Sketch profile this extrude consumes. Evaluated against
    /// [`Context::sketches`] at regeneration time. Only
    /// `qCreatedBy(sketch_id)` is honoured today; other query
    /// variants fall through to the fallback extents.
    pub profiles: ::fielddraft_protocol::Query,

    /// Extrude depth along the local Z axis. Any supported
    /// length unit works; the executor converts to millimetres
    /// via [`Length::to_mm`] before lowering to a box.
    pub depth: ::fielddraft_protocol::Length,

    /// Fallback half-width for tests and for extrudes that
    /// don't yet point at a resolvable sketch. Ignored when
    /// the query successfully resolves to a non-empty sketch.
    #[param(default = 0.0)]
    pub profile_half_x: f32,

    /// Fallback half-height — same story as
    /// [`profile_half_x`].
    #[param(default = 0.0)]
    pub profile_half_y: f32,

    /// Boolean op used to fold the extruded body into the
    /// running feature spine. Defaults to [`BoolOp::Union`],
    /// matching the §4.6 convention that a freshly extruded
    /// body adds material unless the author says otherwise.
    #[param(default = ::fielddraft_protocol::BoolOp::Union)]
    pub operation: BoolOp,

    /// Override for the extruded box origin in world space.
    /// When the sketch resolves, the X/Y components default to
    /// the sketch AABB centre; `center[2]` always supplies the
    /// extrude plane Z coordinate.
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
}

pub struct ExtrudeFeature;

impl FeatureKind for ExtrudeFeature {
    fn type_name(&self) -> &'static str {
        ExtrudeDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        ExtrudeDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ExtrudeDef::parse(def)?;

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

        // K7.3: resolve the `profiles` query against the
        // context's sketch table and lower to a real
        // `Primitive::Extrude` carrying the full Sketch2d
        // profile. The K4.5.8 fallback (Box stand-in) stays
        // for documents that don't yet point at a resolvable
        // sketch, so existing tests keep running unchanged.
        //
        // K7.22 follow-up: when the `profiles` query *does*
        // resolve to a sketch but the sketch has no closed
        // regions (e.g. the user drew lines/points but no
        // rectangle/circle), we used to fall through to the
        // legacy fallback and die on the `profile_half_x > 0`
        // guard with an unhelpful message. Distinguish the
        // three states explicitly: resolved-and-extrudable,
        // resolved-but-empty, no-resolve-use-fallback.
        let mut resolved_sketch: Option<::fielddraft_protocol::Sketch2d> = None;
        let mut sketch_was_found_but_empty = false;
        if let Ok(expr) = crate::queries::parse_query(d.profiles.as_str()) {
            if let crate::queries::QueryExpr::CreatedBy(sketch_id) = expr {
                if let Some(sketch_data) = ctx.sketches.get(&sketch_id) {
                    let sketch2d = crate::sketch_features::build_sketch2d_from_sub_calls(
                        &sketch_data.sub_calls,
                    );
                    if !matches!(sketch2d, ::fielddraft_protocol::Sketch2d::Empty) {
                        resolved_sketch = Some(sketch2d);
                    } else {
                        sketch_was_found_but_empty = true;
                    }
                }
            }
        }

        // K7.22 follow-up: emit a clear, user-facing error when
        // the caller *intended* to drive the extrude from a sketch
        // (profile_half_x/y both left at 0) but the sketch has no
        // extrudable regions. The legacy-API test path that passes
        // explicit non-zero half-extents still falls through to
        // the Box fallback below so existing callers keep working.
        if sketch_was_found_but_empty
            && d.profile_half_x == 0.0
            && d.profile_half_y == 0.0
        {
            return Err(FeatureError::InvalidParameter {
                feature: ExtrudeDef::TYPE_NAME,
                name: "profiles",
                message: "sketch has no closed regions to extrude — draw a rectangle, circle or polygon first".to_string(),
            });
        }

        let name = optional_display_name(def, || format!("Extrude {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);

        let primitive = match resolved_sketch {
            Some(sketch) => Primitive::Extrude {
                sketch,
                half_depth_z: depth_mm / 2.0,
                center: d.center,
            },
            None => {
                // Fallback: legacy hand-written half-extents.
                positive_f32(
                    d.profile_half_x,
                    ExtrudeDef::TYPE_NAME,
                    "profile_half_x",
                )?;
                positive_f32(
                    d.profile_half_y,
                    ExtrudeDef::TYPE_NAME,
                    "profile_half_y",
                )?;
                Primitive::Box {
                    center: d.center,
                    half_extents: [d.profile_half_x, d.profile_half_y, depth_mm / 2.0],
                }
            }
        };

        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op: d.operation,
            primitive,
            transform: Transform {
                translate: [0.0, 0.0, 0.0],
                rotation,
                scale,
            },
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        });
        Ok(())
    }
}

// ─── K8.4: Revolve ───────────────────────────────────────────────
//
// Twin of ExtrudeFeature — consumes a sketch profile, but sweeps
// it around the local Y axis instead of translating it along Z.
// Lowers to Primitive::Revolve, which csg.rs::from_features
// wraps in a CsgNode::Revolve. The sketch is interpreted in
// (r, y) coordinates: `x` becomes the radial distance from
// the Y axis. Authoring convention is `x ≥ 0`; a sketch that
// straddles the axis produces a valid but geometrically silly
// body (the centreline sweeps through itself).

/// `revolve` — Rust-native feature that sweeps a sketch profile
/// around the local Y axis.
#[derive(FeatureSchema)]
#[schema(type = "revolve")]
pub struct RevolveDef {
    /// Sketch profile this revolve consumes. `qCreatedBy(sketch_id)`
    /// is the only query variant honoured today.
    pub profiles: ::fielddraft_protocol::Query,

    /// Boolean op used to fold the revolved body into the running
    /// feature spine. Defaults to [`BoolOp::Union`].
    #[param(default = ::fielddraft_protocol::BoolOp::Union)]
    pub operation: BoolOp,

    /// Override for the revolve centre in world space.
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
}

pub struct RevolveFeature;

impl FeatureKind for RevolveFeature {
    fn type_name(&self) -> &'static str {
        RevolveDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        RevolveDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = RevolveDef::parse(def)?;

        // Resolve the `profiles` query against the sketch table.
        // Unlike ExtrudeFeature, we don't fall back to a legacy
        // half-extent stand-in: Revolve needs a real sketch to
        // produce any geometry at all.
        let sketch = (|| -> Option<::fielddraft_protocol::Sketch2d> {
            let expr = crate::queries::parse_query(d.profiles.as_str()).ok()?;
            let crate::queries::QueryExpr::CreatedBy(sketch_id) = expr else {
                return None;
            };
            let sketch_data = ctx.sketches.get(&sketch_id)?;
            let sketch2d = crate::sketch_features::build_sketch2d_from_sub_calls(
                &sketch_data.sub_calls,
            );
            if matches!(sketch2d, ::fielddraft_protocol::Sketch2d::Empty) {
                None
            } else {
                Some(sketch2d)
            }
        })()
        .ok_or(FeatureError::MissingParameter {
            feature: RevolveDef::TYPE_NAME,
            name: "profiles",
        })?;

        let name = optional_display_name(def, || format!("Revolve {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);

        ctx.document.features.push(SpineFeature {
            id: id.clone(),
            name,
            op: d.operation,
            primitive: Primitive::Revolve {
                sketch,
                center: d.center,
            },
            transform: Transform {
                translate: [0.0, 0.0, 0.0],
                rotation,
                scale,
            },
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        });
        Ok(())
    }
}

// ─── K8.0: Linear pattern ────────────────────────────────────────
//
// The first bona-fide parametric pattern feature. Given a
// source feature id, a direction vector, a count and a spacing
// distance, [`LinearPatternFeature`] emits `count - 1` extra
// copies of the source spine entry at offsets
// `i * spacing * unit_dir` for `i = 1..count`. The originals
// stay in place — Onshape semantics.
//
// Copies inherit the source's BoolOp, primitive payload and
// transform verbatim, with the offset added to
// `transform.translate`. Their feature ids are hierarchical
// children of the pattern feature (`pattern_id/1`,
// `pattern_id/2`, …) so provenance still resolves to "this
// clone came from the pattern".
//
// Known limits for this first cut:
//
// * Source must already be on the spine when the pattern
//   runs, i.e. the source call must appear *before* the
//   pattern call in `feature_calls`. Not enforced — a
//   missing source just errors out with `MissingParameter`.
// * Pattern-of-pattern isn't supported: the source lookup
//   walks `document.features` by id, and pattern children
//   are siblings of the pattern feature itself, not under it.

/// `linear_pattern` — replicate a source feature along a
/// direction vector.
#[derive(FeatureSchema)]
#[schema(type = "linear_pattern")]
pub struct LinearPatternDef {
    /// Flat-numeric id of the feature to replicate. Any other
    /// id shape currently errors out via the `Generic`
    /// fallback — the K4.5.1 flat-id scheme is still the
    /// only in-flight numbering.
    pub source: u32,
    /// Direction vector in local frame. Doesn't have to be
    /// normalised — the executor divides by its length at
    /// apply time.
    pub direction: [f32; 3],
    /// Total number of instances *including* the source. Stored
    /// as an `f32` (not `u32`) so the generic FeatureDialog's
    /// number widget can edit it — the derive maps plain integer
    /// types to the `generic → unknown` widget and they'd never
    /// render. `execute` rounds to the nearest positive integer.
    /// `count = 1` is a no-op, `count = 5` means the pattern
    /// adds four offset copies.
    pub count: f32,
    /// Distance between consecutive instances in millimetres.
    pub spacing: ::fielddraft_protocol::Length,
}

pub struct LinearPatternFeature;

impl FeatureKind for LinearPatternFeature {
    fn type_name(&self) -> &'static str {
        LinearPatternDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        LinearPatternDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = LinearPatternDef::parse(def)?;
        let count = d.count.round().max(0.0) as u32;
        if count < 2 {
            // A one-instance pattern is a no-op. Don't error —
            // this is a useful parameter-sweep resting point.
            return Ok(());
        }
        let spacing_mm = d.spacing.to_mm();
        if spacing_mm <= 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: LinearPatternDef::TYPE_NAME,
                name: "spacing",
                message: format!("must be > 0 mm, got {spacing_mm}"),
            });
        }

        // Normalise the direction. A zero-length vector is
        // invalid — there's no axis to replicate along.
        let dx = d.direction[0];
        let dy = d.direction[1];
        let dz = d.direction[2];
        let len = (dx * dx + dy * dy + dz * dz).sqrt();
        if len < 1e-6 {
            return Err(FeatureError::InvalidParameter {
                feature: LinearPatternDef::TYPE_NAME,
                name: "direction",
                message: "must be a non-zero vector".to_string(),
            });
        }
        let unit = [dx / len, dy / len, dz / len];

        // Look up the source spine feature by its flat id.
        let source_fid = FeatureId::from_u32(d.source);
        let source_feature = ctx
            .document
            .features
            .iter()
            .find(|f| f.id == source_fid)
            .ok_or(FeatureError::MissingParameter {
                feature: LinearPatternDef::TYPE_NAME,
                name: "source",
            })?
            .clone();

        // Emit `count - 1` offset copies. `i = 0` would be the
        // source itself, which is already on the spine.
        // Clones carry hierarchical child ids under the
        // pattern — `sync_feature_calls` drops those via its
        // parent filter so the wire form stays authoritative.
        for i in 1..count {
            let i_f = i as f32;
            let offset = [
                unit[0] * spacing_mm * i_f,
                unit[1] * spacing_mm * i_f,
                unit[2] * spacing_mm * i_f,
            ];
            let mut clone = source_feature.clone();
            clone.id = id.child(i.to_string());
            clone.name = format!("{} {}", source_feature.name, i);
            clone.transform.translate = [
                source_feature.transform.translate[0] + offset[0],
                source_feature.transform.translate[1] + offset[1],
                source_feature.transform.translate[2] + offset[2],
            ];
            ctx.document.features.push(clone);
        }

        Ok(())
    }
}

// ─── K8.1: Circular pattern ──────────────────────────────────────
//
// Twin of the linear pattern: instead of offsetting clones by
// a constant translation along a direction vector, each clone is
// rotated around an `axis` through a `center` pivot by an
// angle that scales with its index. Default sweep is a full
// 360° divided evenly over `count` instances; arcs come in by
// overriding `angle_deg`.
//
// The math per clone at index `i ∈ 1..count`:
//   θ        = (i / count) · angle_deg · π / 180
//   q_delta  = axis_angle_to_quat(unit_axis, θ)
//   offset   = source.translate − center
//   clone.translate = q_delta · offset + center
//   clone.rotation  = q_delta · source.rotation
//
// This composes the pattern's rotation on top of whatever
// transform the source feature already carries, so you can
// chain circular-pattern a rotated box and every clone stays
// rotated relative to its own local axis.

/// `circular_pattern` — replicate a source feature around an
/// axis through a pivot point.
#[derive(FeatureSchema)]
#[schema(type = "circular_pattern")]
pub struct CircularPatternDef {
    pub source: u32,
    /// Rotation axis in local frame. The executor normalises
    /// it; a zero vector errors out.
    pub axis: [f32; 3],
    /// Pivot point the axis passes through.
    #[param(default = [0.0, 0.0, 0.0])]
    pub center: [f32; 3],
    /// Total number of instances including the source. `f32`
    /// for dialog-widget compatibility — `execute` rounds to
    /// the nearest positive integer. See [`LinearPatternDef::count`]
    /// for the full rationale.
    pub count: f32,
    /// Total sweep in degrees. Defaults to a full 360° when
    /// omitted from the call.
    #[param(default = 360.0)]
    pub angle_deg: f32,
}

pub struct CircularPatternFeature;

impl FeatureKind for CircularPatternFeature {
    fn type_name(&self) -> &'static str {
        CircularPatternDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        CircularPatternDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = CircularPatternDef::parse(def)?;
        let count = d.count.round().max(0.0) as u32;
        if count < 2 {
            return Ok(());
        }
        // Normalise the axis.
        let ax = d.axis[0];
        let ay = d.axis[1];
        let az = d.axis[2];
        let axis_len = (ax * ax + ay * ay + az * az).sqrt();
        if axis_len < 1e-6 {
            return Err(FeatureError::InvalidParameter {
                feature: CircularPatternDef::TYPE_NAME,
                name: "axis",
                message: "must be a non-zero vector".to_string(),
            });
        }
        let unit = [ax / axis_len, ay / axis_len, az / axis_len];

        let source_fid = FeatureId::from_u32(d.source);
        let source_feature = ctx
            .document
            .features
            .iter()
            .find(|f| f.id == source_fid)
            .ok_or(FeatureError::MissingParameter {
                feature: CircularPatternDef::TYPE_NAME,
                name: "source",
            })?
            .clone();

        // Per-instance angle step.
        let step_rad =
            (d.angle_deg / count as f32) * std::f32::consts::PI / 180.0;

        for i in 1..count {
            let theta = step_rad * i as f32;
            let half = theta * 0.5;
            let s = half.sin();
            let q_delta = [unit[0] * s, unit[1] * s, unit[2] * s, half.cos()];

            // Rotate the source translate around the pivot.
            let offset = [
                source_feature.transform.translate[0] - d.center[0],
                source_feature.transform.translate[1] - d.center[1],
                source_feature.transform.translate[2] - d.center[2],
            ];
            let rotated = crate::quat_rotate(q_delta, offset);
            let new_translate = [
                rotated[0] + d.center[0],
                rotated[1] + d.center[1],
                rotated[2] + d.center[2],
            ];

            let mut clone = source_feature.clone();
            clone.id = id.child(i.to_string());
            clone.name = format!("{} {}", source_feature.name, i);
            clone.transform.translate = new_translate;
            clone.transform.rotation =
                crate::quat_multiply(q_delta, source_feature.transform.rotation);
            ctx.document.features.push(clone);
        }

        Ok(())
    }
}

// ─── K8.2: Mirror ────────────────────────────────────────────────
//
// Completes the K8 Pattern block with the degenerate case: a
// single clone produced by reflecting the source feature's
// translate through a plane defined by a point and a normal.
// The rotation is kept verbatim — reflecting a rotation would
// flip chirality, which is a separate feature request (and
// would need a sign flip on one sketch axis to actually matter
// at the geometry level). For K8.2 the aim is "this body appears
// on the other side of the plane"; chiral-aware mirroring comes
// later with body-level symmetry.

/// `mirror` — reflect a source feature's translate through a
/// plane.
#[derive(FeatureSchema)]
#[schema(type = "mirror")]
pub struct MirrorDef {
    pub source: u32,
    /// Plane normal in local frame. The executor normalises it;
    /// a zero vector errors out.
    pub plane_normal: [f32; 3],
    /// A point the plane passes through. Defaults to the origin
    /// so the default is "mirror through the YZ / XZ / XY plane"
    /// depending on the normal axis picked by the caller.
    #[param(default = [0.0, 0.0, 0.0])]
    pub plane_point: [f32; 3],
}

pub struct MirrorFeature;

impl FeatureKind for MirrorFeature {
    fn type_name(&self) -> &'static str {
        MirrorDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        MirrorDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = MirrorDef::parse(def)?;

        let nx = d.plane_normal[0];
        let ny = d.plane_normal[1];
        let nz = d.plane_normal[2];
        let nlen = (nx * nx + ny * ny + nz * nz).sqrt();
        if nlen < 1e-6 {
            return Err(FeatureError::InvalidParameter {
                feature: MirrorDef::TYPE_NAME,
                name: "plane_normal",
                message: "must be a non-zero vector".to_string(),
            });
        }
        let n = [nx / nlen, ny / nlen, nz / nlen];

        let source_fid = FeatureId::from_u32(d.source);
        let source_feature = ctx
            .document
            .features
            .iter()
            .find(|f| f.id == source_fid)
            .ok_or(FeatureError::MissingParameter {
                feature: MirrorDef::TYPE_NAME,
                name: "source",
            })?
            .clone();

        // Reflect p through the plane: p' = p - 2 * ((p - p0) · n) * n
        let p = source_feature.transform.translate;
        let rel = [
            p[0] - d.plane_point[0],
            p[1] - d.plane_point[1],
            p[2] - d.plane_point[2],
        ];
        let dot = rel[0] * n[0] + rel[1] * n[1] + rel[2] * n[2];
        let reflected = [
            p[0] - 2.0 * dot * n[0],
            p[1] - 2.0 * dot * n[1],
            p[2] - 2.0 * dot * n[2],
        ];

        let mut clone = source_feature.clone();
        clone.id = id.child("1");
        clone.name = format!("{} mirror", source_feature.name);
        clone.transform.translate = reflected;
        ctx.document.features.push(clone);

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
fn optional_scale(def: &Value) -> [f32; 3] {
    match def.get("scale") {
        Some(Value::Number(n)) => {
            let s = n.as_f64().unwrap_or(1.0) as f32;
            [s, s, s]
        }
        Some(Value::Array(arr)) if arr.len() == 3 => {
            let x = arr[0].as_f64().unwrap_or(1.0) as f32;
            let y = arr[1].as_f64().unwrap_or(1.0) as f32;
            let z = arr[2].as_f64().unwrap_or(1.0) as f32;
            [x, y, z]
        }
        _ => [1.0, 1.0, 1.0],
    }
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

// ─── Boolean Feature ────────────────────────────────────────────
//
// Dedicated Boolean feature with tabbed dialog (Union / Subtract /
// Intersect). The schema is hand-crafted to emit the "tabs" widget
// structure; the derive is used only for parse().

#[derive(FeatureSchema)]
#[schema(type = "boolean")]
pub struct BooleanDef {
    #[param(name = "Operation", default = "Union".to_string())]
    pub operation: String,
    #[param(name = "Tools")]
    pub tools: MultiQuery,
    #[param(name = "Targets", default = MultiQuery::new(""))]
    pub targets: MultiQuery,
    #[param(name = "Keep tools", default = false)]
    pub keep_tools: bool,
}

pub struct BooleanFeature;

impl FeatureKind for BooleanFeature {
    fn type_name(&self) -> &'static str {
        BooleanDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        serde_json::json!({
            "type": "boolean",
            "params": [
                {
                    "key": "operation",
                    "name": "Operation",
                    "type": "tabs",
                    "required": false,
                    "tabs": [
                        {
                            "label": "Union",
                            "children": [
                                { "key": "tools", "name": "Tools", "type": "multi_query", "required": true }
                            ]
                        },
                        {
                            "label": "Subtract",
                            "children": [
                                { "key": "tools", "name": "Tools", "type": "multi_query", "required": true },
                                { "key": "targets", "name": "Targets", "type": "multi_query", "required": true }
                            ]
                        },
                        {
                            "label": "Intersect",
                            "children": [
                                { "key": "tools", "name": "Tools", "type": "multi_query", "required": true }
                            ]
                        }
                    ]
                },
                { "key": "keep_tools", "name": "Keep tools", "type": "bool", "required": false }
            ]
        })
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = BooleanDef::parse(def)?;

        let op = match d.operation.as_str() {
            "Subtract" => BoolOp::Difference,
            "Intersect" => BoolOp::Intersect,
            _ => BoolOp::Union,
        };

        let resolve_ids = |mq: &MultiQuery| -> Vec<FeatureId> {
            mq.queries()
                .iter()
                .filter_map(|q| {
                    let inner = q.strip_prefix("qCreatedBy(")?.strip_suffix(')')?;
                    inner.parse::<FeatureId>().ok()
                })
                .collect()
        };

        let tool_ids = resolve_ids(&d.tools);
        if tool_ids.is_empty() {
            return Ok(());
        }

        // The first tool in spine order must keep op=Union so it
        // seeds a BodyPartition. Subsequent tools get the selected
        // op so they attach to that body via AABB overlap.
        let first_tool_pos = ctx
            .document
            .features
            .iter()
            .position(|f| tool_ids.contains(&f.id));

        for (idx, feature) in ctx.document.features.iter_mut().enumerate() {
            if !tool_ids.contains(&feature.id) {
                continue;
            }
            match op {
                BoolOp::Union => {
                    feature.op = BoolOp::Union;
                }
                BoolOp::Intersect => {
                    if tool_ids.len() > 1 && Some(idx) == first_tool_pos {
                        feature.op = BoolOp::Union;
                    } else {
                        feature.op = BoolOp::Intersect;
                    }
                }
                BoolOp::Difference => {
                    feature.op = BoolOp::Difference;
                }
            }
        }

        Ok(())
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
        assert_eq!(registry.len(), 23);
        assert!(registry.get("box").is_some());
        assert!(registry.get("sphere").is_some());
        assert!(registry.get("cylinder").is_some());
        assert!(registry.get("extrude").is_some());
        assert!(registry.get("revolve").is_some());
        assert!(registry.get("hole").is_some());
        assert!(registry.get("torus").is_some());
        assert!(registry.get("thread").is_some());
        assert!(registry.get("cone").is_some());
        assert!(registry.get("capsule").is_some());
        assert!(registry.get("sketch").is_some());
        assert!(registry.get("linear_pattern").is_some());
        assert!(registry.get("circular_pattern").is_some());
        assert!(registry.get("mirror").is_some());
        assert!(registry.get("hex_prism").is_some());
        assert!(registry.get("gyroid").is_some());
        assert!(registry.get("ellipsoid").is_some());
    }

    // ─── K6a: schema lookup on the registry ───────────────────

    #[test]
    fn schema_for_returns_the_derive_generated_schema_per_type() {
        let registry = FeatureRegistry::with_builtins();
        let box_schema = registry.schema_for("box").expect("box schema");
        assert_eq!(box_schema["type"], json!("box"));
        assert!(box_schema["params"].is_array());

        let extrude_schema = registry
            .schema_for("extrude")
            .expect("extrude schema");
        assert_eq!(extrude_schema["type"], json!("extrude"));
        let params = extrude_schema["params"].as_array().unwrap();
        assert_eq!(params.len(), 6);
    }

    #[test]
    fn schema_for_returns_none_for_unknown_types() {
        let registry = FeatureRegistry::with_builtins();
        assert!(registry.schema_for("nonexistent_type").is_none());
    }

    #[test]
    fn type_names_contains_all_four_builtins() {
        let registry = FeatureRegistry::with_builtins();
        let mut names = registry.type_names();
        names.sort();
        assert_eq!(
            names,
            vec![
                "boolean",
                "box",
                "capsule",
                "circular_pattern",
                "cone",
                "cylinder",
                "ellipsoid",
                "extrude",
                "fillet_edge",
                "gyroid",
                "hex_prism",
                "hole",
                "imported_mesh",
                "linear_pattern",
                "mate_connector",
                "mirror",
                "revolve",
                "sketch",
                "sphere",
                "sweep",
                "thread",
                "torus",
                "transform"
            ]
        );
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
            fn schema_json(&self) -> Value {
                json!({"type": "box", "params": []})
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
        // exactly — eleven params after shell_thickness was added.
        let schema = BoxDef::schema_json();
        assert_eq!(schema["type"], json!("box"));
        let params = schema["params"].as_array().unwrap();
        assert_eq!(params.len(), 11);

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
                    scale: [1.0, 1.0, 1.0],
                },
                blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
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
        // K6a.1: BoolOp flows through the generic `enum` slot and
        // carries its variant list + a `horizontal_enum` UI hint.
        assert_eq!(map["operation"]["type"], json!("enum"));
        assert_eq!(map["operation"]["required"], json!(false));
        assert_eq!(map["operation"]["ui_hint"], json!("horizontal_enum"));
        let variants = map["operation"]["variants"]
            .as_array()
            .expect("operation variants");
        let variant_keys: Vec<&str> = variants
            .iter()
            .map(|v| v["key"].as_str().unwrap())
            .collect();
        assert_eq!(variant_keys, vec!["union", "difference", "intersect"]);
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

    // ─── K8.0: linear pattern ─────────────────────────────────

    #[test]
    fn linear_pattern_emits_count_minus_one_offset_clones() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let box_id = id(1);
        let pattern_id = id(2);

        let box_call = FeatureCall::new(
            "box",
            box_id.clone(),
            json!({
                "half_extents": [5.0, 5.0, 5.0],
                "center": [0.0, 0.0, 0.0],
                "translate": [0.0, 0.0, 0.0],
                "op": "union"
            }),
        );
        let pattern_call = FeatureCall::new(
            "linear_pattern",
            pattern_id.clone(),
            json!({
                "source": 1,
                "direction": [1.0, 0.0, 0.0],
                "count": 4,
                "spacing": { "value": 20.0, "unit": "mm" }
            }),
        );
        registry
            .regenerate(&mut ctx, &[box_call, pattern_call])
            .unwrap();

        // Source (1) + three clones = four spine entries.
        assert_eq!(doc.features.len(), 4);
        let source = doc
            .features
            .iter()
            .find(|f| f.id == box_id)
            .expect("source box");
        assert_eq!(source.transform.translate, [0.0, 0.0, 0.0]);

        // Clones live at pattern_id/1, /2, /3 with 20 mm
        // increments along +X.
        for i in 1..4_u32 {
            let clone_id = pattern_id.child(i.to_string());
            let clone = doc
                .features
                .iter()
                .find(|f| f.id == clone_id)
                .expect("clone spine entry");
            let expected_x = 20.0_f32 * i as f32;
            assert!((clone.transform.translate[0] - expected_x).abs() < 1e-5);
            // Primitive payload is cloned verbatim.
            if let (Primitive::Box { half_extents, .. }, Primitive::Box { half_extents: src_he, .. }) =
                (&clone.primitive, &source.primitive)
            {
                assert_eq!(half_extents, src_he);
            } else {
                panic!("expected box primitives");
            }
        }
    }

    #[test]
    fn linear_pattern_count_1_is_a_noop() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        registry
            .regenerate(
                &mut ctx,
                &[
                    FeatureCall::new(
                        "box",
                        id(1),
                        json!({
                            "half_extents": [1.0, 1.0, 1.0],
                            "op": "union"
                        }),
                    ),
                    FeatureCall::new(
                        "linear_pattern",
                        id(2),
                        json!({
                            "source": 1,
                            "direction": [1.0, 0.0, 0.0],
                            "count": 1,
                            "spacing": { "value": 20.0, "unit": "mm" }
                        }),
                    ),
                ],
            )
            .unwrap();
        // Just the source box — no clones added.
        assert_eq!(doc.features.len(), 1);
    }

    #[test]
    fn linear_pattern_normalises_the_direction_vector() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        registry
            .regenerate(
                &mut ctx,
                &[
                    FeatureCall::new(
                        "box",
                        id(1),
                        json!({
                            "half_extents": [1.0, 1.0, 1.0],
                            "op": "union"
                        }),
                    ),
                    FeatureCall::new(
                        "linear_pattern",
                        id(2),
                        json!({
                            "source": 1,
                            // Non-unit vector — the executor
                            // must normalise so spacing still
                            // lands at 10 mm, not 10 × √3 mm.
                            "direction": [3.0, 4.0, 0.0],
                            "count": 2,
                            "spacing": { "value": 10.0, "unit": "mm" }
                        }),
                    ),
                ],
            )
            .unwrap();
        let clone = &doc.features[1];
        // Unit (3,4,0) = (0.6, 0.8, 0). Offset at i=1 is 10 mm
        // along that direction → (6, 8, 0).
        assert!((clone.transform.translate[0] - 6.0).abs() < 1e-4);
        assert!((clone.transform.translate[1] - 8.0).abs() < 1e-4);
        assert!((clone.transform.translate[2] - 0.0).abs() < 1e-4);
    }

    #[test]
    fn linear_pattern_errors_on_missing_source() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let err = registry
            .regenerate(
                &mut ctx,
                &[FeatureCall::new(
                    "linear_pattern",
                    id(1),
                    json!({
                        "source": 999,
                        "direction": [1.0, 0.0, 0.0],
                        "count": 3,
                        "spacing": { "value": 20.0, "unit": "mm" }
                    }),
                )],
            )
            .unwrap_err();
        assert!(matches!(
            err,
            FeatureError::MissingParameter {
                feature: "linear_pattern",
                ..
            }
        ));
    }

    // ─── K8.1: circular pattern ───────────────────────────────

    #[test]
    fn circular_pattern_places_clones_around_y_axis_at_even_angles() {
        // A unit box at (10, 0, 0) circular-patterned around
        // the Y axis 4 times should land clones at (0, 0, -10),
        // (-10, 0, 0), (0, 0, 10) — ±1e-4 since the rotation
        // math goes through f32 trig.
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);

        let box_call = FeatureCall::new(
            "box",
            id(1),
            json!({
                "half_extents": [1.0, 1.0, 1.0],
                "translate": [10.0, 0.0, 0.0],
                "op": "union"
            }),
        );
        let pattern_call = FeatureCall::new(
            "circular_pattern",
            id(2),
            json!({
                "source": 1,
                "axis": [0.0, 1.0, 0.0],
                "center": [0.0, 0.0, 0.0],
                "count": 4,
                "angle_deg": 360.0
            }),
        );
        registry
            .regenerate(&mut ctx, &[box_call, pattern_call])
            .unwrap();

        // 1 source + 3 clones.
        assert_eq!(doc.features.len(), 4);
        let expectations = [
            (1_u32, [0.0, 0.0, -10.0]),
            (2_u32, [-10.0, 0.0, 0.0]),
            (3_u32, [0.0, 0.0, 10.0]),
        ];
        for (i, expected) in expectations {
            let clone_id = id(2).child(i.to_string());
            let clone = doc
                .features
                .iter()
                .find(|f| f.id == clone_id)
                .unwrap_or_else(|| panic!("missing clone {i}"));
            for k in 0..3 {
                assert!(
                    (clone.transform.translate[k] - expected[k]).abs() < 1e-4,
                    "clone {i} axis {k}: expected {} got {}",
                    expected[k],
                    clone.transform.translate[k]
                );
            }
        }
    }

    #[test]
    fn circular_pattern_count_1_is_a_noop() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        registry
            .regenerate(
                &mut ctx,
                &[
                    FeatureCall::new(
                        "box",
                        id(1),
                        json!({
                            "half_extents": [1.0, 1.0, 1.0],
                            "translate": [5.0, 0.0, 0.0],
                            "op": "union"
                        }),
                    ),
                    FeatureCall::new(
                        "circular_pattern",
                        id(2),
                        json!({
                            "source": 1,
                            "axis": [0.0, 1.0, 0.0],
                            "count": 1,
                        }),
                    ),
                ],
            )
            .unwrap();
        // No clones — just the source.
        assert_eq!(doc.features.len(), 1);
    }

    #[test]
    fn circular_pattern_respects_the_center_pivot_for_non_origin_rotations() {
        // Source at (10, 0, 2), pivot at (5, 0, 0), axis +Y,
        // 2 instances at 180° sweep: the one clone sits at the
        // opposite side of the pivot from the source.
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        registry
            .regenerate(
                &mut ctx,
                &[
                    FeatureCall::new(
                        "sphere",
                        id(1),
                        json!({
                            "radius": 1.0,
                            "translate": [10.0, 0.0, 2.0],
                            "op": "union"
                        }),
                    ),
                    FeatureCall::new(
                        "circular_pattern",
                        id(2),
                        json!({
                            "source": 1,
                            "axis": [0.0, 1.0, 0.0],
                            "center": [5.0, 0.0, 0.0],
                            "count": 2,
                            "angle_deg": 180.0
                        }),
                    ),
                ],
            )
            .unwrap();
        // 180° step = 90° per instance. At i=1 the clone sits
        // 90° around the Y axis from the source, relative to
        // the pivot. Offset (10-5, 0, 2-0) = (5, 0, 2). 90°
        // CCW around +Y maps (x, y, z) → (z, y, -x) = (2, 0, -5).
        // Plus pivot: (7, 0, -5).
        let clone = &doc.features[1];
        assert!((clone.transform.translate[0] - 7.0).abs() < 1e-4);
        assert!((clone.transform.translate[1] - 0.0).abs() < 1e-4);
        assert!((clone.transform.translate[2] + 5.0).abs() < 1e-4);
    }

    #[test]
    fn circular_pattern_errors_on_zero_axis() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let err = registry
            .regenerate(
                &mut ctx,
                &[
                    FeatureCall::new(
                        "box",
                        id(1),
                        json!({ "half_extents": [1.0, 1.0, 1.0], "op": "union" }),
                    ),
                    FeatureCall::new(
                        "circular_pattern",
                        id(2),
                        json!({
                            "source": 1,
                            "axis": [0.0, 0.0, 0.0],
                            "count": 4
                        }),
                    ),
                ],
            )
            .unwrap_err();
        assert!(matches!(
            err,
            FeatureError::InvalidParameter {
                feature: "circular_pattern",
                name: "axis",
                ..
            }
        ));
    }

    // ─── K8.11: formula engine ────────────────────────────────

    #[test]
    fn regenerate_resolves_length_formulas_against_variables() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        doc.variables.insert("width".to_string(), 50.0);
        let mut ctx = Context::new(&mut doc);
        let call = FeatureCall::new(
            "hole",
            id(1),
            json!({
                // Diameter references a variable via the
                // K8.11 sigil; depth stays a plain literal.
                "diameter": { "value": "=width / 5", "unit": "mm" },
                "depth": { "value": 40.0, "unit": "mm" },
                "translate": [0.0, 0.0, 0.0],
            }),
        );
        registry.regenerate(&mut ctx, &[call]).unwrap();
        assert_eq!(doc.features.len(), 1);
        let f = &doc.features[0];
        match &f.primitive {
            Primitive::Cylinder { radius, .. } => {
                // width(50) / 5 / 2 (hole → radius) = 5
                assert!((*radius - 5.0).abs() < 1e-5);
            }
            other => panic!("expected Primitive::Cylinder, got {other:?}"),
        }
    }

    #[test]
    fn regenerate_surfaces_unknown_variable_errors() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let call = FeatureCall::new(
            "hole",
            id(1),
            json!({
                "diameter": { "value": "=never_defined", "unit": "mm" },
                "depth": { "value": 40.0, "unit": "mm" },
            }),
        );
        let err = registry.regenerate(&mut ctx, &[call]).unwrap_err();
        match err {
            FeatureError::InvalidParameter { feature, message, .. } => {
                assert_eq!(feature, "<formula>");
                assert!(message.contains("never_defined"));
            }
            other => panic!("expected InvalidParameter, got {other:?}"),
        }
    }

    // ─── K8.9: hole ───────────────────────────────────────────

    #[test]
    fn hole_lowers_to_difference_cylinder_with_diameter_halved() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let call = FeatureCall::new(
            "hole",
            id(1),
            json!({
                "diameter": { "value": 6.0, "unit": "mm" },
                "depth": { "value": 40.0, "unit": "mm" },
                "translate": [10.0, 0.0, 0.0],
            }),
        );
        registry.regenerate(&mut ctx, &[call]).unwrap();
        assert_eq!(doc.features.len(), 1);
        let f = &doc.features[0];
        // Spine entry uses the hierarchical child id `1/0` so
        // sync_feature_calls skips it.
        assert_eq!(f.id, id(1).child("0"));
        assert!(f.id.parent().is_some());
        assert_eq!(f.op, BoolOp::Difference);
        assert_eq!(f.transform.translate, [10.0, 0.0, 0.0]);
        match &f.primitive {
            Primitive::Cylinder {
                center,
                radius,
                half_height,
            } => {
                assert_eq!(*center, [0.0, 0.0, 0.0]);
                assert!((*radius - 3.0).abs() < 1e-5);
                assert!((*half_height - 20.0).abs() < 1e-5);
            }
            other => panic!("expected Primitive::Cylinder, got {other:?}"),
        }
    }

    #[test]
    fn hole_rejects_non_positive_diameter() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let err = registry
            .regenerate(
                &mut ctx,
                &[FeatureCall::new(
                    "hole",
                    id(1),
                    json!({
                        "diameter": { "value": 0.0, "unit": "mm" },
                        "depth": { "value": 10.0, "unit": "mm" },
                    }),
                )],
            )
            .unwrap_err();
        assert!(matches!(
            err,
            FeatureError::InvalidParameter {
                feature: "hole",
                name: "diameter",
                ..
            }
        ));
    }

    // ─── K8.7: suppress ───────────────────────────────────────

    #[test]
    fn suppressed_feature_call_is_skipped_by_regenerate() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let mut box_call = FeatureCall::new(
            "box",
            id(1),
            json!({ "half_extents": [1.0, 1.0, 1.0], "op": "union" }),
        );
        let sphere_call = FeatureCall::new(
            "sphere",
            id(2),
            json!({ "radius": 2.0, "op": "union" }),
        );
        box_call.suppressed = true;
        registry
            .regenerate(&mut ctx, &[box_call, sphere_call])
            .unwrap();
        // Only the sphere made it onto the spine.
        assert_eq!(doc.features.len(), 1);
        assert_eq!(doc.features[0].id, id(2));
    }

    // ─── K8.2: mirror ─────────────────────────────────────────

    #[test]
    fn mirror_reflects_translate_through_yz_plane() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        registry
            .regenerate(
                &mut ctx,
                &[
                    FeatureCall::new(
                        "box",
                        id(1),
                        json!({
                            "half_extents": [1.0, 1.0, 1.0],
                            "translate": [10.0, 5.0, -3.0],
                            "op": "union"
                        }),
                    ),
                    FeatureCall::new(
                        "mirror",
                        id(2),
                        json!({
                            "source": 1,
                            "plane_normal": [1.0, 0.0, 0.0]
                        }),
                    ),
                ],
            )
            .unwrap();
        // Source + one mirrored clone.
        assert_eq!(doc.features.len(), 2);
        assert_eq!(doc.features[0].transform.translate, [10.0, 5.0, -3.0]);
        assert_eq!(doc.features[1].transform.translate, [-10.0, 5.0, -3.0]);
    }

    #[test]
    fn mirror_respects_non_origin_plane_point() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        registry
            .regenerate(
                &mut ctx,
                &[
                    FeatureCall::new(
                        "box",
                        id(1),
                        json!({
                            "half_extents": [1.0, 1.0, 1.0],
                            "translate": [20.0, 0.0, 0.0],
                            "op": "union"
                        }),
                    ),
                    FeatureCall::new(
                        "mirror",
                        id(2),
                        json!({
                            "source": 1,
                            "plane_normal": [1.0, 0.0, 0.0],
                            "plane_point": [5.0, 0.0, 0.0]
                        }),
                    ),
                ],
            )
            .unwrap();
        // Reflected through x=5: 20 → -10.
        let t = doc.features[1].transform.translate;
        assert!((t[0] - (-10.0)).abs() < 1e-5);
        assert!(t[1].abs() < 1e-5);
        assert!(t[2].abs() < 1e-5);
    }

    #[test]
    fn mirror_normalises_the_plane_normal() {
        // A non-unit normal must not scale the reflection.
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        registry
            .regenerate(
                &mut ctx,
                &[
                    FeatureCall::new(
                        "box",
                        id(1),
                        json!({
                            "half_extents": [1.0, 1.0, 1.0],
                            "translate": [7.0, 0.0, 0.0],
                            "op": "union"
                        }),
                    ),
                    FeatureCall::new(
                        "mirror",
                        id(2),
                        json!({
                            "source": 1,
                            "plane_normal": [9.0, 0.0, 0.0]
                        }),
                    ),
                ],
            )
            .unwrap();
        let t = doc.features[1].transform.translate;
        assert!((t[0] - (-7.0)).abs() < 1e-5);
    }

    #[test]
    fn mirror_errors_on_zero_normal() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let err = registry
            .regenerate(
                &mut ctx,
                &[
                    FeatureCall::new(
                        "box",
                        id(1),
                        json!({ "half_extents": [1.0, 1.0, 1.0], "op": "union" }),
                    ),
                    FeatureCall::new(
                        "mirror",
                        id(2),
                        json!({
                            "source": 1,
                            "plane_normal": [0.0, 0.0, 0.0]
                        }),
                    ),
                ],
            )
            .unwrap_err();
        assert!(matches!(
            err,
            FeatureError::InvalidParameter {
                feature: "mirror",
                name: "plane_normal",
                ..
            }
        ));
    }

    // ─── K7.1: extrude consumes real sketches ─────────────────

    #[test]
    fn extrude_derives_profile_extents_from_a_resolved_rectangle_sketch() {
        // Build a sketch with a 20x10 rectangle centred at
        // (5, 3), commit it via the registry so the context
        // sketches map gets populated, then run an extrude
        // that references it. The extrude's half-extents must
        // come from the sketch's AABB, not from the
        // `profile_half_x/y` fallback.
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let sketch_id = id(1);
        let extrude_id = id(2);

        let sketch_call = FeatureCall::new(
            "sketch",
            sketch_id.clone(),
            json!({
                "plane": "qCreatedBy(origin_plane_xy)",
                "sub_calls": [
                    {
                        "type": "sketch_rectangle",
                        "id": "r1",
                        "params": {
                            "corner_a": [-5.0, -2.0],
                            "corner_b": [15.0, 8.0],
                        }
                    }
                ]
            }),
        );
        let extrude_call = FeatureCall::new(
            "extrude",
            extrude_id.clone(),
            json!({
                "profiles": format!("qCreatedBy({sketch_id})"),
                "depth": { "value": 12.0, "unit": "mm" },
                // Explicit bogus fallbacks: the resolver should
                // override these with AABB-derived values.
                "profile_half_x": 999.0,
                "profile_half_y": 999.0,
            }),
        );
        registry
            .regenerate(&mut ctx, &[sketch_call, extrude_call])
            .unwrap();

        let extrude_feature = doc
            .features
            .iter()
            .find(|f| f.id == extrude_id)
            .expect("extrude spine entry");
        // K7.3: the extrude now lowers to a real
        // Primitive::Extrude carrying the Sketch2d profile
        // rather than a rectangular Box stand-in.
        match &extrude_feature.primitive {
            Primitive::Extrude {
                sketch,
                half_depth_z,
                center: _,
            } => {
                assert!((half_depth_z - 6.0).abs() < 1e-5);
                let aabb = sketch.aabb();
                assert!((aabb.min[0] - (-5.0)).abs() < 1e-5);
                assert!((aabb.max[0] - 15.0).abs() < 1e-5);
                assert!((aabb.min[1] - (-2.0)).abs() < 1e-5);
                assert!((aabb.max[1] - 8.0).abs() < 1e-5);
            }
            other => panic!("expected Primitive::Extrude, got {other:?}"),
        }
    }

    #[test]
    fn extrude_derives_profile_extents_from_a_resolved_circle_sketch() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let sketch_id = id(1);
        let extrude_id = id(2);

        let sketch_call = FeatureCall::new(
            "sketch",
            sketch_id.clone(),
            json!({
                "plane": "qCreatedBy(origin_plane_xy)",
                "sub_calls": [
                    {
                        "type": "sketch_circle",
                        "id": "c1",
                        "params": { "center": [2.0, 4.0], "radius": 7.0 }
                    }
                ]
            }),
        );
        let extrude_call = FeatureCall::new(
            "extrude",
            extrude_id.clone(),
            json!({
                "profiles": format!("qCreatedBy({sketch_id})"),
                "depth": { "value": 10.0, "unit": "mm" },
                "profile_half_x": 999.0,
                "profile_half_y": 999.0,
            }),
        );
        registry
            .regenerate(&mut ctx, &[sketch_call, extrude_call])
            .unwrap();

        let extrude_feature = doc
            .features
            .iter()
            .find(|f| f.id == extrude_id)
            .expect("extrude spine entry");
        // K7.3: circle sketch no longer lowers to its bounding
        // box — Primitive::Extrude carries the full Sketch2d,
        // so the downstream mesher gets a real cylinder.
        match &extrude_feature.primitive {
            Primitive::Extrude { sketch, .. } => {
                assert!(matches!(
                    sketch,
                    ::fielddraft_protocol::Sketch2d::Prim(
                        ::fielddraft_protocol::Sketch2dPrimitive::Circle { .. },
                    )
                ));
                let aabb = sketch.aabb();
                // Circle centre (2, 4) radius 7 → aabb extents ±7
                // around the centre.
                assert!((aabb.min[0] - (-5.0)).abs() < 1e-5);
                assert!((aabb.max[0] - 9.0).abs() < 1e-5);
            }
            other => panic!("expected Primitive::Extrude, got {other:?}"),
        }
    }

    // ─── K8.4: revolve ────────────────────────────────────────

    #[test]
    fn revolve_lowers_rectangle_sketch_into_primitive_revolve() {
        // Profile: a 4 × 20 mm rectangle at r=5, so revolving
        // it around the Y axis gives a tube.
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let sketch_id = id(1);
        let revolve_id = id(2);

        let sketch_call = FeatureCall::new(
            "sketch",
            sketch_id.clone(),
            json!({
                "plane": "qCreatedBy(origin_plane_xy)",
                "sub_calls": [
                    {
                        "type": "sketch_rectangle",
                        "id": "r1",
                        "params": {
                            "corner_a": [5.0, -10.0],
                            "corner_b": [9.0, 10.0],
                        }
                    }
                ]
            }),
        );
        let revolve_call = FeatureCall::new(
            "revolve",
            revolve_id.clone(),
            json!({
                "profiles": format!("qCreatedBy({sketch_id})"),
            }),
        );
        registry
            .regenerate(&mut ctx, &[sketch_call, revolve_call])
            .unwrap();

        let revolve_feature = doc
            .features
            .iter()
            .find(|f| f.id == revolve_id)
            .expect("revolve spine entry");
        match &revolve_feature.primitive {
            Primitive::Revolve { sketch, center } => {
                assert_eq!(*center, [0.0, 0.0, 0.0]);
                let aabb = sketch.aabb();
                assert!((aabb.min[0] - 5.0).abs() < 1e-5);
                assert!((aabb.max[0] - 9.0).abs() < 1e-5);
                assert!((aabb.min[1] - (-10.0)).abs() < 1e-5);
                assert!((aabb.max[1] - 10.0).abs() < 1e-5);
            }
            other => panic!("expected Primitive::Revolve, got {other:?}"),
        }
    }

    #[test]
    fn revolve_errors_when_sketch_cannot_be_resolved() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let err = registry
            .regenerate(
                &mut ctx,
                &[FeatureCall::new(
                    "revolve",
                    id(1),
                    json!({
                        "profiles": "qCreatedBy(sketch_999)",
                    }),
                )],
            )
            .unwrap_err();
        assert!(matches!(
            err,
            FeatureError::MissingParameter {
                feature: "revolve",
                name: "profiles",
            }
        ));
    }

    #[test]
    fn extrude_falls_back_to_profile_half_extents_when_sketch_is_missing() {
        // Query that doesn't point at any context sketch — the
        // executor must fall through to the hand-written
        // fallback extents so the K4.5.8 test surface stays
        // intact.
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        ExtrudeFeature
            .execute(
                &mut ctx,
                &id(1),
                &extrude_def("qCreatedBy(sketch_999)", 10.0, 3.0, 4.0),
            )
            .unwrap();
        let f = &doc.features[0];
        if let Primitive::Box { half_extents, .. } = &f.primitive {
            assert_eq!(*half_extents, [3.0, 4.0, 5.0]);
        } else {
            panic!("expected Primitive::Box");
        }
    }

    /// Regression for the UX-test bug where clicking the Extrude
    /// toolbar button on a sketch that only has lines/points
    /// crashed with "profile_half_x invalid: must be > 0".
    /// When the profiles query resolves to an actually-stored
    /// sketch *and* the caller left the legacy fallback fields
    /// at 0 (the K7.3 flow), the extrude should return a clean,
    /// user-facing error instead of falling through to the
    /// profile_half_x positive-guard.
    #[test]
    fn extrude_returns_clean_error_when_sketch_is_empty_and_no_fallback_dims() {
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let sketch_id = id(1);
        let extrude_id = id(2);

        let result = registry.regenerate(
            &mut ctx,
            &[
                FeatureCall::new(
                    "sketch",
                    sketch_id.clone(),
                    json!({
                        "plane": "qCreatedBy(origin_plane_xy)",
                        "sub_calls": [
                            {
                                "type": "sketch_line",
                                "id": "l1",
                                "params": {
                                    "start": [0.0, 0.0],
                                    "end": [10.0, 0.0],
                                }
                            }
                        ]
                    }),
                ),
                FeatureCall::new(
                    "extrude",
                    extrude_id.clone(),
                    json!({
                        "profiles": format!("qCreatedBy({sketch_id})"),
                        "depth": { "value": 8.0, "unit": "mm" },
                        "profile_half_x": 0.0,
                        "profile_half_y": 0.0,
                    }),
                ),
            ],
        );
        let err = result.err().expect("extrude must fail on empty sketch with zero fallback");
        match err {
            FeatureError::InvalidParameter { name, message, .. } => {
                assert_eq!(name, "profiles");
                assert!(
                    message.contains("closed regions"),
                    "expected user-facing message, got {message}"
                );
            }
            other => panic!("expected InvalidParameter, got {other:?}"),
        }
    }

    #[test]
    fn extrude_falls_back_when_sketch_has_no_region_primitives() {
        // Sketch with only a line — build_sketch2d_from_sub_calls
        // returns Sketch2d::Empty, and the resolver falls through.
        let registry = FeatureRegistry::with_builtins();
        let mut doc = empty_ctx();
        let mut ctx = Context::new(&mut doc);
        let sketch_id = id(1);
        let extrude_id = id(2);

        registry
            .regenerate(
                &mut ctx,
                &[
                    FeatureCall::new(
                        "sketch",
                        sketch_id.clone(),
                        json!({
                            "plane": "qCreatedBy(origin_plane_xy)",
                            "sub_calls": [
                                {
                                    "type": "sketch_line",
                                    "id": "l1",
                                    "params": {
                                        "start": [0.0, 0.0],
                                        "end": [10.0, 0.0],
                                    }
                                }
                            ]
                        }),
                    ),
                    FeatureCall::new(
                        "extrude",
                        extrude_id.clone(),
                        json!({
                            "profiles": format!("qCreatedBy({sketch_id})"),
                            "depth": { "value": 8.0, "unit": "mm" },
                            "profile_half_x": 2.5,
                            "profile_half_y": 2.5,
                        }),
                    ),
                ],
            )
            .unwrap();
        let extrude_feature = doc
            .features
            .iter()
            .find(|f| f.id == extrude_id)
            .expect("extrude spine entry");
        if let Primitive::Box { half_extents, .. } = &extrude_feature.primitive {
            assert!((half_extents[0] - 2.5).abs() < 1e-5);
            assert!((half_extents[1] - 2.5).abs() < 1e-5);
        } else {
            panic!("expected Primitive::Box");
        }
    }
}
