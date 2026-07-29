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
            // #383/#406: if both adjacent faces are planes and the edge is
            // straight, feed the analytic hard cutter (Onshape rolling-ball
            // subtraction). Convexity + orthogonality are decided later against
            // the live field in `apply_edge_fillets`; here we only gather the
            // geometry. A curved/tessellated neighbour or a non-straight edge
            // leaves `cutter = None` → the fillet stays on the `smin` mask path.
            let cutter = build_fillet_cutter(&hit, d.blend_radius);
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
                cutter,
            });
        }
        Ok(())
    }
}

/// #383/#406: assemble the analytic cutter inputs from a resolved edge hit.
/// Requires both adjacent faces to be planes and the edge polyline to be
/// straight (collinear within tolerance). Returns `None` otherwise — the fillet
/// then stays on the `smin` mask path.
fn build_fillet_cutter(
    hit: &super::context_resolve::FilletEdgeHit,
    radius: f32,
) -> Option<crate::csg::FilletCutter> {
    let normal_a = hit.plane_normal_a?;
    let normal_b = hit.plane_normal_b?;
    let (start, end) = straight_edge_endpoints(&hit.polyline)?;
    Some(crate::csg::FilletCutter {
        normal_a,
        normal_b,
        edge_start: start,
        edge_end: end,
        radius,
    })
}

/// The endpoints of a polyline that is STRAIGHT — every interior vertex lies on
/// the start→end line within tolerance. `None` for a curved/degenerate edge,
/// which is outside the analytic-cutter phase (P1) for both fillet and chamfer.
fn straight_edge_endpoints(poly: &[[f32; 3]]) -> Option<([f32; 3], [f32; 3])> {
    if poly.len() < 2 {
        return None;
    }
    let start = poly[0];
    let end = poly[poly.len() - 1];
    let dir = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len <= 1e-4 {
        return None;
    }
    let t = [dir[0] / len, dir[1] / len, dir[2] / len];
    for p in &poly[1..poly.len() - 1] {
        let w = [p[0] - start[0], p[1] - start[1], p[2] - start[2]];
        let s = w[0] * t[0] + w[1] * t[1] + w[2] * t[2];
        let perp = [w[0] - s * t[0], w[1] - s * t[1], w[2] - s * t[2]];
        let d2 = perp[0] * perp[0] + perp[1] * perp[1] + perp[2] * perp[2];
        if d2 > 1e-4 {
            return None; // curved edge — not a P1 hard cutter case
        }
    }
    Some((start, end))
}

// ─── #414: Chamfer Edge Feature ──────────────────────────────────
//
// The structurally simpler brother of `fillet_edge`: instead of a rolling-ball
// cylinder, a PLANAR bevel is cut along the edge. It reuses the whole fillet
// machinery — the same edge query resolution, the same dihedral gate
// (`csg::edge_cutter_frame`), the same body-level hard subtraction, the same
// cache-hash pass — and differs only in the cutter's profile.

/// `chamfer_edge` — planar bevel along a picked edge. Onshape's `EQUAL_OFFSETS`
/// mode: ONE distance, cut back equally on both adjacent faces (a 45° bevel at
/// an orthogonal edge). `OPPOSITE_OFFSETS` (two distances) and `ANGLE`
/// (distance + angle) are documented gaps — see [`crate::csg::EdgeChamfer`].
#[derive(FeatureSchema)]
#[schema(type = "chamfer_edge")]
pub struct ChamferEdgeDef {
    /// The edge to chamfer (e.g. `qEdge(<id_hash>)`).
    #[param(name = "Edge", entity_filter = "EDGE")]
    pub edge: ::fielddraft_protocol::Query,
    /// Bevel leg length — how far the cut sets back from the edge on EACH
    /// adjacent face. A `Length` like every other physical dimension (#405).
    #[param(name = "Distance", default = ::fielddraft_protocol::Length::mm(2.0))]
    pub distance: ::fielddraft_protocol::Length,
}

pub struct ChamferEdgeFeature;

impl FeatureKind for ChamferEdgeFeature {
    fn type_name(&self) -> &'static str {
        ChamferEdgeDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        ChamferEdgeDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ChamferEdgeDef::parse(def)?;
        let distance = d.distance.to_mm();
        let bad = |name: &'static str, message: String| FeatureError::InvalidParameter {
            feature: ChamferEdgeDef::TYPE_NAME,
            name,
            message,
        };
        if !(distance > 0.0) || !distance.is_finite() {
            return Err(bad("distance", format!("must be > 0, got {distance}")));
        }
        let expr = crate::queries::parse_query(d.edge.as_str())
            .map_err(|e| bad("edge", format!("could not parse edge query: {e}")))?;
        let resolved = ctx.resolve_fillet_edges(&expr);
        if resolved.is_empty() {
            return Err(bad("edge", "edge query resolved to no edge".to_string()));
        }

        // The dihedral gate needs a FIELD to decide which side of each face is
        // outward and whether the edge is convex. Build the hard tree of
        // everything modelled so far once and probe it here, so a chamfer that
        // cannot be cut fails LOUDLY with the reason instead of silently
        // producing nothing at apply time (`apply_edge_chamfers` re-runs the
        // very same gate against the per-body field and skips what it rejects).
        let probe = crate::csg::CsgNode::from_decoded(&ctx.features);
        let mut pending = Vec::new();
        for hit in resolved {
            let (Some(normal_a), Some(normal_b)) = (hit.plane_normal_a, hit.plane_normal_b) else {
                return Err(bad(
                    "edge",
                    "Chamfer braucht zwei planare Nachbarflächen — an dieser Kante ist \
                     mindestens eine Fläche gekrümmt oder tesselliert (P1-Grenze, siehe \
                     docs/FILLET-CUTTER-PLAN.md)"
                        .to_string(),
                ));
            };
            let Some((start, end)) = straight_edge_endpoints(&hit.polyline) else {
                return Err(bad(
                    "edge",
                    "Chamfer braucht eine GERADE Kante — diese Kante ist gekrümmt \
                     (P1-Grenze, siehe docs/FILLET-CUTTER-PLAN.md)"
                        .to_string(),
                ));
            };
            if let Err(why) = crate::csg::edge_cutter_frame(
                &probe, start, end, normal_a, normal_b, distance,
            ) {
                return Err(bad("edge", chamfer_reject_message(why, distance)));
            }
            pending.push(crate::csg::EdgeChamfer {
                chamfer_id: id.clone(),
                normal_a,
                normal_b,
                edge_start: start,
                edge_end: end,
                distance,
            });
        }
        ctx.edge_chamfers.append(&mut pending);
        Ok(())
    }
}

/// Turn a dihedral rejection into a message that names the real limitation
/// rather than leaving the user with a feature that quietly does nothing.
fn chamfer_reject_message(why: crate::csg::EdgeCutterReject, distance: f32) -> String {
    use crate::csg::EdgeCutterReject as R;
    match why {
        R::NonOrthogonal =>
            "Chamfer kann diese Kante noch nicht schneiden: der Flächenwinkel ist kein \
             rechter Winkel. P1 deckt nur orthogonale Kanten ab (P2 in \
             docs/FILLET-CUTTER-PLAN.md verallgemeinert auf schiefe Prismen)."
                .to_string(),
        R::Concave =>
            "Chamfer kann diese Kante noch nicht schneiden: sie ist KONKAV (eine Innenkante). \
             Dort müsste Material hinzugefügt statt weggeschnitten werden — P3 in \
             docs/FILLET-CUTTER-PLAN.md."
                .to_string(),
        R::LargerThanMaterial => format!(
            "Chamfer kann diese Kante noch nicht schneiden: die Fase ({distance} mm) ist \
             größer als das Material an dieser Kante. Der Fräser-Mittelpunkt läge \
             {distance} mm hinter BEIDEN Flächen und damit außerhalb des Körpers — \
             typisch für eine Fase, die breiter ist als die Wand, auf der sie sitzt. \
             Die Kante selbst ist KONVEX (siehe docs/FILLET-CUTTER-PLAN.md)."
        ),
        R::Degenerate => format!(
            "Chamfer kann diese Kante nicht schneiden: die Kantengeometrie ist entartet \
             (Nulllänge, oder eine Flächennormale steht nicht senkrecht auf der Kante). \
             Fase: {distance} mm."
        ),
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
    /// Round radius applied to every hard Boolean junction. A `Length` like
    /// every other physical dimension — `add_round_all` has written the
    /// `{value, unit}` form since its introduction (890710f2), so an `f32`
    /// here made every UI-created round_all fail parsing (#405).
    #[param(default = ::fielddraft_protocol::Length::mm(2.0))]
    pub radius: ::fielddraft_protocol::Length,
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
        let radius = d.radius.to_mm();
        if radius < 0.0 {
            return Err(FeatureError::InvalidParameter {
                feature: RoundAllDef::TYPE_NAME,
                name: "radius",
                message: format!("must be >= 0, got {radius}"),
            });
        }
        // Last `round_all` in the timeline wins (a single global radius).
        ctx.global_round = if radius > 0.0 { Some(radius) } else { None };
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

        // #384 D2: a Twist/Bend has no exact offset (it is not an isometry, Plan
        // §2), and the dualised closed shell erodes via that exact offset path —
        // so refuse rather than leave a silent, metrically wrong wall. Mirror of
        // `OffsetFeature::execute`'s guard: a target body whose partition carries
        // a warp is out of scope for shelling.
        let warped = parts.iter().any(|p| {
            let in_scope = p.feature_indices.iter().any(|&fi| {
                target_ids
                    .iter()
                    .any(|t| ctx.features[fi].id.root_ancestor() == t)
            });
            in_scope
                && p.feature_indices
                    .iter()
                    .any(|&fi| ctx.features[fi].node.contains_warp())
        });
        if warped {
            return Err(FeatureError::InvalidParameter {
                feature: ShellDef::TYPE_NAME,
                name: "open_faces",
                message: "Shell auf einem Körper mit Twist/Bend ist nicht möglich — \
                          eine Verdrehung/Biegung hat keinen exakten Offset, den die \
                          Innenwand-Erosion bräuchte. Entferne den Twist/Bend oder \
                          setze die Shell davor."
                    .to_string(),
            });
        }

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
