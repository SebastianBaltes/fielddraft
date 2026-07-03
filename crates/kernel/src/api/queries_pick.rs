//! Szenen-Queries und Picking (Szenen-AABB, Ray-Pick mit Element-
//! Auflösung).

use wasm_bindgen::prelude::*;

use crate::Kernel;
use crate::bvh::BoundedCsg;
use crate::csg::{CsgNode};
use crate::csg;
use crate::raycast::{raycast, RaycastParams};
#[wasm_bindgen]
impl Kernel {
    /// K8.10: list every document-level variable as a JSON
    /// array of `{name, value}` pairs, sorted by name (the
    /// BTreeMap ordering).
    /// K9.1: return the overall scene AABB as a JSON object
    /// `{min: [x,y,z], max: [x,y,z], size: [w,h,d]}` in mm.
    /// Empty scenes return all-zero extents.
    #[wasm_bindgen(js_name = sceneAabb)]
    pub fn scene_aabb(&self) -> Result<String, JsError> {
        let tree = csg::CsgNode::from_decoded(&self.decoded)
            .apply_edge_fillets(&self.edge_fillets);
        let bb = tree.aabb();
        let size = bb.size();
        let result = serde_json::json!({
            "min": bb.min,
            "max": bb.max,
            "size": size,
        });
        serde_json::to_string(&result).map_err(|e| JsError::new(&e.to_string()))
    }


    // ── picking / provenance ──────────────────────────────────────────
    //
    // K2 wire-format: the kernel takes a world-space ray (origin +
    // direction) and returns a small JSON blob that the frontend can
    // decode without touching the provenance structs directly. On a
    // miss the payload is `{"hit":false}`.

    /// Sphere-trace a ray through the current CSG tree and return a
    /// JSON description of the first hit, if any. See `raycast.rs`
    /// for the numerical scheme.
    #[wasm_bindgen(js_name = pick)]
    pub fn pick(
        &self,
        ox: f32,
        oy: f32,
        oz: f32,
        dx: f32,
        dy: f32,
        dz: f32,
    ) -> String {
        let csg = CsgNode::from_decoded(&self.decoded)
            .apply_edge_fillets(&self.edge_fillets);
        let bounded = BoundedCsg::from_csg(&csg);
        let params = RaycastParams::default();
        match raycast(&bounded, [ox, oy, oz], [dx, dy, dz], params) {
            None => r#"{"hit":false}"#.to_string(),
            Some(hit) => {
                // Two parallel arrays, both tiny and stable — hand-formatted
                // to avoid serde_json::Value churn:
                //   `contributors`: the flat root *feature* ids (sub-index
                //     stripped). This is what selection, query-picking and
                //     edge fillets consume — they map an id to a feature in
                //     the tree, so they need the feature, not the face.
                //   `branches`: the packed BranchIds (feature << SUB_BITS |
                //     sub) in the same order. The analytic B-Rep edge overlay
                //     matches a hovered edge by its colliding-face signature,
                //     which is exactly these packed per-face ids.
                let mut contributors = String::from("[");
                let mut branches = String::from("[");
                for (i, id) in hit.contributors.as_slice().iter().enumerate() {
                    if i > 0 {
                        contributors.push(',');
                        branches.push(',');
                    }
                    contributors.push_str(&id.feature_id().to_string());
                    branches.push_str(&id.as_u32().to_string());
                }
                contributors.push(']');
                branches.push(']');
                format!(
                    r#"{{"hit":true,"point":[{},{},{}],"normal":[{},{},{}],"t":{},"element":"{}","contributors":{},"branches":{}}}"#,
                    hit.point[0],
                    hit.point[1],
                    hit.point[2],
                    hit.normal[0],
                    hit.normal[1],
                    hit.normal[2],
                    hit.t,
                    hit.element.label(),
                    contributors,
                    branches,
                )
            }
        }
    }
}
