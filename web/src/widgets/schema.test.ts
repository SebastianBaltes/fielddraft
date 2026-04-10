import { describe, expect, it } from "vitest";
import {
  ANGLE_UNITS,
  BOOL_OPS,
  LENGTH_UNITS,
  buildDefaultDefinition,
  defaultForWidget,
  parseFeatureSchema,
  readParam,
  updateParam,
  validateParamValue,
  widgetKindFromRawType,
  type FeatureSchema,
} from "./schema";

const extrudeRaw = {
  type: "extrude",
  params: [
    { key: "profiles", name: "Profiles", type: "query", required: true },
    { key: "depth", name: "Depth", type: "length", required: true },
    { key: "profile_half_x", name: "Profile half x", type: "f32", required: true },
    { key: "profile_half_y", name: "Profile half y", type: "f32", required: true },
    { key: "operation", name: "Op", type: "bool_op", required: false },
    { key: "center", name: "Center", type: "vec3", required: false },
  ],
};

// ─── parseFeatureSchema ────────────────────────────────────────

describe("parseFeatureSchema", () => {
  it("parses the extrude schema shape into widget-aware descriptors", () => {
    const schema = parseFeatureSchema(extrudeRaw);
    expect(schema.type).toBe("extrude");
    expect(schema.params).toHaveLength(6);
    expect(schema.params[0]).toEqual({
      key: "profiles",
      label: "Profiles",
      widget: "query",
      required: true,
      rawType: "query",
    });
    expect(schema.params[1].widget).toBe("length");
    expect(schema.params[2].widget).toBe("number");
    expect(schema.params[4].widget).toBe("bool_op");
    expect(schema.params[5].widget).toBe("vec3");
  });

  it("rejects missing fields with actionable errors", () => {
    expect(() => parseFeatureSchema({})).toThrow(/type/);
    expect(() => parseFeatureSchema({ type: "x" })).toThrow(/params/);
    expect(() =>
      parseFeatureSchema({ type: "x", params: [{ key: 7 }] }),
    ).toThrow(/key/);
    expect(() =>
      parseFeatureSchema({
        type: "x",
        params: [{ key: "k", name: "N", type: "f32", required: "yes" }],
      }),
    ).toThrow(/required/);
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
});

// ─── widgetKindFromRawType ─────────────────────────────────────

describe("widgetKindFromRawType", () => {
  it("maps every known kernel hint to a widget kind", () => {
    expect(widgetKindFromRawType("f32")).toBe("number");
    expect(widgetKindFromRawType("vec3")).toBe("vec3");
    expect(widgetKindFromRawType("bool")).toBe("bool");
    expect(widgetKindFromRawType("string")).toBe("string");
    expect(widgetKindFromRawType("bool_op")).toBe("bool_op");
    expect(widgetKindFromRawType("length")).toBe("length");
    expect(widgetKindFromRawType("angle")).toBe("angle");
    expect(widgetKindFromRawType("query")).toBe("query");
  });
});

// ─── defaults ──────────────────────────────────────────────────

describe("buildDefaultDefinition", () => {
  it("emits an initial value for every param matching its widget kind", () => {
    const schema = parseFeatureSchema(extrudeRaw);
    const def = buildDefaultDefinition(schema);
    expect(def.profiles).toBe("");
    expect(def.depth).toEqual({ value: 10, unit: "mm" });
    expect(def.profile_half_x).toBe(0);
    expect(def.profile_half_y).toBe(0);
    expect(def.operation).toBe("union");
    expect(def.center).toEqual([0, 0, 0]);
  });
});

describe("defaultForWidget", () => {
  it("returns a finite number for number widgets", () => {
    expect(defaultForWidget("number")).toBe(0);
  });
  it("returns a zero vec3 for vec3 widgets", () => {
    expect(defaultForWidget("vec3")).toEqual([0, 0, 0]);
  });
  it("returns 10mm for length widgets", () => {
    expect(defaultForWidget("length")).toEqual({ value: 10, unit: "mm" });
  });
  it("returns 0deg for angle widgets", () => {
    expect(defaultForWidget("angle")).toEqual({ value: 0, unit: "deg" });
  });
  it("returns 'union' for bool_op widgets", () => {
    expect(defaultForWidget("bool_op")).toBe("union");
  });
  it("returns null for unknown widgets", () => {
    expect(defaultForWidget("unknown")).toBe(null);
  });
});

// ─── readParam / updateParam ───────────────────────────────────

describe("updateParam + readParam", () => {
  it("updateParam returns a new object with the key replaced", () => {
    const before = { a: 1, b: 2 };
    const after = updateParam(before, "a", 42);
    expect(after).toEqual({ a: 42, b: 2 });
    // Immutability: the original is untouched.
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
    const stale = {} as Record<string, unknown>;
    expect(readParam(stale, depthParam)).toEqual({ value: 10, unit: "mm" });
  });

  it("readParam treats explicit null the same as missing", () => {
    const schema = parseFeatureSchema(extrudeRaw);
    const param = schema.params.find((p) => p.key === "profile_half_x")!;
    const stale = { profile_half_x: null } as Record<string, unknown>;
    expect(readParam(stale, param)).toBe(0);
  });
});

// ─── validateParamValue ────────────────────────────────────────

function paramOf(widget: FeatureSchema["params"][number]["widget"], label = "X") {
  return {
    key: "k",
    label,
    widget,
    required: true,
    rawType: widget,
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

  it("accepts 3-element number arrays for vec3", () => {
    expect(validateParamValue(paramOf("vec3"), [1, 2, 3])).toBeNull();
  });

  it("rejects wrong-length or non-number vec3 values", () => {
    expect(validateParamValue(paramOf("vec3"), [1, 2])).not.toBeNull();
    expect(validateParamValue(paramOf("vec3"), [1, 2, "3"])).not.toBeNull();
    expect(validateParamValue(paramOf("vec3"), "not an array")).not.toBeNull();
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

  it("accepts every BoolOp wire string", () => {
    for (const op of BOOL_OPS) {
      expect(validateParamValue(paramOf("bool_op"), op)).toBeNull();
    }
  });

  it("rejects unknown BoolOp strings", () => {
    expect(
      validateParamValue(paramOf("bool_op"), "subtract"),
    ).not.toBeNull();
  });

  it("accepts strings for query widgets", () => {
    expect(validateParamValue(paramOf("query"), "qCreatedBy(1)")).toBeNull();
    expect(validateParamValue(paramOf("query"), 42)).not.toBeNull();
  });

  it("returns null for unknown-widget fields without probing", () => {
    expect(
      validateParamValue(paramOf("unknown"), { arbitrary: "shape" }),
    ).toBeNull();
  });
});
