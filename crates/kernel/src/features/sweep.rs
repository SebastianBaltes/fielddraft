//! `sweep` — Profil entlang eines 3D-Pfads (modulare Path-Provider).

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, Transform};

use crate::csg::{CsgNode, DecodedFeature};
use crate::path_provider::PathProvider;

use super::merge_scope::{resolve_merge_scope, target_ids_from_queries};
use super::{Context, FeatureError, FeatureKind, FeatureSchema, optional_display_name};
// ─── K13.2: Sweep Feature (curved path) ─────────────────────────
//
// Sweeps a 2D sketch profile along an arbitrary 3D spine, built as a
// `CsgNode::Sweep`. The path source is decoupled behind the modular
// `path_provider` layer; v1 uses the Gizmo3d provider (control points +
// Catmull-Rom spline). The Gatekeeper (in classify) decides parametric
// mesh vs. SDF fallback; the profile is carried in the profile's own frame.

/// The sweep's spine source — one tagged variant per modular path-provider
/// method (see [`crate::path_provider`]). The consumer never branches on
/// this; `execute` resolves it to a `SampledPath`.
#[derive(Debug, Clone, ::serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PathSourceDef {
    /// 3D control points routed in space + Catmull-Rom spline.
    Gizmo3d {
        nodes: Vec<[f32; 3]>,
        #[serde(default)]
        closed: bool,
    },
    /// Planar curve from a 2D sketch (`qCreatedBy(<sketch>)`).
    Sketch2d {
        sketch: ::fielddraft_protocol::Query,
    },
    /// Existing model edges selected by an edge query (`qEdge`/`qEdgeBetween`).
    EdgeChain {
        edges: ::fielddraft_protocol::Query,
    },
    /// Two orthogonal sketches sharing an axis → a 3D space curve (two-view
    /// wire). `sketch_a`/`sketch_b` are `qCreatedBy(<sketch>)` selections.
    OrthogonalSketches {
        sketch_a: ::fielddraft_protocol::Query,
        sketch_b: ::fielddraft_protocol::Query,
    },
}

/// `sweep` — profile swept along a spine resolved from a modular path source.
#[derive(FeatureSchema)]
#[schema(type = "sweep")]
pub struct SweepDef {
    pub profiles: ::fielddraft_protocol::Query,
    /// Modular path source (Gizmo3d control points or a 2D sketch curve).
    pub path: PathSourceDef,
    /// Total profile twist along the spine, in degrees (0 = no twist).
    #[param(default = 0.0)]
    pub twist_deg: f32,
    #[param(default = ::fielddraft_protocol::BoolOp::Add)]
    pub operation: BoolOp,

    /// Merge scope: explicit target bodies for the boolean op, as
    /// `qCreatedBy(<seed_feature>)` queries. Empty + `merge_with_all` off =
    /// no merge (standalone body; warned for Add/Remove/Intersect).
    #[param(name = "Merge scope", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,

    /// Merge with all (Onshape): fold the body into EVERY body it touches
    /// instead of an explicit scope. Off by default — touching solids never
    /// merge implicitly; opt in here or name targets above.
    #[param(name = "Merge with all", default = false)]
    pub merge_with_all: bool,
}

/// Resolve a `qCreatedBy(<sketch>)` query to its ordered 2D curve (closed
/// loop if it bounds a region, else the open drawn curve) plus the sketch's
/// plane frame. Shared by the Sketch2d and OrthogonalSketches path sources.
fn resolve_sketch_curve_2d(
    sketches: &std::collections::HashMap<FeatureId, crate::sketch_features::SketchData>,
    query: &::fielddraft_protocol::Query,
) -> Result<(Vec<[f32; 2]>, bool, crate::sketch_features::SketchFrame), FeatureError> {
    let id = match crate::queries::parse_query(query.as_str()) {
        Ok(crate::queries::QueryExpr::CreatedBy(s)) => s,
        _ => {
            return Err(FeatureError::InvalidParameter {
                feature: SweepDef::TYPE_NAME,
                name: "path",
                message: "sketch query must be qCreatedBy(<sketch>)".to_string(),
            })
        }
    };
    let sk = sketches.get(&id).ok_or(FeatureError::MissingParameter {
        feature: SweepDef::TYPE_NAME,
        name: "path",
    })?;
    let bounded = crate::sketch2d::BoundedSketch2d::from_sketch(
        &crate::sketch_features::build_sketch2d_from_sub_calls(&sk.sub_calls),
    );
    let closed_loop = crate::sketch2d::extract_contours(&bounded, crate::sketch2d::CONTOUR_RESOLUTION)
        .into_iter()
        .next()
        .map(|c| c.outer)
        .filter(|o| o.len() >= 2);
    let (curve, closed) = match closed_loop {
        Some(loop_curve) => (loop_curve, true),
        None => (
            crate::sketch_features::open_path_curve_from_sub_calls(&sk.sub_calls).ok_or(
                FeatureError::InvalidParameter {
                    feature: SweepDef::TYPE_NAME,
                    name: "path",
                    message: "sketch has no usable planar curve".to_string(),
                },
            )?,
            false,
        ),
    };
    Ok((curve, closed, sk.frame))
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

        // Resolve the sketch profile (same convention as extrude/revolve).
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

        // Resolve the spine through the modular path-provider layer — each
        // source method dispatches to its provider; consumers downstream see
        // only the sampled `SweepPath`, never this source.
        let bad_path = |message: String| FeatureError::InvalidParameter {
            feature: SweepDef::TYPE_NAME,
            name: "path",
            message,
        };
        let sampled = match &d.path {
            PathSourceDef::Gizmo3d { nodes, closed } => {
                crate::path_provider::Gizmo3dPath {
                    nodes: nodes.clone(),
                    closed: *closed,
                    samples_per_span: 24,
                }
                .sample()
                .map_err(|_| bad_path("need at least two control points".to_string()))?
            }
            PathSourceDef::Sketch2d { sketch: path_q } => {
                let path_id = match crate::queries::parse_query(path_q.as_str()) {
                    Ok(crate::queries::QueryExpr::CreatedBy(s)) => s,
                    _ => {
                        return Err(bad_path(
                            "sketch path must be qCreatedBy(<sketch>)".to_string(),
                        ))
                    }
                };
                let sk = ctx.sketches.get(&path_id).ok_or(
                    FeatureError::MissingParameter {
                        feature: SweepDef::TYPE_NAME,
                        name: "path",
                    },
                )?;
                let bounded = crate::sketch2d::BoundedSketch2d::from_sketch(
                    &crate::sketch_features::build_sketch2d_from_sub_calls(&sk.sub_calls),
                );
                // A closed loop sweeps around itself; otherwise fall back to
                // the open drawn curve (a path from A to B).
                let closed_loop = crate::sketch2d::extract_contours(
                    &bounded,
                    crate::sketch2d::CONTOUR_RESOLUTION,
                )
                .into_iter()
                .next()
                .map(|c| c.outer)
                .filter(|o| o.len() >= 2);
                let (curve, closed) = match closed_loop {
                    Some(loop_curve) => (loop_curve, true),
                    None => {
                        let open = crate::sketch_features::open_path_curve_from_sub_calls(
                            &sk.sub_calls,
                        )
                        .ok_or_else(|| {
                            bad_path("sketch has no usable planar curve to sweep along".to_string())
                        })?;
                        (open, false)
                    }
                };
                crate::path_provider::Sketch2dPath {
                    curve,
                    origin: sk.frame.origin,
                    x_axis: sk.frame.x_axis,
                    y_axis: sk.frame.y_axis,
                    closed,
                }
                .sample()
                .map_err(|_| bad_path("degenerate sketch path".to_string()))?
            }
            PathSourceDef::EdgeChain { edges } => {
                let expr = crate::queries::parse_query(edges.as_str())
                    .map_err(|e| bad_path(format!("could not parse edge query: {e}")))?;
                let polylines: Vec<Vec<[f32; 3]>> = ctx.resolve_edge_polylines(&expr);
                if polylines.is_empty() {
                    return Err(bad_path("edge query resolved to no edges".to_string()));
                }
                crate::path_provider::EdgeChainPath { polylines }
                    .sample()
                    .map_err(|_| bad_path("edges do not form a usable chain".to_string()))?
            }
            PathSourceDef::OrthogonalSketches { sketch_a, sketch_b } => {
                let (curve_a, _, fa) = resolve_sketch_curve_2d(&ctx.sketches, sketch_a)?;
                let (curve_b, _, fb) = resolve_sketch_curve_2d(&ctx.sketches, sketch_b)?;
                // The two sketch planes must share an axis (their x-axes are the
                // common parameter direction of the two-view construction).
                let dot = fa.x_axis[0] * fb.x_axis[0]
                    + fa.x_axis[1] * fb.x_axis[1]
                    + fa.x_axis[2] * fb.x_axis[2];
                if dot.abs() < 0.999 {
                    return Err(bad_path(
                        "the two sketches must share an axis (orthogonal views)".to_string(),
                    ));
                }
                crate::path_provider::OrthogonalSketchesPath {
                    curve_a,
                    curve_b,
                    origin: fa.origin,
                    shared_axis: fa.x_axis,
                    dir_a: fa.y_axis,
                    dir_b: fb.y_axis,
                    samples: 96,
                }
                .sample()
                .map_err(|_| bad_path("orthogonal sketches do not form a curve".to_string()))?
            }
        };

        let name = optional_display_name(def, || format!("Sweep {id}"));
        let node_id = id.child("0");
        let target_ids = resolve_merge_scope(
            ctx,
            target_ids_from_queries(&d.targets),
            d.merge_with_all,
        );
        if let Some(w) = ctx.merge_scope_warning(d.operation, &target_ids) {
            ctx.warn_feature(id, w);
        }
        ctx.features.push(DecodedFeature {
            id: node_id.clone(),
            name,
            op: d.operation,
            blend_radius: 0.0,
            blend_mode: crate::csg::BlendMode::Fillet,
            target_ids,
            transform: Transform {
                translate: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
            },
            node: CsgNode::Sweep {
                id: node_id,
                sketch: Box::new(sketch),
                path: sampled.points,
                closed: sampled.closed,
                twist: d.twist_deg.to_radians(),
            },
        });
        Ok(())
    }
}
