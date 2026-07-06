//! Per-Body-Mesh-Brücke (zero-copy Pointer), SDF-Opcode-Zugriff,
//! Auflösungs-/LOD-/Aktiv-Body-/Sync-Meshing-Steuerung.

use fielddraft_protocol::{FeatureId};
use wasm_bindgen::prelude::*;

use crate::Kernel;
use crate::body;
use crate::bvh;
use crate::csg;
use crate::sdf_compiler;
#[wasm_bindgen]
impl Kernel {
    // ── per-body mesh bridge (zero-copy) ──────────────────────────────
    //
    // The Dual Contouring mesher writes flat positions/normals/indices
    // into each `Body`'s own `Mesh` on every mutation. JS binds
    // `Float32Array`/`Uint32Array` views straight onto WASM linear
    // memory via the per-body pointers — callers MUST re-acquire the
    // views after every mutating call, because the underlying `Vec`s
    // can be reallocated.
    //
    // Aggregate counts (`meshVertexCount` / `meshIndexCount` /
    // `meshTriangleCount`) are provided as cheap sums over the body
    // list for callers that only need a total (tests, status bars).
    // There is no aggregate pointer buffer any more — the frontend
    // iterates `bodyCount()` and binds one `BufferGeometry` per body.

    /// Sum of vertex counts across every body. O(bodies).
    #[wasm_bindgen(js_name = meshVertexCount)]
    pub fn mesh_vertex_count(&self) -> usize {
        self.bodies.iter().map(|b| b.mesh.vertex_count()).sum()
    }

    /// Sum of index counts across every body. O(bodies).
    #[wasm_bindgen(js_name = meshIndexCount)]
    pub fn mesh_index_count(&self) -> usize {
        self.bodies.iter().map(|b| b.mesh.indices.len()).sum()
    }

    /// Sum of triangle counts across every body. O(bodies).
    #[wasm_bindgen(js_name = meshTriangleCount)]
    pub fn mesh_triangle_count(&self) -> usize {
        self.bodies.iter().map(|b| b.mesh.triangle_count()).sum()
    }


    #[wasm_bindgen(js_name = bodyCount)]
    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }

    /// Stable identifier for the i-th body (= lowest `FeatureId` in
    /// its feature set). Panics only if `i` is out of range — tests
    /// and the wasm-bindgen bridge should always gate the call on
    /// `bodyCount()`.
    ///
    /// Flat-numeric ids pass through as-is. Hierarchical ids
    /// (pattern clones like "3.1") are hashed to a unique u32 with
    /// the high bit set so they never collide with flat counters.
    #[wasm_bindgen(js_name = bodyId)]
    pub fn body_id(&self, i: usize) -> u32 {
        let id = &self.bodies[i].id;
        match id.as_u32_if_flat() {
            Some(n) => n,
            None => {
                let s = id.to_string();
                let mut h: u32 = 0x811c_9dc5;
                for b in s.as_bytes() {
                    h ^= *b as u32;
                    h = h.wrapping_mul(0x0100_0193);
                }
                h | 0x8000_0000
            }
        }
    }

    /// Root feature id that seeded this body. For flat-numeric
    /// bodies this equals `bodyId`; for pattern clones it returns
    /// the pattern feature's flat id so the UI can look up the
    /// feature name even though `bodyId` is a unique hash.
    #[wasm_bindgen(js_name = bodySeedFeatureId)]
    pub fn body_seed_feature_id(&self, i: usize) -> u32 {
        let id = &self.bodies[i].id;
        match id.as_u32_if_flat() {
            Some(n) => n,
            None => {
                let mut root = id;
                while let Some(parent) = root.parent() {
                    root = parent;
                }
                root.as_u32_if_flat().unwrap_or(0)
            }
        }
    }

    /// Per-body version counter. Bumps only when the body's own
    /// mesh was actually rebuilt — not when unrelated bodies
    /// mutated around it.
    #[wasm_bindgen(js_name = bodyVersion)]
    pub fn body_version(&self, i: usize) -> u32 {
        self.bodies[i].version
    }

    #[wasm_bindgen(js_name = bodyVertexCount)]
    pub fn body_vertex_count(&self, i: usize) -> usize {
        self.bodies[i].mesh.vertex_count()
    }

    #[wasm_bindgen(js_name = bodyIndexCount)]
    pub fn body_index_count(&self, i: usize) -> usize {
        self.bodies[i].mesh.indices.len()
    }

    #[wasm_bindgen(js_name = bodyTriangleCount)]
    pub fn body_triangle_count(&self, i: usize) -> usize {
        self.bodies[i].mesh.triangle_count()
    }

    #[wasm_bindgen(js_name = bodyPositionsPtr)]
    pub fn body_positions_ptr(&self, i: usize) -> *const f32 {
        self.bodies[i].mesh.positions.as_ptr()
    }

    #[wasm_bindgen(js_name = bodyNormalsPtr)]
    pub fn body_normals_ptr(&self, i: usize) -> *const f32 {
        self.bodies[i].mesh.normals.as_ptr()
    }

    #[wasm_bindgen(js_name = bodyIndicesPtr)]
    pub fn body_indices_ptr(&self, i: usize) -> *const u32 {
        self.bodies[i].mesh.indices.as_ptr()
    }

    #[wasm_bindgen(js_name = bodyClassificationsPtr)]
    pub fn body_classifications_ptr(&self, i: usize) -> *const u8 {
        self.bodies[i].mesh.classifications.as_ptr()
    }

    #[wasm_bindgen(js_name = bodyContributorsPtr)]
    pub fn body_contributors_ptr(&self, i: usize) -> *const u32 {
        self.bodies[i].mesh.contributors.as_ptr()
    }

    /// K3.6: returns `true` when the i-th body has a pending
    /// translation delta that the frontend should apply as a GPU
    /// matrix instead of re-uploading geometry.
    #[wasm_bindgen(js_name = bodyHasTransformDelta)]
    pub fn body_has_transform_delta(&self, i: usize) -> bool {
        self.bodies[i].transform_delta.is_some()
    }

    /// K3.6: returns a 3-element `Float32Array` [dx, dy, dz] for
    /// the i-th body's pending translation delta. Panics if the
    /// body has no delta — guard with `bodyHasTransformDelta` first.
    #[wasm_bindgen(js_name = bodyTransformDelta)]
    pub fn body_transform_delta(&self, i: usize) -> Vec<f32> {
        let d = self.bodies[i]
            .transform_delta
            .expect("no transform delta");
        vec![d[0], d[1], d[2]]
    }

    #[wasm_bindgen(js_name = bodySdfOpcodesPtr)]
    pub fn body_sdf_opcodes_ptr(&self, i: usize) -> *const f32 {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .map_or(std::ptr::null(), |s| s.opcodes.as_ptr())
    }

    #[wasm_bindgen(js_name = bodySdfOpcodesLen)]
    pub fn body_sdf_opcodes_len(&self, i: usize) -> usize {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .map_or(0, |s| s.opcodes.len())
    }

    /// Evaluate the body's compiled opcode stream on the CPU mirror
    /// interpreter at a world point. This is the parity REFERENCE for the
    /// GPU probe (`window.__fdSdfVmProbe` runs the live TSL interpreter on
    /// the same stream): both sides interpret identical bytes, so any
    /// disagreement is a GPU-side interpreter bug.
    #[wasm_bindgen(js_name = bodySdfEval)]
    pub fn body_sdf_eval(&self, i: usize, x: f32, y: f32, z: f32) -> f32 {
        self.bodies[i].compiled_sdf.as_ref().map_or(f32::MAX, |s| {
            sdf_compiler::eval_opcodes_with_volumes(&s.opcodes, [x, y, z], &s.volumes)
        })
    }

    /// Conservative raymarch step factor (≤1) for this body — the renderer
    /// multiplies its march step by this so a thin-walled revolve/shell isn't
    /// overshot at grazing angles (Ticket 1wnm). 1.0 = full step.
    #[wasm_bindgen(js_name = bodySdfMarchRelax)]
    pub fn body_sdf_march_relax(&self, i: usize) -> f32 {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .map_or(1.0, |s| s.march_relax)
    }

    #[wasm_bindgen(js_name = bodySdfAabb)]
    pub fn body_sdf_aabb(&self, i: usize) -> Vec<f32> {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .map_or(vec![0.0; 6], |s| {
                vec![
                    s.aabb.min[0], s.aabb.min[1], s.aabb.min[2],
                    s.aabb.max[0], s.aabb.max[1], s.aabb.max[2],
                ]
            })
    }

    /// Analytic world AABB of the body that *contains* `feature_id`, packed
    /// `[minx, miny, minz, maxx, maxy, maxz]`, or `[]` when the feature isn't
    /// in any body (or its body is empty). This is the pipeline-maintained
    /// `Body.aabb` — tight for boolean cuts (ticket f88u), unlike the
    /// conservative [`Kernel::body_sdf_aabb`] CSG bound — so the Debug ▸
    /// Part-AABB overlay boxes exactly the selected part's extent.
    #[wasm_bindgen(js_name = bodyAabbForFeature)]
    pub fn body_aabb_for_feature(&self, feature_id: u32) -> Vec<f32> {
        let fid = FeatureId::from_u32(feature_id);
        let idx = match self.decoded.iter().position(|f| f.id == fid) {
            Some(i) => i,
            None => return Vec::new(),
        };
        match self.bodies.iter().find(|b| b.feature_indices.contains(&idx)) {
            Some(b) if !b.aabb.is_empty() => vec![
                b.aabb.min[0], b.aabb.min[1], b.aabb.min[2],
                b.aabb.max[0], b.aabb.max[1], b.aabb.max[2],
            ],
            _ => Vec::new(),
        }
    }

    #[wasm_bindgen(js_name = bodyVolumeCount)]
    pub fn body_volume_count(&self, i: usize) -> usize {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .map_or(0, |s| s.volumes.len())
    }

    #[wasm_bindgen(js_name = bodyVolumeDataPtr)]
    pub fn body_volume_data_ptr(&self, i: usize, vol_idx: usize) -> *const f32 {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .and_then(|s| s.volumes.get(vol_idx))
            .map_or(std::ptr::null(), |v| v.data.as_ptr())
    }

    #[wasm_bindgen(js_name = bodyVolumeDataLen)]
    pub fn body_volume_data_len(&self, i: usize, vol_idx: usize) -> usize {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .and_then(|s| s.volumes.get(vol_idx))
            .map_or(0, |v| v.data.len())
    }

    #[wasm_bindgen(js_name = bodyVolumeDims)]
    pub fn body_volume_dims(&self, i: usize, vol_idx: usize) -> Vec<u32> {
        self.bodies[i]
            .compiled_sdf
            .as_ref()
            .and_then(|s| s.volumes.get(vol_idx))
            .map_or(vec![0, 0, 0], |v| v.dims.to_vec())
    }

    #[wasm_bindgen(js_name = bodyHybridInfoJson)]
    pub fn body_hybrid_info_json(&self, i: usize) -> String {
        // The JS side queries by index from a meshed-body snapshot that can
        // lag the live kernel by a frame (e.g. right after "New" empties the
        // bodies). An out-of-range index must yield an empty info, never panic.
        let Some(body) = self.bodies.get(i) else {
            return serde_json::to_string(&bvh::HybridInfo::default())
                .unwrap_or_else(|_| "{}".into());
        };
        let bounded =
            body::build_bounded_for_body(&self.decoded, &body.feature_indices);
        let info = bounded.classify_for_hybrid();
        serde_json::to_string(&info).unwrap_or_else(|_| "{}".into())
    }


    /// Number of `u32` slots reserved per vertex in the per-body
    /// contributor flat buffer. Matches `csg::MAX_CONTRIBUTORS`.
    #[wasm_bindgen(js_name = meshContributorStride)]
    pub fn mesh_contributor_stride() -> usize {
        csg::MAX_CONTRIBUTORS
    }

    /// Monotonic counter bumped every time the mesh is rebuilt. JS can
    /// use it to debounce uploads ("only rebind the BufferGeometry when
    /// this changes since the last frame").
    #[wasm_bindgen(js_name = meshVersion)]
    pub fn mesh_version(&self) -> u32 {
        self.mesh_version
    }

    /// Voxel grid resolution for the active body. Aliased as
    /// `meshResolution` for backward compatibility with callers
    /// that don't care about LOD yet.
    #[wasm_bindgen(js_name = meshResolution)]
    pub fn mesh_resolution(&self) -> u32 {
        self.active_mesh_resolution
    }

    /// Change the active-body voxel grid resolution and re-mesh.
    /// No-op if the resolution is unchanged or zero. Returns true
    /// if the mesh was actually rebuilt. Aliased as
    /// `setMeshResolution` for backward compatibility.
    #[wasm_bindgen(js_name = setMeshResolution)]
    pub fn set_mesh_resolution(&mut self, resolution: u32) -> bool {
        if resolution == 0 || resolution == self.active_mesh_resolution {
            return false;
        }
        self.active_mesh_resolution = resolution;
        self.rebuild_mesh_internal();
        true
    }

    // ── Mesh algorithm selection ────────────────────────────────────────

    #[wasm_bindgen(js_name = setMeshAlgorithm)]
    pub fn set_mesh_algorithm(&mut self, name: &str) -> bool {
        let algo = match name {
            "mt" => 2,
            _ => 0,
        };
        if algo == self.mesh_algorithm {
            return false;
        }
        self.mesh_algorithm = algo;
        self.bodies.clear();
        self.rebuild_mesh_internal();
        true
    }

    // ── LOD controls (K3 part 4) ──────────────────────────────────────

    #[wasm_bindgen(js_name = contextMeshResolution)]
    pub fn context_mesh_resolution(&self) -> u32 {
        self.context_mesh_resolution
    }

    /// Change the context-body voxel grid resolution and re-mesh
    /// every body that's currently running at context LOD. No-op
    /// if unchanged or zero.
    #[wasm_bindgen(js_name = setContextMeshResolution)]
    pub fn set_context_mesh_resolution(&mut self, resolution: u32) -> bool {
        if resolution == 0 || resolution == self.context_mesh_resolution {
            return false;
        }
        self.context_mesh_resolution = resolution;
        self.rebuild_mesh_internal();
        true
    }

    /// Currently active body id, or `0` when LOD is disabled (every
    /// body treated as active). The zero sentinel keeps the
    /// wasm-bindgen signature a simple `u32` — feature ids are
    /// allocated starting at `1` so `0` is unambiguous.
    #[wasm_bindgen(js_name = activeBodyId)]
    pub fn active_body_id(&self) -> u32 {
        self.active_body_id
            .as_ref()
            .and_then(|id| id.as_u32_if_flat())
            .unwrap_or(0)
    }

    /// Mark the given body id as the active one. Triggers a re-mesh
    /// that promotes `body_id` to active resolution and demotes
    /// whichever body was active before (if any) to context
    /// resolution. Passing `0` clears the active body.
    #[wasm_bindgen(js_name = setActiveBodyId)]
    pub fn set_active_body_id(&mut self, body_id: u32) -> bool {
        let new_active = if body_id == 0 {
            None
        } else {
            Some(FeatureId::from_u32(body_id))
        };
        self.set_active_body_id_internal(new_active)
    }

    /// Convenience: set the active body to whichever body owns the
    /// given feature. Returns true if the active body actually
    /// changed. No-op (returns false) when the feature doesn't
    /// exist or already lives in the active body.
    #[wasm_bindgen(js_name = setActiveBodyByFeature)]
    pub fn set_active_body_by_feature(&mut self, feature_id: u32) -> bool {
        let feature_id = FeatureId::from_u32(feature_id);
        match self.body_containing_feature(&feature_id) {
            Some(id) => self.set_active_body_id_internal(Some(id)),
            None => false,
        }
    }

    /// Clear the active body so every body runs at active LOD
    /// again. Equivalent to `setActiveBodyId(0)`.
    #[wasm_bindgen(js_name = clearActiveBodyId)]
    pub fn clear_active_body_id(&mut self) -> bool {
        self.set_active_body_id_internal(None)
    }

    // ── sync meshing toggle (K3 part 5) ───────────────────────────────

    /// Whether this kernel instance runs the Dual Contouring pass
    /// itself on every mutation. The frontend flips this off on the
    /// main-thread kernel once the mesher web worker is up — the
    /// worker owns its own kernel instance that meshes for real.
    #[wasm_bindgen(js_name = syncMeshing)]
    pub fn sync_meshing(&self) -> bool {
        self.sync_meshing
    }

    /// Enable or disable the synchronous DC pass. Toggling forces a
    /// rebuild so the body list's mesh buffers match the new
    /// setting: enabling meshes every body from scratch, disabling
    /// clears every mesh to the default empty buffer. No-op if the
    /// setting is already at the requested value.
    #[wasm_bindgen(js_name = setSyncMeshing)]
    pub fn set_sync_meshing(&mut self, enabled: bool) -> bool {
        if self.sync_meshing == enabled {
            return false;
        }
        self.sync_meshing = enabled;
        // Drop the body cache so `rebuild_mesh_internal` goes
        // through the fresh-mesh path for every body — otherwise
        // the cache-reuse check (`hash + resolution match`) would
        // keep populated meshes around after disabling, or empty
        // meshes after enabling.
        self.bodies.clear();
        self.rebuild_mesh_internal();
        true
    }

    /// Toggle milestone-1 parametric weaving (exact analytic B-Rep for pure
    /// grundforms). Returns `false` if the flag was already at `enabled`.
    /// Clears the body cache and remeshes so every body re-weaves through the
    /// chosen path — the woven `id_hash`es are identical on both routes, so
    /// existing element selections keep resolving across the toggle.
    #[wasm_bindgen(js_name = setParametricWeave)]
    pub fn set_parametric_weave(&mut self, enabled: bool) -> bool {
        if self.parametric_weave == enabled {
            return false;
        }
        self.parametric_weave = enabled;
        // The render mesh (DC) is identical on both routes — only the woven
        // B-Rep differs. Invalidate each body's B-Rep cache and bump its
        // version so the frontend refetches the now-exact (or now-sampled)
        // edges / faces; no remesh needed.
        for b in &mut self.bodies {
            b.brep = None;
            b.version = b.version.wrapping_add(1);
        }
        self.mesh_version = self.mesh_version.wrapping_add(1);
        true
    }

}
