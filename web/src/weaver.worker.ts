/// <reference lib="webworker" />
/**
 * Dedicated B-Rep weaver worker.
 *
 * Owns a third WASM kernel instance (independent of both the main-thread
 * kernel and the mesher worker) whose only job is to run the analytic
 * B-Rep weave OFF the main thread. The main kernel keeps its B-Rep
 * exports permanently suppressed (cache-or-empty) and, after every
 * commit, posts the stale body ids here; the woven results travel back
 * as MessagePack `(key, BRep)` buffers and are installed into the main
 * kernel via `importBodyBrep` — validated against the placement-invariant
 * cache key, so a result that raced a newer edit simply misses.
 *
 * Wire format:
 *
 * ```
 * main → worker : { type: "weave",
 *                   requestId,
 *                   documentJson,
 *                   parametricWeave,
 *                   bodyIds: number[] }
 *
 * worker → main : { type: "breps",
 *                   requestId,
 *                   items: [{ id, bytes: Uint8Array }, ...] }
 * ```
 *
 * Each `bytes` buffer is transferred (zero-copy). Bodies are woven and
 * RESPONDED ONE REQUEST AT A TIME — the store coalesces rapid-fire
 * schedules and always posts the most recent document, mirroring the
 * mesher worker's latest-wins protocol.
 *
 * Meshing stays disabled here (`setSyncMeshing(false)`) — this kernel
 * never ships render geometry, so `loadJson` skips the Dual Contouring
 * pass entirely. B-Rep export suppression is irrelevant: `weaveBodyBrep`
 * weaves unconditionally (it bypasses the export gate).
 */

import initWasm, {
  Kernel as WasmKernel,
  // eslint-disable-next-line import/no-unresolved
  fdOpCommit,
  // eslint-disable-next-line import/no-unresolved
  fdResolveQuery,
  // eslint-disable-next-line import/no-unresolved
  installScriptRunner,
} from "./wasm/fielddraft_kernel.js";
import {
  makeScriptHost,
  scriptRecordsFrom,
  type QueryHandle,
} from "./workerScriptHost";
import {
  freezeGlobals,
  hashDocumentJson,
  reseedRandom,
  type SandboxScope,
} from "./workerSandbox";

export interface WeaveRequest {
  type: "weave";
  requestId: number;
  documentJson: string;
  parametricWeave: boolean;
  /** Stable body ids (kernel `bodyId`) to weave. */
  bodyIds: number[];
}

export interface WeaveBrepPayload {
  id: number;
  /** MessagePack `(placement-invariant key, BRep)` from `weaveBodyBrep`. */
  bytes: Uint8Array;
}

export interface WeaveBrepsResponse {
  type: "breps";
  requestId: number;
  items: WeaveBrepPayload[];
}

let readyPromise: Promise<WasmKernel> | null = null;
let scriptHost: ReturnType<typeof makeScriptHost> | null = null;

async function ensureKernel(): Promise<WasmKernel> {
  if (!readyPromise) {
    readyPromise = (async () => {
      await initWasm();
      const kernel = new WasmKernel();
      // No render geometry leaves this worker — skip the DC pass on load.
      kernel.setSyncMeshing(false);
      // And no eager face-tagging weave either: `weaveBodyBrep` is the
      // only weave entry point, on exactly the bodies the main thread
      // asked for.
      kernel.setBrepSuppressed(true);

      // Same regeneration sandbox as the mesher worker: documents with
      // FieldSkript features regenerate during `loadJson`, which calls
      // back into the script host synchronously.
      const sandboxScope = self as unknown as SandboxScope;
      freezeGlobals(sandboxScope);
      scriptHost = makeScriptHost({
        commit: (id, typeName, defJson) =>
          JSON.parse(fdOpCommit(id, typeName, defJson)) as string[],
        resolveQuery: (query) =>
          JSON.parse(fdResolveQuery(query)) as QueryHandle[],
      });
      installScriptRunner((scriptId: string, instanceId: string, paramsJson: string) =>
        scriptHost!.run(scriptId, instanceId, paramsJson),
      );

      return kernel;
    })();
  }
  return readyPromise;
}

async function handleWeave(msg: WeaveRequest): Promise<void> {
  const kernel = await ensureKernel();

  reseedRandom(
    self as unknown as SandboxScope,
    hashDocumentJson(msg.documentJson),
  );
  scriptHost?.sync(scriptRecordsFrom(msg.documentJson));

  kernel.loadJson(msg.documentJson);
  kernel.setParametricWeave(msg.parametricWeave);

  const items: WeaveBrepPayload[] = [];
  const transfers: ArrayBuffer[] = [];
  for (const id of msg.bodyIds) {
    // `weaveBodyBrep` returns a fresh JS-owned Uint8Array (wasm-bindgen
    // copies Vec<u8> out of linear memory), so its buffer is directly
    // transferable. Empty buffer = unknown id (document changed under
    // the request) — skip, the next schedule supersedes.
    const bytes = kernel.weaveBodyBrep(id) as Uint8Array<ArrayBuffer>;
    if (bytes.length === 0) continue;
    items.push({ id, bytes });
    transfers.push(bytes.buffer);
  }

  const response: WeaveBrepsResponse = {
    type: "breps",
    requestId: msg.requestId,
    items,
  };
  self.postMessage(response, transfers);
}

self.addEventListener("message", (event: MessageEvent<unknown>) => {
  const msg = event.data as WeaveRequest | null;
  if (!msg || typeof msg !== "object" || msg.type !== "weave") return;
  handleWeave(msg).catch((err) => {
    // eslint-disable-next-line no-console
    console.error("fielddraft weaver worker: weave failed", err);
    // Always answer — the store's inflight flag must clear even on
    // failure, or weaving would wedge for the rest of the session.
    const response: WeaveBrepsResponse = {
      type: "breps",
      requestId: msg.requestId,
      items: [],
    };
    self.postMessage(response);
  });
});
