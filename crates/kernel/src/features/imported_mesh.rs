//! `imported_mesh` — STL/3MF-Import als SDF-Primitiv.

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

use crate::csg::{DecodedFeature, PrimitiveModifiers};

use super::merge_scope::target_ids_from_queries;
use super::{Context, FeatureError, FeatureKind, FeatureSchema, optional_display_name, optional_rotation};
// ─── K13.1: Imported Mesh as CSG Solid ───────────────────────────
//
// Wraps a MeshSdf (triangle buffer + per-tri AABBs + ray-parity
// sign test) as a full CSG leaf. The mesh data serialises inline
// in the FeatureCall params so it survives save/load via
// localStorage. Performance is O(N) per sample — acceptable for
// imports up to ~50K triangles at DC default resolution.

/// `imported_mesh` — an imported triangle mesh that
/// participates in CSG Booleans.
#[derive(FeatureSchema)]
#[schema(type = "imported_mesh")]
pub struct ImportedMeshDef {
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],
    #[param(name = "Scale", default = 1.0)]
    pub uniform_scale: f32,
    #[param(name = "Z-up", default = false)]
    pub swap_yz: bool,
    #[param(default = ::fielddraft_protocol::BoolOp::New)]
    pub op: BoolOp,
    /// Target bodies for the boolean op (Add/Remove/Intersect), as
    /// `qCreatedBy(<seed_feature>)` queries. Empty for New; required otherwise.
    #[param(name = "Targets", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
}

pub struct ImportedMeshFeature;

impl FeatureKind for ImportedMeshFeature {
    fn type_name(&self) -> &'static str {
        ImportedMeshDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        ImportedMeshDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = ImportedMeshDef::parse(def)?;

        // Compact format (v2): positions + indices stored separately.
        // Legacy format (v1): full MeshSdf under "__mesh".
        let mesh: fielddraft_protocol::MeshSdf = if let Some(pos_val) = def.get("__positions") {
            let positions: Vec<f32> = serde_json::from_value(pos_val.clone()).map_err(|e| {
                FeatureError::InvalidParameter {
                    feature: ImportedMeshDef::TYPE_NAME,
                    name: "__positions",
                    message: e.to_string(),
                }
            })?;
            let idx_val = def.get("__indices").ok_or(FeatureError::MissingParameter {
                feature: ImportedMeshDef::TYPE_NAME,
                name: "__indices",
            })?;
            let indices: Vec<u32> = serde_json::from_value(idx_val.clone()).map_err(|e| {
                FeatureError::InvalidParameter {
                    feature: ImportedMeshDef::TYPE_NAME,
                    name: "__indices",
                    message: e.to_string(),
                }
            })?;
            fielddraft_protocol::MeshSdf::from_buffers(&positions, &indices)
        } else {
            let mesh_val = def.get("__mesh").ok_or(FeatureError::MissingParameter {
                feature: ImportedMeshDef::TYPE_NAME,
                name: "__mesh",
            })?;
            serde_json::from_value(mesh_val.clone()).map_err(|e| {
                FeatureError::InvalidParameter {
                    feature: ImportedMeshDef::TYPE_NAME,
                    name: "__mesh",
                    message: e.to_string(),
                }
            })?
        };

        let name = optional_display_name(def, || format!("Imported mesh {id}"));
        let base_rotation = optional_rotation(def);
        let s = if d.uniform_scale > 0.0 { d.uniform_scale } else { 1.0 };
        let scale = [s, s, s];

        let rotation = if d.swap_yz {
            // -90° around X to convert Z-up → Y-up
            let swap: [f32; 4] = [-std::f32::consts::FRAC_1_SQRT_2, 0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2];
            crate::quat_multiply(swap, base_rotation)
        } else {
            base_rotation
        };

        let target_ids = target_ids_from_queries(&d.targets);
        if let Some(w) = ctx.merge_scope_warning(d.op, &target_ids) {
            ctx.warn_feature(id, w);
        }

        ctx.features.push(DecodedFeature::from_parts(
            id.clone(),
            name,
            d.op,
            Primitive::ImportedMesh {
                mesh: Box::new(mesh),
            },
            Transform {
                translate: d.translate,
                rotation,
                scale,
            },
            PrimitiveModifiers::NONE,
            target_ids,
        ));
        Ok(())
    }
}
