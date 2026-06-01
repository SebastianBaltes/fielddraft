/**
 * FieldSkript record + persistence model (S1).
 *
 * A {@link FieldSkriptRecord} is the document-local, serializable form
 * of a runtime-defined feature type. It mirrors the Rust
 * `protocol::FieldSkriptRecord` field-for-field (camelCase `compiledJs`
 * matches the serde rename) so it round-trips through the kernel's
 * `toJson`/`loadJson` inside `Document.scripts` with no translation.
 *
 * S1 scope: author → schema → record → persist. Execution of the
 * `build()` body is S2 (the Rust callback + Context mutation), so the
 * transpiler is only a thin seam here: the esbuild-wasm vs sucrase
 * choice (concept open decision #4) is deferred, and the default is an
 * identity pass that keeps already-JS sources intact.
 */

import type { RawFeatureSchema } from "../widgets/schema";
import type { FeatureDefinition } from "./dsl";
import { buildSchema } from "./dsl";

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

/**
 * A transpiler turns a script's TypeScript source into executable
 * JavaScript. Injected so the heavy esbuild-wasm/sucrase dependency
 * (concept #4) stays out of S1 and out of the hot path — transpilation
 * runs once at save, never during regeneration.
 */
export type Transpiler = (source: string) => string;

/**
 * The S1 default transpiler: an identity pass. It is correct for
 * sources that are already valid JS (the common authoring case until
 * a real TS transpiler lands in S2/S4) and keeps the seam honest —
 * `compiledJs` is always populated, never silently empty.
 */
export const identityTranspiler: Transpiler = (source) => source;

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
  /** Transpiler to compile `source`; defaults to identity. */
  transpile?: Transpiler;
}

/**
 * Build a fresh {@link FieldSkriptRecord} (version 1) from a script's
 * source and evaluated definition.
 */
export function createRecord(input: CreateRecordInput): FieldSkriptRecord {
  const transpile = input.transpile ?? identityTranspiler;
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
  const transpile = input.transpile ?? identityTranspiler;
  return {
    id: prev.id,
    name: input.name ?? input.definition.name,
    source: input.source,
    compiledJs: transpile(input.source),
    schema: buildSchema(input.definition, prev.id),
    version: prev.version + 1,
  };
}
