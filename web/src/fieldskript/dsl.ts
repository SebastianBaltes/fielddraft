/**
 * FieldSkript parameter DSL (S1).
 *
 * The runtime twin of the Rust `#[derive(FeatureSchema)]` macro: a
 * script's `definition` declares its parameters with these builder
 * functions, and {@link buildSchema} lowers them to the exact
 * `RawFeatureSchema` JSON that `widgets/schema.ts::parseFeatureSchema`
 * already consumes — so a scripted feature reuses the whole
 * FeatureDialog with zero new UI work.
 *
 * The builders emit plain JSON objects (no behaviour), keep array
 * order, and only set keys that carry a value — so the wire form is
 * deterministic and byte-compatible with the derive's output.
 *
 * Marken-Regel (README §11): the API is `fd.*`, never Onshape terms.
 */

import type { RawFeatureParam, RawFeatureSchema } from "../widgets/schema";

// ─── value types (for later Def inference, S4) ──────────────────

/** A length value as the LengthWidget round-trips it. */
export interface LengthValue {
  value: number | string;
  unit: string;
}

/** An angle value as the AngleWidget round-trips it. */
export interface AngleValue {
  value: number;
  unit: string;
}

/** A query value: a serialized QueryExpr AST string. */
export type QueryValue = string;

/**
 * A parameter spec. Structurally identical to {@link RawFeatureParam}
 * plus a phantom `__value` carrying the runtime value type, so a
 * future `DefOf<>` can derive a typed `def` for the script body
 * (S4). The phantom is type-only — never present at runtime.
 */
export type Param<TValue = unknown> = RawFeatureParam & {
  readonly __value?: TValue;
};

/** Infer the `def` object type from a tuple of params. */
export type DefOf<P extends readonly Param[]> = {
  [K in P[number] as K["key"]]: K extends Param<infer V> ? V : unknown;
};

// ─── shared option shapes ───────────────────────────────────────

interface BaseOpts {
  /** Display label. Defaults to a prettified `key`. */
  name?: string;
  /** Force required/optional. Defaults to `default === undefined`. */
  required?: boolean;
}

interface NumberOpts extends BaseOpts {
  default?: number;
  min?: number;
  max?: number;
  step?: number;
  unit?: string;
}

// ─── helpers ────────────────────────────────────────────────────

/** Prettify a snake_case key into a Title Case label. */
function prettify(key: string): string {
  const spaced = key.replace(/_/g, " ").trim();
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/** snake_case slug for an auto-derived group key. */
function slug(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "");
}

/** `required` resolution shared by every leaf builder. */
function resolveRequired(opts: BaseOpts, hasDefault: boolean): boolean {
  return opts.required ?? !hasDefault;
}

/**
 * Assemble a param, dropping every key whose value is `undefined`
 * so the emitted JSON only carries present fields (matches the
 * derive, which never emits absent attributes).
 */
function param<TValue>(spec: RawFeatureParam): Param<TValue> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(spec)) {
    if (v !== undefined) out[k] = v;
  }
  return out as unknown as Param<TValue>;
}

// ─── leaf builders ──────────────────────────────────────────────

/** A length field (numeric + unit selector). */
export function length(key: string, opts: NumberOpts = {}): Param<LengthValue> {
  return param<LengthValue>({
    key,
    name: opts.name ?? prettify(key),
    type: "length",
    required: resolveRequired(opts, opts.default !== undefined),
    unit: opts.unit ?? "mm",
    min: opts.min,
    max: opts.max,
    step: opts.step,
    default: opts.default,
  });
}

/** An angle field (numeric + unit selector). */
export function angle(key: string, opts: NumberOpts = {}): Param<AngleValue> {
  return param<AngleValue>({
    key,
    name: opts.name ?? prettify(key),
    type: "angle",
    required: resolveRequired(opts, opts.default !== undefined),
    unit: opts.unit ?? "deg",
    min: opts.min,
    max: opts.max,
    step: opts.step,
    default: opts.default,
  });
}

interface RealOpts extends NumberOpts {
  /** `"slider"` upgrades the f32 to a range slider (needs min/max). */
  uiHint?: "slider";
}

/** A scalar `f32` field; `uiHint: "slider"` renders a range slider. */
export function real(key: string, opts: RealOpts = {}): Param<number> {
  return param<number>({
    key,
    name: opts.name ?? prettify(key),
    type: "f32",
    required: resolveRequired(opts, opts.default !== undefined),
    ui_hint: opts.uiHint,
    min: opts.min,
    max: opts.max,
    step: opts.step,
    unit: opts.unit,
    default: opts.default,
  });
}

interface BoolOpts extends BaseOpts {
  default?: boolean;
  /** `"opposite_direction"` renders a ± flip button. */
  uiHint?: "opposite_direction";
}

/** A boolean field; `uiHint: "opposite_direction"` renders a flip. */
export function bool(key: string, opts: BoolOpts = {}): Param<boolean> {
  return param<boolean>({
    key,
    name: opts.name ?? prettify(key),
    type: "bool",
    required: resolveRequired(opts, opts.default !== undefined),
    ui_hint: opts.uiHint,
    default: opts.default,
  });
}

interface StrOpts extends BaseOpts {
  default?: string;
}

/** A free-text string field. */
export function str(key: string, opts: StrOpts = {}): Param<string> {
  return param<string>({
    key,
    name: opts.name ?? prettify(key),
    type: "string",
    required: resolveRequired(opts, opts.default !== undefined),
    default: opts.default,
  });
}

/** A vector2 field. */
export function vec2(
  key: string,
  opts: BaseOpts & { default?: [number, number]; unit?: string } = {},
): Param<[number, number]> {
  return param<[number, number]>({
    key,
    name: opts.name ?? prettify(key),
    type: "vec2",
    required: resolveRequired(opts, opts.default !== undefined),
    unit: opts.unit,
    default: opts.default,
  });
}

interface Vec3Opts extends BaseOpts {
  default?: [number, number, number];
  unit?: string;
  /** `"color"` renders a colour picker. */
  uiHint?: "color";
}

/** A vector3 field; `uiHint: "color"` renders a colour picker. */
export function vec3(
  key: string,
  opts: Vec3Opts = {},
): Param<[number, number, number]> {
  return param<[number, number, number]>({
    key,
    name: opts.name ?? prettify(key),
    type: "vec3",
    required: resolveRequired(opts, opts.default !== undefined),
    ui_hint: opts.uiHint,
    unit: opts.unit,
    default: opts.default,
  });
}

/** An enum variant: a stable key + its display label. */
export interface Variant {
  key: string;
  label: string;
  /** Child param keys revealed when this variant is selected. */
  reveals?: string[];
}

interface EnumOpts extends BaseOpts {
  default?: string;
  /** `"horizontal_enum"` (default) renders tabs; else a dropdown. */
  uiHint?: "horizontal_enum";
}

/**
 * An enum field. `variants` accepts `{key,label}[]`, a `string[]`
 * (label = prettified key), or a `Record<key,label>`.
 */
export function enumParam(
  key: string,
  variants: Variant[] | string[] | Record<string, string>,
  opts: EnumOpts = {},
): Param<string> {
  const normalized = normalizeVariants(variants);
  if (normalized.length === 0) {
    throw new Error(`enumParam("${key}"): variants must be non-empty`);
  }
  return param<string>({
    key,
    name: opts.name ?? prettify(key),
    type: "enum",
    required: resolveRequired(opts, opts.default !== undefined),
    ui_hint: opts.uiHint ?? "horizontal_enum",
    variants: normalized,
    default: opts.default,
  });
}

function normalizeVariants(
  variants: Variant[] | string[] | Record<string, string>,
): Variant[] {
  if (Array.isArray(variants)) {
    return variants.map((v) =>
      typeof v === "string" ? { key: v, label: prettify(v) } : v,
    );
  }
  return Object.entries(variants).map(([key, label]) => ({ key, label }));
}

interface QueryOpts extends BaseOpts {
  /** EntityType/GeometryType filter expression gating viewport picks. */
  filter?: string;
  /** Max picks; `>1` (or omitted-with-multi intent) → `multi_query`. */
  maxPicks?: number;
}

/**
 * A geometry-reference field. With `maxPicks` other than 1 it lowers
 * to `multi_query`; otherwise a single-pick `query`.
 */
export function query(key: string, opts: QueryOpts = {}): Param<QueryValue> {
  const multi = opts.maxPicks !== undefined && opts.maxPicks !== 1;
  return param<QueryValue>({
    key,
    name: opts.name ?? prettify(key),
    type: multi ? "multi_query" : "query",
    required: opts.required ?? true,
    entity_filter: opts.filter,
    max_picks: opts.maxPicks,
  });
}

// ─── containers ─────────────────────────────────────────────────

interface GroupOpts {
  /** Explicit group key; defaults to a slug of `name`. */
  key?: string;
  /** Name of a sibling `bool` param that expands/collapses the group. */
  drivingParam?: string;
}

/** A collapsible group of child params, driven by a sibling bool. */
export function group(
  name: string,
  opts: GroupOpts,
  children: Param[],
): Param<undefined> {
  return param<undefined>({
    key: opts.key ?? slug(name),
    name,
    type: "group",
    required: false,
    children,
    driving_param: opts.drivingParam,
  });
}

/**
 * Wrap a param so it is only shown when another param equals a value
 * — exactly the dialog's `visible_when` semantics (static, no script
 * run for layout). `equals` may be an array to mean "any of these".
 */
export function visibleWhen<T>(
  paramName: string,
  equals: string | number | boolean | (string | number | boolean)[],
  wrapped: Param<T>,
): Param<T> {
  return param<T>({
    ...(wrapped as RawFeatureParam),
    visible_when: [{ param: paramName, equals }],
  });
}

// ─── feature definition ─────────────────────────────────────────

export interface FeatureSpec {
  /** Display name of the feature type. */
  name: string;
  /** Unicode glyph shown on the Part toolbar button (e.g. "⬡"). */
  icon?: string;
  /** Naming template for inserted instances (e.g. "Counterbore (#d)"). */
  nameTemplate?: string;
  /** Declared parameters, in dialog order. */
  parameters: Param[];
}

/**
 * The value a script's `definition` export holds. Carries the meta
 * needed to register the type and to lower to a schema; the build
 * function is a separate export (S2).
 */
export interface FeatureDefinition {
  readonly name: string;
  readonly icon?: string;
  readonly nameTemplate?: string;
  readonly parameters: Param[];
}

/** Declare a FieldSkript feature's metadata + parameters. */
export function feature(spec: FeatureSpec): FeatureDefinition {
  return {
    name: spec.name,
    icon: spec.icon,
    nameTemplate: spec.nameTemplate,
    parameters: spec.parameters,
  };
}

/**
 * Lower a {@link FeatureDefinition} to the `RawFeatureSchema` JSON the
 * FeatureDialog consumes. `typeName` is the script's stable id (=
 * the FeatureCall `type`).
 */
export function buildSchema(
  def: FeatureDefinition,
  typeName: string,
): RawFeatureSchema {
  return {
    type: typeName,
    params: def.parameters.map((p) => stripPhantom(p)),
  };
}

/** Drop the type-only `__value` phantom if a caller ever set it. */
function stripPhantom(p: Param): RawFeatureParam {
  if (!("__value" in p)) return p;
  const { __value: _drop, ...rest } = p as Param & { __value?: unknown };
  return rest;
}

// ─── the `fd` authoring namespace ───────────────────────────────

/**
 * The `fd.*` authoring surface a script imports. Mirrors the named
 * exports so both `import { fd }` and `import { length }` styles work
 * (concept §4).
 */
export const fd = {
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
} as const;

/**
 * The `fielddraft` module object a transpiled script resolves at
 * save-time definition extraction: the authoring surface only (no
 * geometry `fd.op*` — `build()` is not run when we just read the
 * `definition` export). Mirrors both `import { fd }` and
 * `import { length }` styles.
 */
export function authoringModule(): Record<string, unknown> {
  return { feature, length, angle, real, bool, str, vec2, vec3, enumParam, query, group, visibleWhen, fd };
}
