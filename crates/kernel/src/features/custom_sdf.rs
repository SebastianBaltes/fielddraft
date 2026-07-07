//! `custom_sdf` — user-defined implicit via a math expression.
//!
//! The dialog takes a closed-form expression over the local point
//! `x, y, z` (mm), e.g. the gyroid `sin(x)*cos(y) + sin(y)*cos(z) +
//! sin(z)*cos(x) - 0.3` or a shell `sqrt(x*x+y*y+z*z) - 20`. The
//! expression is compiled ONCE here ([`crate::sdf_expr::compile_expr`])
//! into the [`fielddraft_protocol::SdfExprOp`] tape that
//! [`Primitive::CustomSdf`] carries — parse errors surface as regen
//! errors in the dialog. The field is evaluated as
//! `max(expr / lipschitz, box(size))`: normalised by the declared
//! Lipschitz allowance and clipped to a finite box (see the protocol
//! docs). Custom bodies are SDF-only (no B-Rep weave), like the gyroid.

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

use crate::csg::{DecodedFeature, PrimitiveModifiers};

use super::merge_scope::target_ids_from_queries;
use super::{
    optional_display_name, optional_rotation, optional_scale, positive_f32, Context,
    FeatureError, FeatureKind, FeatureSchema,
};

/// `custom_sdf` — implicit body from a user expression.
#[derive(FeatureSchema)]
#[schema(type = "custom_sdf")]
pub struct CustomSdfDef {
    /// Expression over `x, y, z` (mm). Functions: sin cos sqrt abs exp
    /// ln floor asin acos min max pow atan2; constants pi, tau; `^`.
    #[param(name = "Expression", default = String::from("sqrt(x*x + y*y + z*z) - 15"))]
    pub expression: String,
    /// Lipschitz allowance L (max |gradient| of the expression). The
    /// field is divided by L so marching stays conservative.
    #[param(name = "Lipschitz", default = 1.0)]
    pub lipschitz: f32,
    /// Local bounding half-extents in mm — the field is clipped to
    /// this box.
    #[param(name = "Half extents", default = [20.0, 20.0, 20.0])]
    pub half_extents: [f32; 3],
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    #[param(name = "Bodies", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
    #[param(name = "Blend radius", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub blend_radius: ::fielddraft_protocol::Length,
    #[param(name = "Chamfer size", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub chamfer_size: ::fielddraft_protocol::Length,
    #[param(name = "Shell thickness", default = ::fielddraft_protocol::Length::mm(0.0))]
    pub shell_thickness: ::fielddraft_protocol::Length,
}

pub struct CustomSdfFeature;

impl FeatureKind for CustomSdfFeature {
    fn type_name(&self) -> &'static str {
        CustomSdfDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        CustomSdfDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = CustomSdfDef::parse(def)?;
        positive_f32(d.lipschitz, CustomSdfDef::TYPE_NAME, "lipschitz")?;
        for (i, h) in d.half_extents.iter().enumerate() {
            if !h.is_finite() || *h <= 0.0 {
                return Err(FeatureError::InvalidParameter {
                    feature: CustomSdfDef::TYPE_NAME,
                    name: "half_extents",
                    message: format!("half_extents[{i}] must be positive"),
                });
            }
        }
        let ops = crate::sdf_expr::compile_expr(&d.expression).map_err(|e| {
            FeatureError::InvalidParameter {
                feature: CustomSdfDef::TYPE_NAME,
                name: "expression",
                message: e,
            }
        })?;

        let name = optional_display_name(def, || format!("Custom SDF {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        let target_ids = target_ids_from_queries(&d.targets);

        ctx.features.push(DecodedFeature::from_parts(
            id.child("0"),
            name,
            d.op,
            Primitive::CustomSdf {
                source: d.expression.clone(),
                ops,
                lipschitz: d.lipschitz,
                half_extents: d.half_extents,
            },
            Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            PrimitiveModifiers {
                blend_radius: d.blend_radius.to_mm(),
                chamfer_size: d.chamfer_size.to_mm(),
                shell_thickness: d.shell_thickness.to_mm(),
                ..PrimitiveModifiers::NONE
            },
            target_ids,
        ));
        Ok(())
    }
}
