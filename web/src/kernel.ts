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

/** 8 floats per feature in the packed buffer. Mirrors `FEATURE_STRIDE` in
 *  `crates/kernel/src/lib.rs`. */
export const FEATURE_STRIDE = 8;

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

  // ── outliner ────────────────────────────────────────────────────────

  listFeatures(): FeatureSummary[] {
    const json = this.inner.listFeatures();
    return JSON.parse(json) as FeatureSummary[];
  }

  // ── mutations ───────────────────────────────────────────────────────

  addBox(
    op: BoolOpTagValue,
    center: [number, number, number],
    halfExtents: [number, number, number],
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.Box,
      op,
      center[0],
      center[1],
      center[2],
      halfExtents[0],
      halfExtents[1],
      halfExtents[2],
    );
  }

  addSphere(
    op: BoolOpTagValue,
    center: [number, number, number],
    radius: number,
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.Sphere,
      op,
      center[0],
      center[1],
      center[2],
      radius,
      0,
      0,
    );
  }

  addCylinder(
    op: BoolOpTagValue,
    center: [number, number, number],
    radius: number,
    halfHeight: number,
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.Cylinder,
      op,
      center[0],
      center[1],
      center[2],
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
