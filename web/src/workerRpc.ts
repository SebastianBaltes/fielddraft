/**
 * Dual-API-Shell RPC protocol (K4.5.6).
 *
 * The §4.6 architecture describes two parallel shells over the
 * same Inner API that the Regeneration Worker hosts on
 * `globalThis.fd`:
 *
 *   * The **Inner API** is the API itself — feature bodies that run
 *     inside the worker call `fd.extrude(...)` directly as a
 *     synchronous WASM-binding invocation, no message passing
 *     involved.
 *   * The **Outer API** is an async wrapper that lives in the main
 *     thread. Every main-thread caller (React components,
 *     Playwright E2E tests, future CLI harnesses) calls
 *     `await outerFd.extrude(...)`; the wrapper serialises the
 *     call, posts it to the worker, and awaits the response.
 *
 * This module defines the wire protocol that the Outer API client
 * and the worker-side dispatcher share, plus the pure dispatcher
 * function that the worker uses to route an incoming `fd_call`
 * message back to the Inner API. Both halves are pure (no Worker
 * dependencies), so they unit-test cleanly in vitest.
 *
 * K4.5.6 lands the shell — no real feature methods are routed
 * yet; the testable surface is `version` / `log` / `ping` / `echo`
 * from K4.5.5's Inner API stub. As future slices hang real
 * methods on `globalThis.fd`, they become wire-reachable from the
 * Outer API without any protocol change, because dispatch is
 * method-name + args indirection.
 */

import type { FdInnerApi } from "./workerSandbox";

// ─── wire types ────────────────────────────────────────────────

/** Main-thread → worker: invoke a method on the Inner API. */
export interface FdCallRequest {
  type: "fd_call";
  /** Correlates the response with the calling `OuterApiClient.call` promise. */
  requestId: number;
  /**
   * Name of the method to invoke on `globalThis.fd`. Must refer
   * to a callable property — non-function properties like
   * `version` can still be reached, but the dispatcher treats
   * them as zero-arg getters (see `dispatchFdCall`).
   */
  method: string;
  /**
   * Positional arguments, already structured-cloneable. The Outer
   * API doesn't do any wrapping — whatever the caller passes goes
   * through as-is, subject to postMessage's structured clone
   * constraints.
   */
  args: unknown[];
}

/** Worker → main-thread: result of an `fd_call`. */
export type FdCallResponse =
  | {
      type: "fd_result";
      requestId: number;
      ok: true;
      value: unknown;
    }
  | {
      type: "fd_result";
      requestId: number;
      ok: false;
      /**
       * Stringified error message. We intentionally do not
       * propagate the raw Error object across the wire — class
       * identity doesn't survive structured clone reliably and
       * the stack would leak worker-scoped symbol names to the
       * main thread.
       */
      error: string;
    };

/** Sentinel used by the worker-side handler to tag incoming messages. */
export function isFdCallRequest(value: unknown): value is FdCallRequest {
  if (!value || typeof value !== "object") return false;
  const rec = value as Record<string, unknown>;
  return (
    rec.type === "fd_call" &&
    typeof rec.requestId === "number" &&
    typeof rec.method === "string" &&
    Array.isArray(rec.args)
  );
}

// ─── dispatcher ────────────────────────────────────────────────

/**
 * Route an `fd_call` message to the Inner API.
 *
 * Three outcomes are possible:
 *   * The method exists and is a function → call it with the
 *     provided args, return the result.
 *   * The method exists but is a non-function property (e.g.
 *     `version`, `logs`) → return the property value as if it
 *     were a zero-arg getter. Ignore any extra args.
 *   * The method doesn't exist → throw an Error the caller can
 *     convert to an `fd_result` with `ok: false`.
 *
 * Any error thrown by the Inner API method bubbles up unchanged;
 * the worker's top-level handler catches it, converts to a
 * stringified error, and posts an `ok: false` response.
 */
export function dispatchFdCall(
  api: FdInnerApi,
  method: string,
  args: unknown[],
): unknown {
  // We treat the API as a loose record so we can index it by
  // method name. The strongly typed `FdInnerApi` interface is the
  // contract between authors of `globalThis.fd` and the Inner API
  // consumers; the dispatcher sits one level underneath that and
  // is intentionally structural.
  const bag = api as unknown as Record<string, unknown>;
  if (!(method in bag)) {
    throw new Error(`unknown fd method: ${method}`);
  }
  const slot = bag[method];
  if (typeof slot === "function") {
    return (slot as (...a: unknown[]) => unknown).apply(api, args);
  }
  // Non-function slot — treat as a zero-arg getter. This lets the
  // Outer API read `version` via the same `call("version", [])`
  // idiom as any method, so the client doesn't need two code
  // paths for methods vs properties.
  return slot;
}

/**
 * Build an `FdCallResponse` from the result of `dispatchFdCall`.
 * Wraps the success / failure decision in one helper so the
 * worker's message handler stays short.
 */
export function handleFdCallRequest(
  api: FdInnerApi,
  request: FdCallRequest,
): FdCallResponse {
  try {
    const value = dispatchFdCall(api, request.method, request.args);
    return {
      type: "fd_result",
      requestId: request.requestId,
      ok: true,
      value,
    };
  } catch (err) {
    return {
      type: "fd_result",
      requestId: request.requestId,
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}
