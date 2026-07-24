//! Feature-CRUD: Schema-Lookup, Outliner-Liste, add_*-Konstruktoren,
//! Params-Editing, Suppress/Remove/Reorder.

use fielddraft_protocol::{FeatureId, Primitive};
use wasm_bindgen::prelude::*;

use crate::Kernel;
use crate::features;
/// Compact JS view of one feature for the outliner.
#[derive(serde::Serialize)]
struct FeatureSummary {
    id: FeatureId,
    name: String,
    op: &'static str,
    kind: String,
    translate: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
    /// K8.7: mirrors `FeatureCall.suppressed`. The outliner
    /// reads this to render the eye-icon affordance and to
    /// grey out the row when the feature is skipped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    suppressed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    rolled_back: bool,
    /// Regen error message of a failing feature (ticket sx5a) — the
    /// outliner paints the row red and exposes this as the tooltip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Non-fatal merge-scope warning — the outliner paints the row in the
    /// warning colour and exposes this as the tooltip; the body renders as a
    /// red translucent preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
    /// #384 offset_capability: for an `offset`/`shell` feature, the
    /// machine-readable capability contract of its regen-time expansion
    /// (exact / tolerance_bounded / conservative / unsupported + reasons). The
    /// outliner badges it and the FeatureDialog shows the reasons; omitted for
    /// every other feature kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capability: Option<crate::offset_capability::FeatureCapability>,
}

/// Collect every `qCreatedBy(<feature_id>)` reference embedded in a feature
/// call's params, walking nested arrays/objects and comma-separated
/// MultiQuery strings. Datum references (`qCreatedBy(origin_plane_xy)`) and
/// other non-numeric targets don't parse to a [`FeatureId`] and are skipped.
/// Used by [`Kernel::reorder_feature`] to keep the tree a valid program
/// (every feature references only earlier features).
fn created_by_references(params: &serde_json::Value) -> Vec<FeatureId> {
    fn walk(v: &serde_json::Value, out: &mut Vec<FeatureId>) {
        match v {
            serde_json::Value::String(s) => {
                for part in s.split(',') {
                    if let Some(inner) = part
                        .trim()
                        .strip_prefix("qCreatedBy(")
                        .and_then(|x| x.strip_suffix(')'))
                    {
                        if let Ok(fid) = inner.parse::<FeatureId>() {
                            out.push(fid);
                        }
                    }
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            serde_json::Value::Object(o) => o.values().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(params, &mut out);
    out
}

#[wasm_bindgen]
impl Kernel {
    // ── outliner queries ──────────────────────────────────────────────

    // ── K6a feature-schema lookup ────────────────────────────────────
    //
    // The FeatureDialog generator on the TypeScript side consumes
    // the JSON schema that the K4.5.4a `FeatureSchema` derive
    // emits for every definition struct. These two accessors
    // walk the built-in `FeatureRegistry` to expose the schema
    // and the list of known types to the wasm-bindgen surface;
    // the schemas are serialised as strings so wasm-bindgen
    // doesn't need to learn about `serde_json::Value`.

    /// K6a: list every feature type name the UI can instantiate,
    /// sorted for deterministic UI rendering. The result is a JSON
    /// array of strings, e.g. `["box", "cylinder", "extrude",
    /// "sphere"]`. Built-in registry names are joined with every
    /// document FieldSkript id (a script's id doubles as its
    /// `FeatureCall::type_name`), so saved scripts show up as
    /// instantiable types alongside the built-ins.
    #[wasm_bindgen(js_name = listFeatureTypes)]
    pub fn list_feature_types(&self) -> Result<String, JsError> {
        let registry = features::FeatureRegistry::with_builtins();
        let mut names = registry.type_names();
        names.extend(self.document.scripts.iter().map(|r| r.id.clone()));
        names.sort();
        names.dedup();
        serde_json::to_string(&names).map_err(|e| JsError::new(&e.to_string()))
    }

    /// K6a: look up the FeatureSchema-generated JSON schema for
    /// a feature type. Returns the serialised JSON string or an
    /// error if the type isn't registered. A document FieldSkript
    /// (id == `type_name`) resolves to its stored `schema` blob —
    /// the same RawFeatureSchema the web FeatureDialog consumes — so
    /// the outliner's param dialog works for script instances too;
    /// otherwise the built-in registry is consulted.
    #[wasm_bindgen(js_name = schemaForType)]
    pub fn schema_for_type(&self, type_name: &str) -> Result<String, JsError> {
        if let Some(record) = self.document.scripts.iter().find(|r| r.id == type_name) {
            return serde_json::to_string(&record.schema)
                .map_err(|e| JsError::new(&e.to_string()));
        }
        let registry = features::FeatureRegistry::with_builtins();
        let value = registry
            .schema_for(type_name)
            .ok_or_else(|| JsError::new(&format!("unknown feature type `{type_name}`")))?;
        serde_json::to_string(&value).map_err(|e| JsError::new(&e.to_string()))
    }


    /// Return a JSON array of feature summaries (id, name, op, kind).
    ///
    /// K6b.3a: the list walks `feature_calls` — the K4.5 canonical
    /// wire form — instead of the spine directly. For primitive
    /// calls the summary is pulled from the matching spine entry
    /// (translate, scale, op are authoritative there). For
    /// non-spine calls like `sketch` the summary is synthesised
    /// from the call itself with neutral transform defaults, so
    /// the outliner sees a stable row per sketch even though
    /// sketches never push onto the spine.
    #[wasm_bindgen(js_name = listFeatures)]
    pub fn list_features(&self) -> Result<String, JsError> {
        let mut summaries: Vec<FeatureSummary> = Vec::with_capacity(
            self.document.feature_calls.len(),
        );
        for (idx, call) in self.document.feature_calls.iter().enumerate() {
            let rb = self.rollback_after.is_some_and(|ra| idx >= ra);
            if let Some(spine) = self
                .decoded
                .iter()
                .find(|f| f.id == call.id)
            {
                // DL-61 C2b: for a spine-backed call the call's
                // `type_name` already determines the outliner kind
                // (the decoded feature carries a lowered `node`, not a
                // `Primitive`), so we label straight off the type name.
                summaries.push(FeatureSummary {
                    id: spine.id.clone(),
                    name: spine.name.clone(),
                    op: Self::op_label(spine.op),
                    kind: self.kind_label(&call.type_name),
                    translate: spine.transform.translate,
                    rotation: spine.transform.rotation,
                    scale: spine.transform.scale,
                    suppressed: call.suppressed,
                    rolled_back: rb,
                    error: self.feature_errors.get(&call.id).cloned(),
                    warning: self.feature_warnings.get(&call.id).cloned(),
                    capability: self.feature_capabilities.get(&call.id).cloned(),
                });
            } else {
                // K8.9: hole pushes its spine entry under a
                // hierarchical child id (`<hole>/0`), so the
                // lookup above misses it. Surface the op that
                // matches the kind's known hard-coded semantics
                // here instead of defaulting to "add".
                let op = match call.type_name.as_str() {
                    "hole" => "remove",
                    "gyroid" => "intersect",
                    _ => "add",
                };
                // Try to pull the world-space translate from
                // the call's params so the outliner's ghost
                // meta badge shows the hole's real position.
                let translate = call
                    .params
                    .get("translate")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| {
                        if arr.len() != 3 {
                            return None;
                        }
                        let mut out = [0.0_f32; 3];
                        for (i, item) in arr.iter().enumerate() {
                            out[i] = item.as_f64()? as f32;
                        }
                        Some(out)
                    })
                    .unwrap_or([0.0, 0.0, 0.0]);
                let gizmo_label = match call.type_name.as_str() {
                    "translate" => Some("Translate"),
                    "rotate" => Some("Rotate"),
                    "scale" => Some("Scale"),
                    _ => None,
                };
                let name = if let Some(label) = gizmo_label {
                    let target_id = call.params.get("target_id")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    let target_name = self.decoded.iter()
                        .find(|f| f.id.as_u32_if_flat() == Some(target_id))
                        .map(|f| f.name.as_str())
                        .unwrap_or("?");
                    if let Some(user_name) = call.params.get("name").and_then(|v| v.as_str()) {
                        if !user_name.is_empty() { user_name.to_string() }
                        else { format!("{label} {target_name}") }
                    } else {
                        format!("{label} {target_name}")
                    }
                } else {
                    Self::synth_name_for_call(call)
                };
                summaries.push(FeatureSummary {
                    id: call.id.clone(),
                    name,
                    op,
                    kind: self.kind_label(&call.type_name),
                    translate,
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                    suppressed: call.suppressed,
                    rolled_back: rb,
                    error: self.feature_errors.get(&call.id).cloned(),
                    warning: self.feature_warnings.get(&call.id).cloned(),
                    capability: self.feature_capabilities.get(&call.id).cloned(),
                });
            }
        }
        serde_json::to_string(&summaries).map_err(|e| JsError::new(&e.to_string()))
    }


    /// K7.4: append an `extrude` feature call that lowers the
    /// referenced sketch into a 3D body via the K7.3
    /// `Primitive::Extrude` pipeline. Convenience wrapper over
    /// the full FeatureCall shape so the UI doesn't have to
    /// hand-assemble the params JSON.
    ///
    /// `sketch_id` must point at an existing `sketch`
    /// FeatureCall. `depth_mm` is the total slab depth (the
    /// `Primitive::Extrude` half_depth_z becomes `depth_mm /
    /// 2`). `op` uses the same `BoolOp::TAG_*` tags as
    /// `add_primitive`. `tz` positions the extrude plane along
    /// the world Z axis. `suppressed` creates the call as a
    /// PENDING ghost (ticket x7af, same flow as
    /// `addExtrudeFaces`): the body keeps its pre-extrude shape
    /// until the detail dialog confirms.
    #[wasm_bindgen(js_name = addExtrude)]
    pub fn add_extrude(
        &mut self,
        sketch_id: u32,
        depth_mm: f32,
        op: u32,
        tz: f32,
        suppressed: bool,
    ) -> Result<u32, JsError> {
        let sketch_feature_id = FeatureId::from_u32(sketch_id);
        let exists = self
            .document
            .feature_calls
            .iter()
            .any(|c| c.id == sketch_feature_id && c.type_name == "sketch");
        if !exists {
            return Err(JsError::new(&format!(
                "feature {sketch_id} is not a sketch"
            )));
        }
        let op_wire = Self::op_wire_name(op)?;
        self.push_feature_call_with(
            "extrude",
            serde_json::json!({
                "profiles": format!("qCreatedBy({sketch_feature_id})"),
                "depth": { "value": depth_mm, "unit": "mm" },
                "profile_half_x": 0.0,
                "profile_half_y": 0.0,
                "operation": op_wire,
                "center": [0.0, 0.0, tz],
            }),
            suppressed,
        )
    }

    /// K8.4: append a `revolve` feature call that consumes the
    /// sketch with id `sketch_id` and sweeps its profile around
    /// the local Y axis through `(cx, cy, cz)`. `op` uses the
    /// same `BoolOp::TAG_*` tags as `add_primitive`. Returns
    /// the new feature id.
    #[wasm_bindgen(js_name = addRevolve)]
    pub fn add_revolve(&mut self, sketch_id: u32, op: u32) -> Result<u32, JsError> {
        let op_wire = Self::op_wire_name(op)?;
        // `sketch_id == 0` creates an EMPTY revolve (ticket 1rax): the
        // dialog's query picker fills `profiles` + `axis` afterwards. Until
        // BOTH are resolved the revolve is invalid — it produces NO body and
        // reports a warning (label warn-colour + tooltip) instead of a garbage
        // solid (ticket ni56). The `axis` field is REQUIRED; there is no
        // implicit local-Y fallback.
        if sketch_id == 0 {
            return self.push_feature_call("revolve", serde_json::json!({
                "profiles": "",
                "operation": op_wire,
            }));
        }
        let sketch_feature_id = FeatureId::from_u32(sketch_id);
        let exists = self
            .document
            .feature_calls
            .iter()
            .any(|c| c.id == sketch_feature_id && c.type_name == "sketch");
        if !exists {
            return Err(JsError::new(&format!(
                "feature {sketch_id} is not a sketch"
            )));
        }
        self.push_feature_call("revolve", serde_json::json!({
            "profiles": format!("qCreatedBy({sketch_feature_id})"),
            "operation": op_wire,
        }))
    }

    /// Append a `helix` feature call sweeping the sketch's profile
    /// along a helical path around the sketch-local Y axis (real
    /// screw threads — see `HelixDef`). `pitch_mm` is the axial
    /// advance per revolution, `turns` the revolution count. As with
    /// `addRevolve`, `sketch_id == 0` creates an EMPTY helix whose
    /// `profiles` the dialog's query picker fills afterwards.
    #[wasm_bindgen(js_name = addHelix)]
    pub fn add_helix(
        &mut self,
        sketch_id: u32,
        op: u32,
        pitch_mm: f32,
        turns: f32,
    ) -> Result<u32, JsError> {
        let op_wire = Self::op_wire_name(op)?;
        if sketch_id == 0 {
            return self.push_feature_call("helix", serde_json::json!({
                "profiles": "",
                "operation": op_wire,
                "pitch": { "value": pitch_mm, "unit": "mm" },
                "turns": turns,
            }));
        }
        let sketch_feature_id = FeatureId::from_u32(sketch_id);
        let exists = self
            .document
            .feature_calls
            .iter()
            .any(|c| c.id == sketch_feature_id && c.type_name == "sketch");
        if !exists {
            return Err(JsError::new(&format!(
                "feature {sketch_id} is not a sketch"
            )));
        }
        self.push_feature_call("helix", serde_json::json!({
            "profiles": format!("qCreatedBy({sketch_feature_id})"),
            "operation": op_wire,
            "pitch": { "value": pitch_mm, "unit": "mm" },
            "turns": turns,
        }))
    }

    /// Append a `sweep` feature call: a profile sketch swept along a path
    /// sketch (Sketch2d path source). Both ids must be existing sketches.
    #[wasm_bindgen(js_name = addSweep)]
    pub fn add_sweep(
        &mut self,
        profile_sketch_id: u32,
        path_sketch_id: u32,
        op: u32,
    ) -> Result<u32, JsError> {
        let op_wire = Self::op_wire_name(op)?;
        for (sid, label) in [(profile_sketch_id, "profile"), (path_sketch_id, "path")] {
            if sid == 0 {
                return Err(JsError::new(&format!("sweep needs a {label} sketch")));
            }
            let fid = FeatureId::from_u32(sid);
            let exists = self
                .document
                .feature_calls
                .iter()
                .any(|c| c.id == fid && c.type_name == "sketch");
            if !exists {
                return Err(JsError::new(&format!("feature {sid} is not a sketch")));
            }
        }
        let profile_fid = FeatureId::from_u32(profile_sketch_id);
        let path_fid = FeatureId::from_u32(path_sketch_id);
        self.push_feature_call(
            "sweep",
            serde_json::json!({
                "profiles": format!("qCreatedBy({profile_fid})"),
                "path": { "kind": "sketch2d", "sketch": format!("qCreatedBy({path_fid})") },
                "operation": op_wire,
            }),
        )
    }

    /// Append a `sweep` along a default 3D control-point path (Gizmo3d source).
    /// The user then routes the spine by editing the control points. The
    /// default is a gentle S so the sweep is visible immediately.
    #[wasm_bindgen(js_name = addSweepGizmo)]
    pub fn add_sweep_gizmo(
        &mut self,
        profile_sketch_id: u32,
        op: u32,
    ) -> Result<u32, JsError> {
        let op_wire = Self::op_wire_name(op)?;
        if profile_sketch_id == 0 {
            return Err(JsError::new("sweep needs a profile sketch"));
        }
        let fid = FeatureId::from_u32(profile_sketch_id);
        let exists = self
            .document
            .feature_calls
            .iter()
            .any(|c| c.id == fid && c.type_name == "sketch");
        if !exists {
            return Err(JsError::new(&format!("feature {profile_sketch_id} is not a sketch")));
        }
        self.push_feature_call(
            "sweep",
            serde_json::json!({
                "profiles": format!("qCreatedBy({fid})"),
                "path": {
                    "kind": "gizmo3d",
                    "nodes": [[0.0, 0.0, 0.0], [40.0, 25.0, 0.0], [80.0, 25.0, 20.0], [120.0, 0.0, 20.0]],
                    "closed": false
                },
                "operation": op_wire,
            }),
        )
    }

    /// #385 P4: append a `loft` feature call that interpolates between N ≥ 2
    /// parallel profile cross-sections into a solid. `profiles` is a
    /// comma-separated MultiQuery of sketch-region and/or planar-face queries
    /// (e.g. `qCreatedBy(<sketch>), qFace(<id_hash>)`) — the same
    /// "faces and sketch regions" input as Extrude (ticket k280); the dialog's
    /// query picker accumulates the N profiles in a pick session. An EMPTY
    /// `profiles` string is allowed (like the empty revolve, ticket 1rax): the
    /// loft reports its regen error Onshape-style until the picker supplies at
    /// least two parallel profiles. `op` uses the same `BoolOp::TAG_*` tags as
    /// `add_primitive`. Returns the new feature id.
    #[wasm_bindgen(js_name = addLoft)]
    pub fn add_loft(&mut self, profiles: String, op: u32) -> Result<u32, JsError> {
        let op_wire = Self::op_wire_name(op)?;
        self.push_feature_call("loft", serde_json::json!({
            "profiles": profiles,
            "operation": op_wire,
        }))
    }

    /// #404 P1: append an Onshape-style `hole` feature call. Drills one
    /// identical cutter per location. `locations` is a comma-separated
    /// MultiQuery of point/anchor queries (`qSketchPoint(<sketch>, <entity>)`,
    /// `qCreatedBy(<mate_connector>)`, `qVertex(<hash>)`, faces); `targets` is
    /// the merge-scope MultiQuery of `qCreatedBy(<seed>)` body queries.
    /// `standard_size` is an ISO key (`"M6"`, …) or `"custom"`; when custom,
    /// `diameter_mm` is used. `end_type` is `"blind"` (exact `depth_mm`) or
    /// `"through_all"`. Diameter/depth go over the wire as `Length`
    /// (`{value, unit}`) — a bare f32 would silently fail the parse (#405).
    /// An EMPTY `locations`/`targets` is allowed: the hole reports nothing and
    /// drills nothing until the dialog's pickers supply them.
    #[wasm_bindgen(js_name = addHole)]
    pub fn add_hole(
        &mut self,
        standard_size: String,
        diameter_mm: f32,
        end_type: String,
        depth_mm: f32,
        locations: String,
        targets: String,
    ) -> Result<u32, JsError> {
        self.push_feature_call("hole", serde_json::json!({
            "style": "simple",
            "hole_type": "drilled",
            "standard_size": standard_size,
            "diameter": { "value": diameter_mm, "unit": "mm" },
            "end_type": end_type,
            "depth": { "value": depth_mm, "unit": "mm" },
            "locations": locations,
            "targets": targets,
        }))
    }

    /// K11.0: append a `gyroid` feature call. `cell_size_mm` is
    /// the cell period, `thickness_mm` the wall half-thickness.
    /// Default op is Intersect so the gyroid clips to any
    /// existing bodies. Returns the new id.
    #[wasm_bindgen(js_name = addGyroid)]
    pub fn add_gyroid(
        &mut self,
        cell_size_mm: f32,
        thickness_mm: f32,
        op: u32,
    ) -> Result<u32, JsError> {
        let op_wire = Self::op_wire_name(op)?;
        self.push_feature_call("gyroid", serde_json::json!({
            "cell_size": { "value": cell_size_mm, "unit": "mm" },
            "thickness": { "value": thickness_mm, "unit": "mm" },
            "op": op_wire,
            "blend_radius": { "value": 0.0, "unit": "mm" },
            "chamfer_size": { "value": 0.0, "unit": "mm" },
            "displace_depth": { "value": 0.0, "unit": "mm" },
            "shell_thickness": { "value": 0.0, "unit": "mm" },
        }))
    }

    /// Append a `custom_sdf` feature call: a user-defined implicit from
    /// a math expression over `x, y, z` (mm), clipped to a local box of
    /// `half_extent_mm` and normalised by `lipschitz`. Returns the new id.
    #[wasm_bindgen(js_name = addCustomSdf)]
    pub fn add_custom_sdf(&mut self, half_extent_mm: f32) -> Result<u32, JsError> {
        self.push_feature_call("custom_sdf", serde_json::json!({
            "expression": "sqrt(x*x + y*y + z*z) - 15",
            "lipschitz": 1.0,
            "half_extents": [half_extent_mm, half_extent_mm, half_extent_mm],
            "op": "new",
            "blend_radius": { "value": 0.0, "unit": "mm" },
            "chamfer_size": { "value": 0.0, "unit": "mm" },
            "shell_thickness": { "value": 0.0, "unit": "mm" },
        }))
    }

    #[wasm_bindgen(js_name = addBoolean)]
    pub fn add_boolean(&mut self) -> Result<u32, JsError> {
        self.push_feature_call("boolean", serde_json::json!({
            "operation": "Union",
            "tools": "",
            "targets": "",
            "keep_tools": false,
        }))
    }

    #[wasm_bindgen(js_name = addMaterial)]
    pub fn add_material(&mut self) -> Result<u32, JsError> {
        self.push_feature_call("material", serde_json::json!({
            "targets": "",
            "color": [0.55, 0.65, 0.85],
            "roughness": 0.35,
            "metalness": 0.0,
            "emissive": [0.0, 0.0, 0.0],
            "opacity": 1.0,
            "wireframe": false,
            "density": 0.0,
            "youngs_modulus": 0.0,
            "poissons_ratio": 0.0,
            "yield_tensile": 0.0,
            "ultimate_tensile": 0.0,
            "yield_compressive": 0.0,
        }))
    }

    /// K9.6 / C2: add a mate connector anchored to a B-Rep face. The
    /// face is given as a query string (e.g. `qFace(<id_hash>)`); the
    /// connector's CSYS is resolved from that face during regeneration.
    /// `ou`/`ov` shift the origin in the face's in-plane basis.
    /// Add a `mate_connector` anchored to a B-Rep element. `anchor` is a
    /// `qFace`/`qEdge`/`qVertex` query; the anchor defines +Z (edge direction /
    /// face normal). `t` positions an edge anchor (0.5 = midpoint, ignored
    /// otherwise). `secondary` is a reference face query that orients the
    /// secondary axes (edge) or defines +Z (vertex); empty → auto-picked.
    /// `reorient` rotates the secondary axes in 90° steps about +Z (0..3).
    /// `flip` reverses +Z.
    #[wasm_bindgen(js_name = addMateConnector)]
    pub fn add_mate_connector(
        &mut self,
        anchor: &str,
        t: f32,
        secondary: &str,
        reorient: u32,
        flip: bool,
    ) -> Result<u32, JsError> {
        self.push_feature_call("mate_connector", serde_json::json!({
            "anchor": anchor,
            "t": t,
            "secondary": secondary,
            "reorient": reorient,
            "flip": flip,
        }))
    }

    /// C2: resolved CSYS of every placed `mate_connector`, as a JSON
    /// array of `{ id, origin, normal, x_axis }`. The viewport reads this
    /// to render datum triads instead of re-resolving the face query.
    #[wasm_bindgen(js_name = resolvedMateConnectors)]
    pub fn resolved_mate_connectors(&self) -> Result<String, JsError> {
        serde_json::to_string(&self.mate_connectors).map_err(|e| JsError::new(&e.to_string()))
    }

    /// DL-61: add a fillet_edge feature selecting an edge by query
    /// (e.g. `qEdge(<id_hash>)`). The mask tube and feature pair are
    /// derived from the resolved edge during regeneration.
    #[wasm_bindgen(js_name = addFilletEdge)]
    pub fn add_fillet_edge(
        &mut self,
        edge: String,
        blend_radius: f32,
    ) -> Result<u32, JsError> {
        self.push_feature_call("fillet_edge", serde_json::json!({
            "edge": edge,
            "blend_radius": blend_radius,
        }))
    }

    /// Add a `shell` feature: hollow the body owning the picked face(s)
    /// — planar or curved (any analytic surface) — to a wall of
    /// `thickness_mm`, opening it flush with each face. `open_faces` is one
    /// or more comma-separated face queries (e.g. `qFace(<id_hash>)`).
    #[wasm_bindgen(js_name = addShell)]
    pub fn add_shell(
        &mut self,
        open_faces: String,
        thickness_mm: f32,
    ) -> Result<u32, JsError> {
        self.push_feature_call("shell", serde_json::json!({
            "open_faces": open_faces,
            "thickness": { "value": thickness_mm, "unit": "mm" },
        }))
    }

    /// d: round EVERY Boolean-junction edge of the model with one radius (the
    /// SDF-native "deburr everything" operator). A field blend never
    /// invalidates the solid, and between planar walls the round is an exact
    /// circle. Rounds feature-junction edges; a lone primitive's own edges are
    /// out of scope. `radius_mm == 0` clears the round.
    #[wasm_bindgen(js_name = addRoundAll)]
    pub fn add_round_all(&mut self, radius_mm: f32) -> Result<u32, JsError> {
        self.push_feature_call("round_all", serde_json::json!({
            "radius": { "value": radius_mm, "unit": "mm" },
        }))
    }

    /// Add an `offset` feature (#384 offset_tree): grow (`delta_mm > 0`) or
    /// shrink (`delta_mm < 0`) the surface of the scoped bodies by a signed
    /// amount. `targets` is one or more comma-separated body queries
    /// (`qCreatedBy(<feature_id>)`); empty targets leave the feature waiting for
    /// a scope pick in the dialog. The offset expands to an exact tree at build
    /// time (docs/OFFSET-TREE-PLAN.md §8).
    #[wasm_bindgen(js_name = addOffset)]
    pub fn add_offset(&mut self, targets: String, delta_mm: f32) -> Result<u32, JsError> {
        self.push_feature_call("offset", serde_json::json!({
            "targets": targets,
            "delta": { "value": delta_mm, "unit": "mm" },
        }))
    }

    /// Extrude one or more coplanar B-Rep faces (P1). `faces` is one or more
    /// comma-separated planar face queries (e.g. `qFace(<id_hash>)`); their
    /// boundary loops are projected into their shared plane and swept by
    /// `depth_mm`. `symmetric` grows `±depth/2` about the plane; `false` is a
    /// blind / two-sided extrude `[−depth_back, +depth]` along the face normal
    /// (the default the UI sends, matching Onshape). Adds material (Union).
    /// `suppressed` creates the call as a pending ghost (ticket x7af): the
    /// body keeps its pre-extrude shape until the UI confirms the feature.
    #[wasm_bindgen(js_name = addExtrudeFaces)]
    pub fn add_extrude_faces(
        &mut self,
        faces: String,
        depth_mm: f32,
        symmetric: bool,
        depth_back_mm: f32,
        suppressed: bool,
    ) -> Result<u32, JsError> {
        // Ticket k280: faces ride on the unified `profiles` field now.
        self.push_feature_call_with("extrude", serde_json::json!({
            "profiles": faces,
            "depth": { "value": depth_mm, "unit": "mm" },
            "symmetric": symmetric,
            "depth_back": { "value": depth_back_mm, "unit": "mm" },
        }), suppressed)
    }

    /// Thicken one or more **curved** analytic faces into a thin solid wall by
    /// offsetting their surfaces along the normal (sphere / cylinder / torus).
    /// `faces` is a comma-separated list of `qFace(<id_hash>)` selections;
    /// `thickness_mm` grows the wall outward, `thickness_back_mm` inward (only
    /// when `symmetric` is false), `symmetric` splits the wall about the surface.
    /// `up_to_face` is an optional `qFace(<id_hash>)` target: when non-empty the
    /// wall grows up to that coaxial face and the thickness params are ignored.
    #[wasm_bindgen(js_name = addThicken)]
    pub fn add_thicken(
        &mut self,
        faces: String,
        thickness_mm: f32,
        symmetric: bool,
        thickness_back_mm: f32,
        up_to_face: String,
    ) -> Result<u32, JsError> {
        self.push_feature_call("thicken", serde_json::json!({
            "faces": faces,
            "thickness": { "value": thickness_mm, "unit": "mm" },
            "symmetric": symmetric,
            "thickness_back": { "value": thickness_back_mm, "unit": "mm" },
            "up_to_face": up_to_face,
        }))
    }


    /// K13.1: import triangle data as a solid CSG leaf.
    /// Stores only the original positions+indices in the document (~2MB)
    /// instead of the full MeshSdf with tris/bboxes (~175MB after voxel repair).
    /// The MeshSdf is rebuilt on-demand in feature execution.
    #[wasm_bindgen(js_name = addImportedMesh)]
    pub fn add_imported_mesh(
        &mut self,
        positions: &[f32],
        indices: &[u32],
        op: u32,
    ) -> Result<u32, JsError> {
        let op_wire = Self::op_wire_name(op)?;
        let report = fielddraft_protocol::MeshSdf::from_buffers(positions, indices)
            .check_manifold();
        self.push_feature_call("imported_mesh", serde_json::json!({
            "op": op_wire,
            "__positions": positions,
            "__indices": indices,
            "__watertight": report.is_watertight,
        }))
    }

    /// vector. `source_id` must point at an existing spine
    /// feature; `count` is total instances including the
    /// source. Returns the allocated pattern feature id.
    #[wasm_bindgen(js_name = addLinearPattern)]
    pub fn add_linear_pattern(
        &mut self,
        source_id: u32,
        dx: f32,
        dy: f32,
        dz: f32,
        count: u32,
        spacing_mm: f32,
    ) -> Result<u32, JsError> {
        let source_fid = FeatureId::from_u32(source_id);
        if !self.decoded.iter().any(|f| f.id == source_fid) {
            return Err(JsError::new(&format!(
                "source feature {source_id} not found on the spine"
            )));
        }
        self.push_feature_call("linear_pattern", serde_json::json!({
            "source": format!("qCreatedBy({source_id})"),
            "direction": [dx, dy, dz],
            "count": count as f32,
            "spacing": { "value": spacing_mm, "unit": "mm" },
        }))
    }

    /// K8.1: append a `circular_pattern` feature call that
    /// replicates the source around an axis-through-center
    /// pivot. `angle_deg` is the full sweep (360° for a
    /// closed ring). Returns the new pattern feature id.
    #[wasm_bindgen(js_name = addCircularPattern)]
    #[allow(clippy::too_many_arguments)] // wasm-bindgen geometry API; flat scalar params
    pub fn add_circular_pattern(
        &mut self,
        source_id: u32,
        axis_x: f32,
        axis_y: f32,
        axis_z: f32,
        center_x: f32,
        center_y: f32,
        center_z: f32,
        count: u32,
        angle_deg: f32,
    ) -> Result<u32, JsError> {
        let source_fid = FeatureId::from_u32(source_id);
        if !self.decoded.iter().any(|f| f.id == source_fid) {
            return Err(JsError::new(&format!(
                "source feature {source_id} not found on the spine"
            )));
        }
        self.push_feature_call("circular_pattern", serde_json::json!({
            "source": format!("qCreatedBy({source_id})"),
            "axis": [axis_x, axis_y, axis_z],
            "center": [center_x, center_y, center_z],
            "count": count as f32,
            "angle_deg": angle_deg,
        }))
    }

    /// K8.2: append a `mirror` feature call that reflects the
    /// source feature through a plane. `plane` is a face/datum
    /// query (e.g. `qFace(<hash>)` or `origin_plane_xy`) whose
    /// world-space frame supplies the mirror plane. Returns the
    /// new mirror feature id.
    #[wasm_bindgen(js_name = addMirror)]
    pub fn add_mirror(
        &mut self,
        source_id: u32,
        plane: &str,
    ) -> Result<u32, JsError> {
        let source_fid = FeatureId::from_u32(source_id);
        if !self.decoded.iter().any(|f| f.id == source_fid) {
            return Err(JsError::new(&format!(
                "source feature {source_id} not found on the spine"
            )));
        }
        self.push_feature_call("mirror", serde_json::json!({
            "source": format!("qCreatedBy({source_id})"),
            "plane": plane,
        }))
    }

    /// K7.5: return the current `params` JSON string for a
    /// feature call by id. Used by the Outliner's
    /// FeatureDialog to initialise its edit buffer with the
    /// feature's real values rather than a schema-default
    /// blob.
    #[wasm_bindgen(js_name = getFeatureCallParams)]
    pub fn get_feature_call_params(&self, id: u32) -> Result<String, JsError> {
        let target = FeatureId::from_u32(id);
        let call = self
            .document
            .feature_calls
            .iter()
            .find(|c| c.id == target)
            .ok_or_else(|| JsError::new(&format!("feature {id} not found")))?;
        serde_json::to_string(&call.params).map_err(|e| JsError::new(&e.to_string()))
    }

    /// K7.5: replace the `params` JSON of a feature call in
    /// place and regenerate the spine. Keeps the feature id
    /// stable so the outliner row doesn't jump, and re-runs
    /// the registry so downstream cached state (body meshes,
    /// packed buffer) reflects the new values.
    ///
    /// `coalesce` skips the undo snapshot: a throttled drag
    /// (live extrude) replaces the same call dozens of times
    /// per second — the caller snapshots once via the FIRST,
    /// non-coalesced replace and the whole drag stays a single
    /// undo step (mirrors `apply_gizmo_feature`'s merge). On
    /// regeneration failure a coalesced replace restores the
    /// previous params in place instead of popping undo.
    #[wasm_bindgen(js_name = replaceFeatureCallParams)]
    pub fn replace_feature_call_params(
        &mut self,
        id: u32,
        params_json: &str,
        coalesce: bool,
    ) -> Result<(), JsError> {
        let target = FeatureId::from_u32(id);
        let new_params: serde_json::Value =
            serde_json::from_str(params_json).map_err(|e| {
                JsError::new(&format!("invalid params JSON: {e}"))
            })?;
        let idx = self
            .document
            .feature_calls
            .iter()
            .position(|c| c.id == target)
            .ok_or_else(|| JsError::new(&format!("feature {id} not found")))?;

        if !coalesce {
            self.snapshot();
        }
        // K8.7: preserve the call's suppressed flag across a
        // params replace — otherwise toggling suppression off by
        // renaming or editing any other field would resurrect it.
        let old = &self.document.feature_calls[idx];
        let type_name = old.type_name.clone();
        let suppressed = old.suppressed;
        let old_params = old.params.clone();
        self.document.feature_calls[idx] = fielddraft_protocol::FeatureCall {
            type_name,
            id: target,
            params: new_params,
            suppressed,
        };
        if let Err(err) = self.bootstrap_features_from_calls() {
            if coalesce {
                self.document.feature_calls[idx].params = old_params;
                let _ = self.bootstrap_features_from_calls();
                self.rebuild_packed();
            } else {
                self.undo();
            }
            return Err(JsError::new(&format!(
                "feature regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(())
    }

    /// K8.8: duplicate a feature call. Allocates a fresh id,
    /// clones the call's wire-form params, nudges any
    /// `translate` vector by `(+20, 0, 0)` so the clone doesn't
    /// sit exactly on top of the source, and pushes the new
    /// call to `feature_calls`. Returns the new id. Features
    /// without a translate field (patterns, mirror, sketch)
    /// are cloned verbatim — their geometry is derived from a
    /// referenced source, so the visual offset happens on the
    /// source side instead.
    #[wasm_bindgen(js_name = duplicateFeature)]
    pub fn duplicate_feature(&mut self, id: u32) -> Result<u32, JsError> {
        let target = FeatureId::from_u32(id);
        let idx = self
            .document
            .feature_calls
            .iter()
            .position(|c| c.id == target)
            .ok_or_else(|| JsError::new(&format!("feature {id} not found")))?;

        self.snapshot();
        let source = self.document.feature_calls[idx].clone();
        let new_id = self.document.allocate_id();
        let new_flat = new_id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");

        // Nudge translate along +X so the clone is visible.
        let mut new_params = source.params.clone();
        if let Some(obj) = new_params.as_object_mut() {
            if let Some(t) = obj.get_mut("translate").and_then(|v| v.as_array_mut()) {
                if t.len() == 3 {
                    if let Some(x) = t[0].as_f64() {
                        t[0] = serde_json::json!(x + 20.0);
                    }
                }
            }
        }

        let new_call = fielddraft_protocol::FeatureCall {
            type_name: source.type_name.clone(),
            id: new_id,
            params: new_params,
            suppressed: source.suppressed,
        };
        self.document.feature_calls.push(new_call);
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "duplicate regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(new_flat)
    }


    /// K8.7: toggle a feature call's suppressed flag and
    /// regenerate. A suppressed call is skipped by the
    /// registry's regen loop, so its geometry drops out of the
    /// CSG tree, but the call stays on `feature_calls` so the
    /// user can unsuppress it later. Fails if the id doesn't
    /// match any call or if regeneration errors out (e.g.
    /// unsuppressing a call whose upstream dependency is now
    /// broken).
    #[wasm_bindgen(js_name = setFeatureSuppressed)]
    pub fn set_feature_suppressed(
        &mut self,
        id: u32,
        suppressed: bool,
    ) -> Result<(), JsError> {
        let target = FeatureId::from_u32(id);
        let idx = self
            .document
            .feature_calls
            .iter()
            .position(|c| c.id == target)
            .ok_or_else(|| JsError::new(&format!("feature {id} not found")))?;
        if self.document.feature_calls[idx].suppressed == suppressed {
            return Ok(());
        }

        self.snapshot();
        self.document.feature_calls[idx].suppressed = suppressed;
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "suppress toggle failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(())
    }


    // ── mutations ─────────────────────────────────────────────────────

    /// Append a primitive feature.
    ///
    /// The `tx`/`ty`/`tz` arguments populate the new feature's transform
    /// (`Transform.translate`); the primitive itself is created at its
    /// own origin so that future transform edits remain a single source
    /// of truth.
    ///
    /// `params` is interpreted per kind:
    ///   - box      → (half_x, half_y, half_z)
    ///   - sphere   → (radius, _, _)
    ///   - cylinder → (radius, half_height, _)
    ///   - thread   → (nominal_radius, pitch, depth)  half_height defaults to 5
    #[wasm_bindgen(js_name = addPrimitive)]
    #[allow(clippy::too_many_arguments)] // wasm-bindgen geometry API; flat scalar params
    pub fn add_primitive(
        &mut self,
        kind: u32,
        op: u32,
        tx: f32,
        ty: f32,
        tz: f32,
        p0: f32,
        p1: f32,
        p2: f32,
    ) -> Result<u32, JsError> {
        // DL-61 C2b: addPrimitive is call-first — it builds the
        // primitive's `FeatureCall` directly (the same wire shape the
        // matching `FeatureKind::execute` parses) and regenerates the
        // spine via `push_feature_call`. The per-kind `params` keys
        // mirror each primitive `Def`'s field names exactly.
        let op_wire = Self::op_wire_name(op)?;
        let (type_name, mut params) = match kind {
            Primitive::KIND_BOX => (
                "box",
                serde_json::json!({ "half_extents": [p0, p1, p2] }),
            ),
            Primitive::KIND_SPHERE => ("sphere", serde_json::json!({ "radius": p0 })),
            Primitive::KIND_CYLINDER => (
                "cylinder",
                serde_json::json!({ "radius": p0, "half_height": p1 }),
            ),
            Primitive::KIND_TORUS => (
                "torus",
                serde_json::json!({ "major_radius": p0, "minor_radius": p1 }),
            ),
            Primitive::KIND_THREAD => (
                "thread",
                serde_json::json!({
                    "nominal_radius": p0, "pitch": p1, "depth": p2, "half_height": 5.0,
                }),
            ),
            Primitive::KIND_CONE => (
                "cone",
                serde_json::json!({ "r_bottom": p0, "r_top": p1, "half_height": p2 }),
            ),
            Primitive::KIND_CAPSULE => (
                "capsule",
                serde_json::json!({ "radius": p0, "half_height": p1 }),
            ),
            Primitive::KIND_HEX_PRISM => (
                "hex_prism",
                serde_json::json!({ "apothem": p0, "half_height": p1 }),
            ),
            Primitive::KIND_ELLIPSOID => {
                ("ellipsoid", serde_json::json!({ "radii": [p0, p1, p2] }))
            }
            other => return Err(JsError::new(&format!("invalid primitive kind: {other}"))),
        };
        if let Some(obj) = params.as_object_mut() {
            obj.insert("translate".into(), serde_json::json!([tx, ty, tz]));
            obj.insert("op".into(), serde_json::json!(op_wire));
        }
        self.push_feature_call(type_name, params)
    }

    /// Insert a feature instance of an arbitrary registered type
    /// (built-in OR a document FieldSkript whose id == `type_name`),
    /// with `params_json` as its parameter blob. Returns the new flat
    /// feature id. This is the generic counterpart to the type-specific
    /// `addBox`/`addPrimitive`/… inserters — it's what lets a saved
    /// FieldSkript be instantiated as a feature.
    #[wasm_bindgen(js_name = addFeatureCall)]
    pub fn add_feature_call(&mut self, type_name: &str, params_json: &str) -> Result<u32, JsError> {
        let params: serde_json::Value =
            serde_json::from_str(params_json).map_err(|e| JsError::new(&e.to_string()))?;
        self.push_feature_call(type_name, params)
    }


    /// Remove the feature with the given id. Returns true if anything
    /// was removed.
    #[wasm_bindgen(js_name = removeFeature)]
    pub fn remove_feature(&mut self, id: u32) -> bool {
        let target_id = FeatureId::from_u32(id);

        // DL-61 C2b: `feature_calls` is authoritative, so a delete is
        // just "drop the call(s) and regenerate". The spine (including
        // any descendant clones a Pattern/Mirror/Hole execute path
        // would have produced) falls out of the regeneration.
        if !self.document.feature_calls.iter().any(|c| c.id == target_id) {
            return false;
        }

        self.snapshot();

        // Cascade: a deleted feature takes its dependents with it. Gizmo
        // transform calls (translate/rotate/scale) reference their
        // subject through a `target_id` param; left behind, an orphaned
        // transform poisons *every* later regeneration with "target
        // feature N not found" (it errored here too, but the delete
        // tolerated it — so the orphan lingered and broke the next
        // addPrimitive). Collect the dependency closure to a fixpoint so
        // transform-of-transform chains go too, then drop them all.
        let mut removed: std::collections::HashSet<FeatureId> =
            std::iter::once(target_id).collect();
        loop {
            let mut grew = false;
            for c in &self.document.feature_calls {
                if removed.contains(&c.id) {
                    continue;
                }
                let depends_on_removed = c
                    .params
                    .get("target_id")
                    .and_then(|v| v.as_u64())
                    .map(|t| removed.contains(&FeatureId::from_u32(t as u32)))
                    .unwrap_or(false);
                if depends_on_removed {
                    removed.insert(c.id.clone());
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        self.document
            .feature_calls
            .retain(|c| !removed.contains(&c.id));

        let _ = self.bootstrap_features_from_calls();
        self.rebuild_packed();
        true
    }

    /// Change the boolean op of an existing feature.
    #[wasm_bindgen(js_name = setFeatureOp)]
    pub fn set_feature_op(&mut self, id: u32, op: u32) -> Result<bool, JsError> {
        let wire = Self::op_wire_name(op)?;
        let id = FeatureId::from_u32(id);
        Ok(self.edit_feature_call_params(id, |p| {
            if p.get("op").and_then(|v| v.as_str()) == Some(wire) {
                return false;
            }
            p.insert("op".into(), serde_json::json!(wire));
            true
        }))
    }

    /// Move a feature to a new index. Indices clamp into bounds.
    #[wasm_bindgen(js_name = reorderFeature)]
    pub fn reorder_feature(&mut self, id: u32, target_index: usize) -> bool {
        let id = FeatureId::from_u32(id);
        let from = self.document.feature_calls.iter().position(|c| c.id == id);
        let Some(from) = from else { return false };
        let to = target_index.min(self.document.feature_calls.len().saturating_sub(1));
        if from == to {
            return false;
        }
        // Snapshot semantics: the feature tree is a program — every feature
        // may only reference features BEFORE it. Reject a reorder that would
        // push a feature ahead of a feature it references (or pull a
        // referenced feature behind its consumer); the UI keeps the original
        // order and surfaces the rejection.
        let mut proposed: Vec<_> = self.document.feature_calls.iter().collect();
        let moved = proposed.remove(from);
        proposed.insert(to, moved);
        let pos: std::collections::HashMap<FeatureId, usize> =
            proposed.iter().enumerate().map(|(i, c)| (c.id.clone(), i)).collect();
        for (i, c) in proposed.iter().enumerate() {
            for r in created_by_references(&c.params) {
                if let Some(&ri) = pos.get(&r) {
                    if ri >= i {
                        return false; // forward reference — illegal order
                    }
                }
            }
        }
        self.snapshot();
        let call = self.document.feature_calls.remove(from);
        self.document.feature_calls.insert(to, call);
        // Reordering changes Boolean/regeneration order, so re-derive
        // the spine from the new call order.
        let _ = self.bootstrap_features_from_calls();
        self.rebuild_packed();
        true
    }

}
