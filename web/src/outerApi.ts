/**
 * Outer API client (K4.5.6).
 *
 * The main-thread half of the §4.6 Dual-API shells. Wraps a
 * running Regeneration Worker (the K3 mesher worker promoted in
 * K4.5.5) in an async façade that mirrors `globalThis.fd`: every
 * call is serialised as an `fd_call` message, posted to the
 * worker, and resolved by a matching `fd_result` response.
 *
 * The client is deliberately narrow in K4.5.6 — it exposes only
 * the probe methods (`version`, `ping`, `echo`) that the Inner
 * API stub currently implements. Real methods (`sketch`,
 * `extrude`, `query`, …) hang on this same client as K4.5.8 lands
 * the feature dispatch path; no protocol change is needed.
 *
 * The class takes a `WorkerLike` in its constructor rather than
 * calling `new Worker(...)` itself, so tests can inject a mock
 * worker that runs the dispatcher inline. Production callers in
 * the React store pass the real `Worker` instance they already
 * own (the one that's also doing mesh rebuilds), so a single
 * worker serves both jobs — mesh snapshots *and* Inner-API
 * round-trips share the same JS runtime.
 */

import type { FdCallRequest, FdCallResponse } from "./workerRpc";

/**
 * Minimal view of a Web Worker that the client actually touches.
 * Narrower than the DOM `Worker` type so tests can supply mocks
 * without implementing `terminate`, `onerror`, etc.
 */
export interface WorkerLike {
  postMessage(message: unknown): void;
  addEventListener(
    type: "message",
    listener: (event: MessageEvent<unknown>) => void,
  ): void;
  removeEventListener(
    type: "message",
    listener: (event: MessageEvent<unknown>) => void,
  ): void;
}

interface PendingCall {
  resolve: (value: unknown) => void;
  reject: (reason: Error) => void;
}

/**
 * Async proxy for `globalThis.fd` running in a Regeneration
 * Worker. One instance per worker; internally correlates
 * requests by a monotonic `requestId` so multiple in-flight
 * calls can resolve in any order.
 */
export class OuterApiClient {
  private nextRequestId = 1;
  private readonly pending = new Map<number, PendingCall>();
  private readonly messageListener = (event: MessageEvent<unknown>): void => {
    this.onMessage(event.data);
  };

  constructor(private readonly worker: WorkerLike) {
    worker.addEventListener("message", this.messageListener);
  }

  /**
   * Release the worker listener. Callers that own the worker
   * should call this before tearing the worker down so the
   * client doesn't leak a reference. Outstanding pending calls
   * are rejected with an explicit cancellation error.
   */
  dispose(): void {
    this.worker.removeEventListener("message", this.messageListener);
    for (const { reject } of this.pending.values()) {
      reject(new Error("OuterApiClient disposed with in-flight calls"));
    }
    this.pending.clear();
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
    const requestId = this.nextRequestId++;
    const request: FdCallRequest = {
      type: "fd_call",
      requestId,
      method,
      args,
    };
    return new Promise<unknown>((resolve, reject) => {
      this.pending.set(requestId, { resolve, reject });
      this.worker.postMessage(request);
    });
  }

  private onMessage(data: unknown): void {
    if (!data || typeof data !== "object") return;
    const rec = data as Record<string, unknown>;
    if (rec.type !== "fd_result") return;
    const response = rec as unknown as FdCallResponse;
    const entry = this.pending.get(response.requestId);
    if (!entry) {
      // Response for a request we don't know about — either a
      // stale message after dispose, or a bug in the worker.
      // Drop silently rather than throwing; throwing from a
      // message listener is hard to observe and would break
      // well-behaved callers.
      return;
    }
    this.pending.delete(response.requestId);
    if (response.ok) {
      entry.resolve(response.value);
    } else {
      entry.reject(new Error(response.error));
    }
  }
}
