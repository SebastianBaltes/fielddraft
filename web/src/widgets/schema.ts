/**
 * Feature-schema handling for the FeatureDialog (K6a + K6a.1).
 *
 * The Rust-side `FeatureSchema` derive emits a JSON shape per
 * feature definition struct. K6a.1 widened that shape to cover
 * the Onshape-style extrude dialog: typed enums with variant
 * lists, rendering hints, collapsible groups, conditional
 * visibility, and query-picker metadata.
 *
 * ```json
 * {
 *   "type": "extrude",
 *   "params": [
 *     {
 *       "key": "operation",
 *       "name": "Op",
 *       "type": "enum",
 *       "variants": [
 *         { "key": "union",      "label": "Union" },
 *         { "key": "difference", "label": "Difference" },
 *         { "key": "intersect",  "label": "Intersect" }
 *       ],
 *       "ui_hint": "horizontal_enum",
 *       "required": false
 *     },
 *     { "key": "depth", "name": "Depth", "type": "length", "required": true },
 *     { "key": "flip_depth", "name": "Flip depth", "type": "bool",
 *       "ui_hint": "opposite_direction", "required": false },
 *     {
 *       "type": "group",
 *       "key": "direction_group",
 *       "name": "Direction",
 *       "driving_param": "use_direction",
 *       "children": [ ... ]
 *     }
 *   ]
 * }
 * ```
 *
 * Groups and conditional visibility are authored by hand in
 * fixture schemas for now; the derive learns to emit them as
 * soon as a built-in feature needs them.
 */

// ─── JSON shape (wire form from the Rust derive) ────────────────

/** Raw enum-variant entry as it appears in the schema JSON. */
export interface RawEnumVariant {
  key: string;
  label: string;
  /** K7.21g: when this variant is selected, these child param keys
   *  become visible. Maps to the YAML `reveals:` pattern. */
  reveals?: string[];
}

/** Raw param entry. Groups carry `type: "group"` and a `children`
 *  array; regular params carry a widget-type hint. */
export interface RawFeatureParam {
  key: string;
  name: string;
  type: string;
  required?: boolean;
  ui_hint?: string;
  variants?: RawEnumVariant[];
  visible_when?: { param: string; equals: unknown };
  min?: number;
  max?: number;
  step?: number;
  entity_filter?: string;
  max_picks?: number;
  children?: RawFeatureParam[];
  driving_param?: string;
}

export interface RawFeatureSchema {
  type: string;
  params: RawFeatureParam[];
}

// ─── parsed + widget-aware schema ───────────────────────────────

/** Discriminated widget kind the FeatureDialog dispatches on. */
export type WidgetKind =
  | "number"
  | "vec2"
  | "vec3"
  | "bool"
  | "string"
  | "enum"
  | "length"
  | "angle"
  | "query"
  | "multi_query"
  | "slider"
  | "flip"
  | "group"
  | "tabs"
  | "expandable"
  | "unknown";

export type UiHint = "horizontal_enum" | "opposite_direction" | "slider";

export interface EnumVariant {
  key: string;
  label: string;
}

export interface VisibleWhen {
  param: string;
  equals: string | number | boolean;
}

export interface FeatureParamDescriptor {
  key: string;
  label: string;
  widget: WidgetKind;
  required: boolean;
  rawType: string;
  uiHint?: UiHint;
  variants?: EnumVariant[];
  visibleWhen?: VisibleWhen;
  /** Slider bounds — only set when `widget === "slider"`. */
  min?: number;
  max?: number;
  step?: number;
  /** K6b query picker metadata. Populated when `widget === "query"`. */
  entityFilter?: string;
  maxPicks?: number;
  /** Group/tabs/expandable children. */
  children?: FeatureParamDescriptor[];
  /** Name of a sibling bool param that drives this group's
   *  aufklappen/zuklappen state. */
  drivingParam?: string;
  /** K7.21g: per-variant reveals mapping. Keys are variant keys,
   *  values are arrays of child param keys that become visible
   *  when that variant is selected. */
  reveals?: Record<string, string[]>;
  /** K7.21g: tab definitions for `widget === "tabs"`. Each tab has
   *  a label and a list of child param descriptors. */
  tabs?: { label: string; children: FeatureParamDescriptor[] }[];
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
/**
 * K7.22 UX-fix (9): parameters on primitive feature schemas that
 * are never meant to show up in the editable dialog.
 *
 * Historically, every primitive (box, sphere, cylinder, …) carries
 * these "noise" parameters in its Rust struct so the derive macro
 * can emit them in the wire form and feed them back into the
 * kernel at regen time. But from the user's perspective a box
 * dialog should only show `half_extents` — the rest are managed
 * by dedicated tools (Transform for center/translate, Boolean for
 * op, Fillet/Chamfer/Shell for blend_radius/chamfer_size/shell,
 * Deform for twist/bend, Displace for displace_depth/freq).
 *
 * Hiding is a pure rendering filter — the wire form still carries
 * the fields and the kernel still reads them. Setting them
 * requires a separate feature (or a direct wire-form edit).
 */
const HIDDEN_PRIMITIVE_PARAM_KEYS = new Set<string>([
  "center",
  "translate",
  "op",
  "blend_radius",
  "chamfer_size",
  "twist_rate",
  "bend_rate",
  "displace_depth",
  "displace_freq",
  "shell_thickness",
]);

/** Primitive feature types that get the "size and shape only"
 *  treatment above. Other feature types (extrude, fillet, pattern,
 *  …) are unaffected — they already expose curated dialogs. */
const PRIMITIVE_FEATURE_TYPES = new Set<string>([
  "box",
  "sphere",
  "cylinder",
  "hex_prism",
  "ellipsoid",
  "capsule",
  "cone",
  "torus",
  "thread",
  "hole",
]);

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
  let params = rawParams.map((raw, i) => parseParam(raw, `params[${i}]`));
  // K7.22 (9): drop the primitive noise params from the dialog-
  // visible schema. The wire form on the kernel side still
  // carries them; they're just not rendered by the FeatureDialog.
  if (PRIMITIVE_FEATURE_TYPES.has(typeField)) {
    params = params.filter(
      (p) => p.widget === "group" || !HIDDEN_PRIMITIVE_PARAM_KEYS.has(p.key),
    );
  }
  return { type: typeField, params };
}

function parseParam(raw: unknown, where: string): FeatureParamDescriptor {
  if (!raw || typeof raw !== "object") {
    throw new Error(`parseFeatureSchema: ${where} is not an object`);
  }
  const p = raw as Record<string, unknown>;
  const key = asString(p.key, `${where}.key`);
  const label = prettifyLabel(asString(p.name, `${where}.name`));
  const rawType = asString(p.type, `${where}.type`);
  // `required` is optional for group entries — they have no
  // inherent required-ness. For leaf params the derive always
  // emits it, but hand-authored fixtures may omit it.
  const required =
    typeof p.required === "boolean" ? p.required : rawType !== "group";

  const uiHint = parseUiHint(p.ui_hint, `${where}.ui_hint`);
  const widget = widgetKindFor(rawType, uiHint);

  const descriptor: FeatureParamDescriptor = {
    key,
    label,
    widget,
    required,
    rawType,
    uiHint,
  };

  if (widget === "group" || widget === "expandable") {
    const rawChildren = p.children;
    if (!Array.isArray(rawChildren)) {
      throw new Error(`parseFeatureSchema: ${where}.children must be an array`);
    }
    descriptor.children = rawChildren.map((c, i) =>
      parseParam(c, `${where}.children[${i}]`),
    );
    if (p.driving_param !== undefined) {
      descriptor.drivingParam = asString(
        p.driving_param,
        `${where}.driving_param`,
      );
    }
  }

  if (widget === "tabs") {
    const rawTabs = p.tabs;
    if (Array.isArray(rawTabs)) {
      descriptor.tabs = rawTabs.map((tab, i) => {
        const t = tab as Record<string, unknown>;
        return {
          label: asString(t.label, `${where}.tabs[${i}].label`),
          children: Array.isArray(t.children)
            ? (t.children as unknown[]).map((c, j) => parseParam(c, `${where}.tabs[${i}].children[${j}]`))
            : [],
        };
      });
    }
  }

  if (widget === "enum") {
    const rawVariants = p.variants;
    if (!Array.isArray(rawVariants) || rawVariants.length === 0) {
      throw new Error(
        `parseFeatureSchema: ${where}.variants must be a non-empty array`,
      );
    }
    const revealsMap: Record<string, string[]> = {};
    descriptor.variants = rawVariants.map((v, i) => {
      if (!v || typeof v !== "object") {
        throw new Error(
          `parseFeatureSchema: ${where}.variants[${i}] is not an object`,
        );
      }
      const vr = v as Record<string, unknown>;
      const variantKey = asString(vr.key, `${where}.variants[${i}].key`);
      if (Array.isArray(vr.reveals)) {
        revealsMap[variantKey] = (vr.reveals as unknown[]).map(String);
      }
      return {
        key: variantKey,
        label: asString(vr.label, `${where}.variants[${i}].label`),
        reveals: Array.isArray(vr.reveals) ? (vr.reveals as unknown[]).map(String) : undefined,
      };
    });
    if (Object.keys(revealsMap).length > 0) {
      descriptor.reveals = revealsMap;
    }
  }

  if (widget === "slider") {
    descriptor.min = asFiniteNumber(p.min, `${where}.min`, 0);
    descriptor.max = asFiniteNumber(p.max, `${where}.max`, 1);
    descriptor.step = asFiniteNumber(p.step, `${where}.step`, 0.01);
  }

  if (widget === "query") {
    if (p.entity_filter !== undefined) {
      descriptor.entityFilter = asString(
        p.entity_filter,
        `${where}.entity_filter`,
      );
    }
    if (p.max_picks !== undefined) {
      descriptor.maxPicks = asFiniteNumber(
        p.max_picks,
        `${where}.max_picks`,
        1,
      );
    }
  }

  if (p.visible_when !== undefined) {
    descriptor.visibleWhen = parseVisibleWhen(
      p.visible_when,
      `${where}.visible_when`,
    );
  }

  return descriptor;
}

const UNIT_SUFFIXES = /\s+(deg|rad|mm|cm|m|in)$/i;

function prettifyLabel(label: string): string {
  return label.replace(UNIT_SUFFIXES, "");
}

function asString(value: unknown, where: string): string {
  if (typeof value !== "string") {
    throw new Error(`parseFeatureSchema: ${where} must be a string`);
  }
  return value;
}

function asFiniteNumber(
  value: unknown,
  where: string,
  fallback: number,
): number {
  if (value === undefined || value === null) return fallback;
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`parseFeatureSchema: ${where} must be a finite number`);
  }
  return value;
}

function parseUiHint(value: unknown, where: string): UiHint | undefined {
  if (value === undefined || value === null) return undefined;
  if (typeof value !== "string") {
    throw new Error(`parseFeatureSchema: ${where} must be a string`);
  }
  if (
    value === "horizontal_enum" ||
    value === "opposite_direction" ||
    value === "slider"
  ) {
    return value;
  }
  // Unknown hints are forwarded as `undefined` so the default
  // widget still renders — same escape-hatch philosophy as the
  // `unknown` widget kind.
  return undefined;
}

function parseVisibleWhen(value: unknown, where: string): VisibleWhen {
  if (!value || typeof value !== "object") {
    throw new Error(`parseFeatureSchema: ${where} must be an object`);
  }
  const obj = value as Record<string, unknown>;
  const param = asString(obj.param, `${where}.param`);
  const equals = obj.equals;
  if (
    typeof equals !== "string" &&
    typeof equals !== "number" &&
    typeof equals !== "boolean"
  ) {
    throw new Error(
      `parseFeatureSchema: ${where}.equals must be a string, number, or boolean`,
    );
  }
  return { param, equals };
}

/**
 * Map a kernel-side type hint (plus an optional ui_hint) to the
 * FeatureDialog's widget kind. The ui_hint can upgrade a base
 * type to a richer widget — `bool + opposite_direction` becomes
 * a `flip` button, `f32 + slider` becomes a range slider.
 */
export function widgetKindFor(
  rawType: string,
  uiHint?: UiHint,
): WidgetKind {
  if (rawType === "group") return "group";
  if (rawType === "tabs") return "tabs";
  if (rawType === "expandable") return "expandable";
  if (rawType === "enum") return "enum";
  if (rawType === "bool" && uiHint === "opposite_direction") return "flip";
  if (rawType === "f32" && uiHint === "slider") return "slider";
  switch (rawType) {
    case "f32":
      return "number";
    case "vec2":
      return "vec2";
    case "vec3":
      return "vec3";
    case "bool":
      return "bool";
    case "string":
      return "string";
    case "length":
      return "length";
    case "angle":
      return "angle";
    case "query":
      return "query";
    case "multi_query":
      return "multi_query";
    default:
      return "unknown";
  }
}

// ─── default value construction ─────────────────────────────────

/**
 * Build a JSON object with sensible initial values for every
 * parameter in `schema`, recursing into groups. The FeatureDialog
 * uses this as the starting point when the user creates a fresh
 * feature.
 */
export function buildDefaultDefinition(
  schema: FeatureSchema,
): Record<string, unknown> {
  const def: Record<string, unknown> = {};
  applyDefaults(schema.params, def);
  return def;
}

function applyDefaults(
  params: FeatureParamDescriptor[],
  def: Record<string, unknown>,
): void {
  for (const p of params) {
    if (p.widget === "group") {
      if (p.children) applyDefaults(p.children, def);
      continue;
    }
    def[p.key] = defaultForParam(p);
  }
}

/** Default value for a single parameter, taking variants and
 *  slider bounds into account. */
export function defaultForParam(p: FeatureParamDescriptor): unknown {
  switch (p.widget) {
    case "number":
      return 0;
    case "vec2":
      return [0, 0];
    case "vec3":
      return [0, 0, 0];
    case "bool":
    case "flip":
      return false;
    case "string":
      return "";
    case "enum":
      return p.variants && p.variants.length > 0 ? p.variants[0].key : "";
    case "length":
      return { value: 10, unit: "mm" };
    case "angle":
      return { value: 0, unit: "deg" };
    case "query":
    case "multi_query":
      return "";
    case "slider":
      return p.min ?? 0;
    case "group":
      return undefined;
    case "unknown":
      return null;
  }
}

// ─── per-param read + update helpers ────────────────────────────

/**
 * Immutable updater — returns a new definition object with the
 * single key `paramKey` replaced.
 */
export function updateParam(
  def: Record<string, unknown>,
  paramKey: string,
  next: unknown,
): Record<string, unknown> {
  return { ...def, [paramKey]: next };
}

/** Read the current value for a parameter, falling back to the
 *  default when the key is missing or explicit null. */
export function readParam(
  def: Record<string, unknown>,
  param: FeatureParamDescriptor,
): unknown {
  const value = def[param.key];
  if (value === undefined || value === null) {
    return defaultForParam(param);
  }
  return value;
}

// ─── visibility evaluation ──────────────────────────────────────

/**
 * Evaluate a param's `visible_when` expression against the
 * current definition. Params without `visible_when` are always
 * visible. Groups are additionally hidden when their
 * `drivingParam` is `false` — that's the Onshape "Direction
 * checkbox collapses the group" pattern.
 */
export function isVisible(
  param: FeatureParamDescriptor,
  def: Record<string, unknown>,
): boolean {
  if (param.visibleWhen) {
    const actual = def[param.visibleWhen.param];
    if (actual !== param.visibleWhen.equals) return false;
  }
  return true;
}

/**
 * Is a group currently expanded? Only meaningful for
 * `widget === "group"` nodes. A group without a `drivingParam`
 * is always open; otherwise the driving bool's value wins.
 */
export function isGroupExpanded(
  group: FeatureParamDescriptor,
  def: Record<string, unknown>,
): boolean {
  if (!group.drivingParam) return true;
  const value = def[group.drivingParam];
  return value === true;
}

// ─── value validation ───────────────────────────────────────────

/**
 * Shape validator for a single parameter value. Returns an error
 * string if the value doesn't match the widget kind, or `null`
 * if it's well-formed.
 */
export function validateParamValue(
  param: FeatureParamDescriptor,
  value: unknown,
): string | null {
  switch (param.widget) {
    case "number":
    case "slider":
      return typeof value === "number" && Number.isFinite(value)
        ? null
        : `${param.label} must be a finite number`;
    case "vec2":
      if (!Array.isArray(value) || value.length !== 2) {
        return `${param.label} must be a 2-element array`;
      }
      for (const v of value) {
        if (typeof v !== "number" || !Number.isFinite(v)) {
          return `${param.label} must contain two finite numbers`;
        }
      }
      return null;
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
    case "flip":
      return typeof value === "boolean" ? null : `${param.label} must be a boolean`;
    case "string":
    case "query":
    case "multi_query":
      return typeof value === "string" ? null : `${param.label} must be a string`;
    case "enum": {
      if (typeof value !== "string") {
        return `${param.label} must be a string variant key`;
      }
      const variants = param.variants ?? [];
      return variants.some((v) => v.key === value)
        ? null
        : `${param.label} must be one of ${variants.map((v) => v.key).join(", ")}`;
    }
    case "length":
    case "angle":
      return validateValueUnit(value, param.label, param.widget);
    case "group":
    case "tabs":
    case "expandable":
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
  // K8.12: accept formula strings (`"=width / 2"`) alongside
  // plain numbers — the kernel resolves them at regen time.
  const isFormula = typeof v === "string" && (v as string).startsWith("=");
  if (!isFormula && (typeof v !== "number" || !Number.isFinite(v))) {
    return `${label}: value must be a finite number or a formula (=…)`;
  }
  if (typeof unit !== "string") {
    return `${label}: unit must be a string`;
  }
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
