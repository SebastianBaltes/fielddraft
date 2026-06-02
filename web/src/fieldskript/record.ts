/**
 * FieldSkript record + persistence model (S1/S2).
 *
 * A {@link FieldSkriptRecord} is the document-local, serializable form
 * of a runtime-defined feature type. It mirrors the Rust
 * `protocol::FieldSkriptRecord` field-for-field (camelCase `compiledJs`
 * matches the serde rename) so it round-trips through the kernel's
 * `toJson`/`loadJson` inside `Document.scripts` with no translation.
 *
 * **Save pipeline: `source → transpile → evalDefinition → schema`.** At
 * save time the editor's TypeScript/ESM `source` is transpiled once to
 * the CommonJS `compiledJs` the worker build host runs (S2); that same
 * compiled body is then evaluated top-level to read its `definition`
 * export ({@link evalDefinition}), which lowers to the
 * `RawFeatureSchema` the FeatureDialog consumes. The transpiler is
 * injected ({@link Transpiler}) and defaults to sucrase — see
 * `./transpile`. Transpilation happens once here, never during
 * regeneration.
 */

import type { RawFeatureSchema } from "../widgets/schema";
import { buildSchema } from "./dsl";
import { evalDefinition } from "./evalDefinition";
import { type Transpiler, sucraseTranspiler } from "./transpile";

/**
 * Document-local record of a FieldSkript feature type. Mirrors
 * `protocol::FieldSkriptRecord`.
 */
export interface FieldSkriptRecord {
  /** Stable script-type id; equals the `type` of a FeatureCall. */
  id: string;
  /** Human-facing display name. */
  name: string;
  /** Unicode glyph shown on the Part toolbar button. Defaults to "ƒ". */
  icon: string;
  /** TypeScript source (editor buffer). */
  source: string;
  /** Transpiled JavaScript, produced at save time. */
  compiledJs: string;
  /** RawFeatureSchema extracted from the script's `definition`. */
  schema: RawFeatureSchema;
  /** Bumped on every source change. */
  version: number;
}

export interface CreateRecordInput {
  /** Stable script-type id (also the FeatureCall `type`). */
  id: string;
  /** Source buffer. The `definition` and schema are derived from it. */
  source: string;
  /** Overrides the display name; defaults to the derived `definition.name`. */
  name?: string;
  /** Transpiler to compile `source`; defaults to {@link sucraseTranspiler}. */
  transpile?: Transpiler;
}

/**
 * Build a fresh {@link FieldSkriptRecord} (version 1) from a script's
 * source: transpile to CommonJS, evaluate to read its `definition`
 * export, then lower that to the schema.
 */
export function createRecord(input: CreateRecordInput): FieldSkriptRecord {
  const transpile = input.transpile ?? sucraseTranspiler;
  const compiledJs = transpile(input.source);
  const definition = evalDefinition(compiledJs);
  return {
    id: input.id,
    name: input.name ?? definition.name,
    icon: definition.icon ?? "ƒ",
    source: input.source,
    compiledJs,
    schema: buildSchema(definition, input.id),
    version: 1,
  };
}

/**
 * Produce the next version of a record after a source edit: recompiles
 * and re-extracts the schema from the new source, bumps `version`.
 * Pure — returns a new record, leaving the input untouched.
 */
export function reviseRecord(
  prev: FieldSkriptRecord,
  input: { source: string; name?: string; transpile?: Transpiler },
): FieldSkriptRecord {
  const transpile = input.transpile ?? sucraseTranspiler;
  const compiledJs = transpile(input.source);
  const definition = evalDefinition(compiledJs);
  return {
    id: prev.id,
    name: input.name ?? definition.name,
    icon: definition.icon ?? "ƒ",
    source: input.source,
    compiledJs,
    schema: buildSchema(definition, prev.id),
    version: prev.version + 1,
  };
}
