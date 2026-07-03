//! Gizmo-Transform-Features: `translate`, `rotate`, `scale`
//! samt Placement-Kompositions-Helfer.

use serde_json::Value;

use fielddraft_protocol::{FeatureId, Transform};

use super::{Context, FeatureError, FeatureKind};
// ─── Translate / Rotate / Scale Features ───────────────────────
//
// Three separate gizmo-driven features that each modify one
// component of a target feature's transform. Consecutive gizmo
// drags of the same type on the same target are merged by the
// kernel's `apply_gizmo_*` methods.

fn read_vec3(params: &Value, key: &str, default: [f32; 3]) -> [f32; 3] {
    if let Some(arr) = params.get(key).and_then(|v| v.as_array()) {
        let c = |i: usize| arr.get(i).and_then(|v| v.as_f64()).unwrap_or(default[i] as f64) as f32;
        [c(0), c(1), c(2)]
    } else {
        default
    }
}

fn read_target_id(params: &Value) -> u32 {
    params.get("target_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32
}

fn find_target(
    ctx: &mut Context<'_>,
    target_id: u32,
    feature: &'static str,
) -> Result<usize, FeatureError> {
    let target = FeatureId::from_u32(target_id);
    ctx.features.iter().position(|f| f.id == target)
        .ok_or_else(|| FeatureError::InvalidParameter {
            feature,
            name: "target_id",
            message: format!("target feature {} not found", target_id),
        })
}

/// Compose two transforms: `outer ∘ inner`, i.e. the placement that maps a
/// point through `inner` then `outer` (`outer.apply(inner.apply(p))`). Used to
/// lift a B-Rep face's body-local surface transform into world space via the
/// body `placement`. Assumes rigid (or uniformly-scaled) placements — the
/// shell gate's curved-surface SDF presumes a unit metric, so a non-uniform
/// body scale would distort the opening taper (the common case is rigid).
pub(crate) fn compose_placement(outer: &Transform, inner: &Transform) -> Transform {
    let scaled = [
        inner.translate[0] * outer.scale[0],
        inner.translate[1] * outer.scale[1],
        inner.translate[2] * outer.scale[2],
    ];
    let rotated = crate::quat_rotate(outer.rotation, scaled);
    Transform {
        translate: [
            rotated[0] + outer.translate[0],
            rotated[1] + outer.translate[1],
            rotated[2] + outer.translate[2],
        ],
        rotation: crate::quat_multiply(outer.rotation, inner.rotation),
        scale: [
            outer.scale[0] * inner.scale[0],
            outer.scale[1] * inner.scale[1],
            outer.scale[2] * inner.scale[2],
        ],
    }
}

// ── Translate ──────────────────────────────────────────────────

pub struct TranslateFeature;

impl FeatureKind for TranslateFeature {
    fn type_name(&self) -> &'static str { "translate" }
    fn schema_json(&self) -> Value {
        serde_json::json!({
            "type": "translate",
            "params": [
                {"key": "translate", "name": "Translation", "type": "vec3", "required": false, "unit": "mm"}
            ]
        })
    }
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let idx = find_target(ctx, read_target_id(def), "translate")?;
        ctx.features[idx].transform.translate =
            read_vec3(def, "translate", [0.0, 0.0, 0.0]);
        Ok(())
    }
}

// ── Rotate ─────────────────────────────────────────────────────

pub struct RotateFeature;

impl FeatureKind for RotateFeature {
    fn type_name(&self) -> &'static str { "rotate" }
    fn schema_json(&self) -> Value {
        serde_json::json!({
            "type": "rotate",
            "params": [
                {"key": "rotation", "name": "Rotation", "type": "vec3", "required": false, "unit": "\u{00b0}"}
            ]
        })
    }
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let euler = read_vec3(def, "rotation", [0.0, 0.0, 0.0]);
        let idx = find_target(ctx, read_target_id(def), "rotate")?;
        ctx.features[idx].transform.rotation =
            crate::transform::euler_xyz_to_quat(euler[0], euler[1], euler[2]);
        Ok(())
    }
}

// ── Scale ──────────────────────────────────────────────────────

pub struct ScaleFeature;

impl FeatureKind for ScaleFeature {
    fn type_name(&self) -> &'static str { "scale" }
    fn schema_json(&self) -> Value {
        serde_json::json!({
            "type": "scale",
            "params": [
                {"key": "scale", "name": "Scale", "type": "vec3", "required": false,
                 "default": [1.0, 1.0, 1.0]}
            ]
        })
    }
    fn execute(
        &self,
        ctx: &mut Context<'_>,
        _id: &FeatureId,
        def: &Value,
    ) -> Result<(), FeatureError> {
        let idx = find_target(ctx, read_target_id(def), "scale")?;
        ctx.features[idx].transform.scale =
            read_vec3(def, "scale", [1.0, 1.0, 1.0]);
        Ok(())
    }
}
