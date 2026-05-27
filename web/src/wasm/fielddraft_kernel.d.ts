/* tslint:disable */
/* eslint-disable */

/**
 * Owned by the JS side. Holds the canonical document, the packed
 * renderer buffer, and the undo/redo stacks. The packed buffer's
 * pointer is stable until any mutation that may grow the underlying
 * `Vec` (which can move it on reallocation) — JS callers must
 * re-acquire `primitivesPtr()` after every mutating call.
 */
export class Kernel {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Currently active body id, or `0` when LOD is disabled (every
     * body treated as active). The zero sentinel keeps the
     * wasm-bindgen signature a simple `u32` — feature ids are
     * allocated starting at `1` so `0` is unambiguous.
     */
    activeBodyId(): number;
    addBoolean(): number;
    /**
     * K8.1: append a `circular_pattern` feature call that
     * replicates the source around an axis-through-center
     * pivot. `angle_deg` is the full sweep (360° for a
     * closed ring). Returns the new pattern feature id.
     */
    addCircularPattern(source_id: number, axis_x: number, axis_y: number, axis_z: number, center_x: number, center_y: number, center_z: number, count: number, angle_deg: number): number;
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
     */
    addExtrude(sketch_id: number, depth_mm: number, op: number, tz: number): number;
    /**
     * K10.6: add a fillet_edge feature from detected edge data.
     */
    addFilletEdge(feature_a: number, feature_b: number, p0x: number, p0y: number, p0z: number, p1x: number, p1y: number, p1z: number, mask_radius: number, blend_radius: number): number;
    /**
     * K11.0: append a `gyroid` feature call. `cell_size_mm` is
     * the cell period, `thickness_mm` the wall half-thickness.
     * Default op is Intersect so the gyroid clips to any
     * existing bodies. Returns the new id.
     */
    addGyroid(cell_size_mm: number, thickness_mm: number, op: number): number;
    /**
     * K8.9: append a `hole` feature call. Thin convenience
     * wrapper that serialises the typed diameter/depth/translate
     * into a FeatureCall and bootstraps. Returns the new id.
     */
    addHole(diameter_mm: number, depth_mm: number, tx: number, ty: number, tz: number): number;
    /**
     * K13.1: import triangle data as a solid CSG leaf.
     * Stores only the original positions+indices in the document (~2MB)
     * instead of the full MeshSdf with tris/bboxes (~175MB after voxel repair).
     * The MeshSdf is rebuilt on-demand in feature execution.
     */
    addImportedMesh(positions: Float32Array, indices: Uint32Array, op: number): number;
    /**
     * vector. `source_id` must point at an existing spine
     * feature; `count` is total instances including the
     * source. Returns the allocated pattern feature id.
     */
    addLinearPattern(source_id: number, dx: number, dy: number, dz: number, count: number, spacing_mm: number): number;
    /**
     * K9.6: add a mate connector at a surface point.
     */
    addMateConnector(ox: number, oy: number, oz: number, nx: number, ny: number, nz: number): number;
    addMaterial(): number;
    /**
     * K8.2: append a `mirror` feature call that reflects the
     * source feature through a plane defined by a point and a
     * normal. Returns the new mirror feature id.
     */
    addMirror(source_id: number, normal_x: number, normal_y: number, normal_z: number, point_x: number, point_y: number, point_z: number): number;
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
     */
    addPrimitive(kind: number, op: number, tx: number, ty: number, tz: number, p0: number, p1: number, p2: number): number;
    /**
     * K8.4: append a `revolve` feature call that consumes the
     * sketch with id `sketch_id` and sweeps its profile around
     * the local Y axis through `(cx, cy, cz)`. `op` uses the
     * same `BoolOp::TAG_*` tags as `add_primitive`. Returns
     * the new feature id.
     */
    addRevolve(sketch_id: number, op: number, cx: number, cy: number, cz: number): number;
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
     */
    addSketch(plane_query: string, sub_calls_json: string): number;
    applyGizmoRotate(target_id: number, rx: number, ry: number, rz: number, rw: number): boolean;
    applyGizmoScale(target_id: number, sx: number, sy: number, sz: number): boolean;
    applyGizmoTranslate(target_id: number, tx: number, ty: number, tz: number): boolean;
    bodyClassificationsPtr(i: number): number;
    bodyContributorsPtr(i: number): number;
    bodyCount(): number;
    bodyEdgeContributorA(body: number, edge: number): number;
    bodyEdgeContributorB(body: number, edge: number): number;
    bodyEdgeCount(i: number): number;
    bodyEdgePointCount(body: number, edge: number): number;
    bodyEdgePointsPtr(body: number, edge: number): number;
    bodyFaceArea(body: number, face: number): number;
    bodyFaceCentroid(body: number, face: number): Float32Array;
    bodyFaceContributor(body: number, face: number): number;
    bodyFaceCount(i: number): number;
    bodyFaceNormal(body: number, face: number): Float32Array;
    bodyFaceTriangleCount(body: number, face: number): number;
    bodyFaceTrianglesPtr(body: number, face: number): number;
    /**
     * K3.6: returns `true` when the i-th body has a pending
     * translation delta that the frontend should apply as a GPU
     * matrix instead of re-uploading geometry.
     */
    bodyHasTransformDelta(i: number): boolean;
    bodyHybridInfoJson(i: number): string;
    /**
     * Stable identifier for the i-th body (= lowest `FeatureId` in
     * its feature set). Panics only if `i` is out of range — tests
     * and the wasm-bindgen bridge should always gate the call on
     * `bodyCount()`.
     *
     * Flat-numeric ids pass through as-is. Hierarchical ids
     * (pattern clones like "3.1") are hashed to a unique u32 with
     * the high bit set so they never collide with flat counters.
     */
    bodyId(i: number): number;
    bodyIndexCount(i: number): number;
    bodyIndicesPtr(i: number): number;
    bodyNormalsPtr(i: number): number;
    bodyPositionsPtr(i: number): number;
    bodySdfAabb(i: number): Float32Array;
    bodySdfOpcodesLen(i: number): number;
    bodySdfOpcodesPtr(i: number): number;
    /**
     * Root feature id that seeded this body. For flat-numeric
     * bodies this equals `bodyId`; for pattern clones it returns
     * the pattern feature's flat id so the UI can look up the
     * feature name even though `bodyId` is a unique hash.
     */
    bodySeedFeatureId(i: number): number;
    bodyTopologyJson(i: number): string;
    /**
     * K3.6: returns a 3-element `Float32Array` [dx, dy, dz] for
     * the i-th body's pending translation delta. Panics if the
     * body has no delta — guard with `bodyHasTransformDelta` first.
     */
    bodyTransformDelta(i: number): Float32Array;
    bodyTriangleCount(i: number): number;
    /**
     * Per-body version counter. Bumps only when the body's own
     * mesh was actually rebuilt — not when unrelated bodies
     * mutated around it.
     */
    bodyVersion(i: number): number;
    bodyVertexCount(i: number): number;
    bodyVolumeCount(i: number): number;
    bodyVolumeDataLen(i: number, vol_idx: number): number;
    bodyVolumeDataPtr(i: number, vol_idx: number): number;
    bodyVolumeDims(i: number, vol_idx: number): Uint32Array;
    canRedo(): boolean;
    canUndo(): boolean;
    classifyForStep(): string;
    /**
     * Clear the active body so every body runs at active LOD
     * again. Equivalent to `setActiveBodyId(0)`.
     */
    clearActiveBodyId(): boolean;
    contextMeshResolution(): number;
    /**
     * K10.6: detect B-Rep edges from the current mesh +
     * contributor analysis. Returns a JSON array of edge
     * segments with 3D coordinates + contributor IDs.
     */
    detectEdges(): string;
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
     */
    duplicateFeature(id: number): number;
    evalSdf(x: number, y: number, z: number): number;
    export3mf(): Uint8Array;
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
     */
    exportStlAscii(): string;
    /**
     * Export the current scene as binary STL. Returns a
     * JavaScript `Uint8Array` by virtue of wasm-bindgen's
     * auto-conversion for `Vec<u8>`.
     *
     * Same `&mut self` + sync-rebuild dance as
     * [`export_stl_ascii`].
     */
    exportStlBinary(): Uint8Array;
    featureCount(): number;
    static featureStride(): number;
    /**
     * K7.5: return the current `params` JSON string for a
     * feature call by id. Used by the Outliner's
     * FeatureDialog to initialise its edit buffer with the
     * feature's real values rather than a schema-default
     * blob.
     */
    getFeatureCallParams(id: number): string;
    getRollbackIndex(): number;
    /**
     * K6b.3a: fetch the sub-spine sub_calls for a sketch by
     * feature id, so the UI can reload an existing sketch when
     * the user re-enters it. Returns the JSON array string of
     * `FeatureCall`s or an error if the id doesn't point at a
     * sketch.
     */
    getSketchSubCalls(id: number): string;
    /**
     * K6a: list every feature type name the built-in registry
     * knows about, sorted for deterministic UI rendering. The
     * result is a JSON array of strings, e.g. `["box", "cylinder",
     * "extrude", "sphere"]`.
     */
    listFeatureTypes(): string;
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
     */
    listFeatures(): string;
    /**
     * K8.13: list the connected inside-regions of a sketch.
     * Returns a JSON array of `{index, centroid, aabb, area}`
     * objects. The sketch is identified by its feature id.
     */
    listSketchRegions(sketch_id: number, resolution: number): string;
    /**
     * K6b.2: list every sketch-tool type name the kernel knows
     * about. Sketch tools live in a separate
     * [`sketch_features::SketchToolRegistry`] that the UI's
     * sketch-mode toolbar queries to decide which tool buttons
     * to render. JSON array of strings, sorted alphabetically
     * for deterministic rendering.
     */
    listSketchToolTypes(): string;
    listVariables(): string;
    loadJson(json: string): number;
    /**
     * Number of `u32` slots reserved per vertex in the per-body
     * contributor flat buffer. Matches `csg::MAX_CONTRIBUTORS`.
     */
    static meshContributorStride(): number;
    /**
     * Sum of index counts across every body. O(bodies).
     */
    meshIndexCount(): number;
    /**
     * Voxel grid resolution for the active body. Aliased as
     * `meshResolution` for backward compatibility with callers
     * that don't care about LOD yet.
     */
    meshResolution(): number;
    /**
     * Sum of triangle counts across every body. O(bodies).
     */
    meshTriangleCount(): number;
    /**
     * Monotonic counter bumped every time the mesh is rebuilt. JS can
     * use it to debounce uploads ("only rebind the BufferGeometry when
     * this changes since the last frame").
     */
    meshVersion(): number;
    /**
     * Sum of vertex counts across every body. O(bodies).
     */
    meshVertexCount(): number;
    /**
     * Create a new kernel with an empty document.
     */
    constructor();
    /**
     * Add `(dx, dy, dz)` to a feature's translate. Convenience for
     * keyboard nudging.
     */
    nudgeFeature(id: number, dx: number, dy: number, dz: number): boolean;
    /**
     * Sphere-trace a ray through the current CSG tree and return a
     * JSON description of the first hit, if any. See `raycast.rs`
     * for the numerical scheme.
     */
    pick(ox: number, oy: number, oz: number, dx: number, dy: number, dz: number): string;
    primitivesPtr(): number;
    redo(): boolean;
    redoDepth(): number;
    /**
     * Remove the feature with the given id. Returns true if anything
     * was removed.
     */
    removeFeature(id: number): boolean;
    /**
     * K8.10: remove a document-level variable. No-op if the
     * name isn't in the table; returns `true` when it actually
     * removed something.
     *
     * K8.11: re-bootstraps after the remove so any formula
     * that referenced the departing variable surfaces as a
     * regen error (which rolls the remove back).
     */
    removeVariable(name: string): boolean;
    /**
     * Move a feature to a new index. Indices clamp into bounds.
     */
    reorderFeature(id: number, target_index: number): boolean;
    /**
     * K7.5: replace the `params` JSON of a feature call in
     * place and regenerate the spine. Keeps the feature id
     * stable so the outliner row doesn't jump, and re-runs
     * the registry so downstream cached state (body meshes,
     * packed buffer) reflects the new values.
     */
    replaceFeatureCallParams(id: number, params_json: string): void;
    /**
     * K6b.3b: replace an existing sketch's plane and sub-spine
     * in place, keeping its feature id stable. Used by the UI
     * when the user re-enters a committed sketch to edit it —
     * Finish should not create a new outliner row, it should
     * update the existing one. Fails fast if the target id
     * doesn't point at a sketch.
     */
    replaceSketch(id: number, plane_query: string, sub_calls_json: string): void;
    /**
     * K8.10: list every document-level variable as a JSON
     * array of `{name, value}` pairs, sorted by name (the
     * BTreeMap ordering).
     * K9.1: return the overall scene AABB as a JSON object
     * `{min: [x,y,z], max: [x,y,z], size: [w,h,d]}` in mm.
     * Empty scenes return all-zero extents.
     */
    sceneAabb(): string;
    /**
     * K6b.2: look up the FeatureSchema-generated JSON schema for
     * a sketch tool. Same contract as [`schema_for_type`] but
     * routes through the sketch-tool registry so the disjoint
     * namespaces stay disjoint.
     */
    schemaForSketchToolType(type_name: string): string;
    /**
     * K6a: look up the FeatureSchema-generated JSON schema for
     * a feature type. Returns the serialised JSON string or an
     * error if the type isn't registered.
     */
    schemaForType(type_name: string): string;
    /**
     * Convenience: set the active body to whichever body owns the
     * given feature. Returns true if the active body actually
     * changed. No-op (returns false) when the feature doesn't
     * exist or already lives in the active body.
     */
    setActiveBodyByFeature(feature_id: number): boolean;
    /**
     * Mark the given body id as the active one. Triggers a re-mesh
     * that promotes `body_id` to active resolution and demotes
     * whichever body was active before (if any) to context
     * resolution. Passing `0` clears the active body.
     */
    setActiveBodyId(body_id: number): boolean;
    /**
     * Change the context-body voxel grid resolution and re-mesh
     * every body that's currently running at context LOD. No-op
     * if unchanged or zero.
     */
    setContextMeshResolution(resolution: number): boolean;
    /**
     * Change the boolean op of an existing feature.
     */
    setFeatureOp(id: number, op: number): boolean;
    /**
     * K7.21h: replace the rotation component of a feature's transform.
     * Quaternion order: (x, y, z, w).
     */
    setFeatureRotation(id: number, rx: number, ry: number, rz: number, rw: number): boolean;
    setFeatureScale(id: number, sx: number, sy: number, sz: number): boolean;
    /**
     * K8.7: toggle a feature call's suppressed flag and
     * regenerate. A suppressed call is skipped by the
     * registry's regen loop, so its geometry drops out of the
     * CSG tree, but the call stays on `feature_calls` so the
     * user can unsuppress it later. Fails if the id doesn't
     * match any call or if regeneration errors out (e.g.
     * unsuppressing a call whose upstream dependency is now
     * broken).
     */
    setFeatureSuppressed(id: number, suppressed: boolean): void;
    /**
     * Replace the full transform (translate, quaternion, scale).
     */
    setFeatureTransform(id: number, tx: number, ty: number, tz: number, qx: number, qy: number, qz: number, qw: number, sx: number, sy: number, sz: number): boolean;
    /**
     * Replace the translate component of a feature's transform.
     */
    setFeatureTranslate(id: number, tx: number, ty: number, tz: number): boolean;
    /**
     * Change the active-body voxel grid resolution and re-mesh.
     * No-op if the resolution is unchanged or zero. Returns true
     * if the mesh was actually rebuilt. Aliased as
     * `setMeshResolution` for backward compatibility.
     */
    setMeshResolution(resolution: number): boolean;
    setRollbackIndex(index: number): void;
    /**
     * Enable or disable the synchronous DC pass. Toggling forces a
     * rebuild so the body list's mesh buffers match the new
     * setting: enabling meshes every body from scratch, disabling
     * clears every mesh to the default empty buffer. No-op if the
     * setting is already at the requested value.
     */
    setSyncMeshing(enabled: boolean): boolean;
    /**
     * K8.10: upsert a document-level variable. Snapshots so
     * undo/redo walks variable edits the same way it walks
     * feature edits.
     *
     * K8.11: after the write, re-bootstraps from calls so
     * every formula that references the variable picks up
     * the new value on the next mesh pass. Pre-K8.11 this
     * was a pure state edit with no geometry side effect.
     */
    setVariable(name: string, value: number): void;
    stepCommands(): string;
    /**
     * Whether this kernel instance runs the Dual Contouring pass
     * itself on every mutation. The frontend flips this off on the
     * main-thread kernel once the mesher web worker is up — the
     * worker owns its own kernel instance that meshes for real.
     */
    syncMeshing(): boolean;
    toJson(): string;
    undo(): boolean;
    undoDepth(): number;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_kernel_free: (a: number, b: number) => void;
    readonly kernel_activeBodyId: (a: number) => number;
    readonly kernel_addBoolean: (a: number) => [number, number, number];
    readonly kernel_addCircularPattern: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => [number, number, number];
    readonly kernel_addExtrude: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly kernel_addFilletEdge: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number) => [number, number, number];
    readonly kernel_addGyroid: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly kernel_addHole: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly kernel_addImportedMesh: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly kernel_addLinearPattern: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number, number];
    readonly kernel_addMateConnector: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number, number];
    readonly kernel_addMaterial: (a: number) => [number, number, number];
    readonly kernel_addMirror: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => [number, number, number];
    readonly kernel_addPrimitive: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [number, number, number];
    readonly kernel_addRevolve: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number];
    readonly kernel_addSketch: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly kernel_applyGizmoRotate: (a: number, b: number, c: number, d: number, e: number, f: number) => number;
    readonly kernel_applyGizmoScale: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly kernel_applyGizmoTranslate: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly kernel_bodyClassificationsPtr: (a: number, b: number) => number;
    readonly kernel_bodyContributorsPtr: (a: number, b: number) => number;
    readonly kernel_bodyCount: (a: number) => number;
    readonly kernel_bodyEdgeContributorA: (a: number, b: number, c: number) => number;
    readonly kernel_bodyEdgeContributorB: (a: number, b: number, c: number) => number;
    readonly kernel_bodyEdgeCount: (a: number, b: number) => number;
    readonly kernel_bodyEdgePointCount: (a: number, b: number, c: number) => number;
    readonly kernel_bodyEdgePointsPtr: (a: number, b: number, c: number) => number;
    readonly kernel_bodyFaceArea: (a: number, b: number, c: number) => number;
    readonly kernel_bodyFaceCentroid: (a: number, b: number, c: number) => [number, number];
    readonly kernel_bodyFaceContributor: (a: number, b: number, c: number) => number;
    readonly kernel_bodyFaceCount: (a: number, b: number) => number;
    readonly kernel_bodyFaceNormal: (a: number, b: number, c: number) => [number, number];
    readonly kernel_bodyFaceTriangleCount: (a: number, b: number, c: number) => number;
    readonly kernel_bodyFaceTrianglesPtr: (a: number, b: number, c: number) => number;
    readonly kernel_bodyHasTransformDelta: (a: number, b: number) => number;
    readonly kernel_bodyHybridInfoJson: (a: number, b: number) => [number, number];
    readonly kernel_bodyId: (a: number, b: number) => number;
    readonly kernel_bodyIndexCount: (a: number, b: number) => number;
    readonly kernel_bodyIndicesPtr: (a: number, b: number) => number;
    readonly kernel_bodyNormalsPtr: (a: number, b: number) => number;
    readonly kernel_bodyPositionsPtr: (a: number, b: number) => number;
    readonly kernel_bodySdfAabb: (a: number, b: number) => [number, number];
    readonly kernel_bodySdfOpcodesLen: (a: number, b: number) => number;
    readonly kernel_bodySdfOpcodesPtr: (a: number, b: number) => number;
    readonly kernel_bodySeedFeatureId: (a: number, b: number) => number;
    readonly kernel_bodyTopologyJson: (a: number, b: number) => [number, number];
    readonly kernel_bodyTransformDelta: (a: number, b: number) => [number, number];
    readonly kernel_bodyTriangleCount: (a: number, b: number) => number;
    readonly kernel_bodyVersion: (a: number, b: number) => number;
    readonly kernel_bodyVertexCount: (a: number, b: number) => number;
    readonly kernel_bodyVolumeCount: (a: number, b: number) => number;
    readonly kernel_bodyVolumeDataLen: (a: number, b: number, c: number) => number;
    readonly kernel_bodyVolumeDataPtr: (a: number, b: number, c: number) => number;
    readonly kernel_bodyVolumeDims: (a: number, b: number, c: number) => [number, number];
    readonly kernel_canRedo: (a: number) => number;
    readonly kernel_canUndo: (a: number) => number;
    readonly kernel_classifyForStep: (a: number) => [number, number];
    readonly kernel_clearActiveBodyId: (a: number) => number;
    readonly kernel_contextMeshResolution: (a: number) => number;
    readonly kernel_detectEdges: (a: number) => [number, number, number, number];
    readonly kernel_duplicateFeature: (a: number, b: number) => [number, number, number];
    readonly kernel_evalSdf: (a: number, b: number, c: number, d: number) => number;
    readonly kernel_export3mf: (a: number) => [number, number];
    readonly kernel_exportStlAscii: (a: number) => [number, number];
    readonly kernel_exportStlBinary: (a: number) => [number, number];
    readonly kernel_featureCount: (a: number) => number;
    readonly kernel_featureStride: () => number;
    readonly kernel_getFeatureCallParams: (a: number, b: number) => [number, number, number, number];
    readonly kernel_getRollbackIndex: (a: number) => number;
    readonly kernel_getSketchSubCalls: (a: number, b: number) => [number, number, number, number];
    readonly kernel_listFeatureTypes: (a: number) => [number, number, number, number];
    readonly kernel_listFeatures: (a: number) => [number, number, number, number];
    readonly kernel_listSketchRegions: (a: number, b: number, c: number) => [number, number, number, number];
    readonly kernel_listSketchToolTypes: (a: number) => [number, number, number, number];
    readonly kernel_listVariables: (a: number) => [number, number, number, number];
    readonly kernel_loadJson: (a: number, b: number, c: number) => [number, number, number];
    readonly kernel_meshContributorStride: () => number;
    readonly kernel_meshIndexCount: (a: number) => number;
    readonly kernel_meshResolution: (a: number) => number;
    readonly kernel_meshTriangleCount: (a: number) => number;
    readonly kernel_meshVersion: (a: number) => number;
    readonly kernel_meshVertexCount: (a: number) => number;
    readonly kernel_new: () => number;
    readonly kernel_nudgeFeature: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly kernel_pick: (a: number, b: number, c: number, d: number, e: number, f: number, g: number) => [number, number];
    readonly kernel_primitivesPtr: (a: number) => number;
    readonly kernel_redo: (a: number) => number;
    readonly kernel_redoDepth: (a: number) => number;
    readonly kernel_removeFeature: (a: number, b: number) => number;
    readonly kernel_removeVariable: (a: number, b: number, c: number) => number;
    readonly kernel_reorderFeature: (a: number, b: number, c: number) => number;
    readonly kernel_replaceFeatureCallParams: (a: number, b: number, c: number, d: number) => [number, number];
    readonly kernel_replaceSketch: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number];
    readonly kernel_sceneAabb: (a: number) => [number, number, number, number];
    readonly kernel_schemaForSketchToolType: (a: number, b: number, c: number) => [number, number, number, number];
    readonly kernel_schemaForType: (a: number, b: number, c: number) => [number, number, number, number];
    readonly kernel_setActiveBodyByFeature: (a: number, b: number) => number;
    readonly kernel_setActiveBodyId: (a: number, b: number) => number;
    readonly kernel_setContextMeshResolution: (a: number, b: number) => number;
    readonly kernel_setFeatureOp: (a: number, b: number, c: number) => [number, number, number];
    readonly kernel_setFeatureRotation: (a: number, b: number, c: number, d: number, e: number, f: number) => number;
    readonly kernel_setFeatureScale: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly kernel_setFeatureSuppressed: (a: number, b: number, c: number) => [number, number];
    readonly kernel_setFeatureTransform: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number) => number;
    readonly kernel_setFeatureTranslate: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly kernel_setMeshResolution: (a: number, b: number) => number;
    readonly kernel_setRollbackIndex: (a: number, b: number) => void;
    readonly kernel_setSyncMeshing: (a: number, b: number) => number;
    readonly kernel_setVariable: (a: number, b: number, c: number, d: number) => [number, number];
    readonly kernel_stepCommands: (a: number) => [number, number];
    readonly kernel_syncMeshing: (a: number) => number;
    readonly kernel_toJson: (a: number) => [number, number, number, number];
    readonly kernel_undo: (a: number) => number;
    readonly kernel_undoDepth: (a: number) => number;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
