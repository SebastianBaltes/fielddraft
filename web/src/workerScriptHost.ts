/**
 * FieldSkript build-time host (S2c).
 *
 * The worker realm where scripted features actually run. When the
 * kernel's `regenerate` reaches a {@link FieldSkriptRecord}'s feature it
 * calls back into JS through the Rust `installScriptRunner` import; this
 * module is what that callback drives. It:
 *
 *   1. compiles each record's `compiledJs` into a `build()` function
 *      ({@link compileBuild}), once per document, and
 *   2. runs the matching `build(context, id, def)` synchronously when
 *      asked ({@link makeScriptHost} `.run`), letting the body emit
 *      geometry via the `fd.op*` surface.
 *
 * **The op surface never re-implements a built-in** (concept #2): every
 * `fd.opBox(...)` just forwards a `(id, type_name, defJson)` descriptor
 * to the Rust `fdOpCommit`, which reuses the built-in's own `execute`.
 * The geometry logic stays a single Rust path; JS only names the op.
 *
 * **Compiled form contract.** `compiledJs` must be **CommonJS** — the
 * body is evaluated with `(require, module, exports)` and the `build()`
 * comes from `module.exports.default`. `require("fielddraft")` resolves
 * to the authoring + op module composed here; any other specifier is
 * rejected. Producing CJS from the editor's TypeScript is the save-time
 * transpiler's job (concept open decision #4); this host only consumes
 * it, so it can be unit-tested against CJS fixtures with no toolchain.
 */

import {
  feature,
  length,
  angle,
  real,
  bool,
  str,
  vec2,
  vec3,
  enumParam,
  query,
  group,
  visibleWhen,
  fd as authoringFd,
} from "./fieldskript/dsl";

/** A handle to geometry an `fd.op*` call committed, queryable later. */
export interface OpHandle {
  readonly id: string;
}

/** A resolved B-Rep element handle returned by `fd.q.*`. */
export interface QueryHandle {
  readonly kind: "face" | "edge" | "vertex";
  /** Stable element id (u64) as a decimal string. */
  readonly idHash: string;
  /** Feature ids that the query referenced. */
  readonly contributors: readonly string[];
}

/**
 * The hierarchical build id handed to `build()`. `sub(name)` extends it
 * for a sub-operation (`"10"` → `"10.bore"`), matching the kernel's
 * dotted {@link FeatureId} form so a committed op lands under its
 * instance.
 */
export interface BuildId {
  sub(name: string): BuildId;
  toString(): string;
}

/** The opaque token a `build()` threads through its `fd.op*` calls. */
export interface BuildContext {
  readonly _brand: "fd.Context";
}

/** A compiled script body: `build(context, id, def)`. */
export type BuildFn = (
  context: BuildContext,
  id: BuildId,
  def: Record<string, unknown>,
) => void;

/**
 * Resolve+run a sub-feature call (script→script recursion or
 * script→built-in commit). This is the runtime form of the §2.4
 * trampoline: given a target `type`, it either runs another compiled
 * `build()` recursively in pure JS (sharing `context`) or commits a
 * built-in op. The host injects the full capability; {@link buildFdModule}
 * carries a built-ins-only default.
 */
export type CallFn = (
  context: BuildContext,
  id: BuildId,
  type: string,
  def: Record<string, unknown>,
) => OpHandle;

/**
 * The single capability the host needs from the kernel: commit one
 * built-in op into the live regeneration scratch and return the ids of
 * the feature(s) it created. In the worker this wraps the wasm
 * `fdOpCommit` export; tests pass a fake.
 */
export interface ScriptHostDeps {
  commit(id: string, typeName: string, defJson: string): string[];

  /**
   * Resolve a canonical query string against the live regeneration
   * geometry, returning the matched B-Rep element handles. In the worker
   * this wraps the wasm `fdResolveQuery` export; tests pass a fake.
   */
  resolveQuery(query: string): QueryHandle[];
}

/**
 * `fd.op*` → built-in `type_name`. Only ops backed by a real kernel
 * built-in are wired; the kernel rejects an unknown type, so this map
 * is the single source of truth for which ops a script may call.
 */
const OP_TO_BUILTIN: Record<string, string> = {
  opBox: "box",
  opSphere: "sphere",
  opCylinder: "cylinder",
  opExtrude: "extrude",
  opRevolve: "revolve",
  opHole: "hole",
  opFillet: "fillet_edge",
  opBoolean: "boolean",
};

/** Build the dotted-id object `build()` derives its sub-op ids from. */
export function makeBuildId(id: string): BuildId {
  return {
    sub: (name: string) => makeBuildId(`${id}.${name}`),
    toString: () => id,
  };
}

/**
 * Compose the `fielddraft` module a `build()` imports: the S1 authoring
 * builders (so a script's top-level `fd.feature(...)` still evaluates)
 * plus the S2 `fd.op*` geometry surface bound to `deps.commit` and the
 * S3 `fd.q` query surface bound to `resolveQuery`. Each `fd.q.*` method
 * builds a canonical query string and delegates to the resolver. The
 * bare default (no resolver injected) throws — the query surface must
 * not silently no-op.
 *
 * `fd.call` is the §2.4 sub-feature trampoline. When the host injects a
 * full `call` capability ({@link makeScriptHost}) it resolves both
 * script→script recursion and script→built-in commits. The bare
 * default here handles **built-ins only** — at this level there is no
 * script registry to recurse into — so script→script composition
 * requires the host-injected capability.
 */
export function buildFdModule(
  deps: ScriptHostDeps,
  call?: CallFn,
  resolveQuery?: (query: string) => QueryHandle[],
): Record<string, unknown> {
  const ops: Record<string, (c: BuildContext, id: BuildId, def: Record<string, unknown>) => OpHandle> = {};
  for (const [op, typeName] of Object.entries(OP_TO_BUILTIN)) {
    ops[op] = (_context, id, def) => {
      const handles = deps.commit(String(id), typeName, JSON.stringify(def ?? {}));
      return { id: handles[0] ?? String(id) };
    };
  }

  // The bare default for `fd.q`: with no kernel resolver injected there
  // is no live regeneration geometry to query, so it throws rather than
  // silently no-op. The host (`makeScriptHost`) threads the real one.
  const defaultResolveQuery = (_query: string): QueryHandle[] => {
    throw new Error("fd.q requires the kernel query resolver (host-injected)");
  };
  const resolve = resolveQuery ?? defaultResolveQuery;

  // Canonical id form: a raw string passes through, an `OpHandle` yields
  // its `.id`. Mirrors how `fd.call`/`fd.op*` accept the dotted build id.
  const idOf = (t: OpHandle | string): string =>
    typeof t === "string" ? t : t.id;

  // Element-query addressing: a raw string is taken as the decimal id_hash,
  // a `QueryHandle` yields its `.idHash`. This is the element-level analogue
  // of `idOf` — `fd.q.face/edge/vertex` address one specific B-Rep element by
  // its stable id_hash (the FROZEN `qFace/qEdge/qVertex` contract).
  const idHashOf = (h: QueryHandle | string): string =>
    typeof h === "string" ? h : h.idHash;

  const q = {
    createdBy: (target: OpHandle | string): QueryHandle[] =>
      resolve(`qCreatedBy(${idOf(target)})`),
    faceOf: (target: OpHandle | string): QueryHandle[] =>
      resolve(`qFaceOf(${idOf(target)})`),
    edgeBetween: (a: OpHandle | string, b: OpHandle | string): QueryHandle[] =>
      resolve(`qEdgeBetween(${idOf(a)}, ${idOf(b)})`),
    /** Address one face by its stable decimal id_hash. */
    face: (h: QueryHandle | string): QueryHandle[] =>
      resolve(`qFace(${idHashOf(h)})`),
    /** Address one edge by its stable decimal id_hash. */
    edge: (h: QueryHandle | string): QueryHandle[] =>
      resolve(`qEdge(${idHashOf(h)})`),
    /** Address one vertex by its stable decimal id_hash. */
    vertex: (h: QueryHandle | string): QueryHandle[] =>
      resolve(`qVertex(${idHashOf(h)})`),
    evaluate: (query: string): QueryHandle[] => resolve(query),
  };

  // The bare default resolves built-ins only: with no script registry at
  // this level it cannot recurse into another `build()`, so it just
  // commits `type` as a built-in op (the kernel rejects unknown types).
  // Script→script composition needs the host-injected `call`.
  const defaultCall: CallFn = (_context, id, type, def) => {
    const handles = deps.commit(String(id), type, JSON.stringify(def ?? {}));
    return { id: handles[0] ?? String(id) };
  };

  const fd = {
    feature,
    length,
    angle,
    real,
    bool,
    str,
    vec2,
    vec3,
    enumParam,
    query,
    group,
    visibleWhen,
    ...ops,
    q,
    call: call ?? defaultCall,
  };
  // Mirror the authoring named exports so both `import { fd }` and
  // `import { length }` resolve, exactly like the real package (§4).
  return {
    feature,
    length,
    angle,
    real,
    bool,
    str,
    vec2,
    vec3,
    enumParam,
    query,
    group,
    visibleWhen,
    fd,
    // Keep a reference to the authoring-only surface for parity checks.
    __authoring: authoringFd,
  };
}

/**
 * Evaluate a CommonJS `compiledJs` body and return its default-exported
 * `build()`. `require("fielddraft")` yields `fdModule`; any other
 * specifier throws. Throws if the body exports no callable default.
 */
export function compileBuild(
  compiledJs: string,
  fdModule: Record<string, unknown>,
): BuildFn {
  const moduleObj = { exports: {} as Record<string, unknown> };
  const requireShim = (name: string): unknown => {
    if (name === "fielddraft") return fdModule;
    throw new Error(
      `FieldSkript build may only import "fielddraft", not "${name}"`,
    );
  };
  // The compiled body is trusted document content running in the frozen
  // determinism realm (no network/timers/clock); `new Function` is the
  // CJS factory boundary. Not user-arbitrary in the security sense —
  // see the §11 risk register.
  // eslint-disable-next-line @typescript-eslint/no-implied-eval, no-new-func
  // STRICT MODE IS LOAD-BEARING (ticket #347). `new Function` bodies are SLOPPY
  // by default, and in sloppy mode a plain call binds `this` to `globalThis` —
  // so `this.fetch(…)` would walk straight past every restriction we place on
  // agent-written scripts. Sucrase's `imports` transform happens to emit a
  // "use strict" prologue today; we do not want that guarantee to depend on a
  // transpiler detail, so we state it ourselves. Prepending it twice is a no-op.
  const factory = new Function(
    "require",
    "module",
    "exports",
    '"use strict";\n' + compiledJs,
  ) as (
    require: (name: string) => unknown,
    module: { exports: Record<string, unknown> },
    exports: Record<string, unknown>,
  ) => void;
  factory(requireShim, moduleObj, moduleObj.exports);

  const build = moduleObj.exports.default ?? moduleObj.exports.build;
  if (typeof build !== "function") {
    throw new Error("compiled FieldSkript has no callable default build() export");
  }
  return build as BuildFn;
}

/** A document's script records, as the worker reads them from the wire. */
export interface ScriptRecordLite {
  id: string;
  compiledJs: string;
}

/**
 * Extract a document's FieldSkript records (id + compiled body) so a build
 * host can refresh its `build()` table before regeneration runs. Tolerant of
 * malformed JSON / a script-free document: a parse failure or missing
 * `scripts` yields an empty list, and regeneration proceeds as a pure
 * built-in document. Shared by every realm that hosts the runner (the mesher
 * worker and the main-thread host).
 */
export function scriptRecordsFrom(documentJson: string): ScriptRecordLite[] {
  try {
    const doc = JSON.parse(documentJson) as {
      scripts?: { id?: unknown; compiledJs?: unknown }[];
    };
    if (!Array.isArray(doc.scripts)) return [];
    return doc.scripts
      .filter(
        (s): s is { id: string; compiledJs: string } =>
          typeof s?.id === "string" && typeof s?.compiledJs === "string",
      )
      .map((s) => ({ id: s.id, compiledJs: s.compiledJs }));
  } catch {
    return [];
  }
}

/**
 * The stateful host the worker holds across rebuilds. `sync` recompiles
 * the `build()` table from the current document's scripts; `run` is the
 * function handed to the wasm `installScriptRunner` — invoked
 * synchronously per script-feature instance during regeneration.
 */
export function makeScriptHost(deps: ScriptHostDeps): {
  sync(scripts: ScriptRecordLite[]): void;
  run(scriptId: string, instanceId: string, paramsJson: string): void;
} {
  const builds = new Map<string, BuildFn>();

  // The §2.4 call stack: the chain of script types currently executing,
  // shared by `run` (top-level) and `fd.call` (re-entrant). Recursion is
  // detected by membership, runaway composition by depth — both
  // deterministic since execution is synchronous and single-threaded.
  const callStack: string[] = [];
  const MAX_CALL_DEPTH = 64;

  // The full trampoline. A registered script type recurses into its
  // `build()` **in pure JS** sharing the same `context` — no wasm
  // boundary crossing for orchestration; only the inner `fd.op*` commits
  // cross, as usual. An unregistered type is treated as a built-in op and
  // committed (the kernel rejects unknown types). The guard runs before
  // the recursive descent so cycles and overflow throw deterministically.
  const call: CallFn = (context, id, type, def) => {
    const build = builds.get(type);
    if (build) {
      if (callStack.includes(type)) {
        throw new Error(
          `FieldSkript call cycle: ${[...callStack, type].join(" -> ")}`,
        );
      }
      if (callStack.length >= MAX_CALL_DEPTH) {
        throw new Error(`FieldSkript call depth ${MAX_CALL_DEPTH} exceeded`);
      }
      callStack.push(type);
      try {
        build(context, id, def ?? {});
      } finally {
        callStack.pop();
      }
      return { id: String(id) };
    }
    // Not a registered script → treat as a built-in op.
    const handles = deps.commit(String(id), type, JSON.stringify(def ?? {}));
    return { id: handles[0] ?? String(id) };
  };

  const fdModule = buildFdModule(deps, call, deps.resolveQuery);

  return {
    sync(scripts) {
      builds.clear();
      for (const record of scripts) {
        if (!record.compiledJs) continue;
        builds.set(record.id, compileBuild(record.compiledJs, fdModule));
      }
    },
    run(scriptId, instanceId, paramsJson) {
      const build = builds.get(scriptId);
      if (!build) {
        throw new Error(`no compiled build() for FieldSkript "${scriptId}"`);
      }
      // `run` participates in the same cycle guard as `fd.call`, so a
      // top-level script that `fd.call`s itself (or via a cycle back to
      // itself) is caught just like a nested one.
      const context: BuildContext = { _brand: "fd.Context" };
      const id = makeBuildId(instanceId);
      const def = paramsJson ? (JSON.parse(paramsJson) as Record<string, unknown>) : {};
      callStack.push(scriptId);
      try {
        build(context, id, def);
      } finally {
        callStack.pop();
      }
    },
  };
}
