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
//! The trait is called `FeatureKind`, not `Feature`. The fat
//! `protocol::Feature` spine struct it once collided with is gone
//! (DL-61 C2b): the wire form is `Vec<FeatureCall>` and the decoded
//! runtime form is [`crate::csg::DecodedFeature`]. Each `FeatureKind`
//! lowers a `FeatureCall` into one or more `DecodedFeature`s.

use std::collections::HashMap;
use std::fmt;

use serde_json::Value;

use fielddraft_protocol::{BoolOp, Document, FeatureId};

use crate::csg::DecodedFeature;

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

// ─── Modul-Layout ────────────────────────────────────────────────
//
// features.rs wurde mechanisch in ein Modul-Verzeichnis gesplittet
// (eine Datei pro Feature-Familie). Die flachen `crate::features::*`-
// Pfade bleiben über die Re-Exports unten stabil.

mod context_resolve;

pub mod boolean;
pub mod custom_sdf;
pub mod extrude;
pub mod fillet_shell;
pub mod gyroid;
pub mod helix;
pub mod hole;
pub mod imported_mesh;
pub mod material;
pub mod mate_connector;
pub mod merge_scope;
pub mod patterns;
pub mod primitives;
pub mod revolve;
pub mod sweep;
pub mod thicken;
pub mod transforms;

pub use boolean::*;
pub use custom_sdf::*;
pub use extrude::*;
pub use fillet_shell::*;
pub use gyroid::*;
pub use helix::*;
pub use hole::*;
pub use imported_mesh::*;
pub use material::*;
pub use mate_connector::*;
pub use patterns::*;
pub use primitives::*;
pub use revolve::*;
pub use sweep::*;
pub use thicken::*;
pub use transforms::*;

// Crate-interne Helfer, die Tests bzw. Nachbarmodule unter dem alten
// flachen Pfad erwarten.
#[cfg(test)]
pub(crate) use context_resolve::place_point;
pub(crate) use merge_scope::merge_scope_warning;
#[cfg(test)]
pub(crate) use revolve::revolve_axis_alignment;


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
    /// DL-61 C2b: the decoded feature spine accumulator. Each `execute`
    /// pushes its [`DecodedFeature`] entries here (and the transform/
    /// boolean/pattern features mutate earlier entries in place); the
    /// kernel consumes the finished list directly once regeneration
    /// completes. The protocol [`Document`] no longer carries a
    /// `features` field — `feature_calls` is the sole persisted form.
    pub features: Vec<DecodedFeature>,
    /// K6b.1: solved sketch state per sketch feature id.
    pub sketches: HashMap<FeatureId, crate::sketch::SketchData>,
    /// K10.4: edge fillets collected during regen, applied as a
    /// post-processing pass on the CsgNode tree.
    pub edge_fillets: Vec<crate::csg::EdgeFillet>,
    /// d: the global round radius from the last `round_all` feature, applied
    /// as a post-pass after `edge_fillets` (rounds every remaining Boolean
    /// junction). `None` when no `round_all` is active.
    pub global_round: Option<f32>,
    /// C0: shared lazy B-Rep cache over `features`. A feature can
    /// resolve element-level queries (`qFace(h)`, `qCreatedBy(id)`, …)
    /// against the geometry produced by every feature *so far* —
    /// mid-loop. The content key auto-invalidates whenever a feature
    /// pushes, so no manual clearing is needed between `execute` calls.
    pub brep_cache: crate::body::BrepCache,
    /// C2: resolved CSYS of every `mate_connector` placed this regen.
    /// Captured by the kernel after regeneration so the viewport can
    /// render datum triads without re-resolving the face query.
    pub mate_connectors: Vec<ResolvedMateConnector>,
    /// Per-feature merge-scope warnings (non-fatal). A feature-implicit
    /// boolean (Add/Remove/Intersect) whose merge scope resolves to no
    /// target body records a warning here — geometry still regenerates (the
    /// body shows as a red translucent preview), but the UI flags the
    /// feature so the user picks a scope or "Merge with all".
    pub feature_warnings: Vec<(FeatureId, String)>,
}

impl<'a> Context<'a> {
    pub fn new(document: &'a mut Document) -> Self {
        Self {
            document,
            features: Vec::new(),
            sketches: HashMap::new(),
            edge_fillets: Vec::new(),
            global_round: None,
            brep_cache: crate::body::BrepCache::new(),
            mate_connectors: Vec::new(),
            feature_warnings: Vec::new(),
        }
    }

    /// C0: resolve a query against the geometry woven from the features
    /// accumulated so far. Lazily (re)weaves via [`Context::brep_cache`]
    /// then delegates to [`crate::queries::resolve_query_topo`].
    pub fn resolve_topo(
        &mut self,
        expr: &crate::queries::QueryExpr,
    ) -> Vec<crate::brep::naming::TopoRef> {
        let woven = self.brep_cache.ensure(&self.features);
        let breps: Vec<crate::brep::BRep> = woven.iter().map(|(b, _)| b.clone()).collect();
        crate::queries::resolve_query_topo(&breps, expr)
    }

    /// Seed feature id of every body partitioned from the features decoded so
    /// far — i.e. the bodies that exist *before* the feature currently being
    /// executed. This is the "all bodies" set a "Merge with all" boolean
    /// folds into (see [`merge_scope::resolve_merge_scope`]).
    pub fn current_body_seed_ids(&self) -> Vec<FeatureId> {
        crate::body::partition(&self.features)
            .iter()
            .map(|p| p.seed_id(&self.features))
            .collect()
    }

    /// Merge-scope warning for a feature-implicit boolean, accounting for
    /// whether any body exists *before* this feature (a scope-less first `Add`
    /// is an implicit "New", not a warning). See [`merge_scope_warning`].
    pub fn merge_scope_warning(&self, op: BoolOp, resolved_targets: &[FeatureId]) -> Option<String> {
        let has_prior_bodies = !self.current_body_seed_ids().is_empty();
        merge_scope_warning(op, resolved_targets, has_prior_bodies)
    }

    /// Record a non-fatal merge-scope warning for `id` (see
    /// [`Context::feature_warnings`]).
    pub fn warn_feature(&mut self, id: &FeatureId, message: String) {
        self.feature_warnings.push((id.clone(), message));
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
    /// in the serialised feature tree. Built-ins return their static
    /// `TYPE_NAME`; a FieldSkript [`ScriptFeature`](crate::script_runtime::ScriptFeature)
    /// borrows its runtime `record.id`, so the return is `&self`-bound,
    /// not `'static`.
    fn type_name(&self) -> &str;

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
        r.register(Box::new(ThickenFeature));
        r.register(Box::new(RevolveFeature));
        r.register(Box::new(HelixFeature));
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
        r.register(Box::new(RoundAllFeature));
        r.register(Box::new(ShellFeature));
        r.register(Box::new(ImportedMeshFeature));
        r.register(Box::new(GyroidFeature));
        r.register(Box::new(CustomSdfFeature));
        r.register(Box::new(LinearPatternFeature));
        r.register(Box::new(CircularPatternFeature));
        r.register(Box::new(MirrorFeature));
        r.register(Box::new(crate::sketch::SketchFeature::new()));
        r.register(Box::new(TranslateFeature));
        r.register(Box::new(RotateFeature));
        r.register(Box::new(ScaleFeature));
        r.register(Box::new(BooleanFeature));
        r.register(Box::new(MaterialFeature));
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
    /// Walk the calls in document order and execute each one. A failing
    /// feature is **recorded and skipped** instead of fail-stopping the
    /// whole document (Onshape semantics, ticket sx5a): it contributes no
    /// geometry, the rest of the model regenerates normally, and the UI
    /// surfaces the per-feature error (red title + tooltip). Returns the
    /// collected per-feature errors — empty means a clean regen.
    pub fn regenerate(
        &self,
        ctx: &mut Context<'_>,
        calls: &[FeatureCall],
        rollback_after: Option<usize>,
    ) -> RegenErrors {
        // K8.11: snapshot the variables table once per
        // regenerate — execute paths don't mutate it, and
        // cloning here means the param resolver can borrow
        // without tangling with the `ctx.document` mutable
        // borrow.
        // The formula resolver only needs name → numeric value; the variable
        // kind is a UI concern, so project the table down to an f32 map.
        let variables: std::collections::BTreeMap<String, f32> = ctx
            .document
            .variables
            .iter()
            .map(|(name, var)| (name.clone(), var.value))
            .collect();
        let mut errors: Vec<(FeatureId, FeatureError)> = Vec::new();
        for (i, call) in calls.iter().enumerate() {
            if call.suppressed || rollback_after.is_some_and(|ra| i >= ra) {
                continue;
            }
            let Some(feature) = self.features.get(&call.type_name) else {
                errors.push((
                    call.id.clone(),
                    FeatureError::UnknownFeatureType(call.type_name.clone()),
                ));
                continue;
            };
            // K8.11: resolve any formula-valued params (e.g.
            // `{value: "=width/2", unit: "mm"}`) against the
            // document variables before handing them off to
            // `execute`. Features see plain numeric values and
            // don't need to know the formula engine exists.
            let mut resolved = call.params.clone();
            if let Err(e) =
                crate::expr::resolve_params_with_variables(&mut resolved, &variables)
            {
                errors.push((
                    call.id.clone(),
                    FeatureError::InvalidParameter {
                        feature: "<formula>",
                        name: "params",
                        message: format!("{} (in {})", e, call.type_name),
                    },
                ));
                continue;
            }
            if let Err(e) = feature.execute(ctx, &call.id, &resolved) {
                errors.push((call.id.clone(), e));
            }
        }
        RegenErrors(errors)
    }
}

/// Per-feature errors collected by the error-tolerant [`FeatureRegistry::
/// regenerate`] (ticket sx5a) — one entry per failing call, in document
/// order. The `unwrap`/`unwrap_err`/`expect`/`is_err` mirrors keep the
/// pre-tolerance `Result` call-site ergonomics: "unwrap" asserts a clean
/// regen, "unwrap_err" asserts at least one failure and yields the first.
#[derive(Debug, Default)]
#[must_use = "a regen may have recorded per-feature errors"]
pub struct RegenErrors(pub Vec<(FeatureId, FeatureError)>);

impl RegenErrors {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_ok(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_err(&self) -> bool {
        !self.0.is_empty()
    }

    /// First failing feature's error, if any.
    pub fn err(self) -> Option<FeatureError> {
        self.0.into_iter().next().map(|(_, e)| e)
    }

    /// Panic if any feature failed (mirror of the old `Result::unwrap`).
    #[track_caller]
    pub fn unwrap(self) {
        if let Some((id, err)) = self.0.first() {
            panic!("feature {id} failed to regenerate: {err}");
        }
    }

    /// Panic with `msg` if any feature failed (mirror of `Result::expect`).
    #[track_caller]
    pub fn expect(self, msg: &str) {
        if let Some((id, err)) = self.0.first() {
            panic!("{msg}: feature {id} failed to regenerate: {err}");
        }
    }

    /// Panic if the regen was CLEAN; otherwise return the first failing
    /// feature's error (mirror of the old `Result::unwrap_err`).
    #[track_caller]
    pub fn unwrap_err(self) -> FeatureError {
        match self.0.into_iter().next() {
            Some((_, e)) => e,
            None => panic!("regenerate succeeded but an error was expected"),
        }
    }

    /// Like [`Self::unwrap_err`] but with a caller message (mirror of
    /// `Result::expect_err`).
    #[track_caller]
    pub fn expect_err(self, msg: &str) -> FeatureError {
        match self.0.into_iter().next() {
            Some((_, e)) => e,
            None => panic!("{msg}: regenerate succeeded but an error was expected"),
        }
    }
}


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


// ─── tests ───────────────────────────────────────────────────────

#[cfg(test)]
#[path = "../features_tests.rs"]
mod tests;
