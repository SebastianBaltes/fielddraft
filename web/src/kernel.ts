/**
 * Thin TypeScript wrapper around the Rust WASM kernel.
 *
 * The central concern here is the **zero-copy memory bridge**: the kernel
 * owns a packed `Vec<f32>` of feature parameters in WASM linear memory;
 * JS reads it through a `Float32Array` view that aliases that same memory,
 * with no intermediate JSON serialization. The view must be re-acquired
 * whenever the underlying `Vec` could have been reallocated (every
 * mutating call), since WASM memory growth invalidates old views.
 */

// eslint-disable-next-line import/no-unresolved
import initWasm, { Kernel as WasmKernel } from "./wasm/fielddraft_kernel.js";

interface WasmExports {
  memory: WebAssembly.Memory;
}

/** 16 floats per feature in the packed buffer. Mirrors `FEATURE_STRIDE`
 *  in `crates/kernel/src/lib.rs`. Each feature occupies four `vec4f`s
 *  in shader land. */
export const FEATURE_STRIDE = 16;

/** Number of contributor id slots per mesh vertex. Mirrors
 *  `csg::MAX_CONTRIBUTORS` on the Rust side. */
export const MESH_CONTRIBUTOR_STRIDE = 4;

/** Sentinel written into unused contributor slots (= `u32::MAX`). */
export const CONTRIBUTOR_SENTINEL = 0xffffffff;

/** B-Rep proxy element classification returned by `pick()` and stored
 *  per mesh vertex. */
export type ProxyElement = "face" | "edge" | "vertex";

export interface PickHit {
  hit: true;
  point: [number, number, number];
  t: number;
  element: ProxyElement;
  contributors: number[];
}
export interface PickMiss {
  hit: false;
}
export type PickResult = PickHit | PickMiss;

/** Numeric tags for primitive kinds. Mirror `Primitive::KIND_*` in protocol. */
export const PrimitiveKind = {
  Box: 0,
  Sphere: 1,
  Cylinder: 2,
} as const;
export type PrimitiveKindTag = (typeof PrimitiveKind)[keyof typeof PrimitiveKind];

/** Numeric tags for boolean ops. Mirror `BoolOp::TAG_*` in protocol. */
export const BoolOpTag = {
  Union: 0,
  Difference: 1,
  Intersect: 2,
} as const;
export type BoolOpTagValue = (typeof BoolOpTag)[keyof typeof BoolOpTag];

export type BoolOpName = "union" | "difference" | "intersect";
export type PrimitiveKindName = "box" | "sphere" | "cylinder";
/** K6b.3a: the kernel's `listFeatures` can now return entries
 *  whose kind is a non-primitive feature type like "sketch" or
 *  "extrude". The outliner handles them generically by keying
 *  glyphs / labels off the string. */
export type FeatureKindName = PrimitiveKindName | "sketch" | "extrude" | "unknown";

export interface FeatureSummary {
  id: number;
  name: string;
  op: BoolOpName;
  kind: FeatureKindName;
  translate: [number, number, number];
  scale: number;
}

let wasmInitPromise: Promise<WasmExports> | null = null;

async function ensureInit(): Promise<WasmExports> {
  if (!wasmInitPromise) {
    wasmInitPromise = initWasm() as Promise<WasmExports>;
  }
  return wasmInitPromise;
}

/**
 * High-level kernel handle owned by the React app. Wraps the wasm-bindgen
 * `Kernel` class.
 */
export class Kernel {
  private readonly inner: WasmKernel;
  private readonly memory: WebAssembly.Memory;

  private constructor(inner: WasmKernel, memory: WebAssembly.Memory) {
    this.inner = inner;
    this.memory = memory;
  }

  static async create(): Promise<Kernel> {
    const exports = await ensureInit();
    if (!exports?.memory) {
      throw new Error("fielddraft kernel: wasm memory export not found");
    }
    const inner = new WasmKernel();
    return new Kernel(inner, exports.memory);
  }

  // ── packed buffer (zero-copy bridge) ────────────────────────────────

  /**
   * Re-acquire a view on the packed feature buffer. **Do not cache**
   * across mutations — wasm-bindgen may grow the `memory` buffer, which
   * invalidates any `Float32Array` aliasing it.
   */
  primitivesView(): Float32Array {
    const ptr = this.inner.primitivesPtr();
    const count = this.inner.featureCount();
    return new Float32Array(this.memory.buffer, ptr, count * FEATURE_STRIDE);
  }

  featureCount(): number {
    return this.inner.featureCount();
  }

  // ── mesh bridge (zero-copy) ─────────────────────────────────────────
  //
  // The kernel re-runs the Dual Contouring pass on every mutation and
  // stores positions / normals / indices as three flat `Vec`s. These
  // views alias those `Vec`s directly — **never cache them** across a
  // mutating call, because `Vec::push` can relocate the backing
  // allocation and invalidate the view's buffer/offset pair.

  // ── aggregate counts (sum over bodies) ──────────────────────────────
  //
  // The aggregate pointer buffer from K3 part 1 is gone — the viewport
  // iterates bodies via the per-body views below. These counts stay
  // around as cheap totals for status bars and tests.

  meshVertexCount(): number {
    return this.inner.meshVertexCount();
  }

  meshIndexCount(): number {
    return this.inner.meshIndexCount();
  }

  meshTriangleCount(): number {
    return this.inner.meshTriangleCount();
  }

  meshVersion(): number {
    return this.inner.meshVersion();
  }

  // ── per-body mesh bridge (K3 part 3) ───────────────────────────────
  //
  // Each body owns its own contiguous positions / normals / indices /
  // classifications / contributors buffers inside WASM linear memory.
  // `bodyCount()` changes only when bodies are added, removed or
  // merged; `bodyVersion(i)` bumps only when body `i` was actually
  // re-meshed, so the viewport can rebind exactly the bodies that
  // changed. All views alias the WASM buffers directly — never cache
  // them across a mutating call.

  bodyCount(): number {
    return this.inner.bodyCount();
  }

  bodyId(i: number): number {
    return this.inner.bodyId(i);
  }

  bodyVersion(i: number): number {
    return this.inner.bodyVersion(i);
  }

  bodyVertexCount(i: number): number {
    return this.inner.bodyVertexCount(i);
  }

  bodyIndexCount(i: number): number {
    return this.inner.bodyIndexCount(i);
  }

  bodyTriangleCount(i: number): number {
    return this.inner.bodyTriangleCount(i);
  }

  bodyPositionsView(i: number): Float32Array {
    const ptr = this.inner.bodyPositionsPtr(i);
    const count = this.inner.bodyVertexCount(i);
    return new Float32Array(this.memory.buffer, ptr, count * 3);
  }

  bodyNormalsView(i: number): Float32Array {
    const ptr = this.inner.bodyNormalsPtr(i);
    const count = this.inner.bodyVertexCount(i);
    return new Float32Array(this.memory.buffer, ptr, count * 3);
  }

  bodyIndicesView(i: number): Uint32Array {
    const ptr = this.inner.bodyIndicesPtr(i);
    const count = this.inner.bodyIndexCount(i);
    return new Uint32Array(this.memory.buffer, ptr, count);
  }

  bodyClassificationsView(i: number): Uint8Array {
    const ptr = this.inner.bodyClassificationsPtr(i);
    const count = this.inner.bodyVertexCount(i);
    return new Uint8Array(this.memory.buffer, ptr, count);
  }

  bodyContributorsView(i: number): Uint32Array {
    const ptr = this.inner.bodyContributorsPtr(i);
    const count = this.inner.bodyVertexCount(i);
    return new Uint32Array(this.memory.buffer, ptr, count * MESH_CONTRIBUTOR_STRIDE);
  }

  /**
   * Cast a world-space ray through the current CSG tree and return
   * the first surface hit, or `{ hit: false }` on a miss. Delegates
   * to the Rust kernel's sphere-tracing raycaster and parses its
   * compact JSON payload.
   */
  pick(
    origin: [number, number, number],
    direction: [number, number, number],
  ): PickResult {
    const json = this.inner.pick(
      origin[0],
      origin[1],
      origin[2],
      direction[0],
      direction[1],
      direction[2],
    );
    return JSON.parse(json) as PickResult;
  }

  meshResolution(): number {
    return this.inner.meshResolution();
  }

  setMeshResolution(resolution: number): boolean {
    return this.inner.setMeshResolution(resolution);
  }

  // ── LOD (K3 part 4) ─────────────────────────────────────────────────
  //
  // Context bodies — everything that isn't the body the user is
  // currently editing — run at a coarser voxel grid, so a single
  // context re-mesh (for example after a bridging Union that merges
  // two bodies) is a fraction of the cost of an active re-mesh.
  // `activeBodyId === 0` means "no LOD hint set" — every body runs
  // at active resolution, identical to the K3 part 1–3 behaviour.

  contextMeshResolution(): number {
    return this.inner.contextMeshResolution();
  }

  setContextMeshResolution(resolution: number): boolean {
    return this.inner.setContextMeshResolution(resolution);
  }

  activeBodyId(): number {
    return this.inner.activeBodyId();
  }

  /**
   * Mark the body with the given stable id as the active one.
   * Pass `0` (or call `clearActiveBodyId()`) to disable LOD again.
   */
  setActiveBodyId(bodyId: number): boolean {
    return this.inner.setActiveBodyId(bodyId);
  }

  clearActiveBodyId(): boolean {
    return this.inner.clearActiveBodyId();
  }

  /**
   * Convenience: look up whichever body owns the given feature id
   * and mark that body as active. No-op when the feature doesn't
   * exist or already lives in the active body.
   */
  setActiveBodyByFeature(featureId: number): boolean {
    return this.inner.setActiveBodyByFeature(featureId);
  }

  // ── sync meshing toggle (K3 part 5) ─────────────────────────────────
  //
  // The main-thread kernel flips this off once the mesher web worker
  // is ready. After that, mutations still update the document (and
  // the CSG tree used by `pick`) but skip the Dual Contouring pass
  // — a second kernel instance inside the worker owns the meshing.

  syncMeshing(): boolean {
    return this.inner.syncMeshing();
  }

  setSyncMeshing(enabled: boolean): boolean {
    return this.inner.setSyncMeshing(enabled);
  }

  // ── outliner ────────────────────────────────────────────────────────

  listFeatures(): FeatureSummary[] {
    const json = this.inner.listFeatures();
    return JSON.parse(json) as FeatureSummary[];
  }

  // ── K6a: feature-schema lookup ─────────────────────────────────────

  /**
   * List every feature type name the built-in registry knows
   * about. Sorted alphabetically by the Rust side so the result
   * is stable across calls and safe to key React lists on.
   */
  listFeatureTypes(): string[] {
    return JSON.parse(this.inner.listFeatureTypes()) as string[];
  }

  /**
   * Load the JSON schema a feature type declares via the Rust
   * `FeatureSchema` derive. The shape is
   * `{ type: string, params: Array<{ key, name, type, required }> }`
   * — see `FeatureSchemaJson` in `web/src/widgets/schema.ts`
   * for the TypeScript type.
   */
  schemaForType(typeName: string): unknown {
    return JSON.parse(this.inner.schemaForType(typeName));
  }

  /**
   * K6b.2: list every sketch-tool type name the kernel knows.
   * Sketch tools live in a registry disjoint from the top-level
   * feature registry; K6b.1 ships two built-ins (`sketch_point`,
   * `sketch_line`), K6b.3+ grows the set. Sorted alphabetically.
   */
  listSketchToolTypes(): string[] {
    return JSON.parse(this.inner.listSketchToolTypes()) as string[];
  }

  /**
   * K6b.2: load the schema for a single sketch tool. Same JSON
   * shape as [`schemaForType`] so the generic FeatureDialog can
   * render it without knowing whether it's a top-level feature
   * or a sketch-local tool.
   */
  schemaForSketchToolType(typeName: string): unknown {
    return JSON.parse(this.inner.schemaForSketchToolType(typeName));
  }

  /**
   * K6b.3a: commit a sketch from the UI session as a `sketch`
   * FeatureCall on the kernel's canonical wire list. `subCalls`
   * is a JSON-ready array of the sketch-tool FeatureCalls the
   * session accumulated; it's stringified here so the
   * wasm-bindgen surface can stay `String`-in, `u32`-out.
   * Returns the new feature id.
   */
  addSketch(
    planeQuery: string,
    subCalls: ReadonlyArray<{ type: string; params: Record<string, unknown> }>,
  ): number {
    // The kernel side expects FeatureCall JSON objects with
    // `type`, `id`, `params`. We don't have sketch-tool-level
    // ids yet — K6b.3a allocates synthetic strings so the
    // protocol's `FeatureId::FromStr` accepts them. These are
    // purely internal to the sketch sub-spine and don't collide
    // with the top-level document id space.
    const prepared = subCalls.map((c, i) => ({
      type: c.type,
      id: String(i + 1),
      params: c.params,
    }));
    return this.inner.addSketch(planeQuery, JSON.stringify(prepared));
  }

  /** K6b.3a: read back a sketch's sub-spine by feature id. */
  getSketchSubCalls(
    id: number,
  ): Array<{ type: string; id: string; params: Record<string, unknown> }> {
    const json = this.inner.getSketchSubCalls(id);
    return JSON.parse(json) as Array<{
      type: string;
      id: string;
      params: Record<string, unknown>;
    }>;
  }

  // ── mutations ───────────────────────────────────────────────────────

  addBox(
    op: BoolOpTagValue,
    translate: [number, number, number],
    halfExtents: [number, number, number],
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.Box,
      op,
      translate[0],
      translate[1],
      translate[2],
      halfExtents[0],
      halfExtents[1],
      halfExtents[2],
    );
  }

  addSphere(
    op: BoolOpTagValue,
    translate: [number, number, number],
    radius: number,
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.Sphere,
      op,
      translate[0],
      translate[1],
      translate[2],
      radius,
      0,
      0,
    );
  }

  addCylinder(
    op: BoolOpTagValue,
    translate: [number, number, number],
    radius: number,
    halfHeight: number,
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.Cylinder,
      op,
      translate[0],
      translate[1],
      translate[2],
      radius,
      halfHeight,
      0,
    );
  }

  removeFeature(id: number): boolean {
    return this.inner.removeFeature(id);
  }

  setFeatureOp(id: number, op: BoolOpTagValue): boolean {
    return this.inner.setFeatureOp(id, op);
  }

  reorderFeature(id: number, targetIndex: number): boolean {
    return this.inner.reorderFeature(id, targetIndex);
  }

  setFeatureTranslate(id: number, x: number, y: number, z: number): boolean {
    return this.inner.setFeatureTranslate(id, x, y, z);
  }

  nudgeFeature(id: number, dx: number, dy: number, dz: number): boolean {
    return this.inner.nudgeFeature(id, dx, dy, dz);
  }

  // ── undo / redo ─────────────────────────────────────────────────────

  undo(): boolean {
    return this.inner.undo();
  }

  redo(): boolean {
    return this.inner.redo();
  }

  canUndo(): boolean {
    return this.inner.canUndo();
  }

  canRedo(): boolean {
    return this.inner.canRedo();
  }

  // ── document I/O ────────────────────────────────────────────────────

  toJson(): string {
    return this.inner.toJson();
  }

  loadJson(json: string): number {
    return this.inner.loadJson(json);
  }

  // ── debug helper ────────────────────────────────────────────────────

  evalSdf(x: number, y: number, z: number): number {
    return this.inner.evalSdf(x, y, z);
  }
}
