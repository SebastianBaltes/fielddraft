/**
 * Thin TypeScript wrapper around the Rust WASM kernel.
 *
 * The central concern here is the **zero-copy memory bridge**: the kernel
 * owns a packed `Vec<f32>` of primitive parameters in WASM linear memory;
 * JS reads it through a `Float32Array` view that aliases that same memory,
 * with no intermediate JSON serialization. The view must be re-acquired
 * whenever the underlying `Vec` could have been reallocated (e.g. after a
 * document load), since WASM memory growth invalidates old views.
 */

// eslint-disable-next-line import/no-unresolved
import initWasm, { Kernel as WasmKernel } from "./wasm/fielddraft_kernel.js";

/** Shape of the wasm-pack init() return: the instance's exports object. */
interface WasmExports {
  memory: WebAssembly.Memory;
}

export const PRIMITIVE_STRIDE = 8;

/** Kinds must match `KIND_*` constants in `crates/kernel/src/lib.rs`. */
export const PrimitiveKind = {
  Box: 0,
  Sphere: 1,
  Cylinder: 2,
} as const;

export type Primitive =
  | { kind: "box"; center: [number, number, number]; half_extents: [number, number, number] }
  | { kind: "sphere"; center: [number, number, number]; radius: number }
  | {
      kind: "cylinder";
      center: [number, number, number];
      radius: number;
      half_height: number;
    };

export interface Document {
  schema_version: number;
  name: string;
  primitives: Primitive[];
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
 * `Kernel` class and re-projects its packed primitive buffer as a
 * `Float32Array` view on demand.
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

  /**
   * Re-acquire a view on the packed primitive buffer. **Do not cache** across
   * document mutations — wasm-bindgen may grow the `memory` buffer, which
   * invalidates any `Float32Array` aliasing it.
   */
  primitivesView(): Float32Array {
    const ptr = this.inner.primitivesPtr();
    const count = this.inner.primitiveCount();
    return new Float32Array(this.memory.buffer, ptr, count * PRIMITIVE_STRIDE);
  }

  /** Number of primitives in the current document. */
  primitiveCount(): number {
    return this.inner.primitiveCount();
  }

  /** Serialize the current document to a JSON string. */
  toJson(): string {
    return this.inner.toJson();
  }

  /**
   * Replace the current document with one parsed from JSON. Returns the new
   * primitive count. Invalidates any previously-held `primitivesView()`.
   */
  loadJson(json: string): number {
    return this.inner.loadJson(json);
  }

  /** Debug helper: evaluate the composite SDF at a point. */
  evalSdf(x: number, y: number, z: number): number {
    return this.inner.evalSdf(x, y, z);
  }
}
