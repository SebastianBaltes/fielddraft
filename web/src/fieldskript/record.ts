/**
 * FieldSkript record + persistence model (S1).
 *
 * A {@link FieldSkriptRecord} is the document-local, serializable form
 * of a runtime-defined feature type. It mirrors the Rust
 * `protocol::FieldSkriptRecord` field-for-field (camelCase `compiledJs`
 * matches the serde rename) so it round-trips through the kernel's
 * `toJson`/`loadJson` inside `Document.scripts` with no translation.
 *
 * Author → schema → record → persist. At save time `source` is
 * transpiled to the CommonJS `compiledJs` the worker build host runs
 * (S2); the transpiler is injected ({@link Transpiler}) and defaults to
 * sucrase — see `./transpile`. Transpilation happens once here, never
 * during regeneration.
 */

import type { RawFeatureSchema } from "../widgets/schema";
import type { FeatureDefinition } from "./dsl";
import { buildSchema } from "./dsl";
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
  /** Source buffer. */
  source: string;
  /**
   * The evaluated `definition` export of the script. In S1 callers
   * pass it directly (the DSL is imported in tests / a controlled
   * eval); the save-time `transpile → eval → definition` pipeline is
   * wired when the transpiler decision is made.
   */
  definition: FeatureDefinition;
  /** Overrides the display name; defaults to `definition.name`. */
  name?: string;
  /** Transpiler to compile `source`; defaults to {@link sucraseTranspiler}. */
  transpile?: Transpiler;
}

/**
 * Build a fresh {@link FieldSkriptRecord} (version 1) from a script's
 * source and evaluated definition.
 */
export function createRecord(input: CreateRecordInput): FieldSkriptRecord {
  const transpile = input.transpile ?? sucraseTranspiler;
  return {
    id: input.id,
    name: input.name ?? input.definition.name,
    source: input.source,
    compiledJs: transpile(input.source),
    schema: buildSchema(input.definition, input.id),
    version: 1,
  };
}

/**
 * Produce the next version of a record after a source edit: recompiles
 * and re-extracts the schema, bumps `version`. Pure — returns a new
 * record, leaving the input untouched.
 */
export function reviseRecord(
  prev: FieldSkriptRecord,
  input: { source: string; definition: FeatureDefinition; name?: string; transpile?: Transpiler },
): FieldSkriptRecord {
  const transpile = input.transpile ?? sucraseTranspiler;
  return {
    id: prev.id,
    name: input.name ?? input.definition.name,
    source: input.source,
    compiledJs: transpile(input.source),
    schema: buildSchema(input.definition, prev.id),
    version: prev.version + 1,
  };
}
