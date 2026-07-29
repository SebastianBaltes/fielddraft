//! `gyroid` — TPMS-Infill-Feature.

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

use crate::csg::{DecodedFeature, PrimitiveModifiers};

use super::merge_scope::{expand_to_owner_body_members, target_ids_from_queries};
use super::{Context, FeatureError, FeatureKind, FeatureSchema, optional_display_name, optional_rotation, optional_scale, positive_f32};
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
    #[param(name = "Bodies", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    /// Distance between repeating cells in mm.
    pub cell_size: ::fielddraft_protocol::Length,
    /// Wall half-thickness in mm. Larger = thicker walls.
    pub thickness: ::fielddraft_protocol::Length,
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::Intersect)]
    pub op: BoolOp,
    #[param(name = "Blend radius", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub blend_radius: ::fielddraft_protocol::Length,
    #[param(name = "Chamfer size", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub chamfer_size: ::fielddraft_protocol::Length,
    #[param(name = "Twist rate", default = 0.0)]
    pub twist_rate: f32,
    #[param(name = "Bend rate", default = 0.0)]
    pub bend_rate: f32,
    #[param(name = "Displace depth", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub displace_depth: ::fielddraft_protocol::Length,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,
    #[param(name = "Shell thickness", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub shell_thickness: ::fielddraft_protocol::Length,
}

pub struct GyroidFeature;

impl FeatureKind for GyroidFeature {
    fn type_name(&self) -> &'static str {
        GyroidDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        let mut schema = GyroidDef::schema_json();
        if let Some(params) = schema.get_mut("params").and_then(|p| p.as_array_mut()) {
            for param in params.iter_mut() {
                if param.get("key").and_then(|k| k.as_str()) == Some("op") {
                    if let Some(variants) =
                        param.get_mut("variants").and_then(|v| v.as_array_mut())
                    {
                        variants.retain(|v| {
                            !matches!(
                                v.get("key").and_then(|k| k.as_str()),
                                Some("add") | Some("new")
                            )
                        });
                    }
                }
            }
        }
        schema
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

        let op = match d.op {
            op if op.is_union() => BoolOp::Intersect,
            other => other,
        };

        let name = optional_display_name(def, || format!("Gyroid {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);

        // #456: an explicit scope names a BODY, not one of its members.
        let target_ids =
            expand_to_owner_body_members(&ctx.features, &target_ids_from_queries(&d.targets));

        ctx.features.push(DecodedFeature::from_parts(
            id.child("0"),
            name,
            op,
            Primitive::Gyroid {
                cell_size: cell_mm,
                thickness: thick_mm,
            },
            Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            PrimitiveModifiers {
                blend_radius: d.blend_radius.to_mm(),
                chamfer_size: d.chamfer_size.to_mm(),
                twist_rate: d.twist_rate,
                bend_rate: d.bend_rate,
                displace_depth: d.displace_depth.to_mm(),
                displace_freq: d.displace_freq,
                shell_thickness: d.shell_thickness.to_mm(),
            },
            target_ids,
        ));
        Ok(())
    }
}
