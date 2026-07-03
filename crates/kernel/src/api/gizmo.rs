//! Gizmo-getriebene Transform-Mutationen: translate/rotate/scale,
//! Nudge und komplette Transform-Übernahme.

use fielddraft_protocol::{FeatureId};
use wasm_bindgen::prelude::*;

use crate::Kernel;
use crate::{quat_to_euler_xyz};
/// Read a 3-element float vector out of a `FeatureCall` params
/// object, falling back to `default` when the key is absent or
/// malformed. Used by the call-first transform mutators to detect
/// no-op edits against the current wire value.
fn json_vec3(
    params: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: [f32; 3],
) -> [f32; 3] {
    match params.get(key).and_then(|v| v.as_array()) {
        Some(a) if a.len() == 3 => [
            a[0].as_f64().unwrap_or(default[0] as f64) as f32,
            a[1].as_f64().unwrap_or(default[1] as f64) as f32,
            a[2].as_f64().unwrap_or(default[2] as f64) as f32,
        ],
        _ => default,
    }
}

/// Quaternion sibling of [`json_vec3`] (default identity rotation).
fn json_vec4(
    params: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: [f32; 4],
) -> [f32; 4] {
    match params.get(key).and_then(|v| v.as_array()) {
        Some(a) if a.len() == 4 => [
            a[0].as_f64().unwrap_or(default[0] as f64) as f32,
            a[1].as_f64().unwrap_or(default[1] as f64) as f32,
            a[2].as_f64().unwrap_or(default[2] as f64) as f32,
            a[3].as_f64().unwrap_or(default[3] as f64) as f32,
        ],
        _ => default,
    }
}

#[wasm_bindgen]
impl Kernel {
    /// Replace the translate component of a feature's transform.
    #[wasm_bindgen(js_name = setFeatureTranslate)]
    pub fn set_feature_translate(
        &mut self,
        id: u32,
        tx: f32,
        ty: f32,
        tz: f32,
    ) -> bool {
        let id = FeatureId::from_u32(id);
        self.edit_feature_call_params(id, |p| {
            if json_vec3(p, "translate", [0.0, 0.0, 0.0]) == [tx, ty, tz] {
                return false;
            }
            p.insert("translate".into(), serde_json::json!([tx, ty, tz]));
            true
        })
    }

    /// K7.21h: replace the rotation component of a feature's transform.
    /// Quaternion order: (x, y, z, w).
    #[wasm_bindgen(js_name = setFeatureRotation)]
    pub fn set_feature_rotation(
        &mut self,
        id: u32,
        rx: f32,
        ry: f32,
        rz: f32,
        rw: f32,
    ) -> bool {
        let id = FeatureId::from_u32(id);
        let rot = [rx, ry, rz, rw];
        self.edit_feature_call_params(id, |p| {
            if json_vec4(p, "rotation", [0.0, 0.0, 0.0, 1.0]) == rot {
                return false;
            }
            p.insert("rotation".into(), serde_json::json!(rot));
            true
        })
    }

    #[wasm_bindgen(js_name = setFeatureScale)]
    pub fn set_feature_scale(&mut self, id: u32, sx: f32, sy: f32, sz: f32) -> bool {
        let id = FeatureId::from_u32(id);
        self.edit_feature_call_params(id, |p| {
            let cur = json_vec3(p, "scale", [1.0, 1.0, 1.0]);
            if (cur[0] - sx).abs() < 1e-7
                && (cur[1] - sy).abs() < 1e-7
                && (cur[2] - sz).abs() < 1e-7
            {
                return false;
            }
            p.insert("scale".into(), serde_json::json!([sx, sy, sz]));
            true
        })
    }

    fn apply_gizmo_feature(
        &mut self,
        type_name: &str,
        target_id: u32,
        params: serde_json::Value,
    ) -> bool {
        let target = FeatureId::from_u32(target_id);
        if !self.decoded.iter().any(|f| f.id == target) {
            return false;
        }

        let should_merge = self.document.feature_calls.last().is_some_and(|c| {
            c.type_name == type_name
                && !c.suppressed
                && c.params.get("target_id")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32 == target_id
        });

        if should_merge {
            let idx = self.document.feature_calls.len() - 1;
            self.document.feature_calls[idx].params = params;
        } else {
            self.snapshot();
            let id = self.document.allocate_id();
            self.document.feature_calls.push(
                fielddraft_protocol::FeatureCall {
                    id,
                    type_name: type_name.to_string(),
                    params,
                    suppressed: false,
                },
            );
        }

        let _ = self.bootstrap_features_from_calls();
        self.rebuild_packed();
        true
    }

    #[wasm_bindgen(js_name = applyGizmoTranslate)]
    pub fn apply_gizmo_translate(
        &mut self,
        target_id: u32,
        tx: f32, ty: f32, tz: f32,
    ) -> bool {
        let params = serde_json::json!({
            "target_id": target_id,
            "translate": [tx, ty, tz],
        });
        self.apply_gizmo_feature("translate", target_id, params)
    }

    #[wasm_bindgen(js_name = applyGizmoRotate)]
    pub fn apply_gizmo_rotate(
        &mut self,
        target_id: u32,
        rx: f32, ry: f32, rz: f32, rw: f32,
    ) -> bool {
        let euler = quat_to_euler_xyz(rx, ry, rz, rw);
        let params = serde_json::json!({
            "target_id": target_id,
            "rotation": [euler[0], euler[1], euler[2]],
        });
        self.apply_gizmo_feature("rotate", target_id, params)
    }

    #[wasm_bindgen(js_name = applyGizmoScale)]
    pub fn apply_gizmo_scale(
        &mut self,
        target_id: u32,
        sx: f32, sy: f32, sz: f32,
    ) -> bool {
        let params = serde_json::json!({
            "target_id": target_id,
            "scale": [sx, sy, sz],
        });
        self.apply_gizmo_feature("scale", target_id, params)
    }

    /// Add `(dx, dy, dz)` to a feature's translate. Convenience for
    /// keyboard nudging.
    #[wasm_bindgen(js_name = nudgeFeature)]
    pub fn nudge_feature(&mut self, id: u32, dx: f32, dy: f32, dz: f32) -> bool {
        if dx == 0.0 && dy == 0.0 && dz == 0.0 {
            return false;
        }
        let id = FeatureId::from_u32(id);
        self.edit_feature_call_params(id, |p| {
            let cur = json_vec3(p, "translate", [0.0, 0.0, 0.0]);
            p.insert(
                "translate".into(),
                serde_json::json!([cur[0] + dx, cur[1] + dy, cur[2] + dz]),
            );
            true
        })
    }

    /// Replace the full transform (translate, quaternion, scale).
    #[wasm_bindgen(js_name = setFeatureTransform)]
    #[allow(clippy::too_many_arguments)] // wasm-bindgen geometry API; flat scalar params
    pub fn set_feature_transform(
        &mut self,
        id: u32,
        tx: f32,
        ty: f32,
        tz: f32,
        qx: f32,
        qy: f32,
        qz: f32,
        qw: f32,
        sx: f32,
        sy: f32,
        sz: f32,
    ) -> bool {
        let id = FeatureId::from_u32(id);
        self.edit_feature_call_params(id, |p| {
            let t = json_vec3(p, "translate", [0.0, 0.0, 0.0]);
            let r = json_vec4(p, "rotation", [0.0, 0.0, 0.0, 1.0]);
            let s = json_vec3(p, "scale", [1.0, 1.0, 1.0]);
            if t == [tx, ty, tz]
                && r == [qx, qy, qz, qw]
                && s == [sx, sy, sz]
            {
                return false;
            }
            p.insert("translate".into(), serde_json::json!([tx, ty, tz]));
            p.insert("rotation".into(), serde_json::json!([qx, qy, qz, qw]));
            p.insert("scale".into(), serde_json::json!([sx, sy, sz]));
            true
        })
    }

}
