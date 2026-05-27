/// <reference lib="webworker" />
/**
 * Dedicated mesher worker.
 *
 * Owns a second WASM kernel instance (completely independent of the
 * main-thread kernel) and runs the Dual Contouring pass off-thread.
 * The main thread keeps its own kernel for document state, undo
 * history and picking — it only needs to deliver the current
 * document JSON to this worker whenever it wants a fresh mesh.
 *
 * Wire format:
 *
 * ```
 * main → worker : { type: "rebuild",
 *                   requestId,
 *                   documentJson,
 *                   activeMeshResolution,
 *                   contextMeshResolution,
 *                   activeBodyId }
 *
 * worker → main : { type: "bodies",
 *                   requestId,
 *                   bodies: [{
 *                     id, version,
 *                     positions, normals, indices,
 *                     classifications, contributors,
 *                   }, ...] }
 * ```
 *
 * Every TypedArray in the response is transferred via the
 * structured clone transfer list, so ownership of the underlying
 * ArrayBuffer moves to the main thread in O(1) — no copy, no
 * post-hoc invalidation. The worker can't touch its own copy of
 * those buffers after posting; it builds a fresh set on the next
 * rebuild.
 */

// eslint-disable-next-line import/no-unresolved
import initWasm, { Kernel as WasmKernel } from "./wasm/fielddraft_kernel.js";
import { MESH_CONTRIBUTOR_STRIDE } from "./kernel";
import {
  freezeGlobals,
  hashDocumentJson,
  installInnerApi,
  reseedRandom,
  type FdInnerApi,
  type SandboxScope,
} from "./workerSandbox";
import {
  handleFdCallRequest,
  isFdCallRequest,
  type FdCallResponse,
} from "./workerRpc";

interface WasmExports {
  memory: WebAssembly.Memory;
}

interface ReadyKernel {
  kernel: WasmKernel;
  memory: WebAssembly.Memory;
}

export interface MesherBodyPayload {
  id: number;
  seedId: number;
  version: number;
  positions: Float32Array;
  normals: Float32Array;
  indices: Uint32Array;
  classifications: Uint8Array;
  contributors: Uint32Array;
  transformDelta: [number, number, number] | null;
  topologyJson: string;
}

export interface MesherRebuildRequest {
  type: "rebuild";
  requestId: number;
  documentJson: string;
  activeMeshResolution: number;
  contextMeshResolution: number;
  /** Stable body id to treat as "active" for LOD, or `0` for none. */
  activeBodyId: number;
}

export interface MesherBodiesResponse {
  type: "bodies";
  requestId: number;
  bodies: MesherBodyPayload[];
}

export type MesherRequest = MesherRebuildRequest;
export type MesherResponse = MesherBodiesResponse;

let readyPromise: Promise<ReadyKernel> | null = null;

/**
 * The Inner API instance installed on `globalThis.fd` once
 * `ensureKernel` has finished booting. The `fd_call` message
 * handler below reads it synchronously, which is safe because the
 * worker's message loop never fires before the first `rebuild`
 * resolves — and the first `rebuild` itself awaits `ensureKernel`.
 * For extra safety the handler explicitly checks for `null` and
 * responds with an error if an `fd_call` somehow arrives before
 * boot is complete.
 */
let innerApi: FdInnerApi | null = null;

async function ensureKernel(): Promise<ReadyKernel> {
  if (!readyPromise) {
    readyPromise = (async () => {
      // wasm-bindgen's default init path calls `fetch` to load the
      // kernel binary, so we MUST drive the freeze *after* init
      // returns. Freezing earlier would strip `fetch` from under
      // the loader and break the worker on boot.
      const exports = (await initWasm()) as WasmExports;
      if (!exports?.memory) {
        throw new Error("fielddraft mesher worker: wasm memory export not found");
      }
      const kernel = new WasmKernel();
      // The worker kernel keeps sync meshing enabled — it's the
      // one actually doing the work. Default is already `true`,
      // but make it explicit so a future default flip on the
      // Rust side doesn't silently break the worker.
      if (!kernel.syncMeshing()) {
        kernel.setSyncMeshing(true);
      }

      // K4.5.5: from this point on, the worker's global scope is
      // the Regeneration Worker sandbox — no network, no timers,
      // frozen wall clock, and `globalThis.fd` is the Inner API
      // that feature bodies will call into as K4.5.6+ lands the
      // dispatch path. `reseedRandom` is deferred to each rebuild
      // so the PRNG tracks the document state.
      const sandboxScope = self as unknown as SandboxScope;
      freezeGlobals(sandboxScope);
      innerApi = installInnerApi(sandboxScope);

      return { kernel, memory: exports.memory };
    })();
  }
  return readyPromise;
}

/**
 * Copy the per-body meshes out of WASM linear memory into owned
 * TypedArrays. Returns the body payload list plus a flat list of
 * backing ArrayBuffers for the postMessage transfer list.
 */
function snapshotBodies(
  kernel: WasmKernel,
  memory: WebAssembly.Memory,
): { bodies: MesherBodyPayload[]; transfers: ArrayBuffer[] } {
  const count = kernel.bodyCount();
  const bodies: MesherBodyPayload[] = [];
  const transfers: ArrayBuffer[] = [];
  for (let i = 0; i < count; i++) {
    const vertexCount = kernel.bodyVertexCount(i);
    const indexCount = kernel.bodyIndexCount(i);

    // Each view aliases WASM linear memory directly. `.slice()`
    // snapshots it into a fresh JS-owned ArrayBuffer, which is
    // what we hand to `postMessage` as a transferable. The
    // subsequent transfer detaches the buffer from the worker's
    // JS heap — which is fine because we never touch the payload
    // again after posting.
    const positions = new Float32Array(
      memory.buffer,
      kernel.bodyPositionsPtr(i),
      vertexCount * 3,
    ).slice();
    const normals = new Float32Array(
      memory.buffer,
      kernel.bodyNormalsPtr(i),
      vertexCount * 3,
    ).slice();
    const indices = new Uint32Array(
      memory.buffer,
      kernel.bodyIndicesPtr(i),
      indexCount,
    ).slice();
    const classifications = new Uint8Array(
      memory.buffer,
      kernel.bodyClassificationsPtr(i),
      vertexCount,
    ).slice();
    const contributors = new Uint32Array(
      memory.buffer,
      kernel.bodyContributorsPtr(i),
      vertexCount * MESH_CONTRIBUTOR_STRIDE,
    ).slice();

    let transformDelta: [number, number, number] | null = null;
    if (kernel.bodyHasTransformDelta(i)) {
      const d = kernel.bodyTransformDelta(i);
      transformDelta = [d[0], d[1], d[2]];
    }

    let topologyJson = '{"faces":[],"edges":[]}';
    try {
      topologyJson = kernel.bodyTopologyJson(i);
    } catch { /* topology extraction may fail for imported meshes */ }

    bodies.push({
      id: kernel.bodyId(i),
      seedId: kernel.bodySeedFeatureId(i),
      version: kernel.bodyVersion(i),
      positions,
      normals,
      indices,
      classifications,
      contributors,
      transformDelta,
      topologyJson,
    });

    transfers.push(
      positions.buffer,
      normals.buffer,
      indices.buffer,
      classifications.buffer,
      contributors.buffer,
    );
  }
  return { bodies, transfers };
}

async function handleRebuild(msg: MesherRebuildRequest): Promise<void> {
  const { kernel, memory } = await ensureKernel();

  // K4.5.5: reseed the PRNG at the start of every rebuild so that
  // any feature body consuming `Math.random` during this
  // regeneration draws from a sequence that's fully determined by
  // the current document state. Two regenerations of the same
  // document JSON hit the same seed and therefore observe the
  // same random sequence — the determinism invariant §4.6
  // formalises.
  reseedRandom(
    self as unknown as SandboxScope,
    hashDocumentJson(msg.documentJson),
  );

  // Load the document — this triggers the worker kernel's own
  // `rebuild_mesh_internal`, which walks the partition pass and
  // invokes Dual Contouring for every body whose payload hash
  // changed since the previous request. The kernel's internal
  // cache persists across messages, so unchanged bodies are
  // reused without re-meshing.
  kernel.loadJson(msg.documentJson);

  // Then apply the LOD knobs. Each setter short-circuits to a
  // no-op if the value matches the current setting and only
  // re-meshes when something actually changed.
  kernel.setMeshResolution(msg.activeMeshResolution);
  kernel.setContextMeshResolution(msg.contextMeshResolution);
  kernel.setActiveBodyId(msg.activeBodyId);

  const { bodies, transfers } = snapshotBodies(kernel, memory);
  const response: MesherBodiesResponse = {
    type: "bodies",
    requestId: msg.requestId,
    bodies,
  };
  // `self.postMessage(message, transferables)` inside a dedicated
  // worker — the triple-slash reference at the top of the file
  // pulls in the worker lib so TS knows this signature.
  self.postMessage(response, transfers);
}

async function handleFdCall(
  msg: import("./workerRpc").FdCallRequest,
): Promise<void> {
  // Awaiting `ensureKernel` here guarantees the Inner API has
  // been installed before we dispatch, regardless of whether
  // `fd_call` or `rebuild` is the first message the worker sees.
  await ensureKernel();
  const response: FdCallResponse = innerApi
    ? handleFdCallRequest(innerApi, msg)
    : {
        type: "fd_result",
        requestId: msg.requestId,
        ok: false,
        error: "fd Inner API not yet initialised",
      };
  self.postMessage(response);
}

self.addEventListener("message", (event: MessageEvent<unknown>) => {
  const msg = event.data;
  if (!msg || typeof msg !== "object") return;

  // K4.5.6: the worker now speaks two message types. Mesh
  // rebuilds stay on the K3 path; fd_call messages route the
  // Dual-API shell's Outer-→Inner round-trip through the pure
  // dispatcher in workerRpc.ts. Worker-side errors never throw
  // out of the message loop — they come back as structured
  // responses so the Outer API client can reject the pending
  // promise cleanly.
  if (isFdCallRequest(msg)) {
    handleFdCall(msg).catch((err) => {
      // eslint-disable-next-line no-console
      console.error("fielddraft mesher worker: fd_call failed", err);
      const errorResponse: FdCallResponse = {
        type: "fd_result",
        requestId: msg.requestId,
        ok: false,
        error: String(err),
      };
      self.postMessage(errorResponse);
    });
    return;
  }

  const rebuild = msg as MesherRequest;
  if (rebuild.type !== "rebuild") return;
  handleRebuild(rebuild).catch((err) => {
    // eslint-disable-next-line no-console
    console.error("fielddraft mesher worker: rebuild failed", err);
    const errorResponse: MesherBodiesResponse = {
      type: "bodies",
      requestId: (rebuild as MesherRebuildRequest).requestId,
      bodies: [],
    };
    self.postMessage(errorResponse);
  });
});
