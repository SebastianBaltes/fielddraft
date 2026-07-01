import { describe, expect, it } from "vitest";
import {
  ANGLE_UNITS,
  LENGTH_UNITS,
  buildDefaultDefinition,
  defaultForParam,
  isGroupExpanded,
  isVisible,
  parseFeatureSchema,
  readParam,
  updateParam,
  validateParamValue,
  widgetKindFor,
  type FeatureParamDescriptor,
  type FeatureSchema,
} from "./schema";

// ─── raw schema fixtures ───────────────────────────────────────

const extrudeRaw = {
  type: "extrude",
  params: [
    { key: "profiles", name: "Profiles", type: "multi_query", required: true },
    { key: "depth", name: "Depth", type: "length", required: true },
    { key: "profile_half_x", name: "Profile half x", type: "f32", required: true },
    { key: "profile_half_y", name: "Profile half y", type: "f32", required: true },
    {
      key: "operation",
      name: "Op",
      type: "enum",
      required: false,
      ui_hint: "horizontal_enum",
      variants: [
        { key: "add", label: "Add" },
        { key: "new", label: "New" },
        { key: "remove", label: "Remove" },
        { key: "intersect", label: "Intersect" },
      ],
    },
    { key: "center", name: "Center", type: "vec3", required: false },
  ],
};

// ─── parseFeatureSchema ────────────────────────────────────────

describe("parseFeatureSchema", () => {
  it("parses the extrude schema shape into widget-aware descriptors", () => {
    const schema = parseFeatureSchema(extrudeRaw);
    expect(schema.type).toBe("extrude");
    // Ticket r2zx: profile_half_x/y are hidden from the dialog → 4 of 6 shown.
    expect(schema.params).toHaveLength(4);
    expect(schema.params.map((p) => p.key)).toEqual([
      "profiles",
      "depth",
      "operation",
      "center",
    ]);
    expect(schema.params[0]).toMatchObject({
      key: "profiles",
      label: "Profiles",
      widget: "multi_query",
      required: true,
      rawType: "multi_query",
    });
    expect(schema.params[1].widget).toBe("length");
    expect(schema.params[2].widget).toBe("enum");
    expect(schema.params[2].uiHint).toBe("horizontal_enum");
    expect(schema.params[2].variants).toEqual([
      { key: "add", label: "Add" },
      { key: "new", label: "New" },
      { key: "remove", label: "Remove" },
      { key: "intersect", label: "Intersect" },
    ]);
    expect(schema.params[3].widget).toBe("vec3");
  });

  it("rejects missing fields with actionable errors", () => {
    expect(() => parseFeatureSchema({})).toThrow(/type/);
    expect(() => parseFeatureSchema({ type: "x" })).toThrow(/params/);
    expect(() =>
      parseFeatureSchema({ type: "x", params: [{ key: 7 }] }),
    ).toThrow(/key/);
    // Enums must carry a non-empty variants list.
    expect(() =>
      parseFeatureSchema({
        type: "x",
        params: [
          { key: "op", name: "Op", type: "enum", required: false },
        ],
      }),
    ).toThrow(/variants/);
  });

  it("falls back to the `unknown` widget kind for unknown type hints", () => {
    const schema = parseFeatureSchema({
      type: "x",
      params: [
        { key: "blob", name: "Blob", type: "generic", required: true },
        { key: "thing", name: "Thing", type: "custom_enum", required: false },
      ],
    });
    expect(schema.params[0].widget).toBe("unknown");
    expect(schema.params[1].widget).toBe("unknown");
    expect(schema.params[0].rawType).toBe("generic");
  });

  it("upgrades bool+opposite_direction to a flip widget", () => {
    const schema = parseFeatureSchema({
      type: "x",
      params: [
        {
          key: "flip_depth",
          name: "Flip depth",
          type: "bool",
          ui_hint: "opposite_direction",
          required: false,
        },
      ],
    });
    expect(schema.params[0].widget).toBe("flip");
    expect(schema.params[0].uiHint).toBe("opposite_direction");
  });

  it("upgrades f32+slider to a slider widget and parses bounds", () => {
    const schema = parseFeatureSchema({
      type: "x",
      params: [
        {
          key: "transparency",
          name: "Transparency",
          type: "f32",
          ui_hint: "slider",
          min: 0,
          max: 1,
          step: 0.05,
          required: false,
        },
      ],
    });
    const p = schema.params[0];
    expect(p.widget).toBe("slider");
    expect(p.min).toBe(0);
    expect(p.max).toBe(1);
    expect(p.step).toBe(0.05);
  });

  it("parses nested groups with a driving bool and visible_when", () => {
    const schema = parseFeatureSchema({
      type: "extrude",
      params: [
        {
          key: "use_direction",
          name: "Use direction",
          type: "bool",
          required: false,
        },
        {
          type: "group",
          key: "direction_group",
          name: "Direction",
          driving_param: "use_direction",
          children: [
            {
              key: "direction",
              name: "Extrude direction",
              type: "query",
              required: false,
              visible_when: { param: "use_direction", equals: true },
            },
          ],
        },
      ],
    });
    expect(schema.params[1].widget).toBe("group");
    expect(schema.params[1].drivingParam).toBe("use_direction");
    const child = schema.params[1].children?.[0];
    expect(child?.visibleWhen).toEqual({ param: "use_direction", equals: true });
  });
});

// ─── widgetKindFor ─────────────────────────────────────────────

describe("widgetKindFor", () => {
  it("maps every known kernel hint to a widget kind", () => {
    expect(widgetKindFor("f32")).toBe("number");
    expect(widgetKindFor("vec2")).toBe("vec2");
    expect(widgetKindFor("vec3")).toBe("vec3");
    expect(widgetKindFor("bool")).toBe("bool");
    expect(widgetKindFor("string")).toBe("string");
    expect(widgetKindFor("enum")).toBe("enum");
    expect(widgetKindFor("length")).toBe("length");
    expect(widgetKindFor("angle")).toBe("angle");
    expect(widgetKindFor("query")).toBe("query");
    expect(widgetKindFor("group")).toBe("group");
  });

  it("respects ui_hint upgrades", () => {
    expect(widgetKindFor("bool", "opposite_direction")).toBe("flip");
    expect(widgetKindFor("f32", "slider")).toBe("slider");
    // horizontal_enum is a rendering variant of the enum widget,
    // not a different kind — the dispatch happens inside EnumWidget.
    expect(widgetKindFor("enum", "horizontal_enum")).toBe("enum");
  });
});

// ─── defaults ──────────────────────────────────────────────────

describe("buildDefaultDefinition", () => {
  it("emits an initial value for every leaf param", () => {
    const schema = parseFeatureSchema(extrudeRaw);
    const def = buildDefaultDefinition(schema);
    expect(def.profiles).toBe("");
    expect(def.depth).toEqual({ value: 1, unit: "mm" });
    // profile_half_x/y are hidden (ticket r2zx) → not in the default def; the
    // kernel falls back to their 0.0 param default.
    expect(def.profile_half_x).toBeUndefined();
    expect(def.profile_half_y).toBeUndefined();
    // Enum default is the first variant.
    expect(def.operation).toBe("add");
    expect(def.center).toEqual([0, 0, 0]);
  });

  it("recurses into groups for default values", () => {
    const schema = parseFeatureSchema({
      type: "x",
      params: [
        { key: "use_dir", name: "Use dir", type: "bool", required: false },
        {
          type: "group",
          key: "dir_group",
          name: "Direction",
          driving_param: "use_dir",
          children: [
            { key: "dir_q", name: "Direction query", type: "query", required: false },
          ],
        },
      ],
    });
    const def = buildDefaultDefinition(schema);
    expect(def.use_dir).toBe(false);
    expect(def.dir_q).toBe("");
    // Group nodes themselves don't contribute a key.
    expect(def.dir_group).toBeUndefined();
  });
});

describe("defaultForParam", () => {
  const p = (over: Partial<FeatureParamDescriptor>): FeatureParamDescriptor => ({
    key: "k",
    label: "L",
    widget: "number",
    required: true,
    rawType: "f32",
    ...over,
  });

  it("returns 0 for number widgets", () => {
    expect(defaultForParam(p({ widget: "number" }))).toBe(0);
  });
  it("returns a zero vec3 for vec3 widgets", () => {
    expect(defaultForParam(p({ widget: "vec3" }))).toEqual([0, 0, 0]);
  });
  it("returns a zero vec2 for vec2 widgets", () => {
    expect(defaultForParam(p({ widget: "vec2" }))).toEqual([0, 0]);
  });
  it("returns 1mm for length widgets", () => {
    expect(defaultForParam(p({ widget: "length" }))).toEqual({
      value: 1,
      unit: "mm",
    });
  });
  it("returns 0deg for angle widgets", () => {
    expect(defaultForParam(p({ widget: "angle" }))).toEqual({
      value: 0,
      unit: "deg",
    });
  });
  it("WRAPS a scalar length default into {value, unit}", () => {
    // Regression: `fd.length("size", {default: 20})` lowers to a scalar
    // `defaultValue: 20`; it must come back as {value, unit}, not raw 20,
    // or `def.size.value` is undefined → NaN → kernel rejects half_extents.
    expect(defaultForParam(p({ widget: "length", defaultValue: 20 }))).toEqual({
      value: 20,
      unit: "mm",
    });
  });
  it("WRAPS a scalar angle default into {value, unit}", () => {
    expect(defaultForParam(p({ widget: "angle", defaultValue: 45 }))).toEqual({
      value: 45,
      unit: "deg",
    });
  });
  it("passes an already-wrapped length default through unchanged", () => {
    const wrapped = { value: 7, unit: "cm" };
    expect(defaultForParam(p({ widget: "length", defaultValue: wrapped }))).toEqual(
      wrapped,
    );
  });
  it("returns the first variant key for enum widgets", () => {
    const def = defaultForParam(
      p({
        widget: "enum",
        variants: [
          { key: "add", label: "Add" },
          { key: "remove", label: "Remove" },
        ],
      }),
    );
    expect(def).toBe("add");
  });
  it("returns false for flip widgets", () => {
    expect(defaultForParam(p({ widget: "flip" }))).toBe(false);
  });
  it("returns the slider minimum", () => {
    expect(
      defaultForParam(p({ widget: "slider", min: 0.25, max: 1, step: 0.05 })),
    ).toBe(0.25);
  });
  it("returns null for unknown widgets", () => {
    expect(defaultForParam(p({ widget: "unknown" }))).toBe(null);
  });
});

// ─── readParam / updateParam ───────────────────────────────────

describe("updateParam + readParam", () => {
  it("updateParam returns a new object with the key replaced", () => {
    const before = { a: 1, b: 2 };
    const after = updateParam(before, "a", 42);
    expect(after).toEqual({ a: 42, b: 2 });
    expect(before).toEqual({ a: 1, b: 2 });
    expect(after).not.toBe(before);
  });

  it("readParam returns the stored value when present", () => {
    const schema = parseFeatureSchema(extrudeRaw);
    const def = { ...buildDefaultDefinition(schema), depth: { value: 50, unit: "cm" } };
    const depthParam = schema.params.find((p) => p.key === "depth")!;
    expect(readParam(def, depthParam)).toEqual({ value: 50, unit: "cm" });
  });

  it("readParam falls back to the widget default when the key is missing", () => {
    const schema = parseFeatureSchema(extrudeRaw);
    const depthParam = schema.params.find((p) => p.key === "depth")!;
    expect(readParam({}, depthParam)).toEqual({ value: 1, unit: "mm" });
  });
});

// ─── visibility / group expansion ──────────────────────────────

describe("isVisible", () => {
  it("returns true when visible_when is absent", () => {
    const param: FeatureParamDescriptor = {
      key: "k",
      label: "L",
      widget: "number",
      required: true,
      rawType: "f32",
    };
    expect(isVisible(param, {})).toBe(true);
  });

  it("gates a param on the driving param matching a variant key", () => {
    const param: FeatureParamDescriptor = {
      key: "depth",
      label: "Depth",
      widget: "length",
      required: true,
      rawType: "length",
      visibleWhen: { param: "end_condition", equals: "blind" },
    };
    expect(isVisible(param, { end_condition: "blind" })).toBe(true);
    expect(isVisible(param, { end_condition: "through_all" })).toBe(false);
  });
});

describe("isGroupExpanded", () => {
  it("is always open when no driving param is set", () => {
    const group: FeatureParamDescriptor = {
      key: "g",
      label: "G",
      widget: "group",
      required: false,
      rawType: "group",
      children: [],
    };
    expect(isGroupExpanded(group, {})).toBe(true);
  });

  it("follows the driving bool's value", () => {
    const group: FeatureParamDescriptor = {
      key: "g",
      label: "G",
      widget: "group",
      required: false,
      rawType: "group",
      children: [],
      drivingParam: "use_direction",
    };
    expect(isGroupExpanded(group, { use_direction: true })).toBe(true);
    expect(isGroupExpanded(group, { use_direction: false })).toBe(false);
    expect(isGroupExpanded(group, {})).toBe(false);
  });
});

// ─── validateParamValue ────────────────────────────────────────

function paramOf(
  widget: FeatureParamDescriptor["widget"],
  extras: Partial<FeatureParamDescriptor> = {},
): FeatureParamDescriptor {
  return {
    key: "k",
    label: "X",
    widget,
    required: true,
    rawType: widget,
    ...extras,
  };
}

describe("validateParamValue", () => {
  it("accepts finite numbers for number widgets", () => {
    expect(validateParamValue(paramOf("number"), 3.14)).toBeNull();
    expect(validateParamValue(paramOf("number"), 0)).toBeNull();
  });

  it("rejects NaN / Infinity / non-number for number widgets", () => {
    expect(validateParamValue(paramOf("number"), Number.NaN)).not.toBeNull();
    expect(validateParamValue(paramOf("number"), Number.POSITIVE_INFINITY)).not.toBeNull();
    expect(validateParamValue(paramOf("number"), "5")).not.toBeNull();
  });

  it("accepts 2-element number arrays for vec2", () => {
    expect(validateParamValue(paramOf("vec2"), [1, 2])).toBeNull();
  });

  it("rejects wrong-length or non-number vec2 values", () => {
    expect(validateParamValue(paramOf("vec2"), [1])).not.toBeNull();
    expect(validateParamValue(paramOf("vec2"), [1, "2"])).not.toBeNull();
    expect(validateParamValue(paramOf("vec2"), "not an array")).not.toBeNull();
  });

  it("accepts 3-element number arrays for vec3", () => {
    expect(validateParamValue(paramOf("vec3"), [1, 2, 3])).toBeNull();
  });

  it("rejects wrong-length or non-number vec3 values", () => {
    expect(validateParamValue(paramOf("vec3"), [1, 2])).not.toBeNull();
    expect(validateParamValue(paramOf("vec3"), [1, 2, "3"])).not.toBeNull();
    expect(validateParamValue(paramOf("vec3"), "not an array")).not.toBeNull();
  });

  // Ticket x2m7: a `=…` formula string is a valid scalar — for plain numbers
  // and for individual vec components — while a non-formula string stays invalid.
  it("accepts formula strings for number and vec components", () => {
    expect(validateParamValue(paramOf("number"), "=Seitenlänge")).toBeNull();
    expect(validateParamValue(paramOf("vec2"), [1, "=w"])).toBeNull();
    expect(validateParamValue(paramOf("vec3"), ["=w", 2, "=h / 2"])).toBeNull();
  });

  it("still rejects non-formula strings and keeps colours strictly numeric", () => {
    expect(validateParamValue(paramOf("number"), "5")).not.toBeNull();
    expect(validateParamValue(paramOf("vec3"), [1, 2, "3"])).not.toBeNull();
    // A colour channel must stay a number — no formulas.
    expect(validateParamValue(paramOf("color"), [0.1, 0.2, "=c"])).not.toBeNull();
  });

  it("accepts every supported length unit", () => {
    for (const unit of LENGTH_UNITS) {
      expect(
        validateParamValue(paramOf("length"), { value: 1, unit }),
      ).toBeNull();
    }
  });

  it("rejects unknown length units", () => {
    expect(
      validateParamValue(paramOf("length"), { value: 1, unit: "furlong" }),
    ).not.toBeNull();
  });

  it("accepts every supported angle unit", () => {
    for (const unit of ANGLE_UNITS) {
      expect(
        validateParamValue(paramOf("angle"), { value: 45, unit }),
      ).toBeNull();
    }
  });

  it("accepts variant keys for enum widgets", () => {
    const param = paramOf("enum", {
      variants: [
        { key: "add", label: "Add" },
        { key: "remove", label: "Remove" },
      ],
    });
    expect(validateParamValue(param, "add")).toBeNull();
    expect(validateParamValue(param, "remove")).toBeNull();
  });

  it("rejects unknown variant keys", () => {
    const param = paramOf("enum", {
      variants: [{ key: "add", label: "Add" }],
    });
    expect(validateParamValue(param, "subtract")).not.toBeNull();
  });

  it("accepts booleans for flip widgets", () => {
    expect(validateParamValue(paramOf("flip"), true)).toBeNull();
    expect(validateParamValue(paramOf("flip"), "yes")).not.toBeNull();
  });

  it("accepts finite numbers for slider widgets", () => {
    expect(validateParamValue(paramOf("slider"), 0.5)).toBeNull();
    expect(validateParamValue(paramOf("slider"), "half")).not.toBeNull();
  });

  it("accepts strings for query widgets", () => {
    expect(validateParamValue(paramOf("query"), "qCreatedBy(1)")).toBeNull();
    expect(validateParamValue(paramOf("query"), 42)).not.toBeNull();
  });

  it("returns null for group and unknown widgets without probing", () => {
    expect(
      validateParamValue(paramOf("unknown"), { arbitrary: "shape" }),
    ).toBeNull();
    expect(validateParamValue(paramOf("group"), null)).toBeNull();
  });
});

// ─── FeatureSchema is accessible as a type-only export ─────────

describe("schema type exports", () => {
  it("exposes FeatureSchema with a params array", () => {
    const schema: FeatureSchema = parseFeatureSchema(extrudeRaw);
    expect(Array.isArray(schema.params)).toBe(true);
  });
});

describe("K7.21g: reveals + tabs parsing", () => {
  it("parses reveals on enum variants", () => {
    const raw = {
      type: "test",
      params: [
        {
          key: "end_condition",
          name: "End condition",
          type: "enum",
          variants: [
            { key: "blind", label: "Blind", reveals: ["depth", "symmetric"] },
            { key: "through_all", label: "Through All", reveals: [] },
          ],
        },
      ],
    };
    const schema = parseFeatureSchema(raw);
    const p = schema.params[0];
    expect(p.reveals).toBeDefined();
    expect(p.reveals!["blind"]).toEqual(["depth", "symmetric"]);
    expect(p.reveals!["through_all"]).toEqual([]);
  });

  it("parses tabs widget type", () => {
    const raw = {
      type: "test",
      params: [
        {
          key: "mode_tabs",
          name: "Mode",
          type: "tabs",
          tabs: [
            { label: "Solid", children: [{ key: "depth", name: "Depth", type: "length" }] },
            { label: "Surface", children: [] },
          ],
        },
      ],
    };
    const schema = parseFeatureSchema(raw);
    const p = schema.params[0];
    expect(p.widget).toBe("tabs");
    expect(p.tabs).toHaveLength(2);
    expect(p.tabs![0].label).toBe("Solid");
    expect(p.tabs![0].children).toHaveLength(1);
  });

  it("validates tabs and expandable as valid widget kinds", () => {
    expect(validateParamValue({ key: "x", label: "X", widget: "tabs", required: false, rawType: "tabs" }, "Solid")).toBeNull();
    expect(validateParamValue({ key: "x", label: "X", widget: "expandable", required: false, rawType: "expandable" }, true)).toBeNull();
  });
});
