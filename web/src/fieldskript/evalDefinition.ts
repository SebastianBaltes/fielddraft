/**
 * FieldSkript save-time definition extraction (S2).
 *
 * Reads a script's `definition` export out of its transpiled CommonJS
 * body so {@link buildSchema} can lower it to a `RawFeatureSchema` at
 * save time. The mechanics mirror `compileBuild` in `workerScriptHost`
 * (the CJS `new Function` factory + a `require("fielddraft")` shim) but
 * read `module.exports.definition` instead of `.default`.
 *
 * **Top level only.** This evaluates the module's top level — the
 * declarative `fd.feature(...)` call that produces `definition` — but
 * never calls the exported `build()`. So the `require("fielddraft")`
 * shim only needs the authoring surface ({@link authoringModule}), not
 * the geometry `fd.op*` ops; a `build()` body referencing `fd.opBox`
 * does not break extraction because it is never invoked here.
 */

import { authoringModule, type FeatureDefinition } from "./dsl";

/**
 * Evaluate a CommonJS `compiledJs` body and return its `definition`
 * export. `require("fielddraft")` resolves to the authoring module; any
 * other specifier throws. Throws if the body exports no valid
 * `definition`.
 */
export function evalDefinition(compiledJs: string): FeatureDefinition {
  const moduleObj = { exports: {} as Record<string, unknown> };
  const requireShim = (name: string): unknown => {
    if (name === "fielddraft") return authoringModule();
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

  const def = moduleObj.exports.definition;
  if (
    typeof def !== "object" ||
    def === null ||
    typeof (def as { name?: unknown }).name !== "string" ||
    !Array.isArray((def as { parameters?: unknown }).parameters)
  ) {
    throw new Error(
      "compiled FieldSkript exports no valid `definition` (export const definition = fd.feature({...}))",
    );
  }
  return def as FeatureDefinition;
}
