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
 * The single capability the host needs from the kernel: commit one
 * built-in op into the live regeneration scratch and return the ids of
 * the feature(s) it created. In the worker this wraps the wasm
 * `fdOpCommit` export; tests pass a fake.
 */
export interface ScriptHostDeps {
  commit(id: string, typeName: string, defJson: string): string[];
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
 * plus the S2 `fd.op*` geometry surface bound to `deps.commit`. `fd.q`
 * and `fd.call` are declared but fail loudly — they are later stages
 * (S2b query bridge / S3) and must not silently no-op.
 */
export function buildFdModule(deps: ScriptHostDeps): Record<string, unknown> {
  const ops: Record<string, (c: BuildContext, id: BuildId, def: Record<string, unknown>) => OpHandle> = {};
  for (const [op, typeName] of Object.entries(OP_TO_BUILTIN)) {
    ops[op] = (_context, id, def) => {
      const handles = deps.commit(String(id), typeName, JSON.stringify(def ?? {}));
      return { id: handles[0] ?? String(id) };
    };
  }

  const laterStage = (what: string, stage: string) => () => {
    throw new Error(`${what} is a later FieldSkript stage (${stage})`);
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
    q: {
      createdBy: laterStage("fd.q.createdBy", "S3 query system"),
      evaluate: laterStage("fd.q.evaluate", "S3 query system"),
    },
    call: laterStage("fd.call", "S2b sub-feature trampoline"),
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
  const factory = new Function("require", "module", "exports", compiledJs) as (
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
 * The stateful host the worker holds across rebuilds. `sync` recompiles
 * the `build()` table from the current document's scripts; `run` is the
 * function handed to the wasm `installScriptRunner` — invoked
 * synchronously per script-feature instance during regeneration.
 */
export function makeScriptHost(deps: ScriptHostDeps): {
  sync(scripts: ScriptRecordLite[]): void;
  run(scriptId: string, instanceId: string, paramsJson: string): void;
} {
  const fdModule = buildFdModule(deps);
  const builds = new Map<string, BuildFn>();

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
      const context: BuildContext = { _brand: "fd.Context" };
      const id = makeBuildId(instanceId);
      const def = paramsJson ? (JSON.parse(paramsJson) as Record<string, unknown>) : {};
      build(context, id, def);
    },
  };
}
