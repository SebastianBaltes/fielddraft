//! Das Top-Level-`sketch`-Feature: löst die Ebenen-Query, führt die
//! Sub-Calls durch die Tool-Registry aus und re-resolved assoziative
//! Kanten-Projektionen (`sketch_project`).

use serde_json::Value;

use fielddraft_protocol::{FeatureCall, FeatureId, Query};
use fielddraft_solver::{SketchState};

use crate::features::{Context, FeatureError, FeatureKind};

use super::SketchData;
use super::frame::SketchFrame;
use super::tools::{SketchProjectDef, SketchToolRegistry};
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
                    "entity_filter": "FACE",
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
                let arr = slot
                    .as_array()
                    .ok_or_else(|| FeatureError::InvalidParameter {
                        feature: SketchFeature::TYPE_NAME,
                        name: "sub_calls",
                        message: "expected an array of FeatureCalls".to_string(),
                    })?;
                let mut out = Vec::with_capacity(arr.len());
                for (i, entry) in arr.iter().enumerate() {
                    let call: FeatureCall = serde_json::from_value(entry.clone()).map_err(|e| {
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

/// Project a world-space edge polyline onto a sketch frame, returning the
/// 2D `(start, end)` endpoints in the frame's `(u, v)` coordinates.
///
/// A straight edge round-trips exactly; a curved edge degrades to its chord
/// (first→last sample) — the same convention [`Context::resolve_axis_world_segment`]
/// uses for an edge-as-axis. Returns `None` for a degenerate (<2-point)
/// polyline. Pure and frame-only so it is unit-testable without a B-Rep
/// (ticket jcb7).
pub fn project_edge_polyline_to_frame(
    polyline: &[[f32; 3]],
    frame: &SketchFrame,
) -> Option<([f32; 2], [f32; 2])> {
    if polyline.len() < 2 {
        return None;
    }
    let start = frame.world_to_local_2d(polyline[0]);
    let end = frame.world_to_local_2d(polyline[polyline.len() - 1]);
    Some((start, end))
}

/// Walk the sub-calls and, for every `sketch_project`, re-resolve its `edge`
/// query against the live B-Rep and overwrite the cached `start`/`end` params
/// with the current projection onto `frame`. This is what makes a projected
/// edge ASSOCIATIVE: the coordinates the downstream `apply` materialises are
/// always the body's *current* geometry, never a frozen snapshot (ticket jcb7).
///
/// An edge that no longer resolves (source feature deleted, etc.) keeps its
/// cached fallback coordinates — the projection stays put rather than
/// vanishing or erroring the whole sketch.
pub(crate) fn resolve_projected_sub_calls(
    ctx: &mut Context<'_>,
    frame: &SketchFrame,
    sub_calls: &mut [FeatureCall],
) {
    for call in sub_calls.iter_mut() {
        if call.type_name != SketchProjectDef::TYPE_NAME {
            continue;
        }
        let Some(edge_slot) = call.params.get("edge") else {
            continue;
        };
        let Ok(query) = serde_json::from_value::<Query>(edge_slot.clone()) else {
            continue;
        };
        let Ok(expr) = crate::queries::parse_query(query.as_str()) else {
            continue;
        };
        let polylines = ctx.resolve_edge_polylines(&expr);
        let Some(poly) = polylines.iter().find(|p| p.len() >= 2) else {
            continue;
        };
        let Some((start, end)) = project_edge_polyline_to_frame(poly, frame) else {
            continue;
        };
        if let Some(obj) = call.params.as_object_mut() {
            obj.insert("start".to_string(), serde_json::json!([start[0], start[1]]));
            obj.insert("end".to_string(), serde_json::json!([end[0], end[1]]));
        }
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

        // C1: resolve the sketch's 3D placement frame from the `plane`
        // query. Four resolution paths, no silent fallback:
        //   * `qCreatedBy(origin_plane_*)` → the well-known datum frame
        //     (`origin_plane_xy` is the identity frame).
        //   * `qCreatedBy(<mate_connector>)` → that connector's CSYS
        //     (#409; Onshape anchors sketches to connectors, not faces).
        //   * a face-yielding query (`qFace(h)`, or `qCreatedBy`/`qFaceOf`
        //     of a prior feature's face) → that face's plane frame.
        //   * anything else → a hard error.
        let expr = crate::queries::parse_query(parsed.plane.as_str()).map_err(|e| {
            FeatureError::InvalidParameter {
                feature: SketchFeature::TYPE_NAME,
                name: "plane",
                message: format!("could not parse plane query: {e}"),
            }
        })?;
        let frame = ctx
            .resolve_plane_frame(&expr)
            .ok_or(FeatureError::InvalidParameter {
                feature: SketchFeature::TYPE_NAME,
                name: "plane",
                message: "plane query resolved to no datum and no face".to_string(),
            })?;

        // jcb7: associative re-resolution. Before building the sketch state,
        // refresh every `sketch_project` sub-call's cached endpoints from the
        // live B-Rep so the projection tracks the body across edits. The
        // rewritten `sub_calls` flow into both the state and the stashed
        // `SketchData`, so downstream consumers (profile, lowering) see the
        // current coordinates too.
        let mut sub_calls = parsed.sub_calls;
        resolve_projected_sub_calls(ctx, &frame, &mut sub_calls);

        let mut state = SketchState::with_origin();
        self.tools.apply_all(&mut state, &sub_calls)?;
        ctx.sketches.insert(
            id.clone(),
            SketchData {
                state,
                sub_calls,
                frame,
            },
        );
        Ok(())
    }
}
