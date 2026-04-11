//! Sketch sub-feature-tree: per-sketch `Vec<FeatureCall>` spine.
//!
//! K6b.1, the first slice of the sketch editor. This module stands
//! up the kernel-side infrastructure that the memory note in
//! `project_sketch_subtree.md` requires: every sketch owns its own
//! fully-typed feature spine, every sketch tool is a feature with
//! a schema, and regeneration walks the sub-spine the same way
//! top-level regeneration walks the document spine.
//!
//! ## Shape of the design
//!
//! * A new top-level `FeatureKind` called `sketch` lives in the
//!   main [`FeatureRegistry`]. Its definition JSON carries two
//!   fields:
//!     - `plane` — a [`Query`] identifying the planar target the
//!       sketch is locked against. K6b.1 stores the query
//!       verbatim but does not resolve it; the face-lock UX is
//!       K6b.2's job.
//!     - `sub_calls` — an ordered array of nested
//!       [`FeatureCall`]s, the sketch's own feature spine.
//!
//! * Each entry of `sub_calls` dispatches through a **separate**
//!   [`SketchToolRegistry`]. That registry's types implement
//!   [`SketchToolKind`], not [`FeatureKind`] — a sketch tool's
//!   job is to push entities into a [`SketchState`], not to
//!   mutate the document spine. Keeping the two registries
//!   disjoint means a top-level call like `box` can't accidentally
//!   appear inside a sketch and vice versa; the type namespaces
//!   stay clean and regeneration errors point at the right layer.
//!
//! * The sketch feature's `execute` builds a fresh [`SketchState`],
//!   walks its sub_calls through the sketch-tool registry, and
//!   stashes the resulting state on
//!   [`Context::sketches`] keyed by the sketch's feature id. Later
//!   slices (K6b.2+) will route that state through the PlaneGCS
//!   solver and resolve downstream `qSketchRegion` queries against
//!   it. K6b.1 stops short of solving because the solver lives in
//!   JavaScript — the state is ready for the JS-side wrapper to
//!   pick up.
//!
//! ## Built-in sketch tools in K6b.1
//!
//! * `sketch_point { at: [x, y] }` — one free point.
//! * `sketch_line  { start: [x, y], end: [x, y] }` — two points
//!   plus a line entity linking them.
//!
//! That's the minimum set needed to smoke-test the sub-spine
//! pipeline end-to-end. Rectangle / Circle / Arc / Trim / Mirror /
//! Offset land in K6b.3 and K6b.4, each as their own
//! `SketchToolKind` impl.

use std::collections::HashMap;

use serde_json::Value;

use fielddraft_protocol::{FeatureCall, FeatureId, Query};
use fielddraft_solver::{Constraint, ConstraintId, EntityId, Point2, SketchState};

use crate::features::{Context, FeatureError, FeatureKind, FeatureSchema};

// ─── sketch-tool layer ──────────────────────────────────────────

/// One sketch-local tool. Stateless, like [`FeatureKind`], but
/// mutates a [`SketchState`] instead of a [`Context`]. Sketch tools
/// have no access to the document spine — they live entirely
/// inside the sketch they're registered against, which is how the
/// sub-spine stays isolated from the top-level feature tree.
pub trait SketchToolKind: Send + Sync {
    /// Registry key — stable, snake_case, matches the `type_name`
    /// field on a [`FeatureCall`] that appears inside a sketch's
    /// `sub_calls` list.
    fn type_name(&self) -> &'static str;

    /// Schema JSON for this tool's definition struct, same shape
    /// as [`FeatureKind::schema_json`]. Used by the K6b.2
    /// Sketch-toolbar to feed the generic FeatureDialog.
    fn schema_json(&self) -> Value;

    /// Validate the definition JSON and push entities / constraints
    /// into `state`. On error the sketch-level `execute` stops —
    /// same fail-stop rule as the top-level registry.
    fn apply(
        &self,
        state: &mut SketchState,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError>;
}

/// Registry of sketch tools keyed by `type_name`. Separate from
/// [`crate::features::FeatureRegistry`] so sketch-local types and
/// document-level types live in disjoint namespaces.
pub struct SketchToolRegistry {
    tools: HashMap<String, Box<dyn SketchToolKind>>,
}

impl Default for SketchToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SketchToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Empty registry pre-populated with every built-in sketch
    /// tool.
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        r.register(Box::new(SketchPointTool));
        r.register(Box::new(SketchLineTool));
        r.register(Box::new(SketchRectangleTool));
        r.register(Box::new(SketchHorizontalConstraintTool));
        r.register(Box::new(SketchVerticalConstraintTool));
        r.register(Box::new(SketchCoincidentConstraintTool));
        r
    }

    pub fn register(&mut self, tool: Box<dyn SketchToolKind>) {
        self.tools.insert(tool.type_name().to_string(), tool);
    }

    pub fn get(&self, type_name: &str) -> Option<&dyn SketchToolKind> {
        self.tools.get(type_name).map(|b| b.as_ref())
    }

    pub fn type_names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub fn schema_for(&self, type_name: &str) -> Option<Value> {
        self.tools.get(type_name).map(|t| t.schema_json())
    }

    /// Walk a sub-spine, dispatching each call through `get` and
    /// applying it to `state`. Fails fast like the top-level
    /// regenerator.
    pub fn apply_all(
        &self,
        state: &mut SketchState,
        calls: &[FeatureCall],
    ) -> Result<(), FeatureError> {
        for call in calls {
            let tool = self
                .tools
                .get(&call.type_name)
                .ok_or_else(|| FeatureError::UnknownFeatureType(call.type_name.clone()))?;
            tool.apply(state, &call.id, &call.params)?;
        }
        Ok(())
    }
}

// ─── sketch_point ───────────────────────────────────────────────

/// `sketch_point` — a free 2D point in sketch coordinates.
#[derive(FeatureSchema)]
#[schema(type = "sketch_point")]
pub struct SketchPointDef {
    pub at: [f32; 2],
}

pub struct SketchPointTool;

impl SketchToolKind for SketchPointTool {
    fn type_name(&self) -> &'static str {
        SketchPointDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchPointDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchPointDef::parse(def)?;
        state.add_point(Point2::new(d.at[0] as f64, d.at[1] as f64));
        Ok(())
    }
}

// ─── sketch_line ────────────────────────────────────────────────

/// `sketch_line` — a 2D line segment between two endpoints. The
/// endpoints become their own points in the sketch state; the
/// line entity links them.
#[derive(FeatureSchema)]
#[schema(type = "sketch_line")]
pub struct SketchLineDef {
    pub start: [f32; 2],
    pub end: [f32; 2],
}

pub struct SketchLineTool;

impl SketchToolKind for SketchLineTool {
    fn type_name(&self) -> &'static str {
        SketchLineDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchLineDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchLineDef::parse(def)?;
        let start = state.add_point(Point2::new(d.start[0] as f64, d.start[1] as f64));
        let end = state.add_point(Point2::new(d.end[0] as f64, d.end[1] as f64));
        state.add_line(start, end);
        Ok(())
    }
}

// ─── K6b.3e: rectangle ──────────────────────────────────────────
//
// `sketch_rectangle` is the first *composite* sketch tool: a
// single sub-spine call that materialises into four points,
// four lines and four H/V constraints in one apply pass. The
// UI sees one entry in the sub-spine list and one preview on
// the 2D canvas, but the underlying `SketchState` is wired up
// the same way Onshape's corner-rectangle landing is — four
// horizontal/vertical-constrained edges the solver can keep
// axis-aligned through downstream edits.
//
// The allocation order below is load-bearing: `buildDraftSketchState`
// on the TS side replicates it byte-for-byte so predicted entity
// ids match the kernel's. Do not reorder the `add_point` /
// `add_line` / `add_constraint` calls without updating the TS
// mirror.

/// `sketch_rectangle` — axis-aligned rectangle defined by two
/// diagonal corners. Internally lowers to four corner points,
/// four edge lines and four H/V constraints.
#[derive(FeatureSchema)]
#[schema(type = "sketch_rectangle")]
pub struct SketchRectangleDef {
    pub corner_a: [f32; 2],
    pub corner_b: [f32; 2],
}

pub struct SketchRectangleTool;

impl SketchToolKind for SketchRectangleTool {
    fn type_name(&self) -> &'static str {
        SketchRectangleDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchRectangleDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchRectangleDef::parse(def)?;
        let ax = d.corner_a[0] as f64;
        let ay = d.corner_a[1] as f64;
        let bx = d.corner_b[0] as f64;
        let by = d.corner_b[1] as f64;
        // Corners in a fixed allocation order: bl, br, tr, tl.
        // The TS mirror predicts entity ids from this ordering,
        // so changing it is a cross-crate break.
        let min_x = ax.min(bx);
        let min_y = ay.min(by);
        let max_x = ax.max(bx);
        let max_y = ay.max(by);
        let bl = state.add_point(Point2::new(min_x, min_y));
        let br = state.add_point(Point2::new(max_x, min_y));
        let tr = state.add_point(Point2::new(max_x, max_y));
        let tl = state.add_point(Point2::new(min_x, max_y));
        // Edges, same order: bottom, right, top, left.
        let bottom = state.add_line(bl, br);
        let right = state.add_line(br, tr);
        let top = state.add_line(tr, tl);
        let left = state.add_line(tl, bl);
        // Horizontal first (bottom, top), then vertical (right,
        // left) — the TS constraint-id mirror uses this order.
        state.add_constraint(|id| Constraint::Horizontal { id, line: bottom });
        state.add_constraint(|id| Constraint::Horizontal { id, line: top });
        state.add_constraint(|id| Constraint::Vertical { id, line: right });
        state.add_constraint(|id| Constraint::Vertical { id, line: left });
        Ok(())
    }
}

// ─── K6b.3d: constraint tools ───────────────────────────────────
//
// K6b.3d teaches the sketch sub-spine to carry constraint calls
// alongside the geometric ones. Each constraint tool references
// sketch-local entity ids via raw `u32` params — the UI predicts
// those ids by replicating the kernel's entity allocation order
// in `buildDraftSketchState`, so the committed calls target the
// same entities the kernel will create at regeneration time.
//
// The constraints land in `SketchState::constraints`. Actual
// solving happens in JavaScript via the K5a `solveSketch`
// wrapper — the kernel just persists the constraint network so
// it survives save / load / edit cycles.

/// `sketch_constraint_horizontal` — locks a line to the
/// sketch-plane X axis.
#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_horizontal")]
pub struct SketchHorizontalConstraintDef {
    pub line: u32,
}

pub struct SketchHorizontalConstraintTool;

impl SketchToolKind for SketchHorizontalConstraintTool {
    fn type_name(&self) -> &'static str {
        SketchHorizontalConstraintDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchHorizontalConstraintDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchHorizontalConstraintDef::parse(def)?;
        state.add_constraint(|cid: ConstraintId| Constraint::Horizontal {
            id: cid,
            line: EntityId(d.line),
        });
        Ok(())
    }
}

/// `sketch_constraint_vertical` — locks a line to the sketch-plane
/// Y axis.
#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_vertical")]
pub struct SketchVerticalConstraintDef {
    pub line: u32,
}

pub struct SketchVerticalConstraintTool;

impl SketchToolKind for SketchVerticalConstraintTool {
    fn type_name(&self) -> &'static str {
        SketchVerticalConstraintDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchVerticalConstraintDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchVerticalConstraintDef::parse(def)?;
        state.add_constraint(|cid: ConstraintId| Constraint::Vertical {
            id: cid,
            line: EntityId(d.line),
        });
        Ok(())
    }
}

/// `sketch_constraint_coincident` — forces two sketch points to
/// coincide. Used by the K6b.3c endpoint snap when the user
/// starts a new line on top of an existing point.
#[derive(FeatureSchema)]
#[schema(type = "sketch_constraint_coincident")]
pub struct SketchCoincidentConstraintDef {
    pub a: u32,
    pub b: u32,
}

pub struct SketchCoincidentConstraintTool;

impl SketchToolKind for SketchCoincidentConstraintTool {
    fn type_name(&self) -> &'static str {
        SketchCoincidentConstraintDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        SketchCoincidentConstraintDef::schema_json()
    }

    fn apply(
        &self,
        state: &mut SketchState,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = SketchCoincidentConstraintDef::parse(def)?;
        state.add_constraint(|cid: ConstraintId| Constraint::Coincident {
            id: cid,
            a: EntityId(d.a),
            b: EntityId(d.b),
        });
        Ok(())
    }
}

// ─── top-level `sketch` feature ─────────────────────────────────

/// `sketch` — a top-level feature that owns a sub-spine of sketch
/// tools. The definition JSON is parsed by hand because it carries
/// a nested `Vec<FeatureCall>` that the `FeatureSchema` derive
/// does not model (the derive intentionally stays flat).
pub struct SketchFeature {
    tools: SketchToolRegistry,
}

impl Default for SketchFeature {
    fn default() -> Self {
        Self {
            tools: SketchToolRegistry::with_builtins(),
        }
    }
}

impl SketchFeature {
    pub const TYPE_NAME: &'static str = "sketch";

    pub fn new() -> Self {
        Self::default()
    }

    /// Schema JSON that the K6b.2 outliner can hand to the generic
    /// FeatureDialog. K6b.1 emits a minimal shape: a `query`
    /// parameter for the plane and a `generic` sub_calls param
    /// that falls into the JSON-textarea escape hatch on the TS
    /// side. K6b.2 will specialise the sub_calls display with a
    /// dedicated tree widget.
    pub fn static_schema_json() -> Value {
        serde_json::json!({
            "type": Self::TYPE_NAME,
            "params": [
                {
                    "key": "plane",
                    "name": "Plane",
                    "type": "query",
                    "required": true,
                },
                {
                    "key": "sub_calls",
                    "name": "Sketch tools",
                    "type": "generic",
                    "required": false,
                },
            ],
        })
    }

    pub fn tools(&self) -> &SketchToolRegistry {
        &self.tools
    }
}

/// Parsed view of a sketch feature's definition JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedSketchDef {
    pub plane: Query,
    pub sub_calls: Vec<FeatureCall>,
}

impl ParsedSketchDef {
    pub fn parse(def: &Value) -> Result<Self, FeatureError> {
        let plane_slot = def.get("plane").ok_or(FeatureError::MissingParameter {
            feature: SketchFeature::TYPE_NAME,
            name: "plane",
        })?;
        let plane: Query = serde_json::from_value(plane_slot.clone()).map_err(|e| {
            FeatureError::InvalidParameter {
                feature: SketchFeature::TYPE_NAME,
                name: "plane",
                message: format!("expected a Query string: {e}"),
            }
        })?;

        let sub_calls = match def.get("sub_calls") {
            None | Some(Value::Null) => Vec::new(),
            Some(slot) => {
                let arr = slot.as_array().ok_or_else(|| FeatureError::InvalidParameter {
                    feature: SketchFeature::TYPE_NAME,
                    name: "sub_calls",
                    message: "expected an array of FeatureCalls".to_string(),
                })?;
                let mut out = Vec::with_capacity(arr.len());
                for (i, entry) in arr.iter().enumerate() {
                    let call: FeatureCall =
                        serde_json::from_value(entry.clone()).map_err(|e| {
                            FeatureError::InvalidParameter {
                                feature: SketchFeature::TYPE_NAME,
                                name: "sub_calls",
                                message: format!("entry {i} is not a FeatureCall: {e}"),
                            }
                        })?;
                    out.push(call);
                }
                out
            }
        };

        Ok(Self { plane, sub_calls })
    }
}

impl FeatureKind for SketchFeature {
    fn type_name(&self) -> &'static str {
        Self::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        Self::static_schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let parsed = ParsedSketchDef::parse(def)?;
        let _plane = parsed.plane; // K6b.2 will resolve this
        let mut state = SketchState::with_origin();
        self.tools.apply_all(&mut state, &parsed.sub_calls)?;
        ctx.sketches.insert(id.clone(), state);
        Ok(())
    }
}

// ─── tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use fielddraft_protocol::{Document, FeatureCall, FeatureId};
    use serde_json::json;

    use crate::features::{Context, FeatureError, FeatureRegistry};

    fn id(n: u32) -> FeatureId {
        FeatureId::from_u32(n)
    }

    #[test]
    fn sketch_point_tool_pushes_a_point() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        let call = FeatureCall::new("sketch_point", id(1), json!({ "at": [3.0, 4.0] }));
        tools.apply_all(&mut state, std::slice::from_ref(&call)).unwrap();
        // origin + the new point
        assert_eq!(state.entities.len(), 2);
    }

    #[test]
    fn sketch_line_tool_pushes_two_points_and_a_line() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::new(); // no origin — keep the count easy
        let call = FeatureCall::new(
            "sketch_line",
            id(1),
            json!({ "start": [0.0, 0.0], "end": [10.0, 0.0] }),
        );
        tools.apply_all(&mut state, std::slice::from_ref(&call)).unwrap();
        // two points + one line
        assert_eq!(state.entities.len(), 3);
    }

    #[test]
    fn unknown_sketch_tool_type_is_reported() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::new();
        let call = FeatureCall::new("not_a_tool", id(1), json!({}));
        let err = tools.apply_all(&mut state, std::slice::from_ref(&call)).unwrap_err();
        assert_eq!(err, FeatureError::UnknownFeatureType("not_a_tool".to_string()));
    }

    #[test]
    fn sketch_feature_regenerates_sub_spine_into_context_sketches() {
        let mut registry = FeatureRegistry::new();
        registry.register(Box::new(SketchFeature::new()));

        let sketch_id = id(1);
        let call = FeatureCall::new(
            SketchFeature::TYPE_NAME,
            sketch_id.clone(),
            json!({
                "plane": "qCreatedBy(origin_plane_xy)",
                "sub_calls": [
                    {
                        "type": "sketch_point",
                        "id": "100",
                        "params": { "at": [0.0, 0.0] }
                    },
                    {
                        "type": "sketch_line",
                        "id": "101",
                        "params": {
                            "start": [0.0, 0.0],
                            "end": [20.0, 0.0]
                        }
                    }
                ]
            }),
        );

        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        registry.regenerate(&mut ctx, std::slice::from_ref(&call)).unwrap();

        let state = ctx
            .sketches
            .get(&sketch_id)
            .expect("sketch should be stashed under its feature id");
        // origin (1) + sketch_point (1) + sketch_line's two points + line (3)
        assert_eq!(state.entities.len(), 5);
    }

    #[test]
    fn sketch_feature_rejects_unknown_sub_tool() {
        let mut registry = FeatureRegistry::new();
        registry.register(Box::new(SketchFeature::new()));

        let call = FeatureCall::new(
            SketchFeature::TYPE_NAME,
            id(1),
            json!({
                "plane": "qCreatedBy(origin_plane_xy)",
                "sub_calls": [
                    { "type": "not_a_tool", "id": "100", "params": {} }
                ]
            }),
        );

        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        let err = registry
            .regenerate(&mut ctx, std::slice::from_ref(&call))
            .unwrap_err();
        assert_eq!(err, FeatureError::UnknownFeatureType("not_a_tool".to_string()));
    }

    #[test]
    fn sketch_feature_requires_plane() {
        let mut registry = FeatureRegistry::new();
        registry.register(Box::new(SketchFeature::new()));

        let call = FeatureCall::new(SketchFeature::TYPE_NAME, id(1), json!({}));
        let mut doc = Document::default();
        let mut ctx = Context::new(&mut doc);
        let err = registry
            .regenerate(&mut ctx, std::slice::from_ref(&call))
            .unwrap_err();
        assert!(matches!(
            err,
            FeatureError::MissingParameter {
                feature: SketchFeature::TYPE_NAME,
                name: "plane"
            }
        ));
    }

    #[test]
    fn sketch_feature_roundtrips_through_json() {
        let call = FeatureCall::new(
            SketchFeature::TYPE_NAME,
            id(1),
            json!({
                "plane": "qCreatedBy(origin_plane_xy)",
                "sub_calls": [
                    { "type": "sketch_point", "id": "100", "params": { "at": [1.0, 2.0] } }
                ]
            }),
        );
        let wire = serde_json::to_string(&call).unwrap();
        let back: FeatureCall = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, call);
    }

    #[test]
    fn sketch_feature_is_included_in_with_builtins() {
        let registry = FeatureRegistry::with_builtins();
        assert!(registry.get(SketchFeature::TYPE_NAME).is_some());
    }

    #[test]
    fn sketch_registry_type_names_lists_all_builtins() {
        // K6b.3e adds sketch_rectangle to the built-in set.
        let tools = SketchToolRegistry::with_builtins();
        let mut names = tools.type_names();
        names.sort();
        assert_eq!(
            names,
            vec![
                "sketch_constraint_coincident".to_string(),
                "sketch_constraint_horizontal".to_string(),
                "sketch_constraint_vertical".to_string(),
                "sketch_line".to_string(),
                "sketch_point".to_string(),
                "sketch_rectangle".to_string(),
            ]
        );
    }

    #[test]
    fn sketch_rectangle_tool_emits_four_points_four_lines_and_four_constraints() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        let call = FeatureCall::new(
            "sketch_rectangle",
            id(1),
            json!({ "corner_a": [0.0, 0.0], "corner_b": [20.0, 10.0] }),
        );
        tools.apply_all(&mut state, std::slice::from_ref(&call)).unwrap();
        let point_count = state
            .entities
            .iter()
            .filter(|e| matches!(e, fielddraft_solver::Entity::Point { .. }))
            .count();
        let line_count = state
            .entities
            .iter()
            .filter(|e| matches!(e, fielddraft_solver::Entity::Line { .. }))
            .count();
        assert_eq!(point_count, 4);
        assert_eq!(line_count, 4);
        assert_eq!(state.constraints.len(), 4);
        let h = state
            .constraints
            .iter()
            .filter(|c| matches!(c, Constraint::Horizontal { .. }))
            .count();
        let v = state
            .constraints
            .iter()
            .filter(|c| matches!(c, Constraint::Vertical { .. }))
            .count();
        assert_eq!(h, 2);
        assert_eq!(v, 2);
    }

    #[test]
    fn sketch_rectangle_normalises_diagonal_corners() {
        // Drag from top-right to bottom-left should still produce
        // a rectangle whose bl corner is at (min_x, min_y).
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        tools
            .apply_all(
                &mut state,
                std::slice::from_ref(&FeatureCall::new(
                    "sketch_rectangle",
                    id(1),
                    json!({ "corner_a": [20.0, 10.0], "corner_b": [0.0, 0.0] }),
                )),
            )
            .unwrap();
        // First allocated entity is bl, which must be at (0, 0).
        let bl = state
            .entities
            .iter()
            .find(|e| matches!(e, fielddraft_solver::Entity::Point { id: fielddraft_solver::EntityId(1), .. }))
            .unwrap();
        if let fielddraft_solver::Entity::Point { at, .. } = bl {
            assert!((at.x - 0.0).abs() < 1e-9);
            assert!((at.y - 0.0).abs() < 1e-9);
        } else {
            panic!("expected point entity");
        }
    }

    #[test]
    fn sketch_horizontal_constraint_tool_pushes_a_horizontal_constraint() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        // Seed a line so we have an entity id to reference.
        let line_call = FeatureCall::new(
            "sketch_line",
            id(1),
            json!({ "start": [0.0, 0.0], "end": [10.0, 0.5] }),
        );
        let cstr_call = FeatureCall::new(
            "sketch_constraint_horizontal",
            id(2),
            // start-point=1, end-point=2, line=3 under with_origin().
            json!({ "line": 3 }),
        );
        tools
            .apply_all(&mut state, &[line_call, cstr_call])
            .unwrap();
        assert_eq!(state.constraints.len(), 1);
        assert!(matches!(
            state.constraints[0],
            Constraint::Horizontal { line: EntityId(3), .. }
        ));
    }

    #[test]
    fn sketch_vertical_and_coincident_constraint_tools_wire_through() {
        let tools = SketchToolRegistry::with_builtins();
        let mut state = SketchState::with_origin();
        // Two free points so the coincident constraint has refs.
        tools
            .apply_all(
                &mut state,
                &[
                    FeatureCall::new("sketch_point", id(1), json!({ "at": [5.0, 5.0] })),
                    FeatureCall::new("sketch_point", id(2), json!({ "at": [5.0, 6.0] })),
                    FeatureCall::new(
                        "sketch_constraint_coincident",
                        id(3),
                        json!({ "a": 1, "b": 2 }),
                    ),
                    FeatureCall::new(
                        "sketch_line",
                        id(4),
                        json!({ "start": [0.0, 0.0], "end": [0.5, 10.0] }),
                    ),
                    FeatureCall::new(
                        "sketch_constraint_vertical",
                        id(5),
                        json!({ "line": 5 }),
                    ),
                ],
            )
            .unwrap();
        assert_eq!(state.constraints.len(), 2);
        assert!(state.constraints.iter().any(|c| matches!(
            c,
            Constraint::Coincident { a: EntityId(1), b: EntityId(2), .. }
        )));
        assert!(state.constraints.iter().any(|c| matches!(
            c,
            Constraint::Vertical { line: EntityId(5), .. }
        )));
    }

    #[test]
    fn sketch_point_schema_reports_vec2_shape() {
        let schema = SketchPointDef::schema_json();
        let params = schema.get("params").unwrap().as_array().unwrap();
        assert_eq!(params.len(), 1);
        let at = &params[0];
        assert_eq!(at.get("key").unwrap(), "at");
        assert_eq!(at.get("type").unwrap(), "vec2");
        assert_eq!(at.get("required").unwrap(), true);
    }
}
