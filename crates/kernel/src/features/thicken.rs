//! `thicken` — Flächen-Offset zu Solid (beidseitig/einseitig).

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, Transform};

use crate::csg::{CsgNode, DecodedFeature};

use super::{Context, FeatureError, FeatureKind, FeatureSchema, optional_display_name};
// ─── Thicken: offset a curved analytic face into a thin solid ─────
//
// The curved counterpart to the face-extrude path above. Where a planar face
// reduces to `Primitive::Extrude` (linear sweep of its 2D boundary), a curved
// analytic face reduces to the **difference of two coaxial offset solid
// primitives** — `Sphere(r+out) − Sphere(r−in)`, `Cyl(r+out) − Cyl(r−in)`,
// `Torus(R, r+out) − Torus(R, r−in)` — so the thickened wall inherits the whole
// solid pipeline (SDF, weave, picking) for free (see
// [`crate::brep::face_thicken`]). First throw: sphere / cylinder / torus.

/// `thicken` — make a thin solid wall from one or more **curved** analytic
/// B-Rep faces by offsetting their surfaces along the normal.
#[derive(FeatureSchema)]
#[schema(type = "thicken")]
pub struct ThickenDef {
    /// Curved B-Rep face(s) to thicken. Comma-separated `qFace(<id_hash>)`
    /// selections; each becomes its own offset-solid shell and they are unioned.
    #[param(name = "Faces", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "FACE")]
    pub faces: ::fielddraft_protocol::MultiQuery,

    /// Wall thickness grown along the face normal (outside the source solid).
    /// When `symmetric` is set this is split half in / half out instead.
    #[param(name = "Thickness", default = ::fielddraft_protocol::Length::mm(2.0))]
    pub thickness: ::fielddraft_protocol::Length,

    /// `true` splits the wall symmetrically about the surface (`±thickness/2`).
    /// `false` (default, Onshape-style) grows `thickness` outward and
    /// `inner_thickness` inward.
    #[param(name = "Symmetric", default = false)]
    pub symmetric: bool,

    /// Inward wall depth (along −normal) for a two-sided thicken. Only consulted
    /// when `symmetric` is `false`; `0` (default) makes a pure outward wall.
    #[param(name = "Inner thickness", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub thickness_back: ::fielddraft_protocol::Length,

    /// Optional coaxial **target** face: when set, the wall grows from each source
    /// surface *up to* this face instead of by a fixed `thickness` (Onshape's
    /// "Up to face" end condition). The target must be the same coaxial analytic
    /// kind as the source (concentric sphere / coaxial cylinder / coaxial torus);
    /// the signed gap picks the direction. Empty (default) → fixed-thickness mode.
    #[param(name = "Up to face", default = ::fielddraft_protocol::Query::new(""), entity_filter = "FACE")]
    pub up_to_face: ::fielddraft_protocol::Query,

    /// Boolean op used to fold the thickened wall into the running spine.
    /// Defaults to [`BoolOp::Add`] so the wall merges into the body
    /// whose face it grew from.
    #[param(default = ::fielddraft_protocol::BoolOp::Add)]
    pub operation: BoolOp,
}

/// The analytic wall node for one face: `Difference(outer, inner)` of the two
/// coaxial offset solids, keyed under the thicken feature `id` (sub `{qi}o` /
/// `{qi}i`). The general (non-analytic) walls build their own `Shell(Offset(..))`
/// node in [`crate::brep::face_offset`]; this is only the per-primitive path.
pub(crate) fn thicken_diff_node(
    id: &FeatureId,
    qi: usize,
    s: &crate::brep::face_thicken::ThickenSolid,
) -> CsgNode {
    let diff = CsgNode::Difference(
        Box::new(CsgNode::Leaf {
            id: id.child(format!("{qi}o")),
            primitive: s.outer.clone(),
        }),
        Box::new(CsgNode::Leaf {
            id: id.child(format!("{qi}i")),
            primitive: s.inner.clone(),
        }),
    );
    // Partial (cut) wall: bound the full-revolution difference to the wall's
    // own angular interval with a sector prism (see
    // [`crate::brep::face_thicken::SectorTrim`]). The prism is an extruded
    // pie wedge; `Primitive::Extrude` sweeps its sketch along local Z, so a
    // +90° rotation about X maps that axis onto the wall's surface-local +Y —
    // under which the sketch's polar angle coincides with the wall azimuth
    // `atan2(z, x)` (x_sketch = x, y_sketch = z).
    let Some(t) = &s.sector else { return diff };
    let sector = CsgNode::Leaf {
        id: id.child(format!("{qi}w")),
        primitive: fielddraft_protocol::Primitive::Extrude {
            sketch: fielddraft_protocol::Sketch2d::Prim(
                fielddraft_protocol::Sketch2dPrimitive::Sector {
                    center: [0.0, 0.0],
                    radius: t.radius,
                    start_angle: t.start_angle,
                    sweep_angle: t.sweep_angle,
                },
            ),
            half_depth_z: t.half_height,
            // Extrude-local z maps to surface-local −y under the rotation
            // below, so the slab centre picks up a sign flip.
            center: [0.0, 0.0, -t.center_y],
        },
    };
    let rot_x_90 = fielddraft_protocol::Transform {
        rotation: [std::f32::consts::FRAC_1_SQRT_2, 0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2],
        ..fielddraft_protocol::Transform::IDENTITY
    };
    CsgNode::Intersection(
        Box::new(diff),
        Box::new(CsgNode::Transform(rot_x_90, Box::new(sector))),
    )
}

pub struct ThickenFeature;

impl FeatureKind for ThickenFeature {
    fn type_name(&self) -> &'static str {
        ThickenDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        ThickenDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ThickenDef::parse(def)?;

        // No faces picked yet — the dialog was just opened. Emit nothing rather
        // than failing the regen; the wall appears once a face is picked.
        let face_queries: Vec<String> =
            d.faces.queries().iter().map(|s| s.to_string()).collect();
        if face_queries.is_empty() {
            return Ok(());
        }

        // Up-to-face mode: when a target face is given the wall grows up to it
        // and the fixed thickness / direction params are ignored. Otherwise the
        // thickness is split per the direction mode below.
        let target = d.up_to_face.as_str().trim();
        let walls = if !target.is_empty() {
            ctx.resolve_thicken_to_face(id, &face_queries, target)
                .map_err(|message| FeatureError::InvalidParameter {
                    feature: ThickenDef::TYPE_NAME,
                    name: "up_to_face",
                    message,
                })?
        } else {
            let t = d.thickness.to_mm();
            if t <= 0.0 {
                return Err(FeatureError::InvalidParameter {
                    feature: ThickenDef::TYPE_NAME,
                    name: "thickness",
                    message: format!("must be > 0 mm, got {t}"),
                });
            }
            // Direction modes → (outward offset, inward offset).
            let back = d.thickness_back.to_mm().max(0.0);
            let (outer_off, inner_off) =
                if d.symmetric { (t / 2.0, t / 2.0) } else { (t, back) };

            ctx.resolve_thicken_faces(id, &face_queries, outer_off, inner_off)
                .map_err(|message| FeatureError::InvalidParameter {
                    feature: ThickenDef::TYPE_NAME,
                    name: "faces",
                    message,
                })?
        };

        let name = optional_display_name(def, || format!("Thicken {id}"));

        // Each face resolves to a wall node in its own (carrier-/surface-)local
        // frame plus its local → world placement. A single face keeps a separable
        // placement (feature.transform = world frame → DL-56 move-cache); multiple
        // faces bake each world transform into the node and union the walls.
        let (transform, node) = if walls.len() == 1 {
            let (node, world) = walls.into_iter().next().unwrap();
            (world, node)
        } else {
            let mut acc: Option<CsgNode> = None;
            for (node, world) in walls {
                let placed = CsgNode::Transform(world, Box::new(node));
                acc = Some(match acc {
                    None => placed,
                    Some(a) => CsgNode::Union(Box::new(a), Box::new(placed)),
                });
            }
            (Transform::IDENTITY, acc.unwrap())
        };

        ctx.features.push(DecodedFeature {
            id: id.clone(),
            name,
            op: d.operation,
            blend_radius: 0.0,
            blend_mode: crate::csg::BlendMode::Fillet,
            target_ids: vec![],
            transform,
            node,
        });
        Ok(())
    }
}
