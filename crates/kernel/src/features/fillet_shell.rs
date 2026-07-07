//! `fillet_edge` und `shell` — kantenselektive Fillets (SMin-Maske)
//! und Aushöhlen mit Wandstärke.

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, Transform};

use crate::csg::DecodedFeature;

use super::{Context, FeatureError, FeatureKind, FeatureSchema};
// ─── K10.4: Fillet Edge Feature ──────────────────────────────────
//
// A feature that applies per-edge spatial-mask fillets to the
// CSG tree. The edge fillets are stored in the document and
// applied as a post-processing pass after from_features builds
// the base tree. The feature itself doesn't push a spine entry —
// it modifies the tree shape by replacing Union/Difference
// nodes with MaskedBlend where the edge specification matches.

/// DL-61: the mask tube radius as a multiple of the blend radius. The
/// tube must enclose the full ~`blend_radius` perpendicular reach of the
/// smooth blend without bleeding into neighbouring edges. Tunable.
pub const FILLET_MASK_SCALE: f32 = 1.5;

/// `fillet_edge` — per-edge fillet selected by an edge query. The edge's
/// branch-pair signature yields the feature pair at the junction; its
/// polyline becomes the SMin mask tube.
#[derive(FeatureSchema)]
#[schema(type = "fillet_edge")]
pub struct FilletEdgeDef {
    /// The edge to fillet (e.g. `qEdge(<id_hash>)`).
    #[param(name = "Edge", entity_filter = "EDGE")]
    pub edge: ::fielddraft_protocol::Query,
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
        id: &FeatureId,
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
        let expr = crate::queries::parse_query(d.edge.as_str()).map_err(|e| {
            FeatureError::InvalidParameter {
                feature: FilletEdgeDef::TYPE_NAME,
                name: "edge",
                message: format!("could not parse edge query: {e}"),
            }
        })?;
        let resolved = ctx.resolve_fillet_edges(&expr);
        if resolved.is_empty() {
            return Err(FeatureError::InvalidParameter {
                feature: FilletEdgeDef::TYPE_NAME,
                name: "edge",
                message: "edge query resolved to no edge".to_string(),
            });
        }
        let mask_radius = d.blend_radius * FILLET_MASK_SCALE;
        for hit in resolved {
            ctx.edge_fillets.push(crate::csg::EdgeFillet {
                fillet_id: id.clone(),
                feature_a: hit.feature_a,
                feature_b: hit.feature_b,
                branch_a: hit.branch_a,
                branch_b: hit.branch_b,
                mask_polyline: hit.polyline,
                mask_radius,
                blend_radius: d.blend_radius,
                blend_mode: crate::csg::BlendMode::Fillet,
            });
        }
        Ok(())
    }
}

// ─── Round all edges (global): deburr every Boolean junction ───

/// `round_all` — round EVERY Boolean-junction edge of the model with one
/// radius. The SDF-native "deburr everything" operator: a field blend can
/// never invalidate the solid, and between planar walls the round is an exact
/// circle (see [`crate::csg::smin_poly`]). Rounds feature-junction edges;
/// a lone primitive's own edges are out of scope (see
/// [`crate::csg::CsgNode::apply_global_round`]).
#[derive(FeatureSchema)]
#[schema(type = "round_all")]
pub struct RoundAllDef {
    /// Round radius applied to every hard Boolean junction.
    #[param(default = 2.0)]
    pub radius: f32,
}

pub struct RoundAllFeature;

impl FeatureKind for RoundAllFeature {
    fn type_name(&self) -> &'static str {
        RoundAllDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        RoundAllDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = RoundAllDef::parse(def)?;
        if d.radius < 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: RoundAllDef::TYPE_NAME,
                name: "radius",
                message: format!("must be >= 0, got {}", d.radius),
            });
        }
        // Last `round_all` in the timeline wins (a single global radius).
        ctx.global_round = if d.radius > 0.0 { Some(d.radius) } else { None };
        Ok(())
    }
}

// ─── Shell: hollow a body, opening it at selected faces ───
//
// The classic CAD shell-with-removed-faces. The user picks one or more
// B-Rep faces (planar or curved — any analytic surface); the body they
// belong to is hollowed to a wall of `thickness`, broken open flush with
// each picked face. It lowers to the sentinel node `Shell(Empty, t,
// open_faces)` on a `Difference` feature, which `body::partition` docks into
// the target body and `CsgNode::from_decoded` turns into an inward
// `CsgNode::Shell` wrapping that body's whole subtree (see `wrap_shell`).
// Each `OpenSurface` carries the face's analytic `HalfSpace` + world
// placement, so the gate opens flush with cylinders/spheres/cones too.
#[derive(FeatureSchema)]
#[schema(type = "shell")]
pub struct ShellDef {
    /// The planar face(s) to remove, opening the shell there
    /// (e.g. `qFace(<id_hash>)`, comma-separated for several).
    #[param(name = "Open faces", entity_filter = "FACE")]
    pub open_faces: ::fielddraft_protocol::MultiQuery,
    /// Wall thickness, grown inward from the surface.
    #[param(name = "Thickness", default = ::fielddraft_protocol::Length::mm(2.0))]
    pub thickness: ::fielddraft_protocol::Length,
}

pub struct ShellFeature;

impl FeatureKind for ShellFeature {
    fn type_name(&self) -> &'static str {
        ShellDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        ShellDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ShellDef::parse(def)?;
        // No faces picked yet — the feature was just created and its dialog is
        // still being filled. Emit nothing rather than failing the regen; once
        // a face is picked the shell appears.
        if d.open_faces.queries().is_empty() {
            return Ok(());
        }
        let thickness = d.thickness.to_mm();
        if thickness <= 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: ShellDef::TYPE_NAME,
                name: "thickness",
                message: format!("must be > 0, got {thickness}"),
            });
        }

        // Resolve every picked face to a world-space open surface (planar or
        // curved), recording the feature each face belongs to so we can find the
        // body to hollow.
        let mut open_faces: Vec<crate::csg::OpenSurface> = Vec::new();
        let mut owner: Option<FeatureId> = None;
        for q in d.open_faces.queries() {
            let expr = crate::queries::parse_query(q).map_err(|e| {
                FeatureError::InvalidParameter {
                    feature: ShellDef::TYPE_NAME,
                    name: "open_faces",
                    message: format!("could not parse face query `{q}`: {e}"),
                }
            })?;
            match ctx.resolve_open_face(&expr) {
                Some((surface, face_owner)) => {
                    open_faces.push(surface);
                    owner.get_or_insert(face_owner);
                }
                None => {
                    return Err(FeatureError::InvalidParameter {
                        feature: ShellDef::TYPE_NAME,
                        name: "open_faces",
                        message: format!(
                            "face query `{q}` resolved to no analytic face (only \
                             tessellated/TPMS faces cannot be opened)"
                        ),
                    });
                }
            }
        }
        let Some(owner) = owner else {
            return Err(FeatureError::InvalidParameter {
                feature: ShellDef::TYPE_NAME,
                name: "open_faces",
                message: "shell needs at least one face to open".to_string(),
            });
        };

        // Gather every feature of the body that owns the first picked face — the
        // full target set so `wrap_shell` wraps the whole (possibly fused) body,
        // not just one feature.
        let parts = crate::body::partition(&ctx.features);
        let target_ids: Vec<FeatureId> = parts
            .iter()
            .find(|p| {
                p.feature_indices.iter().any(|&fi| {
                    ctx.features[fi].id.root_ancestor() == &owner
                })
            })
            .map(|p| {
                p.feature_indices
                    .iter()
                    .map(|&fi| ctx.features[fi].id.clone())
                    .collect()
            })
            .unwrap_or_else(|| vec![owner.clone()]);

        ctx.features.push(DecodedFeature {
            id: id.clone(),
            name: "Shell".to_string(),
            // `Difference` is purely a partition hint (dock into the target
            // body); `from_decoded` intercepts the sentinel node before any
            // boolean fold.
            op: BoolOp::Remove,
            blend_radius: 0.0,
            blend_mode: crate::csg::BlendMode::Fillet,
            target_ids,
            transform: Transform::IDENTITY,
            node: crate::csg::CsgNode::Shell(
                Box::new(crate::csg::CsgNode::Empty),
                thickness,
                open_faces,
            ),
        });
        Ok(())
    }
}
