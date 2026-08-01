import { describe, it, expect } from "vitest";
import {
  fd,
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
  buildSchema,
} from "./dsl";
import { parseFeatureSchema, widgetKindFor } from "../widgets/schema";

describe("FieldSkript param DSL — leaf builders", () => {
  it("length defaults unit to mm and is required without a default", () => {
    expect(length("diameter", { name: "Diameter" })).toEqual({
      key: "diameter",
      name: "Diameter",
      type: "length",
      required: true,
      unit: "mm",
    });
  });

  it("a provided default flips required to false and emits the default", () => {
    expect(length("depth", { default: 20 })).toEqual({
      key: "depth",
      name: "Depth", // prettified from key
      type: "length",
      required: false,
      unit: "mm",
      default: 20,
    });
  });

  it("real with uiHint slider carries min/max/step", () => {
    const p = real("ratio", { uiHint: "slider", min: 0, max: 1, step: 0.05, default: 0.5 });
    expect(p).toEqual({
      key: "ratio",
      name: "Ratio",
      type: "f32",
      required: false,
      ui_hint: "slider",
      min: 0,
      max: 1,
      step: 0.05,
      default: 0.5,
    });
    expect(widgetKindFor(p.type, p.ui_hint as never)).toBe("slider");
  });

  it("bool with opposite_direction maps to a flip widget", () => {
    const p = bool("flip", { uiHint: "opposite_direction" });
    expect(p.type).toBe("bool");
    expect(p.ui_hint).toBe("opposite_direction");
    expect(widgetKindFor(p.type, p.ui_hint as never)).toBe("flip");
  });

  it("angle defaults unit to deg", () => {
    expect(angle("twist").unit).toBe("deg");
  });

  it("str produces a string param", () => {
    expect(str("label", { default: "" })).toMatchObject({ type: "string", required: false });
  });

  it("vec3 + color uiHint maps to color widget", () => {
    const p = vec3("tint", { uiHint: "color" });
    expect(widgetKindFor(p.type, p.ui_hint as never)).toBe("color");
  });

  it("vec2 produces a vec2 param", () => {
    expect(vec2("uv").type).toBe("vec2");
  });

  it("omits absent keys entirely (no undefined noise)", () => {
    const p = bool("on");
    expect(Object.keys(p).sort()).toEqual(["key", "name", "required", "type"]);
  });
});

describe("FieldSkript param DSL — enum", () => {
  it("normalizes {key,label}[] and defaults to horizontal_enum", () => {
    const p = enumParam("op", [
      { key: "union", label: "Union" },
      { key: "difference", label: "Difference" },
    ]);
    expect(p).toEqual({
      key: "op",
      name: "Op",
      type: "enum",
      required: true,
      ui_hint: "horizontal_enum",
      variants: [
        { key: "union", label: "Union" },
        { key: "difference", label: "Difference" },
      ],
    });
  });

  it("accepts a string[] (label = prettified key)", () => {
    const p = enumParam("style", ["solid", "wire_frame"]);
    expect(p.variants).toEqual([
      { key: "solid", label: "Solid" },
      { key: "wire_frame", label: "Wire frame" },
    ]);
  });

  it("accepts a Record<key,label>", () => {
    const p = enumParam("mode", { a: "Alpha", b: "Beta" }, { default: "a" });
    expect(p.variants).toEqual([
      { key: "a", label: "Alpha" },
      { key: "b", label: "Beta" },
    ]);
    expect(p.required).toBe(false);
  });

  it("throws on empty variants", () => {
    expect(() => enumParam("x", [])).toThrow(/non-empty/);
  });
});

describe("FieldSkript param DSL — query", () => {
  it("single pick → query with entity_filter", () => {
    const p = query("faces", { filter: "FACE", maxPicks: 1 });
    expect(p).toEqual({
      key: "faces",
      name: "Faces",
      type: "query",
      required: true,
      entity_filter: "FACE",
      max_picks: 1,
    });
  });

  it("maxPicks other than 1 → multi_query", () => {
    const p = query("edges", { filter: "EDGE", maxPicks: 8 });
    expect(p.type).toBe("multi_query");
    expect(p.max_picks).toBe(8);
  });
});

describe("FieldSkript param DSL — containers", () => {
  it("group derives a slug key and carries driving_param + children", () => {
    const g = group("Counterbore Options", { drivingParam: "use_cbore" }, [
      length("cbDiameter", { default: 14 }),
    ]);
    expect(g.key).toBe("counterbore_options");
    expect(g.type).toBe("group");
    expect(g.required).toBe(false);
    expect(g.driving_param).toBe("use_cbore");
    expect(g.children).toHaveLength(1);
  });

  it("visibleWhen wraps a param with a visible_when predicate", () => {
    const p = visibleWhen("flip", true, length("offset", { default: 0 }));
    expect(p.visible_when).toEqual([{ param: "flip", equals: true }]);
    expect(p.type).toBe("length");
  });
});

describe("buildSchema → parseFeatureSchema round-trip", () => {
  // A faithful reproduction of the concept §4 Counterbore example.
  const definition = feature({
    name: "Counterbore Hole",
    nameTemplate: "Counterbore (#diameter)",
    parameters: [
      query("faces", { name: "Placement faces", filter: "FACE", maxPicks: 1 }),
      length("diameter", { name: "Diameter", min: 0, default: 8, max: 1000 }),
      length("depth", { name: "Depth", min: 0, default: 20 }),
      bool("flip", { name: "Flip", uiHint: "opposite_direction" }),
      enumParam("fit", [
        { key: "close", label: "Close" },
        { key: "free", label: "Free" },
      ]),
      group("Counterbore", { drivingParam: "use_cbore" }, [
        length("cbDiameter", { name: "C'bore Ø", default: 14 }),
        length("cbDepth", { name: "C'bore depth", default: 5 }),
      ]),
    ],
  });

  it("produces a schema with the script id as `type`", () => {
    const schema = buildSchema(definition, "counterbore_hole");
    expect(schema.type).toBe("counterbore_hole");
    expect(schema.params).toHaveLength(6);
  });

  it("parses cleanly through the FeatureDialog schema parser", () => {
    const raw = buildSchema(definition, "counterbore_hole");
    const parsed = parseFeatureSchema(raw);
    expect(parsed.type).toBe("counterbore_hole");

    const byKey = Object.fromEntries(parsed.params.map((p) => [p.key, p]));
    expect(byKey.faces.widget).toBe("query");
    expect(byKey.faces.entityFilter).toBe("FACE");
    expect(byKey.diameter.widget).toBe("length");
    expect(byKey.diameter.required).toBe(false);
    expect(byKey.flip.widget).toBe("flip");
    expect(byKey.fit.widget).toBe("enum");
    expect(byKey.fit.variants).toHaveLength(2);

    const grp = byKey.Counterbore ?? byKey.counterbore;
    expect(grp.widget).toBe("group");
    expect(grp.drivingParam).toBe("use_cbore");
    expect(grp.children).toHaveLength(2);
  });

  it("the fd namespace mirrors the named builders", () => {
    expect(fd.length).toBe(length);
    expect(fd.feature).toBe(feature);
    expect(fd.enumParam).toBe(enumParam);
  });
});
