//! `material` — Farbe/Material eines Ziel-Features.

use serde_json::Value;

use fielddraft_protocol::{FeatureId};

use super::{Context, FeatureError, FeatureKind, FeatureSchema};
// ─── Material ────────────────────────────────────────────────────
//
// Non-geometric feature that assigns PBR + engineering material
// properties to one or more target bodies.  The kernel stores it
// as a FeatureCall (no spine entry); the frontend reads the params
// to drive shader uniforms.

#[derive(FeatureSchema)]
#[schema(type = "material")]
pub struct MaterialDef {
    #[param(name = "Bodies", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,

    // ── PBR optical ──────────────────────────────────────────
    #[param(name = "Color", default = [0.55, 0.65, 0.85], ui_hint = "color")]
    pub color: [f32; 3],
    #[param(name = "Roughness", default = 0.35, ui_hint = "slider")]
    pub roughness: f32,
    #[param(name = "Metalness", default = 0.0, ui_hint = "slider")]
    pub metalness: f32,
    #[param(name = "Emissive", default = [0.0, 0.0, 0.0], ui_hint = "color")]
    pub emissive: [f32; 3],
    #[param(name = "Opacity", default = 1.0, ui_hint = "slider")]
    pub opacity: f32,
    #[param(name = "Wireframe", default = false)]
    pub wireframe: bool,
    #[param(name = "Displace depth", default = 0.0)]
    pub displace_depth: f32,
    #[param(name = "Displace freq", default = 0.0)]
    pub displace_freq: f32,

    // ── Engineering ──────────────────────────────────────────
    #[param(name = "Density (kg/m³)", default = 0.0)]
    pub density: f32,
    #[param(name = "Young's modulus (GPa)", default = 0.0)]
    pub youngs_modulus: f32,
    #[param(name = "Poisson's ratio", default = 0.0)]
    pub poissons_ratio: f32,
    #[param(name = "Yield tensile (MPa)", default = 0.0)]
    pub yield_tensile: f32,
    #[param(name = "Ultimate tensile (MPa)", default = 0.0)]
    pub ultimate_tensile: f32,
    #[param(name = "Yield compressive (MPa)", default = 0.0)]
    pub yield_compressive: f32,
}

pub struct MaterialFeature;

impl FeatureKind for MaterialFeature {
    fn type_name(&self) -> &'static str {
        MaterialDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        MaterialDef::schema_json()
    }

    fn execute(
        &self,
        _ctx: &mut Context<'_>,
        _id: &FeatureId,
        _def: &Value,
    ) -> Result<(), FeatureError> {
        Ok(())
    }
}
