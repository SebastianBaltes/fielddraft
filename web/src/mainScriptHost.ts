/**
 * FieldSkript build host — main-thread instance.
 *
 * The mesher worker is not the only realm that regenerates a scripted
 * document. The main-thread kernel (held by the store) re-runs the feature
 * spine synchronously on every mutation — it needs the spine for the AABB,
 * picking/selection, the outliner and undo. That regenerate walks the same
 * `ScriptFeature::execute` path the worker does, and when it reaches a
 * scripted instance it calls back into JS through the Rust
 * `installScriptRunner` import. Without a runner installed on *this* thread
 * the call fails with `NoRunner`.
 *
 * So we stand up the SAME {@link makeScriptHost} here as the worker does.
 * The crucial point is that `build()` is cheap on the main thread: it only
 * emits lightweight `(id, type_name, defJson)` op-descriptors via
 * `fdOpCommit` — the heavy Dual Contouring meshing stays in the worker. The
 * free wasm exports the host needs (`fdOpCommit`, `fdResolveQuery`,
 * `installScriptRunner`) are global functions touching a thread_local
 * scratch, NOT the `Kernel` object, so they are callable on the main thread
 * exactly as in the worker — including re-entrantly while `regenerate` holds
 * the Kernel borrow.
 *
 * Wiring mirrors {@link ./mesher.worker.ts} `ensureKernel`: build the host
 * once after wasm init, register it as the runner, and refresh its compiled
 * `build()` table whenever the document's scripts change (load / upsert /
 * remove) so a later instance regenerate sees the right bodies.
 */

import {
  // eslint-disable-next-line import/no-unresolved
  fdOpCommit,
  // eslint-disable-next-line import/no-unresolved
  fdResolveQuery,
  // eslint-disable-next-line import/no-unresolved
  installScriptRunner,
} from "./wasm/fielddraft_kernel.js";
import {
  makeScriptHost,
  type ScriptRecordLite,
  type QueryHandle,
} from "./workerScriptHost";

/**
 * The singleton main-thread build host. Installed once at kernel boot and
 * kept for the page's lifetime; only its compiled `build()` table is
 * refreshed as scripts change.
 */
let host: ReturnType<typeof makeScriptHost> | null = null;

/**
 * Stand up the build host and register it as the kernel's script runner.
 * Idempotent — a second call is a no-op. MUST be called only after wasm
 * init has completed, since it imports the free wasm exports.
 */
export function installMainScriptHost(): void {
  if (host) return;
  host = makeScriptHost({
    commit: (id, typeName, defJson) =>
      JSON.parse(fdOpCommit(id, typeName, defJson)) as string[],
    resolveQuery: (query) =>
      JSON.parse(fdResolveQuery(query)) as QueryHandle[],
  });
  installScriptRunner(
    (scriptId: string, instanceId: string, paramsJson: string) =>
      host!.run(scriptId, instanceId, paramsJson),
  );
}

/**
 * Refresh the host's compiled `build()` table from the current document's
 * scripts. A no-op before {@link installMainScriptHost} ran.
 */
export function syncMainScriptHost(scripts: ScriptRecordLite[]): void {
  host?.sync(scripts);
}
