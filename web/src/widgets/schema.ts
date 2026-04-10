/**
 * Feature-schema handling for the K6a widget kit.
 *
 * The Rust-side `FeatureSchema` derive (K4.5.3 + K4.5.4a +
 * K4.5.4f) emits a JSON shape per feature definition struct:
 *
 * ```json
 * {
 *   "type": "extrude",
 *   "params": [
 *     { "key": "profiles",      "name": "Profiles",       "type": "query",    "required": true  },
 *     { "key": "depth",         "name": "Depth",          "type": "length",   "required": true  },
 *     { "key": "profile_half_x","name": "Profile half x", "type": "f32",      "required": true  },
 *     { "key": "profile_half_y","name": "Profile half y", "type": "f32",      "required": true  },
 *     { "key": "operation",     "name": "Op",             "type": "bool_op",  "required": false },
 *     { "key": "center",        "name": "Center",         "type": "vec3",     "required": false }
 *   ]
 * }
 * ```
 *
 * This module parses that shape into a typed `FeatureSchema`,
 * maps the string type hints to widget kinds the React tree can
 * dispatch on, and builds initial definition-value blobs that a
 * freshly-instantiated FeatureDialog can mount on. Everything
 * here is pure TypeScript — no React, no kernel calls — so the
 * logic tests under `schema.test.ts` run as fast plain vitest
 * cases.
 */

// ─── JSON shape (wire form from the Rust derive) ────────────────

/**
 * Raw schema shape as the kernel's `schemaForType` accessor
 * hands it back. Narrower than `unknown` but still close to the
 * JSON because the Rust side controls the exact string constants
 * in `type` and field names.
 */
export interface RawFeatureSchema {
  type: string;
  params: RawFeatureParam[];
}

export interface RawFeatureParam {
  key: string;
  name: string;
  /**
   * Widget hint emitted by the Rust derive. Known values:
   * `f32`, `vec3`, `bool`, `string`, `bool_op`, `length`,
   * `angle`, `query`, `generic`.
   */
  type: string;
  required: boolean;
}

// ─── parsed + widget-aware schema ───────────────────────────────

/**
 * Discriminated widget kind the FeatureDialog dispatches on.
 * `Unknown` is the escape hatch for schemas the K6a kit doesn't
 * yet render natively — the dialog falls back to a JSON text
 * editor for that field so authoring stays unblocked.
 */
export type WidgetKind =
  | "number"
  | "vec3"
  | "bool"
  | "string"
  | "bool_op"
  | "length"
  | "angle"
  | "query"
  | "unknown";

export interface FeatureParamDescriptor {
  key: string;
  label: string;
  widget: WidgetKind;
  required: boolean;
  /**
   * The raw kernel-side type hint that produced this widget
   * kind. Useful for diagnostics and for the "unknown" fallback,
   * which shows the original string so the user knows what the
   * derive is asking for.
   */
  rawType: string;
}

export interface FeatureSchema {
  type: string;
  params: FeatureParamDescriptor[];
}

/**
 * Parse the kernel-returned JSON into a widget-aware schema.
 * Throws on shape errors so bugs in the Rust side surface
 * immediately rather than silently rendering a broken dialog.
 */
export function parseFeatureSchema(value: unknown): FeatureSchema {
  if (!value || typeof value !== "object") {
    throw new Error("parseFeatureSchema: expected a JSON object");
  }
  const obj = value as Record<string, unknown>;
  const typeField = obj.type;
  if (typeof typeField !== "string" || typeField.length === 0) {
    throw new Error("parseFeatureSchema: missing `type` string");
  }
  const rawParams = obj.params;
  if (!Array.isArray(rawParams)) {
    throw new Error("parseFeatureSchema: missing `params` array");
  }
  const params: FeatureParamDescriptor[] = rawParams.map((raw, index) => {
    if (!raw || typeof raw !== "object") {
      throw new Error(`parseFeatureSchema: params[${index}] is not an object`);
    }
    const p = raw as Record<string, unknown>;
    const key = asString(p.key, `params[${index}].key`);
    const label = asString(p.name, `params[${index}].name`);
    const rawType = asString(p.type, `params[${index}].type`);
    const required = asBool(p.required, `params[${index}].required`);
    return {
      key,
      label,
      widget: widgetKindFromRawType(rawType),
      required,
      rawType,
    };
  });
  return { type: typeField, params };
}

function asString(value: unknown, where: string): string {
  if (typeof value !== "string") {
    throw new Error(`parseFeatureSchema: ${where} must be a string`);
  }
  return value;
}

function asBool(value: unknown, where: string): boolean {
  if (typeof value !== "boolean") {
    throw new Error(`parseFeatureSchema: ${where} must be a boolean`);
  }
  return value;
}

/**
 * Map a kernel-side type hint to the FeatureDialog's widget
 * kind. Unknown hints land on `"unknown"` so the dialog can
 * still render them with a generic JSON editor.
 */
export function widgetKindFromRawType(rawType: string): WidgetKind {
  switch (rawType) {
    case "f32":
      return "number";
    case "vec3":
      return "vec3";
    case "bool":
      return "bool";
    case "string":
      return "string";
    case "bool_op":
      return "bool_op";
    case "length":
      return "length";
    case "angle":
      return "angle";
    case "query":
      return "query";
    default:
      return "unknown";
  }
}

// ─── default value construction ─────────────────────────────────

/**
 * Build a JSON object with sensible initial values for every
 * parameter in `schema`. The FeatureDialog uses this as the
 * starting point when the user creates a fresh feature; editing
 * logic then overlays user-supplied values on top.
 *
 * The defaults are intentionally neutral — a length of `10 mm`,
 * an angle of `0 deg`, a number of `0`, a bool of `false`, an
 * empty string, `BoolOp::Union`, a zeroed vec3, etc. They're
 * not meant to compile into a working feature — that's the
 * user's job — just to give the widgets something typed to
 * render at mount time without null-check churn.
 */
export function buildDefaultDefinition(
  schema: FeatureSchema,
): Record<string, unknown> {
  const def: Record<string, unknown> = {};
  for (const p of schema.params) {
    def[p.key] = defaultForWidget(p.widget);
  }
  return def;
}

export function defaultForWidget(widget: WidgetKind): unknown {
  switch (widget) {
    case "number":
      return 0;
    case "vec3":
      return [0, 0, 0];
    case "bool":
      return false;
    case "string":
      return "";
    case "bool_op":
      return "union";
    case "length":
      return { value: 10, unit: "mm" };
    case "angle":
      return { value: 0, unit: "deg" };
    case "query":
      return "";
    case "unknown":
      return null;
  }
}

// ─── per-param read + update helpers ────────────────────────────

/**
 * Immutable updater — returns a new definition object with the
 * single key `paramKey` replaced. Used by FeatureDialog's
 * onChange handlers so each widget triggers exactly one React
 * state update per edit.
 */
export function updateParam(
  def: Record<string, unknown>,
  paramKey: string,
  next: unknown,
): Record<string, unknown> {
  return { ...def, [paramKey]: next };
}

/**
 * Read the current value for a parameter out of a definition
 * blob, falling back to the widget's default if the key is
 * missing. The FeatureDialog uses this when it mounts on a
 * definition that was produced before the current schema
 * version — older definitions simply get the new defaults for
 * the fields they don't carry yet.
 */
export function readParam(
  def: Record<string, unknown>,
  param: FeatureParamDescriptor,
): unknown {
  const value = def[param.key];
  if (value === undefined || value === null) {
    return defaultForWidget(param.widget);
  }
  return value;
}

// ─── value validation ───────────────────────────────────────────

/**
 * Shape validator for a single parameter value. Returns an
 * error string if the value doesn't match the widget kind, or
 * `null` if it's well-formed. Used by the FeatureDialog's
 * per-field error display and by the commit path that pushes
 * the definition back to the kernel.
 */
export function validateParamValue(
  param: FeatureParamDescriptor,
  value: unknown,
): string | null {
  switch (param.widget) {
    case "number":
      return typeof value === "number" && Number.isFinite(value)
        ? null
        : `${param.label} must be a finite number`;
    case "vec3":
      if (!Array.isArray(value) || value.length !== 3) {
        return `${param.label} must be a 3-element array`;
      }
      for (const v of value) {
        if (typeof v !== "number" || !Number.isFinite(v)) {
          return `${param.label} must contain three finite numbers`;
        }
      }
      return null;
    case "bool":
      return typeof value === "boolean" ? null : `${param.label} must be a boolean`;
    case "string":
    case "query":
      return typeof value === "string" ? null : `${param.label} must be a string`;
    case "bool_op":
      return value === "union" || value === "difference" || value === "intersect"
        ? null
        : `${param.label} must be one of union / difference / intersect`;
    case "length":
    case "angle":
      return validateValueUnit(value, param.label, param.widget);
    case "unknown":
      return null;
  }
}

function validateValueUnit(
  value: unknown,
  label: string,
  widget: "length" | "angle",
): string | null {
  if (!value || typeof value !== "object") {
    return `${label} must be a {value, unit} object`;
  }
  const obj = value as Record<string, unknown>;
  const v = obj.value;
  const unit = obj.unit;
  if (typeof v !== "number" || !Number.isFinite(v)) {
    return `${label}: value must be a finite number`;
  }
  if (typeof unit !== "string") {
    return `${label}: unit must be a string`;
  }
  // Cast widens the two readonly tuple types to a plain
  // `readonly string[]` so the membership check type-checks
  // across both branches.
  const allowed: readonly string[] =
    widget === "length" ? LENGTH_UNITS : ANGLE_UNITS;
  if (!allowed.includes(unit)) {
    return `${label}: unit must be one of ${allowed.join(", ")}`;
  }
  return null;
}

/** Supported length units — must stay in sync with
 *  `LengthUnit` in `crates/protocol/src/lib.rs`. */
export const LENGTH_UNITS = ["mm", "cm", "m", "in"] as const;
export type LengthUnit = (typeof LENGTH_UNITS)[number];

/** Supported angle units — must stay in sync with `AngleUnit`
 *  in `crates/protocol/src/lib.rs`. */
export const ANGLE_UNITS = ["deg", "rad"] as const;
export type AngleUnit = (typeof ANGLE_UNITS)[number];

/** Supported BoolOp wire strings — must stay in sync with
 *  `BoolOp` in `crates/protocol/src/lib.rs`. */
export const BOOL_OPS = ["union", "difference", "intersect"] as const;
export type BoolOpWire = (typeof BOOL_OPS)[number];
