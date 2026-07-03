//! `hole` — Bohrungs-Feature (simple/counterbore/countersink).

use serde_json::Value;

use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

use crate::csg::{DecodedFeature, PrimitiveModifiers};

use super::merge_scope::target_ids_from_queries;
use super::{Context, FeatureError, FeatureKind, FeatureSchema, optional_display_name, optional_rotation, optional_scale, positive_f32};
// ─── K8.9: Hole ──────────────────────────────────────────────────
//
// HoleFeature is a parametric subtract-cylinder. It carries the
// same geometry as a `cylinder` call with op=difference, but the
// dialog-facing schema uses `Length`-typed `diameter` and `depth`
// fields (Onshape parity) so users don't have to mentally halve
// the diameter into a radius or double the depth into a
// half-height. The execute path lowers to Primitive::Cylinder
// with half_extent math done at the model-unit boundary, which
// keeps the mesher and BVH completely ignorant of the dedicated
// Hole kind.
//
// The `op` field is intentionally absent: a hole that adds
// material isn't a hole. If the user wants to turn it into a
// union or intersect they can delete it and add a regular
// cylinder feature instead.

/// `hole` — parametric cylinder subtract with diameter + depth.
#[derive(FeatureSchema)]
#[schema(type = "hole")]
pub struct HoleDef {
    /// Diameter at the mouth of the hole.
    pub diameter: ::fielddraft_protocol::Length,
    /// Total depth along the local Y axis.
    pub depth: ::fielddraft_protocol::Length,
    /// World-space position of the hole's centre.
    #[param(default = [0.0, 0.0, 0.0])]
    pub translate: [f32; 3],

    /// Merge scope: the body/bodies the hole drills, as
    /// `qCreatedBy(<seed_feature>)` queries. MANDATORY — a hole with no scope
    /// is flagged with a warning and drills nothing (Onshape semantics: scope
    /// is always explicit).
    #[param(name = "Scope", default = ::fielddraft_protocol::MultiQuery::new(""), entity_filter = "BODY")]
    pub targets: ::fielddraft_protocol::MultiQuery,
}

pub struct HoleFeature;

impl FeatureKind for HoleFeature {
    fn type_name(&self) -> &'static str {
        HoleDef::TYPE_NAME
    }

    fn schema_json(&self) -> Value {
        HoleDef::schema_json()
    }

    fn execute(
        &self,
        ctx: &mut Context<'_>,
        id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let d = HoleDef::parse(def)?;
        let diameter_mm = d.diameter.to_mm();
        let depth_mm = d.depth.to_mm();
        positive_f32(diameter_mm, HoleDef::TYPE_NAME, "diameter")?;
        positive_f32(depth_mm, HoleDef::TYPE_NAME, "depth")?;

        let name = optional_display_name(def, || format!("Hole {id}"));
        let rotation = optional_rotation(def);
        let scale = optional_scale(def);
        // Scope is mandatory: a hole drills the bodies named in `targets`.
        // With none chosen it warns and drills nothing (partition seeds it as
        // a standalone red preview).
        let target_ids = target_ids_from_queries(&d.targets);
        if let Some(w) = ctx.merge_scope_warning(BoolOp::Remove, &target_ids) {
            ctx.warn_feature(id, w);
        }
        // Spine entry uses a hierarchical child id (`<hole>/0`)
        // so `Document::sync_feature_calls` drops it on the
        // next sync — the authoritative wire form is the
        // original `hole` FeatureCall, and we don't want a
        // stray `cylinder` FeatureCall to be synthesised from
        // the derived primitive. Same pattern the K8.0 pattern
        // features use for their spine clones.
        ctx.features.push(DecodedFeature::from_parts(
            id.child("0"),
            name,
            BoolOp::Remove,
            Primitive::Cylinder {
                center: [0.0, 0.0, 0.0],
                radius: diameter_mm / 2.0,
                half_height: depth_mm / 2.0,
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
