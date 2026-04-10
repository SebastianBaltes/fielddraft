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

export interface FeatureSummary {
  id: number;
  name: string;
  op: BoolOpName;
  kind: PrimitiveKindName;
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

  meshPositionsView(): Float32Array {
    const ptr = this.inner.meshPositionsPtr();
    const count = this.inner.meshVertexCount();
    return new Float32Array(this.memory.buffer, ptr, count * 3);
  }

  meshNormalsView(): Float32Array {
    const ptr = this.inner.meshNormalsPtr();
    const count = this.inner.meshVertexCount();
    return new Float32Array(this.memory.buffer, ptr, count * 3);
  }

  meshIndicesView(): Uint32Array {
    const ptr = this.inner.meshIndicesPtr();
    const count = this.inner.meshIndexCount();
    return new Uint32Array(this.memory.buffer, ptr, count);
  }

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

  meshClassificationsView(): Uint8Array {
    const ptr = this.inner.meshClassificationsPtr();
    const count = this.inner.meshVertexCount();
    return new Uint8Array(this.memory.buffer, ptr, count);
  }

  meshContributorsView(): Uint32Array {
    const ptr = this.inner.meshContributorsPtr();
    const count = this.inner.meshVertexCount();
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

  // ── outliner ────────────────────────────────────────────────────────

  listFeatures(): FeatureSummary[] {
    const json = this.inner.listFeatures();
    return JSON.parse(json) as FeatureSummary[];
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
