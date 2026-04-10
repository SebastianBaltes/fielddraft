/**
 * Regeneration-Worker sandbox helpers (K4.5.5).
 *
 * The FieldDraft regeneration worker (physically the mesher worker
 * from K3, promoted to the §4.6 "Regeneration Worker" role in K4.5)
 * executes feature bodies in its own JavaScript runtime. For the
 * CAD determinism invariant to hold — "regenerating the same
 * feature tree twice produces bit-identical meshes" — the worker
 * must not expose non-deterministic or side-effectful globals to
 * user code. This module implements the two primitives that
 * enforce that contract:
 *
 *   * `freezeGlobals(scope)` strips network and timer APIs and
 *     clamps `Date.now` to a constant.
 *   * `reseedRandom(scope, seed)` replaces `Math.random` with a
 *     deterministic mulberry32 PRNG that only advances when
 *     callers draw from it.
 *
 * Both are pure functions that take a scope object as their first
 * argument, so they're trivially unit-testable against a mock
 * scope in vitest. The worker wires them to the real `self`/
 * `globalThis` at startup (`freezeGlobals`, `installInnerApi`) and
 * per-rebuild (`reseedRandom`, keyed on a hash of the current
 * document JSON, so the random sequence is stable for a given
 * document state).
 *
 * This is not a hard sandbox. A motivated attacker can still find
 * leaks via intrinsics or non-captured globals. The goal is the
 * determinism contract, not security — the README risk register
 * (§11 risk 5) spells out the trade-off explicitly.
 */

/** Minimal view of a globalThis-shaped object that the sandbox touches. */
export interface SandboxScope {
  fetch?: unknown;
  setTimeout?: unknown;
  setInterval?: unknown;
  clearTimeout?: unknown;
  clearInterval?: unknown;
  Date: DateConstructor;
  Math: Math;
  fd?: FdInnerApi;
}

/**
 * Shape of the `globalThis.fd` Inner API that feature bodies see at
 * runtime. K4.5.5 lands the injection hook and the deterministic
 * `log` sink. K4.5.6 adds two pure round-trip methods — `ping` and
 * `echo` — whose only job is to prove that the Outer → Inner RPC
 * plumbing transports arguments and return values correctly, so
 * the Dual-API shells can be landed and tested before real
 * feature dispatch (K4.5.8) lights up.
 *
 * Future slices will hang `sketch` / `extrude` / `query` / … on
 * this same object; the RPC dispatcher in `workerRpc.ts` routes
 * messages to any method that lives on `fd` by name, so new
 * methods become wire-reachable as soon as they're installed.
 */
export interface FdInnerApi {
  /** API version string — bumped when a breaking Inner-API change ships. */
  readonly version: string;
  /** Deterministic log sink. Accumulates into the `logs` array. */
  log: (...args: unknown[]) => void;
  /** Snapshot of every log call since the worker started, in order. */
  readonly logs: ReadonlyArray<ReadonlyArray<unknown>>;
  /**
   * Round-trip probe: returns `"pong: <msg>"`. Exists so the Outer
   * API client in the main thread can verify that arguments reach
   * the worker and a textual response comes back.
   */
  ping: (msg: string) => string;
  /**
   * Round-trip probe: returns its argument unchanged. Exists so
   * tests can verify that arbitrary structured-cloneable values
   * (numbers, arrays, nested objects) survive a round-trip
   * through the postMessage channel without shape drift.
   */
  echo: <T>(value: T) => T;
}

export const FD_INNER_API_VERSION = "0.1.0";

/**
 * Strip the non-deterministic / side-effectful globals from a scope.
 *
 * After this call, feature bodies running in the scope cannot:
 *   - make network requests (`fetch` is deleted)
 *   - schedule callbacks (`setTimeout` / `setInterval` are deleted,
 *     together with their paired clearers)
 *   - observe wall-clock time (`Date.now` always returns `0`; the
 *     `Date` constructor itself is *not* removed because some
 *     innocuous helpers use it for string formatting, but every
 *     temporal read is anchored to the epoch)
 *
 * The wired scope is returned so the worker can chain calls if it
 * wants to.
 */
export function freezeGlobals(scope: SandboxScope): SandboxScope {
  // `delete` is the right tool here: the properties become genuinely
  // absent, so a feature body doing `typeof fetch` sees
  // `"undefined"` and any direct call throws a clean ReferenceError
  // or TypeError. Setting them to `undefined` would leave the
  // property on the scope and give a confusing "fetch is not a
  // function" instead.
  const mutable = scope as unknown as Record<string, unknown>;
  delete mutable.fetch;
  delete mutable.setTimeout;
  delete mutable.setInterval;
  delete mutable.clearTimeout;
  delete mutable.clearInterval;

  // `Date.now` goes to a zero stub. We intentionally do not touch
  // `new Date(...)` — code that formats a date for display should
  // keep working, but anything that reads the clock gets a frozen
  // epoch timestamp. This matches the "all temporal reads return
  // 0" wording in §4.6.
  scope.Date.now = () => 0;

  return scope;
}

/**
 * Replace `scope.Math.random` with a mulberry32 PRNG seeded from
 * the given 32-bit seed. The seed can come from anywhere, but in
 * the worker it's typically `hashDocumentJson(documentJson)` so the
 * random sequence is stable for a given document state.
 *
 * Calling `reseedRandom` repeatedly is safe — each call installs a
 * fresh PRNG, so the worker can reseed at the start of every
 * regeneration without leaking state from the previous round.
 */
export function reseedRandom(scope: SandboxScope, seed: number): void {
  scope.Math.random = mulberry32(seed);
}

/**
 * Install the Inner API as `scope.fd`. K4.5.5 ships the skeleton —
 * a marker version string plus a deterministic log sink. Future
 * slices will hang real methods (`sketch`, `extrude`, `query`) on
 * this same object as the dispatch path lands.
 *
 * Returns the installed API so callers can hold onto it directly,
 * which is how the unit tests verify the log sink.
 */
export function installInnerApi(scope: SandboxScope): FdInnerApi {
  const logs: unknown[][] = [];
  const api: FdInnerApi = {
    version: FD_INNER_API_VERSION,
    log: (...args) => {
      logs.push(args);
    },
    logs,
    ping: (msg) => `pong: ${msg}`,
    echo: (value) => value,
  };
  scope.fd = api;
  return api;
}

/**
 * mulberry32 — a tiny 32-bit PRNG that the worker uses as its
 * `Math.random` replacement. Fine for non-crypto CAD determinism
 * needs and very cheap to reseed. Attribution: Tommy Ettinger's
 * public-domain implementation, ported straight to TypeScript.
 */
export function mulberry32(seed: number): () => number {
  let state = seed | 0;
  return () => {
    state = (state + 0x6d2b79f5) | 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * FNV-1a 32-bit hash of a string. Used to derive a PRNG seed from
 * a document's JSON form so the same document always produces the
 * same random sequence. Deterministic, cheap, and small enough to
 * fit in a 32-bit seed without truncation.
 */
export function hashDocumentJson(json: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < json.length; i++) {
    hash ^= json.charCodeAt(i);
    // The mul-by-prime step uses `Math.imul` for a real 32-bit
    // multiply; plain `*` in JS promotes to double and drops the
    // low bits over long strings.
    hash = Math.imul(hash, 0x01000193);
  }
  return hash >>> 0;
}
