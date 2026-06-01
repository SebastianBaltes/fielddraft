/* @ts-self-types="./fielddraft_kernel.d.ts" */

/**
 * Owned by the JS side. Holds the canonical document and the
 * undo/redo stacks.
 */
export class Kernel {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        KernelFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_kernel_free(ptr, 0);
    }
    /**
     * Currently active body id, or `0` when LOD is disabled (every
     * body treated as active). The zero sentinel keeps the
     * wasm-bindgen signature a simple `u32` — feature ids are
     * allocated starting at `1` so `0` is unambiguous.
     * @returns {number}
     */
    activeBodyId() {
        const ret = wasm.kernel_activeBodyId(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * @returns {number}
     */
    addBoolean() {
        const ret = wasm.kernel_addBoolean(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * K8.1: append a `circular_pattern` feature call that
     * replicates the source around an axis-through-center
     * pivot. `angle_deg` is the full sweep (360° for a
     * closed ring). Returns the new pattern feature id.
     * @param {number} source_id
     * @param {number} axis_x
     * @param {number} axis_y
     * @param {number} axis_z
     * @param {number} center_x
     * @param {number} center_y
     * @param {number} center_z
     * @param {number} count
     * @param {number} angle_deg
     * @returns {number}
     */
    addCircularPattern(source_id, axis_x, axis_y, axis_z, center_x, center_y, center_z, count, angle_deg) {
        const ret = wasm.kernel_addCircularPattern(this.__wbg_ptr, source_id, axis_x, axis_y, axis_z, center_x, center_y, center_z, count, angle_deg);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * K7.4: append an `extrude` feature call that lowers the
     * referenced sketch into a 3D body via the K7.3
     * `Primitive::Extrude` pipeline. Convenience wrapper over
     * the full FeatureCall shape so the UI doesn't have to
     * hand-assemble the params JSON.
     *
     * `sketch_id` must point at an existing `sketch`
     * FeatureCall. `depth_mm` is the total slab depth (the
     * `Primitive::Extrude` half_depth_z becomes `depth_mm /
     * 2`). `op` uses the same `BoolOp::TAG_*` tags as
     * `add_primitive`. `tz` positions the extrude plane along
     * the world Z axis.
     * @param {number} sketch_id
     * @param {number} depth_mm
     * @param {number} op
     * @param {number} tz
     * @returns {number}
     */
    addExtrude(sketch_id, depth_mm, op, tz) {
        const ret = wasm.kernel_addExtrude(this.__wbg_ptr, sketch_id, depth_mm, op, tz);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * K10.6: add a fillet_edge feature from detected edge data.
     * @param {number} feature_a
     * @param {number} feature_b
     * @param {number} p0x
     * @param {number} p0y
     * @param {number} p0z
     * @param {number} p1x
     * @param {number} p1y
     * @param {number} p1z
     * @param {number} mask_radius
     * @param {number} blend_radius
     * @returns {number}
     */
    addFilletEdge(feature_a, feature_b, p0x, p0y, p0z, p1x, p1y, p1z, mask_radius, blend_radius) {
        const ret = wasm.kernel_addFilletEdge(this.__wbg_ptr, feature_a, feature_b, p0x, p0y, p0z, p1x, p1y, p1z, mask_radius, blend_radius);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * K11.0: append a `gyroid` feature call. `cell_size_mm` is
     * the cell period, `thickness_mm` the wall half-thickness.
     * Default op is Intersect so the gyroid clips to any
     * existing bodies. Returns the new id.
     * @param {number} cell_size_mm
     * @param {number} thickness_mm
     * @param {number} op
     * @returns {number}
     */
    addGyroid(cell_size_mm, thickness_mm, op) {
        const ret = wasm.kernel_addGyroid(this.__wbg_ptr, cell_size_mm, thickness_mm, op);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * K8.9: append a `hole` feature call. Thin convenience
     * wrapper that serialises the typed diameter/depth/translate
     * into a FeatureCall and bootstraps. Returns the new id.
     * @param {number} diameter_mm
     * @param {number} depth_mm
     * @param {number} tx
     * @param {number} ty
     * @param {number} tz
     * @returns {number}
     */
    addHole(diameter_mm, depth_mm, tx, ty, tz) {
        const ret = wasm.kernel_addHole(this.__wbg_ptr, diameter_mm, depth_mm, tx, ty, tz);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * K13.1: import triangle data as a solid CSG leaf.
     * Stores only the original positions+indices in the document (~2MB)
     * instead of the full MeshSdf with tris/bboxes (~175MB after voxel repair).
     * The MeshSdf is rebuilt on-demand in feature execution.
     * @param {Float32Array} positions
     * @param {Uint32Array} indices
     * @param {number} op
     * @returns {number}
     */
    addImportedMesh(positions, indices, op) {
        const ptr0 = passArrayF32ToWasm0(positions, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArray32ToWasm0(indices, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.kernel_addImportedMesh(this.__wbg_ptr, ptr0, len0, ptr1, len1, op);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * vector. `source_id` must point at an existing spine
     * feature; `count` is total instances including the
     * source. Returns the allocated pattern feature id.
     * @param {number} source_id
     * @param {number} dx
     * @param {number} dy
     * @param {number} dz
     * @param {number} count
     * @param {number} spacing_mm
     * @returns {number}
     */
    addLinearPattern(source_id, dx, dy, dz, count, spacing_mm) {
        const ret = wasm.kernel_addLinearPattern(this.__wbg_ptr, source_id, dx, dy, dz, count, spacing_mm);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * K9.6: add a mate connector at a surface point.
     * @param {number} ox
     * @param {number} oy
     * @param {number} oz
     * @param {number} nx
     * @param {number} ny
     * @param {number} nz
     * @returns {number}
     */
    addMateConnector(ox, oy, oz, nx, ny, nz) {
        const ret = wasm.kernel_addMateConnector(this.__wbg_ptr, ox, oy, oz, nx, ny, nz);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * @returns {number}
     */
    addMaterial() {
        const ret = wasm.kernel_addMaterial(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * K8.2: append a `mirror` feature call that reflects the
     * source feature through a plane defined by a point and a
     * normal. Returns the new mirror feature id.
     * @param {number} source_id
     * @param {number} normal_x
     * @param {number} normal_y
     * @param {number} normal_z
     * @param {number} point_x
     * @param {number} point_y
     * @param {number} point_z
     * @returns {number}
     */
    addMirror(source_id, normal_x, normal_y, normal_z, point_x, point_y, point_z) {
        const ret = wasm.kernel_addMirror(this.__wbg_ptr, source_id, normal_x, normal_y, normal_z, point_x, point_y, point_z);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * Append a primitive feature.
     *
     * The `tx`/`ty`/`tz` arguments populate the new feature's transform
     * (`Transform.translate`); the primitive itself is created at its
     * own origin so that future transform edits remain a single source
     * of truth.
     *
     * `params` is interpreted per kind:
     *   - box      → (half_x, half_y, half_z)
     *   - sphere   → (radius, _, _)
     *   - cylinder → (radius, half_height, _)
     *   - thread   → (nominal_radius, pitch, depth)  half_height defaults to 5
     * @param {number} kind
     * @param {number} op
     * @param {number} tx
     * @param {number} ty
     * @param {number} tz
     * @param {number} p0
     * @param {number} p1
     * @param {number} p2
     * @returns {number}
     */
    addPrimitive(kind, op, tx, ty, tz, p0, p1, p2) {
        const ret = wasm.kernel_addPrimitive(this.__wbg_ptr, kind, op, tx, ty, tz, p0, p1, p2);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * K8.4: append a `revolve` feature call that consumes the
     * sketch with id `sketch_id` and sweeps its profile around
     * the local Y axis through `(cx, cy, cz)`. `op` uses the
     * same `BoolOp::TAG_*` tags as `add_primitive`. Returns
     * the new feature id.
     * @param {number} sketch_id
     * @param {number} op
     * @param {number} cx
     * @param {number} cy
     * @param {number} cz
     * @returns {number}
     */
    addRevolve(sketch_id, op, cx, cy, cz) {
        const ret = wasm.kernel_addRevolve(this.__wbg_ptr, sketch_id, op, cx, cy, cz);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * K6b.3a: commit a sketch from the UI's sub-spine draft. The
     * caller hands in a plane query string plus a JSON array of
     * `FeatureCall`s (the sketch-tool calls accumulated by
     * `SketchSession.subCalls` in the TS store). The kernel
     * pushes a `sketch`-type `FeatureCall` onto `feature_calls`,
     * regenerates the document so the `sketch` feature's
     * `execute` runs and stashes the solved `SketchState` in
     * `Context::sketches`, and returns the allocated feature id.
     *
     * Regeneration uses the same `bootstrap_features_from_calls`
     * path the load-from-JSON hook uses: clear `features`, walk
     * `feature_calls`, let each registered `FeatureKind::execute`
     * rebuild the spine as it goes. Primitive entries land in
     * `features`, the new `sketch` entry contributes to the
     * context-side `sketches` map. After the walk succeeds,
     * `rebuild_packed` resyncs everything and the mesher sees
     * the same spine it would have seen if the sketch had been
     * there all along.
     * @param {string} plane_query
     * @param {string} sub_calls_json
     * @returns {number}
     */
    addSketch(plane_query, sub_calls_json) {
        const ptr0 = passStringToWasm0(plane_query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(sub_calls_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.kernel_addSketch(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * @param {number} target_id
     * @param {number} rx
     * @param {number} ry
     * @param {number} rz
     * @param {number} rw
     * @returns {boolean}
     */
    applyGizmoRotate(target_id, rx, ry, rz, rw) {
        const ret = wasm.kernel_applyGizmoRotate(this.__wbg_ptr, target_id, rx, ry, rz, rw);
        return ret !== 0;
    }
    /**
     * @param {number} target_id
     * @param {number} sx
     * @param {number} sy
     * @param {number} sz
     * @returns {boolean}
     */
    applyGizmoScale(target_id, sx, sy, sz) {
        const ret = wasm.kernel_applyGizmoScale(this.__wbg_ptr, target_id, sx, sy, sz);
        return ret !== 0;
    }
    /**
     * @param {number} target_id
     * @param {number} tx
     * @param {number} ty
     * @param {number} tz
     * @returns {boolean}
     */
    applyGizmoTranslate(target_id, tx, ty, tz) {
        const ret = wasm.kernel_applyGizmoTranslate(this.__wbg_ptr, target_id, tx, ty, tz);
        return ret !== 0;
    }
    /**
     * Counts of the body's feature-based B-Rep entities as a JSON
     * object: `{"vertices","edges","faces","shells","solids",
     * "watertight"}`. `watertight` reflects the body's single outer
     * shell's `is_closed` flag.
     *
     * This is the **real** B-Rep topology (sharp corners / edges /
     * trimmed faces from the analytic weave). The B-Rep is woven
     * lazily on first call and cached on the body, so this query is
     * cheap on repeat (DL-30). Smooth-only bodies (e.g. a lone sphere)
     * carry no sharp
     * features and legitimately report zero/near-zero counts —
     * feature-based B-Rep captures sharp topology only (DL-29).
     * @param {number} i
     * @returns {string}
     */
    bodyBrepCounts(i) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.kernel_bodyBrepCounts(this.__wbg_ptr, i);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Per-body analytic B-Rep edges, sampled to **world-space** polylines for
     * viewport rendering and hover. Returns a JSON array; each edge is
     * `{"a":<branch u32>,"b":<branch u32>,"points":[x,y,z, x,y,z, …]}`, where
     * `a`/`b` are the canonical colliding-branch signature. Those two ids are
     * the same `as_u32()` branch values that [`Kernel::pick`] reports in its
     * `contributors` array, so the frontend can match a hovered edge to its
     * polyline. Points are sampled in the body-local frame and folded to world
     * by the body's `placement` (DL-56), exactly like the STEP export. Reuses
     * the same lazily-woven, cached B-Rep as the counts / STEP queries.
     * @param {number} i
     * @returns {string}
     */
    bodyBrepEdges(i) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.kernel_bodyBrepEdges(this.__wbg_ptr, i);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * @param {number} i
     * @returns {number}
     */
    bodyClassificationsPtr(i) {
        const ret = wasm.kernel_bodyClassificationsPtr(this.__wbg_ptr, i);
        return ret >>> 0;
    }
    /**
     * @param {number} i
     * @returns {number}
     */
    bodyContributorsPtr(i) {
        const ret = wasm.kernel_bodyContributorsPtr(this.__wbg_ptr, i);
        return ret >>> 0;
    }
    /**
     * @returns {number}
     */
    bodyCount() {
        const ret = wasm.kernel_bodyCount(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * K3.6: returns `true` when the i-th body has a pending
     * translation delta that the frontend should apply as a GPU
     * matrix instead of re-uploading geometry.
     * @param {number} i
     * @returns {boolean}
     */
    bodyHasTransformDelta(i) {
        const ret = wasm.kernel_bodyHasTransformDelta(this.__wbg_ptr, i);
        return ret !== 0;
    }
    /**
     * @param {number} i
     * @returns {string}
     */
    bodyHybridInfoJson(i) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.kernel_bodyHybridInfoJson(this.__wbg_ptr, i);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Stable identifier for the i-th body (= lowest `FeatureId` in
     * its feature set). Panics only if `i` is out of range — tests
     * and the wasm-bindgen bridge should always gate the call on
     * `bodyCount()`.
     *
     * Flat-numeric ids pass through as-is. Hierarchical ids
     * (pattern clones like "3.1") are hashed to a unique u32 with
     * the high bit set so they never collide with flat counters.
     * @param {number} i
     * @returns {number}
     */
    bodyId(i) {
        const ret = wasm.kernel_bodyId(this.__wbg_ptr, i);
        return ret >>> 0;
    }
    /**
     * @param {number} i
     * @returns {number}
     */
    bodyIndexCount(i) {
        const ret = wasm.kernel_bodyIndexCount(this.__wbg_ptr, i);
        return ret >>> 0;
    }
    /**
     * @param {number} i
     * @returns {number}
     */
    bodyIndicesPtr(i) {
        const ret = wasm.kernel_bodyIndicesPtr(this.__wbg_ptr, i);
        return ret >>> 0;
    }
    /**
     * @param {number} i
     * @returns {number}
     */
    bodyNormalsPtr(i) {
        const ret = wasm.kernel_bodyNormalsPtr(this.__wbg_ptr, i);
        return ret >>> 0;
    }
    /**
     * @param {number} i
     * @returns {number}
     */
    bodyPositionsPtr(i) {
        const ret = wasm.kernel_bodyPositionsPtr(this.__wbg_ptr, i);
        return ret >>> 0;
    }
    /**
     * @param {number} i
     * @returns {Float32Array}
     */
    bodySdfAabb(i) {
        const ret = wasm.kernel_bodySdfAabb(this.__wbg_ptr, i);
        var v1 = getArrayF32FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * @param {number} i
     * @returns {number}
     */
    bodySdfOpcodesLen(i) {
        const ret = wasm.kernel_bodySdfOpcodesLen(this.__wbg_ptr, i);
        return ret >>> 0;
    }
    /**
     * @param {number} i
     * @returns {number}
     */
    bodySdfOpcodesPtr(i) {
        const ret = wasm.kernel_bodySdfOpcodesPtr(this.__wbg_ptr, i);
        return ret >>> 0;
    }
    /**
     * Root feature id that seeded this body. For flat-numeric
     * bodies this equals `bodyId`; for pattern clones it returns
     * the pattern feature's flat id so the UI can look up the
     * feature name even though `bodyId` is a unique hash.
     * @param {number} i
     * @returns {number}
     */
    bodySeedFeatureId(i) {
        const ret = wasm.kernel_bodySeedFeatureId(this.__wbg_ptr, i);
        return ret >>> 0;
    }
    /**
     * Serialise body `i`'s feature-based B-Rep as an ISO-10303-21
     * (STEP) AP214 advanced-B-Rep file. Reuses the same lazily-woven,
     * cached B-Rep as [`Kernel::body_brep_counts`]; the returned
     * string is a complete `.step` document ready to download. The
     * product name is derived from the body's stable feature id.
     * @param {number} i
     * @returns {string}
     */
    bodyStepExport(i) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.kernel_bodyStepExport(this.__wbg_ptr, i);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * K3.6: returns a 3-element `Float32Array` [dx, dy, dz] for
     * the i-th body's pending translation delta. Panics if the
     * body has no delta — guard with `bodyHasTransformDelta` first.
     * @param {number} i
     * @returns {Float32Array}
     */
    bodyTransformDelta(i) {
        const ret = wasm.kernel_bodyTransformDelta(this.__wbg_ptr, i);
        var v1 = getArrayF32FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * @param {number} i
     * @returns {number}
     */
    bodyTriangleCount(i) {
        const ret = wasm.kernel_bodyTriangleCount(this.__wbg_ptr, i);
        return ret >>> 0;
    }
    /**
     * Per-body version counter. Bumps only when the body's own
     * mesh was actually rebuilt — not when unrelated bodies
     * mutated around it.
     * @param {number} i
     * @returns {number}
     */
    bodyVersion(i) {
        const ret = wasm.kernel_bodyVersion(this.__wbg_ptr, i);
        return ret >>> 0;
    }
    /**
     * @param {number} i
     * @returns {number}
     */
    bodyVertexCount(i) {
        const ret = wasm.kernel_bodyVertexCount(this.__wbg_ptr, i);
        return ret >>> 0;
    }
    /**
     * @param {number} i
     * @returns {number}
     */
    bodyVolumeCount(i) {
        const ret = wasm.kernel_bodyVolumeCount(this.__wbg_ptr, i);
        return ret >>> 0;
    }
    /**
     * @param {number} i
     * @param {number} vol_idx
     * @returns {number}
     */
    bodyVolumeDataLen(i, vol_idx) {
        const ret = wasm.kernel_bodyVolumeDataLen(this.__wbg_ptr, i, vol_idx);
        return ret >>> 0;
    }
    /**
     * @param {number} i
     * @param {number} vol_idx
     * @returns {number}
     */
    bodyVolumeDataPtr(i, vol_idx) {
        const ret = wasm.kernel_bodyVolumeDataPtr(this.__wbg_ptr, i, vol_idx);
        return ret >>> 0;
    }
    /**
     * @param {number} i
     * @param {number} vol_idx
     * @returns {Uint32Array}
     */
    bodyVolumeDims(i, vol_idx) {
        const ret = wasm.kernel_bodyVolumeDims(this.__wbg_ptr, i, vol_idx);
        var v1 = getArrayU32FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * @returns {boolean}
     */
    canRedo() {
        const ret = wasm.kernel_canRedo(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * @returns {boolean}
     */
    canUndo() {
        const ret = wasm.kernel_canUndo(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * @returns {string}
     */
    classifyForStep() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.kernel_classifyForStep(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Clear the active body so every body runs at active LOD
     * again. Equivalent to `setActiveBodyId(0)`.
     * @returns {boolean}
     */
    clearActiveBodyId() {
        const ret = wasm.kernel_clearActiveBodyId(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * @returns {number}
     */
    contextMeshResolution() {
        const ret = wasm.kernel_contextMeshResolution(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * K8.8: duplicate a feature call. Allocates a fresh id,
     * clones the call's wire-form params, nudges any
     * `translate` vector by `(+20, 0, 0)` so the clone doesn't
     * sit exactly on top of the source, and pushes the new
     * call to `feature_calls`. Returns the new id. Features
     * without a translate field (patterns, mirror, sketch)
     * are cloned verbatim — their geometry is derived from a
     * referenced source, so the visual offset happens on the
     * source side instead.
     * @param {number} id
     * @returns {number}
     */
    duplicateFeature(id) {
        const ret = wasm.kernel_duplicateFeature(this.__wbg_ptr, id);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * @returns {Uint8Array}
     */
    export3mf() {
        const ret = wasm.kernel_export3mf(this.__wbg_ptr);
        var v1 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v1;
    }
    /**
     * Export the current scene as an ASCII STL string.
     * Iterates every body's DC mesh in the order `bodies`
     * holds them; downstream slicers treat the whole blob as
     * one `solid`.
     *
     * Takes `&mut self` because the worker-mode kernel keeps
     * its main-thread `bodies` cache empty (the worker owns
     * the meshes). Export temporarily forces a sync rebuild
     * so the bytes come out non-empty, then leaves the sync
     * state exactly as it found it.
     * @returns {string}
     */
    exportStlAscii() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.kernel_exportStlAscii(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Export the current scene as binary STL. Returns a
     * JavaScript `Uint8Array` by virtue of wasm-bindgen's
     * auto-conversion for `Vec<u8>`.
     *
     * Same `&mut self` + sync-rebuild dance as
     * [`export_stl_ascii`].
     * @returns {Uint8Array}
     */
    exportStlBinary() {
        const ret = wasm.kernel_exportStlBinary(this.__wbg_ptr);
        var v1 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v1;
    }
    /**
     * @returns {number}
     */
    featureCount() {
        const ret = wasm.kernel_featureCount(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * K7.5: return the current `params` JSON string for a
     * feature call by id. Used by the Outliner's
     * FeatureDialog to initialise its edit buffer with the
     * feature's real values rather than a schema-default
     * blob.
     * @param {number} id
     * @returns {string}
     */
    getFeatureCallParams(id) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.kernel_getFeatureCallParams(this.__wbg_ptr, id);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * @returns {number}
     */
    getRollbackIndex() {
        const ret = wasm.kernel_getRollbackIndex(this.__wbg_ptr);
        return ret;
    }
    /**
     * K6b.3a: fetch the sub-spine sub_calls for a sketch by
     * feature id, so the UI can reload an existing sketch when
     * the user re-enters it. Returns the JSON array string of
     * `FeatureCall`s or an error if the id doesn't point at a
     * sketch.
     * @param {number} id
     * @returns {string}
     */
    getSketchSubCalls(id) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.kernel_getSketchSubCalls(this.__wbg_ptr, id);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * K6a: list every feature type name the built-in registry
     * knows about, sorted for deterministic UI rendering. The
     * result is a JSON array of strings, e.g. `["box", "cylinder",
     * "extrude", "sphere"]`.
     * @returns {string}
     */
    listFeatureTypes() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.kernel_listFeatureTypes(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * Return a JSON array of feature summaries (id, name, op, kind).
     *
     * K6b.3a: the list walks `feature_calls` — the K4.5 canonical
     * wire form — instead of the spine directly. For primitive
     * calls the summary is pulled from the matching spine entry
     * (translate, scale, op are authoritative there). For
     * non-spine calls like `sketch` the summary is synthesised
     * from the call itself with neutral transform defaults, so
     * the outliner sees a stable row per sketch even though
     * sketches never push onto the spine.
     * @returns {string}
     */
    listFeatures() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.kernel_listFeatures(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * List the document's FieldSkript records as a JSON array.
     * @returns {string}
     */
    listScripts() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.kernel_listScripts(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * K8.13: list the connected inside-regions of a sketch.
     * Returns a JSON array of `{index, centroid, aabb, area}`
     * objects. The sketch is identified by its feature id.
     * @param {number} sketch_id
     * @param {number} resolution
     * @returns {string}
     */
    listSketchRegions(sketch_id, resolution) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.kernel_listSketchRegions(this.__wbg_ptr, sketch_id, resolution);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * K6b.2: list every sketch-tool type name the kernel knows
     * about. Sketch tools live in a separate
     * [`sketch_features::SketchToolRegistry`] that the UI's
     * sketch-mode toolbar queries to decide which tool buttons
     * to render. JSON array of strings, sorted alphabetically
     * for deterministic rendering.
     * @returns {string}
     */
    listSketchToolTypes() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.kernel_listSketchToolTypes(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * @returns {string}
     */
    listVariables() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.kernel_listVariables(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * @param {string} json
     * @returns {number}
     */
    loadJson(json) {
        const ptr0 = passStringToWasm0(json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.kernel_loadJson(this.__wbg_ptr, ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * Number of `u32` slots reserved per vertex in the per-body
     * contributor flat buffer. Matches `csg::MAX_CONTRIBUTORS`.
     * @returns {number}
     */
    static meshContributorStride() {
        const ret = wasm.kernel_meshContributorStride();
        return ret >>> 0;
    }
    /**
     * Sum of index counts across every body. O(bodies).
     * @returns {number}
     */
    meshIndexCount() {
        const ret = wasm.kernel_meshIndexCount(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Voxel grid resolution for the active body. Aliased as
     * `meshResolution` for backward compatibility with callers
     * that don't care about LOD yet.
     * @returns {number}
     */
    meshResolution() {
        const ret = wasm.kernel_meshResolution(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Sum of triangle counts across every body. O(bodies).
     * @returns {number}
     */
    meshTriangleCount() {
        const ret = wasm.kernel_meshTriangleCount(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Monotonic counter bumped every time the mesh is rebuilt. JS can
     * use it to debounce uploads ("only rebind the BufferGeometry when
     * this changes since the last frame").
     * @returns {number}
     */
    meshVersion() {
        const ret = wasm.kernel_meshVersion(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Sum of vertex counts across every body. O(bodies).
     * @returns {number}
     */
    meshVertexCount() {
        const ret = wasm.kernel_meshVertexCount(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Create a new kernel with an empty document.
     */
    constructor() {
        const ret = wasm.kernel_new();
        this.__wbg_ptr = ret >>> 0;
        KernelFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Add `(dx, dy, dz)` to a feature's translate. Convenience for
     * keyboard nudging.
     * @param {number} id
     * @param {number} dx
     * @param {number} dy
     * @param {number} dz
     * @returns {boolean}
     */
    nudgeFeature(id, dx, dy, dz) {
        const ret = wasm.kernel_nudgeFeature(this.__wbg_ptr, id, dx, dy, dz);
        return ret !== 0;
    }
    /**
     * Sphere-trace a ray through the current CSG tree and return a
     * JSON description of the first hit, if any. See `raycast.rs`
     * for the numerical scheme.
     * @param {number} ox
     * @param {number} oy
     * @param {number} oz
     * @param {number} dx
     * @param {number} dy
     * @param {number} dz
     * @returns {string}
     */
    pick(ox, oy, oz, dx, dy, dz) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.kernel_pick(this.__wbg_ptr, ox, oy, oz, dx, dy, dz);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * @returns {boolean}
     */
    redo() {
        const ret = wasm.kernel_redo(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * @returns {number}
     */
    redoDepth() {
        const ret = wasm.kernel_redoDepth(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Remove the feature with the given id. Returns true if anything
     * was removed.
     * @param {number} id
     * @returns {boolean}
     */
    removeFeature(id) {
        const ret = wasm.kernel_removeFeature(this.__wbg_ptr, id);
        return ret !== 0;
    }
    /**
     * Remove a FieldSkript record by id. Returns `true` when a record
     * was removed.
     * @param {string} id
     * @returns {boolean}
     */
    removeScript(id) {
        const ptr0 = passStringToWasm0(id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.kernel_removeScript(this.__wbg_ptr, ptr0, len0);
        return ret !== 0;
    }
    /**
     * K8.10: remove a document-level variable. No-op if the
     * name isn't in the table; returns `true` when it actually
     * removed something.
     *
     * K8.11: re-bootstraps after the remove so any formula
     * that referenced the departing variable surfaces as a
     * regen error (which rolls the remove back).
     * @param {string} name
     * @returns {boolean}
     */
    removeVariable(name) {
        const ptr0 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.kernel_removeVariable(this.__wbg_ptr, ptr0, len0);
        return ret !== 0;
    }
    /**
     * Move a feature to a new index. Indices clamp into bounds.
     * @param {number} id
     * @param {number} target_index
     * @returns {boolean}
     */
    reorderFeature(id, target_index) {
        const ret = wasm.kernel_reorderFeature(this.__wbg_ptr, id, target_index);
        return ret !== 0;
    }
    /**
     * K7.5: replace the `params` JSON of a feature call in
     * place and regenerate the spine. Keeps the feature id
     * stable so the outliner row doesn't jump, and re-runs
     * the registry so downstream cached state (body meshes,
     * packed buffer) reflects the new values.
     * @param {number} id
     * @param {string} params_json
     */
    replaceFeatureCallParams(id, params_json) {
        const ptr0 = passStringToWasm0(params_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.kernel_replaceFeatureCallParams(this.__wbg_ptr, id, ptr0, len0);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * K6b.3b: replace an existing sketch's plane and sub-spine
     * in place, keeping its feature id stable. Used by the UI
     * when the user re-enters a committed sketch to edit it —
     * Finish should not create a new outliner row, it should
     * update the existing one. Fails fast if the target id
     * doesn't point at a sketch.
     * @param {number} id
     * @param {string} plane_query
     * @param {string} sub_calls_json
     */
    replaceSketch(id, plane_query, sub_calls_json) {
        const ptr0 = passStringToWasm0(plane_query, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(sub_calls_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.kernel_replaceSketch(this.__wbg_ptr, id, ptr0, len0, ptr1, len1);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * K8.10: list every document-level variable as a JSON
     * array of `{name, value}` pairs, sorted by name (the
     * BTreeMap ordering).
     * K9.1: return the overall scene AABB as a JSON object
     * `{min: [x,y,z], max: [x,y,z], size: [w,h,d]}` in mm.
     * Empty scenes return all-zero extents.
     * @returns {string}
     */
    sceneAabb() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.kernel_sceneAabb(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * Serialise *every* body's feature-based B-Rep into ONE ISO-10303-21
     * (STEP) AP214 file, each body a separate `MANIFOLD_SOLID_BREP` solid
     * in a single shape representation (DL-53: bodies stay separate). Each
     * body's B-Rep is lazily woven and cached exactly as in
     * [`Kernel::body_step_export`]; the returned string is a complete
     * `.step` document ready to download.
     * @returns {string}
     */
    sceneStepExport() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.kernel_sceneStepExport(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * K6b.2: look up the FeatureSchema-generated JSON schema for
     * a sketch tool. Same contract as [`schema_for_type`] but
     * routes through the sketch-tool registry so the disjoint
     * namespaces stay disjoint.
     * @param {string} type_name
     * @returns {string}
     */
    schemaForSketchToolType(type_name) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(type_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.kernel_schemaForSketchToolType(this.__wbg_ptr, ptr0, len0);
            var ptr2 = ret[0];
            var len2 = ret[1];
            if (ret[3]) {
                ptr2 = 0; len2 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * K6a: look up the FeatureSchema-generated JSON schema for
     * a feature type. Returns the serialised JSON string or an
     * error if the type isn't registered.
     * @param {string} type_name
     * @returns {string}
     */
    schemaForType(type_name) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(type_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.kernel_schemaForType(this.__wbg_ptr, ptr0, len0);
            var ptr2 = ret[0];
            var len2 = ret[1];
            if (ret[3]) {
                ptr2 = 0; len2 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred3_0 = ptr2;
            deferred3_1 = len2;
            return getStringFromWasm0(ptr2, len2);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * Convenience: set the active body to whichever body owns the
     * given feature. Returns true if the active body actually
     * changed. No-op (returns false) when the feature doesn't
     * exist or already lives in the active body.
     * @param {number} feature_id
     * @returns {boolean}
     */
    setActiveBodyByFeature(feature_id) {
        const ret = wasm.kernel_setActiveBodyByFeature(this.__wbg_ptr, feature_id);
        return ret !== 0;
    }
    /**
     * Mark the given body id as the active one. Triggers a re-mesh
     * that promotes `body_id` to active resolution and demotes
     * whichever body was active before (if any) to context
     * resolution. Passing `0` clears the active body.
     * @param {number} body_id
     * @returns {boolean}
     */
    setActiveBodyId(body_id) {
        const ret = wasm.kernel_setActiveBodyId(this.__wbg_ptr, body_id);
        return ret !== 0;
    }
    /**
     * Change the context-body voxel grid resolution and re-mesh
     * every body that's currently running at context LOD. No-op
     * if unchanged or zero.
     * @param {number} resolution
     * @returns {boolean}
     */
    setContextMeshResolution(resolution) {
        const ret = wasm.kernel_setContextMeshResolution(this.__wbg_ptr, resolution);
        return ret !== 0;
    }
    /**
     * Change the boolean op of an existing feature.
     * @param {number} id
     * @param {number} op
     * @returns {boolean}
     */
    setFeatureOp(id, op) {
        const ret = wasm.kernel_setFeatureOp(this.__wbg_ptr, id, op);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
    /**
     * K7.21h: replace the rotation component of a feature's transform.
     * Quaternion order: (x, y, z, w).
     * @param {number} id
     * @param {number} rx
     * @param {number} ry
     * @param {number} rz
     * @param {number} rw
     * @returns {boolean}
     */
    setFeatureRotation(id, rx, ry, rz, rw) {
        const ret = wasm.kernel_setFeatureRotation(this.__wbg_ptr, id, rx, ry, rz, rw);
        return ret !== 0;
    }
    /**
     * @param {number} id
     * @param {number} sx
     * @param {number} sy
     * @param {number} sz
     * @returns {boolean}
     */
    setFeatureScale(id, sx, sy, sz) {
        const ret = wasm.kernel_setFeatureScale(this.__wbg_ptr, id, sx, sy, sz);
        return ret !== 0;
    }
    /**
     * K8.7: toggle a feature call's suppressed flag and
     * regenerate. A suppressed call is skipped by the
     * registry's regen loop, so its geometry drops out of the
     * CSG tree, but the call stays on `feature_calls` so the
     * user can unsuppress it later. Fails if the id doesn't
     * match any call or if regeneration errors out (e.g.
     * unsuppressing a call whose upstream dependency is now
     * broken).
     * @param {number} id
     * @param {boolean} suppressed
     */
    setFeatureSuppressed(id, suppressed) {
        const ret = wasm.kernel_setFeatureSuppressed(this.__wbg_ptr, id, suppressed);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Replace the full transform (translate, quaternion, scale).
     * @param {number} id
     * @param {number} tx
     * @param {number} ty
     * @param {number} tz
     * @param {number} qx
     * @param {number} qy
     * @param {number} qz
     * @param {number} qw
     * @param {number} sx
     * @param {number} sy
     * @param {number} sz
     * @returns {boolean}
     */
    setFeatureTransform(id, tx, ty, tz, qx, qy, qz, qw, sx, sy, sz) {
        const ret = wasm.kernel_setFeatureTransform(this.__wbg_ptr, id, tx, ty, tz, qx, qy, qz, qw, sx, sy, sz);
        return ret !== 0;
    }
    /**
     * Replace the translate component of a feature's transform.
     * @param {number} id
     * @param {number} tx
     * @param {number} ty
     * @param {number} tz
     * @returns {boolean}
     */
    setFeatureTranslate(id, tx, ty, tz) {
        const ret = wasm.kernel_setFeatureTranslate(this.__wbg_ptr, id, tx, ty, tz);
        return ret !== 0;
    }
    /**
     * @param {string} name
     * @returns {boolean}
     */
    setMeshAlgorithm(name) {
        const ptr0 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.kernel_setMeshAlgorithm(this.__wbg_ptr, ptr0, len0);
        return ret !== 0;
    }
    /**
     * Change the active-body voxel grid resolution and re-mesh.
     * No-op if the resolution is unchanged or zero. Returns true
     * if the mesh was actually rebuilt. Aliased as
     * `setMeshResolution` for backward compatibility.
     * @param {number} resolution
     * @returns {boolean}
     */
    setMeshResolution(resolution) {
        const ret = wasm.kernel_setMeshResolution(this.__wbg_ptr, resolution);
        return ret !== 0;
    }
    /**
     * @param {number} index
     */
    setRollbackIndex(index) {
        wasm.kernel_setRollbackIndex(this.__wbg_ptr, index);
    }
    /**
     * Enable or disable the synchronous DC pass. Toggling forces a
     * rebuild so the body list's mesh buffers match the new
     * setting: enabling meshes every body from scratch, disabling
     * clears every mesh to the default empty buffer. No-op if the
     * setting is already at the requested value.
     * @param {boolean} enabled
     * @returns {boolean}
     */
    setSyncMeshing(enabled) {
        const ret = wasm.kernel_setSyncMeshing(this.__wbg_ptr, enabled);
        return ret !== 0;
    }
    /**
     * K8.10: upsert a document-level variable. Snapshots so
     * undo/redo walks variable edits the same way it walks
     * feature edits.
     *
     * K8.11: after the write, re-bootstraps from calls so
     * every formula that references the variable picks up
     * the new value on the next mesh pass. Pre-K8.11 this
     * was a pure state edit with no geometry side effect.
     * @param {string} name
     * @param {number} value
     */
    setVariable(name, value) {
        const ptr0 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.kernel_setVariable(this.__wbg_ptr, ptr0, len0, value);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * @returns {string}
     */
    stepCommands() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.kernel_stepCommands(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Whether this kernel instance runs the Dual Contouring pass
     * itself on every mutation. The frontend flips this off on the
     * main-thread kernel once the mesher web worker is up — the
     * worker owns its own kernel instance that meshes for real.
     * @returns {boolean}
     */
    syncMeshing() {
        const ret = wasm.kernel_syncMeshing(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * @returns {string}
     */
    toJson() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.kernel_toJson(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * @returns {boolean}
     */
    undo() {
        const ret = wasm.kernel_undo(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * @returns {number}
     */
    undoDepth() {
        const ret = wasm.kernel_undoDepth(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Insert or replace a FieldSkript record (keyed by `id`). Takes
     * the serialised `FieldSkriptRecord` JSON; returns `true` when it
     * replaced an existing record, `false` when it inserted a new one.
     * @param {string} json
     * @returns {boolean}
     */
    upsertScript(json) {
        const ptr0 = passStringToWasm0(json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.kernel_upsertScript(this.__wbg_ptr, ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
}
if (Symbol.dispose) Kernel.prototype[Symbol.dispose] = Kernel.prototype.free;

/**
 * Commit one built-in op into the live script scratch; returns the
 * created-feature handle ids as a JSON array.
 * @param {string} id
 * @param {string} type_name
 * @param {string} params_json
 * @returns {string}
 */
export function fdOpCommit(id, type_name, params_json) {
    let deferred5_0;
    let deferred5_1;
    try {
        const ptr0 = passStringToWasm0(id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(type_name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm0(params_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len2 = WASM_VECTOR_LEN;
        const ret = wasm.fdOpCommit(ptr0, len0, ptr1, len1, ptr2, len2);
        var ptr4 = ret[0];
        var len4 = ret[1];
        if (ret[3]) {
            ptr4 = 0; len4 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred5_0 = ptr4;
        deferred5_1 = len4;
        return getStringFromWasm0(ptr4, len4);
    } finally {
        wasm.__wbindgen_free(deferred5_0, deferred5_1, 1);
    }
}

/**
 * Install the worker's JS `build()` driver as the script runner for
 * this realm. The Rust→JS half of `fd.op*`: when `regenerate`
 * reaches a [`super::ScriptFeature`] it calls `run(scriptId,
 * instanceId, paramsJson)`, which the worker implements by running
 * the matching compiled `build()` in the determinism sandbox. A
 * thrown JS error becomes [`super::ScriptError::JsRun`] and
 * fail-stops the regeneration. Installed once at worker boot — the
 * runner persists in the `thread_local` across rebuilds.
 * @param {Function} run
 */
export function installScriptRunner(run) {
    wasm.installScriptRunner(run);
}

/**
 * The app version this WASM binary was stamped with at build time
 * (see `build.rs`). The web app compares it against `__APP_VERSION__`
 * on boot to catch a stale kernel module. Returns "dev" when built
 * without `FIELDDRAFT_VERSION` (e.g. `cargo test`).
 * @returns {string}
 */
export function wasmVersion() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.wasmVersion();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg_Error_2e59b1b37a9a34c3: function(arg0, arg1) {
            const ret = Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg___wbindgen_debug_string_dd5d2d07ce9e6c57: function(arg0, arg1) {
            const ret = debugString(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_string_get_914df97fcfa788f2: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_81fc77679af83bc6: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg_call_f2ac1622600b957f: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.call(arg1, arg2, arg3, arg4);
            return ret;
        }, arguments); },
        __wbg_error_a6fa202b58aa1cd3: function(arg0, arg1) {
            let deferred0_0;
            let deferred0_1;
            try {
                deferred0_0 = arg0;
                deferred0_1 = arg1;
                console.error(getStringFromWasm0(arg0, arg1));
            } finally {
                wasm.__wbindgen_free(deferred0_0, deferred0_1, 1);
            }
        },
        __wbg_instanceof_Error_e3390d6805733dad: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Error;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_message_7367f8c7d0fa1589: function(arg0) {
            const ret = arg0.message;
            return ret;
        },
        __wbg_new_227d7c05414eb861: function() {
            const ret = new Error();
            return ret;
        },
        __wbg_stack_3b0d974bbf31e44f: function(arg0, arg1) {
            const ret = arg1.stack;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbindgen_cast_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./fielddraft_kernel_bg.js": import0,
    };
}

const KernelFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_kernel_free(ptr >>> 0, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

function debugString(val) {
    // primitive types
    const type = typeof val;
    if (type == 'number' || type == 'boolean' || val == null) {
        return  `${val}`;
    }
    if (type == 'string') {
        return `"${val}"`;
    }
    if (type == 'symbol') {
        const description = val.description;
        if (description == null) {
            return 'Symbol';
        } else {
            return `Symbol(${description})`;
        }
    }
    if (type == 'function') {
        const name = val.name;
        if (typeof name == 'string' && name.length > 0) {
            return `Function(${name})`;
        } else {
            return 'Function';
        }
    }
    // objects
    if (Array.isArray(val)) {
        const length = val.length;
        let debug = '[';
        if (length > 0) {
            debug += debugString(val[0]);
        }
        for(let i = 1; i < length; i++) {
            debug += ', ' + debugString(val[i]);
        }
        debug += ']';
        return debug;
    }
    // Test for built-in
    const builtInMatches = /\[object ([^\]]+)\]/.exec(toString.call(val));
    let className;
    if (builtInMatches && builtInMatches.length > 1) {
        className = builtInMatches[1];
    } else {
        // Failed to match the standard '[object ClassName]'
        return toString.call(val);
    }
    if (className == 'Object') {
        // we're a user defined class or Object
        // JSON.stringify avoids problems with cycles, and is generally much
        // easier than looping through ownProperties of `val`.
        try {
            return 'Object(' + JSON.stringify(val) + ')';
        } catch (_) {
            return 'Object';
        }
    }
    // errors
    if (val instanceof Error) {
        return `${val.name}: ${val.message}\n${val.stack}`;
    }
    // TODO we could test for more things here, like `Set`s and `Map`s.
    return className;
}

function getArrayF32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getFloat32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayU32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

let cachedFloat32ArrayMemory0 = null;
function getFloat32ArrayMemory0() {
    if (cachedFloat32ArrayMemory0 === null || cachedFloat32ArrayMemory0.byteLength === 0) {
        cachedFloat32ArrayMemory0 = new Float32Array(wasm.memory.buffer);
    }
    return cachedFloat32ArrayMemory0;
}

function getStringFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return decodeText(ptr, len);
}

let cachedUint32ArrayMemory0 = null;
function getUint32ArrayMemory0() {
    if (cachedUint32ArrayMemory0 === null || cachedUint32ArrayMemory0.byteLength === 0) {
        cachedUint32ArrayMemory0 = new Uint32Array(wasm.memory.buffer);
    }
    return cachedUint32ArrayMemory0;
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        const idx = addToExternrefTable0(e);
        wasm.__wbindgen_exn_store(idx);
    }
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function passArray32ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 4, 4) >>> 0;
    getUint32ArrayMemory0().set(arg, ptr / 4);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArrayF32ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 4, 4) >>> 0;
    getFloat32ArrayMemory0().set(arg, ptr / 4);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasm;
function __wbg_finalize_init(instance, module) {
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedFloat32ArrayMemory0 = null;
    cachedUint32ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('fielddraft_kernel_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
