/**
 * FieldSkript save-time transpiler (concept open decision #4 → sucrase).
 *
 * Turns the editor's TypeScript/ESM source into the CommonJS form the
 * worker build host consumes: `require("fielddraft")` for the API plus
 * `module.exports.default = build`. This runs ONCE per save on the
 * editor/main-thread side — never during regeneration — so it stays off
 * the synchronous wasm hot path entirely.
 *
 * **Why sucrase, not esbuild-wasm.** We don't bundle. The only import a
 * script may make is `"fielddraft"`, which the worker host resolves to
 * an in-memory module at run time (see {@link compileBuild}); there is
 * nothing to resolve at save time. sucrase strips types and rewrites
 * ESM→CJS file-locally — exactly, and only, what we need, with no WASM
 * blob in the editor bundle. Real type-checking is Monaco's job in the
 * editor (S4), not the transpiler's.
 */

import { transform } from "sucrase";

/**
 * Compiles a script's TypeScript/ESM source into executable JavaScript.
 * Injected into {@link createRecord}/{@link reviseRecord} so the choice
 * of compiler stays in one place and tests can substitute a fake.
 */
export type Transpiler = (source: string) => string;

/**
 * The default transpiler: TypeScript/ESM source → CommonJS. `typescript`
 * strips type syntax; `imports` rewrites `import`/`export` into the
 * `require`/`module.exports` form the worker host evaluates.
 */
export const sucraseTranspiler: Transpiler = (source) =>
  transform(source, { transforms: ["typescript", "imports"] }).code;

/**
 * Identity pass: returns the source untouched. An explicit opt-out for
 * callers/tests that already hold a CommonJS body and want to bypass
 * compilation — not the default.
 */
export const identityTranspiler: Transpiler = (source) => source;
