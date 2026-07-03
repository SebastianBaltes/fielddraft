//! `mate_connector` — CSYS-Anker auf Face/Edge/Vertex inkl. der
//! Mate-Frame-Geometrie-Helfer (Seed-Cross, Reorient, Polyline-Tangente).

use serde_json::Value;

use fielddraft_protocol::FeatureId;

use super::{Context, FeatureError, FeatureKind, FeatureSchema};
// ─── Mate-connector CSYS helpers ─────────────────────────────────
//
// These mirror `crate::sketch::face_plane_frame` exactly (same
// seed-cross convention) so a connector's web-side inference preview and the
// kernel's authoritative resolve agree to f32 on the X spin.

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize3v(v: [f32; 3]) -> [f32; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if n > 1e-9 {
        [v[0] / n, v[1] / n, v[2] / n]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Resolved geometry of a mate-connector anchor: its origin, an optional
/// intrinsic primary (+Z) axis (edge tangent or face normal — `None` for a
/// vertex), and an auto-picked adjacent-face normal that orients the secondary
/// axis (edge) or defines +Z (vertex).
pub struct MateAnchorFrame {
    pub origin: [f32; 3],
    pub primary: Option<[f32; 3]>,
    pub auto_secondary: Option<[f32; 3]>,
}

/// World-space normal of the first planar face on `branch`, or `None`.
pub(crate) fn face_normal_for_branch(
    brep: &crate::brep::BRep,
    placement: &fielddraft_protocol::Transform,
    branch: crate::csg::BranchId,
) -> Option<[f32; 3]> {
    for i in 0..brep.faces.len() {
        if brep.faces[i].branch_id == branch {
            return Some(
                crate::sketch::face_plane_frame(
                    brep,
                    crate::brep::FaceId(i as u32),
                    placement,
                )
                .normal,
            );
        }
    }
    None
}

/// Rotate the in-plane axis `x` (⟂ `z`) by `steps`·90° about `z`. Trig-free:
/// `z × x` is the +90° image, so each quarter turn is a sign/swap.
fn reorient_secondary(x: [f32; 3], z: [f32; 3], steps: u32) -> [f32; 3] {
    let perp = cross3(z, x); // +90°
    match steps & 3 {
        0 => x,
        1 => perp,
        2 => [-x[0], -x[1], -x[2]],
        _ => [-perp[0], -perp[1], -perp[2]],
    }
}

/// In-plane X basis for a normal `n`: cross `n` with the world axis least
/// aligned with it (the `face_plane_frame` convention).
fn seed_cross_x(n: [f32; 3]) -> [f32; 3] {
    let ax = n[0].abs();
    let ay = n[1].abs();
    let az = n[2].abs();
    let seed = if ax <= ay && ax <= az {
        [1.0, 0.0, 0.0]
    } else if ay <= az {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    normalize3v(cross3(seed, n))
}

/// Project `v` onto the plane ⟂ `n` and normalise; falls back to the
/// seed-cross basis if `v` is (near-)parallel to `n`.
fn project_perp(v: [f32; 3], n: [f32; 3]) -> [f32; 3] {
    let d = v[0] * n[0] + v[1] * n[1] + v[2] * n[2];
    let p = [v[0] - d * n[0], v[1] - d * n[1], v[2] - d * n[2]];
    if p[0] * p[0] + p[1] * p[1] + p[2] * p[2] > 1e-12 {
        normalize3v(p)
    } else {
        seed_cross_x(n)
    }
}

/// Point + unit tangent at arc-length fraction `t ∈ [0, 1]` along a world
/// polyline. Returns `None` for an empty polyline.
pub(crate) fn polyline_point_tangent(poly: &[[f32; 3]], t: f32) -> Option<([f32; 3], [f32; 3])> {
    if poly.is_empty() {
        return None;
    }
    if poly.len() == 1 {
        return Some((poly[0], [1.0, 0.0, 0.0]));
    }
    let seg = |a: [f32; 3], b: [f32; 3]| -> f32 {
        ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
    };
    let total: f32 = poly.windows(2).map(|w| seg(w[0], w[1])).sum();
    if total <= 1e-9 {
        return Some((poly[0], [1.0, 0.0, 0.0]));
    }
    let target = t.clamp(0.0, 1.0) * total;
    let mut acc = 0.0f32;
    for i in 0..poly.len() - 1 {
        let a = poly[i];
        let b = poly[i + 1];
        let l = seg(a, b);
        if acc + l >= target || i == poly.len() - 2 {
            let local = if l > 1e-9 { (target - acc) / l } else { 0.0 };
            let p = [
                a[0] + (b[0] - a[0]) * local,
                a[1] + (b[1] - a[1]) * local,
                a[2] + (b[2] - a[2]) * local,
            ];
            let tan = normalize3v([b[0] - a[0], b[1] - a[1], b[2] - a[2]]);
            return Some((p, tan));
        }
        acc += l;
    }
    None
}


// ─── K9.6: Mate Connector ────────────────────────────────────────
//
// An attachable local coordinate system on the model surface.
// Stores an origin point + a normal (Z-axis of the CSYS).
// Doesn't produce geometry — it's a datum feature visible
// in the viewport as a small axis triad. Used by assemblies
// (P1) to define attachment points.

/// `mate_connector` — local coordinate system on the model.
///
/// C2: the connector is anchored to a B-Rep element (face, edge, or
/// vertex), resolved mid-loop against the live geometry rather than
/// carrying a baked world origin/normal. The CSYS is derived from queries
/// only, so it follows the geometry parametrically across edits:
///   * **origin** — the anchor's point: a face centroid, a point at
///     arc-length `t` along an edge, or the vertex itself.
///   * **+Z (primary)** — the outward normal of the `primary` reference
///     face. Empty `primary` means "use the anchor face's own normal"
///     (only valid for a face anchor). For edge/vertex anchors `primary`
///     selects which adjacent face orients the CSYS — Onshape's "primary
///     axis entity"; cycling the orientation = a different adjacent face.
///   * **+X (secondary)** — the edge tangent (projected ⟂ Z) for an edge
///     anchor, else the seed-cross in-plane basis (matching
///     [`crate::sketch::face_plane_frame`]).
#[derive(FeatureSchema)]
#[schema(type = "mate_connector")]
pub struct MateConnectorDef {
    /// What the connector anchors to: a B-Rep face, edge, or vertex
    /// (`qFace`/`qEdge`/`qVertex`). The anchor also defines the primary (+Z)
    /// axis: an EDGE's direction, a FACE's normal. A VERTEX has none, so +Z
    /// comes from the secondary reference face.
    #[param(entity_filter = "FACE, EDGE, VERTEX")]
    pub anchor: ::fielddraft_protocol::Query,
    /// Position along an edge anchor, `[0, 1]` (0.5 = midpoint). Ignored for
    /// face/vertex anchors.
    #[param(name = "Position along edge", default = 0.5)]
    pub t: f32,
    /// Between-entities placement (Phase D): an optional SECOND anchor
    /// (`qFace`/`qEdge`/`qVertex`). When set, the connector origin is the
    /// MIDPOINT of the two anchors' points; the orientation still comes from the
    /// first anchor. Empty → single-entity placement (the common case).
    #[param(name = "Between (2nd anchor)", default = ::fielddraft_protocol::Query::new(""), entity_filter = "FACE, EDGE, VERTEX")]
    pub anchor2: ::fielddraft_protocol::Query,
    /// Position along the 2nd anchor if it is an edge, `[0, 1]` (0.5 = midpoint).
    #[param(name = "2nd position along edge", default = 0.5)]
    pub t2: f32,
    /// Reference FACE that orients the SECONDARY axes (an edge's adjacent face)
    /// or, for a VERTEX anchor, defines +Z. Empty → auto-picked from the
    /// anchor's adjacent geometry. Unused for a face anchor.
    #[param(name = "Secondary reference", default = ::fielddraft_protocol::Query::new(""), entity_filter = "FACE")]
    pub secondary: ::fielddraft_protocol::Query,
    /// Reorient the secondary axes in 90° steps about +Z (0..3 cycles).
    #[param(name = "Reorient secondary", default = 0.0, ui_hint = "cycle90")]
    pub reorient: f32,
    /// Flip the primary (+Z) axis 180°.
    #[param(default = false)]
    pub flip: bool,
    /// Realignment (Phase D): translate the origin along the connector's OWN
    /// axes (X, Y, Z = normal), in millimetres. Applied after the CSYS is
    /// derived, so `[0, 0, 5]` lifts the connector 5 mm along its own +Z.
    #[param(name = "Offset (own axes)", default = [0.0, 0.0, 0.0])]
    pub offset: [f32; 3],
    /// Owner part (Phase D): the body this connector belongs to
    /// (`qCreatedBy(<seed>)`). Organises the connector under a part in the UI
    /// and travels with it. Empty → inferred from the anchor geometry.
    #[param(name = "Owner part", default = ::fielddraft_protocol::Query::new(""), entity_filter = "BODY")]
    pub owner: ::fielddraft_protocol::Query,
}

/// C2: the resolved coordinate system of a placed `mate_connector`,
/// captured during regeneration so the viewport can render its triad
/// without re-resolving the face query on the JS side.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ResolvedMateConnector {
    pub id: FeatureId,
    pub origin: [f32; 3],
    /// CSYS Z-axis (the face normal).
    pub normal: [f32; 3],
    /// CSYS X-axis (the face's in-plane basis vector).
    pub x_axis: [f32; 3],
    /// Owner part's seed feature id, if an explicit `owner` query was given
    /// (Phase D). `None` → not assigned to a specific part.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<u32>,
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
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        // Mate connectors don't produce spine geometry — they're datum
        // features. Resolve the anchor + orientation queries against the live
        // geometry, derive the CSYS, and stash it for the viewport.
        let d = MateConnectorDef::parse(def)?;
        let inval = |name: &'static str, message: String| FeatureError::InvalidParameter {
            feature: MateConnectorDef::TYPE_NAME,
            name,
            message,
        };
        let anchor = crate::queries::parse_query(d.anchor.as_str())
            .map_err(|e| inval("anchor", format!("could not parse anchor query: {e}")))?;

        let anc = ctx
            .resolve_mate_anchor(&anchor, d.t)
            .ok_or_else(|| inval("anchor", "anchor query resolved to nothing".into()))?;

        // The secondary reference face normal: explicit query, else the
        // anchor's auto-picked adjacent face. An EDGE orients its secondary
        // axes by it; a VERTEX derives +Z from it.
        let secondary_str = d.secondary.as_str();
        let sec_normal = if secondary_str.is_empty() {
            anc.auto_secondary
        } else {
            let p = crate::queries::parse_query(secondary_str)
                .map_err(|e| inval("secondary", format!("could not parse secondary query: {e}")))?;
            ctx.resolve_face_frame(&p).map(|f| f.normal)
        };

        // +Z: the anchor's intrinsic axis (edge tangent / face normal), else
        // the secondary face normal (a vertex has no intrinsic axis). No
        // anchor type errors for a missing reference any more (ticket 5j9l).
        let mut normal = anc.primary.or(sec_normal).ok_or_else(|| {
            inval("secondary", "vertex anchor needs an adjacent or reference face".into())
        })?;
        if d.flip {
            normal = [-normal[0], -normal[1], -normal[2]];
        }

        // Base +X (before reorient): for an anchor WITH an intrinsic +Z
        // (edge/face) the secondary face normal projected into the plane orients
        // it; otherwise (vertex, whose +Z already came from that face) the
        // seed-cross basis. Falls back to seed-cross when no reference exists.
        let x0 = if anc.primary.is_some() {
            match sec_normal {
                Some(n) => project_perp(n, normal),
                None => seed_cross_x(normal),
            }
        } else {
            seed_cross_x(normal)
        };
        let steps = (d.reorient.round() as i64).rem_euclid(4) as u32;
        let x_axis = reorient_secondary(x0, normal, steps);

        // Between-entities (Phase D): a 2nd anchor moves the origin to the
        // midpoint of the two anchor points. Orientation stays with the first.
        let mut origin = anc.origin;
        let anchor2_str = d.anchor2.as_str();
        if !anchor2_str.is_empty() {
            let a2 = crate::queries::parse_query(anchor2_str)
                .map_err(|e| inval("anchor2", format!("could not parse 2nd anchor query: {e}")))?;
            let anc2 = ctx
                .resolve_mate_anchor(&a2, d.t2)
                .ok_or_else(|| inval("anchor2", "2nd anchor query resolved to nothing".into()))?;
            origin = [
                (origin[0] + anc2.origin[0]) * 0.5,
                (origin[1] + anc2.origin[1]) * 0.5,
                (origin[2] + anc2.origin[2]) * 0.5,
            ];
        }

        // Realignment offset (Phase D): translate along the connector's OWN
        // axes. Y = Z × X keeps the frame right-handed.
        if d.offset != [0.0, 0.0, 0.0] {
            let y_axis = cross3(normal, x_axis);
            for k in 0..3 {
                origin[k] +=
                    d.offset[0] * x_axis[k] + d.offset[1] * y_axis[k] + d.offset[2] * normal[k];
            }
        }

        // Owner part (Phase D): a `qCreatedBy(<seed>)` names the body this
        // connector belongs to. Capture the seed id for UI grouping.
        let owner_id = {
            let owner_str = d.owner.as_str();
            if owner_str.is_empty() {
                None
            } else {
                match crate::queries::parse_query(owner_str) {
                    Ok(crate::queries::QueryExpr::CreatedBy(seed)) => seed.as_u32_or_root(),
                    _ => None,
                }
            }
        };

        ctx.mate_connectors.push(ResolvedMateConnector {
            id: id.clone(),
            origin,
            normal,
            x_axis,
            owner_id,
        });
        Ok(())
    }
}
