/**
 * Outer API client (K4.5.6).
 *
 * The main-thread half of the §4.6 Dual-API shells. Wraps a
 * running Regeneration Worker (the K3 mesher worker promoted in
 * K4.5.5) in an async façade that mirrors `globalThis.fd`: every
 * call is serialised as an `fd_call` message, posted to the
 * worker, and resolved by a matching `fd_result` response.
 *
 * The correlation/promise plumbing lives in the generic
 * {@link CorrelatedWorkerChannel} (workerChannel.ts) — this module
 * only contributes the `fd_call`/`fd_result` wire mapping and the
 * typed probe methods (`version`, `ping`, `echo`) that the Inner
 * API stub currently implements. Real methods (`sketch`,
 * `extrude`, `query`, …) hang on this same client as the feature
 * dispatch path lands; no protocol change is needed.
 *
 * The class takes a `WorkerLike` in its constructor rather than
 * calling `new Worker(...)` itself, so tests can inject a mock
 * worker that runs the dispatcher inline. Production callers pass
 * the real `Worker` instance they already own (the one that's also
 * doing mesh rebuilds), so a single worker serves both jobs —
 * mesh snapshots *and* Inner-API round-trips share the same JS
 * runtime.
 */

import type { FdCallRequest, FdCallResponse } from "./workerRpc";
import {
  CorrelatedWorkerChannel,
  type ChannelEvent,
  type WorkerLike,
} from "./workerChannel";

/** Map raw worker messages onto channel events; non-`fd_result`
 *  traffic (mesh rebuild responses on the shared worker) is ignored. */
function parseFdResult(data: unknown): ChannelEvent<unknown> | null {
  if (!data || typeof data !== "object") return null;
  const rec = data as Record<string, unknown>;
  if (rec.type !== "fd_result" || typeof rec.requestId !== "number") {
    return null;
  }
  const response = rec as unknown as FdCallResponse;
  return response.ok
    ? { kind: "result", requestId: response.requestId, value: response.value }
    : { kind: "error", requestId: response.requestId, error: response.error };
}

/**
 * Async proxy for `globalThis.fd` running in a Regeneration
 * Worker. One instance per worker; requests are correlated by
 * requestId so multiple in-flight calls can resolve in any order.
 */
export class OuterApiClient {
  private readonly channel: CorrelatedWorkerChannel<unknown>;

  constructor(worker: WorkerLike) {
    this.channel = new CorrelatedWorkerChannel(
      worker,
      parseFdResult,
      "OuterApiClient",
    );
  }

  /**
   * Release the worker listener. Callers that own the worker
   * should call this before tearing the worker down so the
   * client doesn't leak a reference. Outstanding pending calls
   * are rejected with an explicit cancellation error.
   */
  dispose(): void {
    this.channel.dispose();
  }

  /** Round-trip the Inner API version marker. */
  version(): Promise<string> {
    return this.call("version", []) as Promise<string>;
  }

  /** Round-trip probe — returns `"pong: <msg>"`. */
  ping(msg: string): Promise<string> {
    return this.call("ping", [msg]) as Promise<string>;
  }

  /**
   * Structured-clone round-trip probe. The return type mirrors
   * the argument type so TS callers see the expected shape even
   * though the call actually flows through `unknown` on the wire.
   */
  echo<T>(value: T): Promise<T> {
    return this.call("echo", [value]) as Promise<T>;
  }

  /**
   * Low-level method for tests and future slices. Posts an
   * `fd_call` and returns a promise that resolves to whatever
   * the Inner API method returned (or rejects with the worker's
   * error message). Not `private` so the test harness can
   * drive it directly.
   */
  call(method: string, args: unknown[]): Promise<unknown> {
    return this.channel.call((requestId) => {
      const request: FdCallRequest = {
        type: "fd_call",
        requestId,
        method,
        args,
      };
      return { message: request };
    });
  }
}
