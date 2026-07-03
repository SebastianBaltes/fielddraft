//! Der [`Kernel`]: kanonisches Dokument, Undo/Redo-Stacks und die
//! interne Regenerations-/Meshing-Pipeline (rebuild_mesh_internal,
//! bootstrap_features_from_calls, Body-Partition, SDF-Kompilierung).
//! Die wasm-bindgen-API liegt in [`crate::api`]-Modulen.

use fielddraft_protocol::{BoolOp, Document, FeatureId, Primitive};
use wasm_bindgen::prelude::*;

use crate::body::{Body, build_bounded_for_body, build_bounded_with_primitives, detect_uniform_translation, hash_body_payload, hash_body_payload_split, partition};
use crate::body;
use crate::csg::{CsgNode, DecodedFeature};
use crate::csg;
use crate::dc::Mesh;
use crate::dc;
use crate::features;
use crate::mt;
use crate::rfta;
use crate::script_runtime;
use crate::sdf_compiler;
/// Maximum undo / redo depth. The whole document is snapshotted per
/// edit, so depth × document-size ≈ memory cost. For M2 (small linear
/// feature lists) 64 is plenty without being painful.
const HISTORY_DEPTH: usize = 64;

/// Owned by the JS side. Holds the canonical document and the
/// undo/redo stacks.
#[wasm_bindgen]
pub struct Kernel {
    pub(crate) document: Document,
    /// DL-61: the decoded runtime IR, rebuilt from `document.features`
    /// (1:1, same order) on every regeneration / remesh. The geometry
    /// pipeline — partition, hashing, BVH/B-Rep weaving, STEP classify —
    /// consumes this open form rather than the fat `Feature` spine.
    pub(crate) decoded: Vec<DecodedFeature>,
    pub(crate) undo: Vec<Document>,
    pub(crate) redo: Vec<Document>,
    /// Multi-body mesh cache (K3 part 2). Each body has its own
    /// feature subset, BVH and mesh buffer. Mutations that only
    /// touch body `A` leave body `B`'s mesh — and its per-body
    /// `version` counter — untouched.
    pub(crate) bodies: Vec<Body>,
    /// Voxel grid resolution for the currently-edited body (the
    /// active body). Defaults to [`DEFAULT_MESH_RESOLUTION`]. The
    /// legacy `mesh_resolution()` / `setMeshResolution()` API maps
    /// onto this field so existing tests keep working.
    pub(crate) active_mesh_resolution: u32,
    /// Voxel grid resolution for bodies that aren't currently being
    /// edited. Defaults to [`DEFAULT_CONTEXT_MESH_RESOLUTION`] —
    /// half the active resolution, ≈ 8× fewer cells = a meaningful
    /// speedup on every re-mesh that touches a context body.
    pub(crate) context_mesh_resolution: u32,
    /// Id of the body the user is currently editing, if any. `None`
    /// disables LOD entirely: every body is treated as active and
    /// uses `active_mesh_resolution`. When a body id is set, only
    /// that body stays at active resolution; every other body
    /// drops to the context resolution.
    pub(crate) active_body_id: Option<FeatureId>,
    /// When `false`, `rebuild_mesh_internal` still partitions the
    /// feature spine into bodies but skips the Dual Contouring
    /// pass, leaving every body with an empty mesh buffer. The
    /// frontend flips this off at startup so the main-thread
    /// kernel only maintains document state (and the CSG tree
    /// used by `pick`), while a dedicated Web Worker owns a
    /// second kernel instance that actually does the meshing and
    /// streams the result back through the store.
    ///
    /// Default `true` so every Rust test keeps producing real
    /// meshes without needing to opt in.
    pub(crate) sync_meshing: bool,
    /// Mesh algorithm selection: 0 = MDC (default), 1 = RFTA.
    pub(crate) mesh_algorithm: u8,
    /// Milestone-1 parametric weaving: when on, pure grundforms get an exact
    /// analytic B-Rep (CSG branch-pair seeds) instead of an octree-sampled one.
    /// Default `true` — the seed weave resolves sub-cell features (e.g. grazing
    /// near-tangent intersections) the octree cannot, and any body it cannot
    /// resolve analytically falls back to the octree weave per-body, so the
    /// result is never worse than the octree route. Toggle via `setParametricWeave`.
    pub(crate) parametric_weave: bool,
    /// While `true` (web: gizmo drag with "B-Rep rebuild while dragging"
    /// off, and permanently in the mesher worker), the kernel never weaves:
    /// the regen face-tagging pass is skipped and the per-body B-Rep exports
    /// serve only a still-current cache — a body whose geometry changed
    /// reports empty payloads. `setBrepSuppressed(false)` catches up with
    /// ONE weave per stale body and bumps their versions so the frontend
    /// refetches. Runtime-only, never serialised.
    pub(crate) brep_suppressed: bool,
    /// Global version counter bumped on every mesh rebuild (any body
    /// touched). Frontend uses this to detect "anything changed";
    /// per-body freshness is tracked by each `Body`'s own `version`.
    pub(crate) mesh_version: u32,
    /// K10.4: edge fillets collected from FilletEdgeFeature calls
    /// during the last regenerate pass. Applied as a post-processing
    /// step on the CsgNode tree before meshing.
    pub(crate) edge_fillets: Vec<csg::EdgeFillet>,
    /// C2: resolved CSYS of every `mate_connector` from the last
    /// regenerate pass, so the viewport can render datum triads without
    /// re-resolving the face query on the JS side.
    pub(crate) mate_connectors: Vec<features::ResolvedMateConnector>,
    /// Per-feature regen errors from the last `bootstrap_features_from_calls`
    /// (ticket sx5a, Onshape semantics): a failing feature stays in the
    /// document, contributes no geometry, and carries its message here —
    /// `listFeatures` exports it so the UI can paint the row red.
    pub(crate) feature_errors: std::collections::HashMap<FeatureId, String>,
    /// Per-feature NON-fatal merge-scope warnings from the last regen: a
    /// feature-implicit boolean (Add/Remove/Intersect) with no resolved merge
    /// scope. Geometry still regenerates (the body shows as a red translucent
    /// preview); `listFeatures` exports the message so the UI paints the row
    /// in the warning colour with this tooltip.
    pub(crate) feature_warnings: std::collections::HashMap<FeatureId, String>,
    /// Rollback bar position: if set, features at index >= this value
    /// are skipped during regeneration (but stay in the document).
    pub(crate) rollback_after: Option<usize>,
    /// Regen-loop weave cache carried ACROSS regenerations. Installed into
    /// each regenerate `Context` and taken back afterwards, so a feature
    /// that resolves element queries mid-loop (shell's `qFace` open faces)
    /// only pays the octree weave when the upstream features actually
    /// changed — not on every regen/load. Keyed by content hash, so a stale
    /// hit is impossible; without it, loading a weave-heavy document
    /// (spline-revolve + shell) re-wove from scratch every time.
    pub(crate) regen_brep_cache: body::BrepCache,
}

impl Kernel {
    pub(crate) fn snapshot(&mut self) {
        self.undo.push(self.document.clone());
        if self.undo.len() > HISTORY_DEPTH {
            // Keep only the most recent HISTORY_DEPTH snapshots.
            let drop = self.undo.len() - HISTORY_DEPTH;
            self.undo.drain(0..drop);
        }
        self.redo.clear();
    }

    pub(crate) fn rebuild_packed(&mut self) {
        // DL-61 C2b: `feature_calls` is the single source of truth.
        // Every mutator edits a call's params (or the call list) and
        // re-derives the `features` spine via
        // `bootstrap_features_from_calls`, so there is no reverse
        // sync any more — `rebuild_packed` just re-meshes the
        // current document.
        self.rebuild_mesh_internal();
    }

    /// Re-mesh the current document at the current resolution and
    /// bump `mesh_version`. Called by `rebuild_packed` (after every
    /// mutation) and by the explicit `setMeshResolution` API.
    ///
    /// K3 part 2: the pipeline is `features → partition → bodies`,
    /// and each body only gets re-meshed when its payload hash
    /// actually changed. Bodies whose seed feature disappeared are
    /// dropped; bodies whose hash matches an existing one keep their
    /// cached mesh verbatim.
    ///
    /// K3 part 4 adds per-body LOD on top: each body picks between
    /// `active_mesh_resolution` and `context_mesh_resolution` based
    /// on whether its id matches `active_body_id`. When no active
    /// body is set, every body runs at active resolution (backward
    /// compatible with the K3 part 1–3 behaviour). The cache reuse
    /// check compares both the payload hash *and* the resolution
    /// the cached mesh was built at, so switching the active body
    /// correctly re-meshes the two bodies that changed LOD and
    /// leaves the rest untouched.
    pub(crate) fn rebuild_mesh_internal(&mut self) {
        // DL-61 C2b: `self.decoded` is produced solely by
        // `bootstrap_features_from_calls`; every path that changes
        // `feature_calls` (mutators, undo/redo, load) re-bootstraps
        // before re-meshing, so the IR is already current here.
        let partitions = partition(&self.decoded);

        let mut previous: std::collections::HashMap<FeatureId, Body> =
            std::mem::take(&mut self.bodies)
                .into_iter()
                .map(|b| (b.id.clone(), b))
                .collect();

        let mut new_bodies: Vec<Body> = Vec::with_capacity(partitions.len());
        for part in partitions {
            let id = part.seed_id(&self.decoded);
            let (geometry_hash, transform_hash) =
                hash_body_payload_split(&self.decoded, &part.feature_indices);
            let payload_hash = hash_body_payload(&self.decoded, &part.feature_indices);
            let target_resolution = self.target_resolution_for(&id);

            if let Some(mut prev) = previous.remove(&id) {
                if prev.payload_hash == payload_hash
                    && prev.mesh_resolution == target_resolution
                {
                    prev.feature_indices = part.feature_indices;
                    prev.aabb = part.aabb;
                    new_bodies.push(prev);
                    continue;
                }

                // K3.6: affine fast-path — geometry unchanged, only
                // transforms differ, and the change is a uniform
                // translation across all features in the body.
                if prev.geometry_hash == geometry_hash
                    && prev.mesh_resolution == target_resolution
                {
                    let new_transforms: Vec<fielddraft_protocol::Transform> = part
                        .feature_indices
                        .iter()
                        .map(|&i| self.decoded[i].transform)
                        .collect();
                    if let Some(delta) = detect_uniform_translation(
                        &prev.last_mesh_transforms,
                        &new_transforms,
                    ) {
                        let cumulative = match prev.transform_delta {
                            Some([px, py, pz]) => [px + delta[0], py + delta[1], pz + delta[2]],
                            None => delta,
                        };
                        prev.feature_indices = part.feature_indices;
                        prev.aabb = part.aabb;
                        prev.payload_hash = payload_hash;
                        prev.geometry_hash = geometry_hash;
                        prev.transform_hash = transform_hash;
                        prev.transform_delta = Some(cumulative);
                        prev.last_mesh_transforms = new_transforms;
                        prev.compiled_sdf = Some(self.compile_sdf_for(&prev.feature_indices));
                        prev.version = prev.version.wrapping_add(1);
                        new_bodies.push(prev);
                        continue;
                    }
                }

                let mesh = self.build_mesh_for(&part.feature_indices, target_resolution);
                let transforms: Vec<fielddraft_protocol::Transform> = part
                    .feature_indices
                    .iter()
                    .map(|&i| self.decoded[i].transform)
                    .collect();
                // Keep the woven B-Rep across geometry-preserving remeshes
                // (LOD switch, transform change): `ensure_brep` revalidates
                // against its own placement-invariant content key anyway,
                // and re-weaving a weave-heavy body on every active-body
                // switch froze the UI for the whole octree pass.
                if prev.geometry_hash != geometry_hash {
                    prev.brep = None;
                }
                prev.feature_indices = part.feature_indices;
                prev.aabb = part.aabb;
                prev.payload_hash = payload_hash;
                prev.geometry_hash = geometry_hash;
                prev.transform_hash = transform_hash;
                prev.mesh_resolution = target_resolution;
                prev.mesh = mesh;
                prev.compiled_sdf = Some(self.compile_sdf_for(&prev.feature_indices));
                prev.version = prev.version.wrapping_add(1);
                prev.transform_delta = None;
                prev.last_mesh_transforms = transforms;
                new_bodies.push(prev);
            } else {
                let mesh = self.build_mesh_for(&part.feature_indices, target_resolution);
                let transforms: Vec<fielddraft_protocol::Transform> = part
                    .feature_indices
                    .iter()
                    .map(|&i| self.decoded[i].transform)
                    .collect();
                let compiled = self.compile_sdf_for(&part.feature_indices);
                new_bodies.push(Body {
                    id,
                    feature_indices: part.feature_indices,
                    aabb: part.aabb,
                    payload_hash,
                    geometry_hash,
                    transform_hash,
                    mesh_resolution: target_resolution,
                    mesh,
                    version: 1,
                    transform_delta: None,
                    last_mesh_transforms: transforms,
                    compiled_sdf: Some(compiled),
                    // Recomputed lazily on the first `ensure_brep`; the
                    // weave needs it, the mesh path does not.
                    placement: fielddraft_protocol::Transform::IDENTITY,
                    brep: None,
                });
            }
        }

        self.bodies = new_bodies;

        // Brocken A (A3): face-identity tagging. Skipped while B-Rep output
        // is suppressed (gizmo drag) — `setBrepSuppressed(false)` runs the
        // catch-up pass once at drag end.
        if !self.brep_suppressed {
            self.retag_mesh_faces();
        }

        self.mesh_version = self.mesh_version.wrapping_add(1);
    }

    /// Brocken A (A3): tag every freshly-built mesh with per-vertex B-Rep
    /// FACE IDENTITY so a hovered/clicked triangle resolves to a stable
    /// face. Bodies carried through the affine fast-path keep their old
    /// (already-tagged) mesh untouched — their `face_index` is still valid
    /// because correlation is body-LOCAL and a pure move doesn't change
    /// which face a vertex sits on. We therefore only (re)tag meshes whose
    /// `face_index` is missing/out-of-sync, weaving the B-Rep once and
    /// sharing the cache with the later face/vertex exports.
    pub(crate) fn retag_mesh_faces(&mut self) {
        let parametric = self.parametric_weave;
        for i in 0..self.bodies.len() {
            let tc = self.bodies[i].mesh.triangle_count();
            if tc == 0 || self.bodies[i].mesh.face_index.len() == tc {
                continue;
            }
            self.bodies[i].ensure_brep(&self.decoded, parametric);
            let placement = self.bodies[i].placement;
            let brep = self.bodies[i].brep_cached().expect("woven above");
            let tags = crate::brep::face_export::correlate_mesh_faces(
                brep,
                &self.bodies[i].mesh.positions,
                &self.bodies[i].mesh.normals,
                &self.bodies[i].mesh.indices,
                &self.bodies[i].mesh.contributors,
                &placement,
            );
            self.bodies[i].mesh.face_index = tags;
        }
    }

    /// Build a mesh for a body's feature subset at the requested
    /// resolution — or short-circuit to an empty `Mesh` when
    /// `sync_meshing` is off, so the main-thread kernel stops
    /// paying the Dual Contouring cost while the web worker
    /// handles it out-of-process.
    pub(crate) fn build_mesh_for(&self, feature_indices: &[usize], resolution: u32) -> Mesh {
        if !self.sync_meshing {
            return Mesh::default();
        }

        // Passthrough fast-path: a body consisting of a single ImportedMesh
        // feature is rendered directly from its triangles — no DC needed.
        if feature_indices.len() == 1 {
            let f = &self.decoded[feature_indices[0]];
            if let CsgNode::Leaf {
                primitive: Primitive::ImportedMesh { mesh },
                ..
            } = &f.node
            {
                let cid = f.id.as_u32_or_root().unwrap_or(0);
                return Self::mesh_from_imported(mesh, cid);
            }
        }

        let (bounded, prims) = build_bounded_with_primitives(&self.decoded, feature_indices);
        let prim_refs: std::collections::HashMap<u32, &Primitive> =
            prims.iter().map(|(k, v)| (*k, v)).collect();
        match self.mesh_algorithm {
            1 => rfta::reach_for_the_arcs(&bounded, resolution),
            2 => mt::marching_tetrahedra(&bounded, resolution),
            _ => dc::dual_contour_with_primitives(&bounded, resolution, &prim_refs),
        }
    }

    /// Convert an ImportedMesh's triangle data directly into a renderable
    /// Mesh without running Dual Contouring.
    pub(crate) fn mesh_from_imported(mesh: &fielddraft_protocol::MeshSdf, contributor_id: u32) -> Mesh {
        let (positions, normals, indices) = mesh.to_display_mesh();
        let vert_count = positions.len() / 3;
        let classifications = vec![1u8; vert_count];
        let mut contributors = vec![dc::CONTRIBUTOR_SENTINEL; vert_count * csg::MAX_CONTRIBUTORS];
        for v in 0..vert_count {
            contributors[v * csg::MAX_CONTRIBUTORS] = contributor_id;
        }
        Mesh { positions, normals, indices, classifications, contributors, face_index: Vec::new() }
    }

    pub(crate) fn compile_sdf_for(&self, feature_indices: &[usize]) -> sdf_compiler::CompiledSdf {
        let bounded = build_bounded_for_body(&self.decoded, feature_indices);
        sdf_compiler::compile_body(&bounded)
    }

    /// Pick the voxel grid resolution a body should use right now:
    /// the active resolution when no LOD hint is set or when the
    /// body id matches `active_body_id`, the context resolution
    /// otherwise.
    pub(crate) fn target_resolution_for(&self, body_id: &FeatureId) -> u32 {
        match &self.active_body_id {
            Some(active_id) if active_id != body_id => self.context_mesh_resolution,
            _ => self.active_mesh_resolution,
        }
    }

    /// Internal active-body setter that works directly on
    /// `FeatureId` values (no wasm-bindgen u32 shim). All three
    /// public entry points — [`Kernel::set_active_body_id`],
    /// [`Kernel::set_active_body_by_feature`] and
    /// [`Kernel::clear_active_body_id`] — go through here so the
    /// rebuild / no-op rules live in exactly one place.
    pub(crate) fn set_active_body_id_internal(&mut self, new_active: Option<FeatureId>) -> bool {
        if new_active == self.active_body_id {
            return false;
        }
        self.active_body_id = new_active;
        self.rebuild_mesh_internal();
        true
    }

    /// Walk the current bodies and return the stable id of the one
    /// that owns `feature_id`, if any. O(bodies × features_per_body).
    pub(crate) fn body_containing_feature(&self, feature_id: &FeatureId) -> Option<FeatureId> {
        // Translate the FeatureId to an index into the decoded
        // feature list first, then search the bodies for that index.
        let idx = self
            .decoded
            .iter()
            .position(|f| &f.id == feature_id)?;
        self.bodies
            .iter()
            .find(|b| b.feature_indices.contains(&idx))
            .map(|b| b.id.clone())
    }

    pub(crate) fn op_label(op: BoolOp) -> &'static str {
        match op {
            BoolOp::Add => "add",
            BoolOp::New => "new",
            BoolOp::Remove => "remove",
            BoolOp::Intersect => "intersect",
        }
    }


    pub(crate) fn static_kind_name(type_name: &str) -> &'static str {
        match type_name {
            "sketch" => "sketch",
            "box" => "box",
            "sphere" => "sphere",
            "cylinder" => "cylinder",
            "extrude" => "extrude",
            "revolve" => "revolve",
            "helix" => "helix",
            "hole" => "hole",
            "gyroid" => "gyroid",
            "imported_mesh" => "imported_mesh",
            "fillet_edge" => "fillet_edge",
            "sweep" => "sweep",
            "mate_connector" => "mate_connector",
            "linear_pattern" => "linear_pattern",
            "circular_pattern" => "circular_pattern",
            "mirror" => "mirror",
            "boolean" => "boolean",
            "shell" => "shell",
            "thicken" => "thicken",
            "torus" => "torus",
            "translate" => "translate",
            "rotate" => "rotate",
            "scale" => "scale",
            "material" => "material",
            _ => "unknown",
        }
    }

    /// Outliner kind for a feature call. Built-ins map to their static
    /// label; a script instance carries its script id as `type_name`
    /// (not a built-in), so we pass that id straight through when the
    /// document still defines that script — the outliner resolves the
    /// name + icon from `state.scriptTypes` keyed by exactly this id.
    /// A call whose script was deleted falls back to "unknown".
    pub(crate) fn kind_label(&self, type_name: &str) -> String {
        let builtin = Self::static_kind_name(type_name);
        if builtin != "unknown" {
            return builtin.to_string();
        }
        if self.document.scripts.iter().any(|s| s.id == type_name) {
            return type_name.to_string();
        }
        "unknown".to_string()
    }

    /// K6b.3a: synthesise a display name for a feature call that
    /// doesn't match a spine entry. `"Sketch 3"` for a sketch
    /// call with id 3, etc. Primitive calls never land here
    /// because they always have a spine entry.
    pub(crate) fn synth_name_for_call(
        call: &fielddraft_protocol::FeatureCall,
    ) -> String {
        // K8.5: honour a user-set `name` in the call's params
        // so rename through `replaceFeatureCallParams` sticks
        // for feature types that don't have a spine-level
        // `DecodedFeature.name` (sketches, patterns).
        if let Some(name) = call.params.get("name").and_then(|v| v.as_str()) {
            if !name.is_empty() {
                return name.to_string();
            }
        }
        let label = match call.type_name.as_str() {
            "sketch" => "Sketch",
            "revolve" => "Revolve",
            "helix" => "Helix",
            "hole" => "Hole",
            "gyroid" => "Gyroid",
            "imported_mesh" => "Imported mesh",
            "fillet_edge" => "Fillet edge",
            "sweep" => "Sweep",
            "mate_connector" => "Mate connector",
            "linear_pattern" => "Linear pattern",
            "circular_pattern" => "Circular pattern",
            "mirror" => "Mirror",
            "boolean" => "Boolean",
            "shell" => "Shell",
            "thicken" => "Thicken",
            "translate" => "Translate",
            "rotate" => "Rotate",
            "scale" => "Scale",
            other => other,
        };
        format!("{label} {}", call.id)
    }

    pub(crate) fn parse_op(tag: u32) -> Result<BoolOp, JsError> {
        match tag {
            BoolOp::TAG_ADD => Ok(BoolOp::Add),
            BoolOp::TAG_NEW => Ok(BoolOp::New),
            BoolOp::TAG_REMOVE => Ok(BoolOp::Remove),
            BoolOp::TAG_INTERSECT => Ok(BoolOp::Intersect),
            other => Err(JsError::new(&format!("invalid op tag: {other}"))),
        }
    }

    pub(crate) fn op_wire_name(tag: u32) -> Result<&'static str, JsError> {
        match Self::parse_op(tag)? {
            BoolOp::Add => Ok("add"),
            BoolOp::New => Ok("new"),
            BoolOp::Remove => Ok("remove"),
            BoolOp::Intersect => Ok("intersect"),
        }
    }

    /// DL-61 C2b call-first core: locate a top-level `FeatureCall`
    /// by id, hand its params object to `edit`, and — if the edit
    /// reports a real change — snapshot, re-derive the spine via
    /// `bootstrap_features_from_calls`, and re-mesh. Returns false
    /// when no call matches the id or the edit was a no-op (no
    /// snapshot is taken in either case). On a regeneration error
    /// the snapshot is rolled back so a bad edit never leaves a
    /// torn document.
    ///
    /// This replaces the old "poke `document.features` + reverse
    /// sync" flow: `feature_calls` is authoritative, so transform/op
    /// edits write into the call's params and the spine falls out of
    /// a normal regeneration.
    pub(crate) fn edit_feature_call_params(
        &mut self,
        id: FeatureId,
        edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> bool,
    ) -> bool {
        let Some(idx) = self
            .document
            .feature_calls
            .iter()
            .position(|c| c.id == id)
        else {
            return false;
        };
        let mut params = self.document.feature_calls[idx]
            .params
            .as_object()
            .cloned()
            .unwrap_or_default();
        if !edit(&mut params) {
            return false;
        }
        self.snapshot();
        self.document.feature_calls[idx].params = serde_json::Value::Object(params);
        if self.bootstrap_features_from_calls().is_err() {
            self.undo();
            return false;
        }
        self.rebuild_packed();
        true
    }

    pub(crate) fn push_feature_call(
        &mut self,
        type_name: &str,
        params: serde_json::Value,
    ) -> Result<u32, JsError> {
        self.push_feature_call_with(type_name, params, false)
    }

    /// `suppressed: true` creates the call as a PENDING ghost in one undo
    /// step (ticket x7af): the regen loop skips it, so the body keeps its
    /// pre-feature shape until the UI confirms via `setFeatureSuppressed`.
    pub(crate) fn push_feature_call_with(
        &mut self,
        type_name: &str,
        params: serde_json::Value,
        suppressed: bool,
    ) -> Result<u32, JsError> {
        self.snapshot();
        let id = self.document.allocate_id();
        let flat = id
            .as_u32_if_flat()
            .expect("K4.5.1 still issues flat-numeric feature ids");
        let mut call = fielddraft_protocol::FeatureCall::new(type_name, id, params);
        call.suppressed = suppressed;
        self.document.feature_calls.push(call);
        if let Err(err) = self.bootstrap_features_from_calls() {
            self.undo();
            return Err(JsError::new(&format!(
                "{type_name} regeneration failed: {err}"
            )));
        }
        self.rebuild_packed();
        Ok(flat)
    }
}

/// The app version this WASM binary was stamped with at build time
/// (see `build.rs`). The web app compares it against `__APP_VERSION__`
/// on boot to catch a stale kernel module. Returns "dev" when built
/// without `FIELDDRAFT_VERSION` (e.g. `cargo test`).

impl Kernel {
    /// K4.5.4d internal helper. Takes the current
    /// `document.feature_calls` list, clears `document.features`,
    /// and regenerates the spine by walking each call through a
    /// fresh `FeatureRegistry::with_builtins()`. Used by
    /// [`Kernel::load_json`] when the loaded document uses the
    /// step-4d wire shape. Fail-stop per §4.6: the first
    /// registry error halts the walk and bubbles up as a
    /// `FeatureError`.
    ///
    /// The call list is temporarily `mem::take`n out of the
    /// document so the registry's mutable borrow can point at
    /// `self.document` without aliasing the call list itself;
    /// it's restored before the function returns. The subsequent
    /// `rebuild_packed` call (from `load_json`) overwrites it
    /// again via `sync_feature_calls`, but leaving the restore
    /// in place keeps this helper independent of its caller's
    /// sequencing.
    /// Always returns `Ok(())` since the regen loop became error-tolerant
    /// (ticket sx5a): a failing feature is recorded in `feature_errors`
    /// and skipped — it contributes no geometry — instead of fail-stopping
    /// the document. The `Result` signature stays so the many defensive
    /// call sites keep compiling; their revert branches are dead defense.
    pub(crate) fn bootstrap_features_from_calls(&mut self) -> Result<(), features::FeatureError> {
        let mut registry = features::FeatureRegistry::with_builtins();
        // FieldSkript S2b: a document's scripts register as runtime
        // feature kinds keyed by their stable id, so a `FeatureCall`
        // whose `type` is a script id dispatches through the same
        // regenerate path as a built-in. Their geometry is produced by
        // the JS `build()` the worker drives via the script runner; a
        // headless run with no runner fails such a feature explicitly.
        for record in &self.document.scripts {
            registry.register(Box::new(script_runtime::ScriptFeature::new(
                record.id.clone(),
                record.schema.clone(),
            )));
        }
        let calls = std::mem::take(&mut self.document.feature_calls);
        let rollback = self.rollback_after;
        // DL-61 C2b: regeneration accumulates the decoded spine in the
        // Context (the Document no longer holds it) and we move it out as
        // `self.decoded` — the sole producer of the IR. On a fail-stop
        // error the partial spine is still captured so the caller can
        // render what regenerated before the failing call.
        let (errors, spine) = {
            let mut ctx = features::Context::new(&mut self.document);
            // Install the cross-regen weave cache so mid-loop element
            // queries (shell qFace) reuse prior weaves instead of paying
            // the octree on every regenerate; taken back below.
            ctx.brep_cache = std::mem::take(&mut self.regen_brep_cache);
            let errs = registry.regenerate(&mut ctx, &calls, rollback);
            self.regen_brep_cache = std::mem::take(&mut ctx.brep_cache);
            // K10.4: capture edge fillets collected during regen.
            self.edge_fillets = std::mem::take(&mut ctx.edge_fillets);
            // C2: capture resolved mate-connector CSYS for the viewport.
            self.mate_connectors = std::mem::take(&mut ctx.mate_connectors);
            // Merge-scope warnings collected during regen (non-fatal).
            self.feature_warnings = std::mem::take(&mut ctx.feature_warnings)
                .into_iter()
                .map(|(id, w)| (id.root_ancestor().clone(), w))
                .collect();
            (errs, std::mem::take(&mut ctx.features))
        };
        self.document.feature_calls = calls;
        self.decoded = spine;
        self.feature_errors = errors
            .0
            .into_iter()
            .map(|(id, e)| (id, e.to_string()))
            .collect();
        Ok(())
    }


    /// Drop any gizmo transform call whose `target_id` references a
    /// feature that isn't present in `feature_calls`. Runs to a fixpoint
    /// so a transform chained onto a just-pruned transform goes too.
    /// Idempotent and cheap on a healthy document (no dangling refs → no
    /// change). Used by [`Kernel::load_json`] to heal documents written
    /// before `remove_feature` learned to cascade.
    pub(crate) fn prune_dangling_transform_calls(&mut self) {
        loop {
            let present: std::collections::HashSet<FeatureId> =
                self.document.feature_calls.iter().map(|c| c.id.clone()).collect();
            let before = self.document.feature_calls.len();
            self.document.feature_calls.retain(|c| {
                match c.params.get("target_id").and_then(|v| v.as_u64()) {
                    Some(t) => present.contains(&FeatureId::from_u32(t as u32)),
                    None => true,
                }
            });
            if self.document.feature_calls.len() == before {
                break;
            }
        }
    }

}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}
