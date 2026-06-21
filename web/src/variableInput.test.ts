import { describe, expect, it } from "vitest";
import {
  baseUnitLabel,
  evalFormula,
  formulaDependencies,
  looksLikeFormula,
  parseVariableInput,
  resolveVariableTable,
  variableDisplayText,
  type RawVariable,
} from "./variableInput";

describe("parseVariableInput — literals", () => {
  it("parses a plain number as Number", () => {
    const r = parseVariableInput("42");
    expect(r).toEqual({ ok: true, value: 42, kind: "number", isFormula: false });
  });

  it("parses negative and decimal numbers", () => {
    expect(parseVariableInput("-3.5")).toMatchObject({ value: -3.5, kind: "number" });
    expect(parseVariableInput(".25")).toMatchObject({ value: 0.25, kind: "number" });
  });

  it("infers Length from a length unit and normalises to mm", () => {
    const r = parseVariableInput("10 cm");
    expect(r).toMatchObject({ ok: true, value: 100, kind: "length", isFormula: false });
  });

  it("accepts a unit without a space", () => {
    expect(parseVariableInput("5mm")).toMatchObject({ value: 5, kind: "length" });
    expect(parseVariableInput("2m")).toMatchObject({ value: 2000, kind: "length" });
  });

  it("infers Angle from deg / rad / ° and normalises to degrees", () => {
    expect(parseVariableInput("90 deg")).toMatchObject({ value: 90, kind: "angle" });
    expect(parseVariableInput("90°")).toMatchObject({ value: 90, kind: "angle" });
    const rad = parseVariableInput("3.14159 rad");
    expect(rad.ok && Math.abs(rad.value - 180) < 0.01).toBe(true);
  });

  it("rejects an unknown unit", () => {
    const r = parseVariableInput("10 furlong");
    expect(r.ok).toBe(false);
  });

  it("rejects empty input", () => {
    expect(parseVariableInput("   ").ok).toBe(false);
  });

  it("tolerates a leading = sigil on a literal", () => {
    expect(parseVariableInput("=42")).toMatchObject({ value: 42, kind: "number" });
  });
});

describe("parseVariableInput — formulas", () => {
  const known = new Set(["Breite", "Hoehe"]);

  it("recognises an arithmetic formula referencing a variable", () => {
    const r = parseVariableInput("Breite * 2", known);
    expect(r).toMatchObject({ ok: true, isFormula: true, source: "Breite * 2" });
  });

  it("recognises a function call as a formula", () => {
    expect(parseVariableInput("sin(Breite)", known)).toMatchObject({ isFormula: true });
  });

  it("rejects an unknown variable reference up front", () => {
    const r = parseVariableInput("Tiefe + 1", known);
    expect(r.ok).toBe(false);
    expect(r.ok === false && r.error).toContain("Tiefe");
  });

  it("rejects an unknown function", () => {
    const r = parseVariableInput("frob(Breite)", known);
    expect(r.ok).toBe(false);
  });

  it("accepts builtin constants without declaring them", () => {
    expect(parseVariableInput("pi * 2", known)).toMatchObject({ isFormula: true });
  });
});

describe("looksLikeFormula", () => {
  it("is false for literals and true for expressions", () => {
    expect(looksLikeFormula("42")).toBe(false);
    expect(looksLikeFormula("10 cm")).toBe(false);
    expect(looksLikeFormula("Breite + 1")).toBe(true);
    expect(looksLikeFormula("sin(x)")).toBe(true);
  });
});

describe("evalFormula", () => {
  it("honours precedence and parens", () => {
    expect(evalFormula("2 + 3 * 4", {})).toBe(14);
    expect(evalFormula("(2 + 3) * 4", {})).toBe(20);
  });
  it("resolves variables from env", () => {
    expect(evalFormula("w / 2 + 5", { w: 10 })).toBe(10);
  });
  it("evaluates builtins and constants", () => {
    expect(Math.abs(evalFormula("sin(pi / 2)", {}) - 1)).toBeLessThan(1e-6);
    expect(evalFormula("max(3, 7)", {})).toBe(7);
  });
  it("right-associates ^", () => {
    expect(evalFormula("2 ^ 3 ^ 2", {})).toBe(512);
  });
  it("throws on division by zero", () => {
    expect(() => evalFormula("1 / 0", {})).toThrow();
  });
});

describe("formulaDependencies", () => {
  it("lists referenced variables and skips functions/constants", () => {
    const known = new Set(["a", "b"]);
    expect(formulaDependencies("sin(a) + b * pi", known)).toEqual(["a", "b"]);
  });
});

describe("resolveVariableTable", () => {
  const mk = (vs: RawVariable[]) => resolveVariableTable(vs);

  it("resolves a literal table", () => {
    const { resolved, errors } = mk([
      { name: "Breite", value: 100, kind: "length" },
    ]);
    expect(errors.size).toBe(0);
    expect(resolved.get("Breite")?.value).toBe(100);
  });

  it("treats a unit literal's source as display-only, not a formula", () => {
    // `10 cm` carries source text for redisplay but must resolve via its
    // value (100 mm), never be evaluated as an expression (regression: the
    // resolver used to eval any non-empty `formula` and choke on `10 cm`).
    const { resolved, errors } = mk([
      { name: "Breite", value: 100, kind: "length", formula: "10 cm" },
    ]);
    expect(errors.size).toBe(0);
    expect(resolved.get("Breite")?.value).toBe(100);
  });

  it("resolves a formula referencing another variable", () => {
    const { resolved, errors } = mk([
      { name: "Breite", value: 50, kind: "length" },
      { name: "Doppelt", value: NaN, kind: "length", formula: "Breite * 2" },
    ]);
    expect(errors.size).toBe(0);
    expect(resolved.get("Doppelt")?.value).toBe(100);
    expect(resolved.get("Doppelt")?.kind).toBe("length");
  });

  it("resolves sin() of a number variable", () => {
    const { resolved } = mk([
      { name: "x", value: 0, kind: "number" },
      { name: "s", value: NaN, kind: "number", formula: "sin(x)" },
    ]);
    expect(resolved.get("s")?.value).toBe(0);
  });

  it("detects a direct cycle A=B, B=A", () => {
    const { errors } = mk([
      { name: "A", value: NaN, kind: "number", formula: "B" },
      { name: "B", value: NaN, kind: "number", formula: "A" },
    ]);
    expect(errors.has("A") || errors.has("B")).toBe(true);
    const msg = errors.get("A") ?? errors.get("B") ?? "";
    expect(msg.toLowerCase()).toContain("cycle");
  });

  it("detects a self-cycle A=A", () => {
    const { errors } = mk([
      { name: "A", value: NaN, kind: "number", formula: "A + 1" },
    ]);
    expect(errors.has("A")).toBe(true);
  });

  it("detects an indirect cycle A->B->C->A", () => {
    const { errors } = mk([
      { name: "A", value: NaN, kind: "number", formula: "B" },
      { name: "B", value: NaN, kind: "number", formula: "C" },
      { name: "C", value: NaN, kind: "number", formula: "A" },
    ]);
    expect(errors.size).toBeGreaterThan(0);
  });

  it("reports an unknown reference without crashing", () => {
    const { errors } = mk([
      { name: "A", value: NaN, kind: "number", formula: "Ghost" },
    ]);
    expect(errors.has("A")).toBe(true);
  });
});

describe("display helpers", () => {
  it("baseUnitLabel maps kinds to suffixes", () => {
    expect(baseUnitLabel("length")).toBe("mm");
    expect(baseUnitLabel("angle")).toBe("°");
    expect(baseUnitLabel("number")).toBe("");
  });
  it("variableDisplayText prefers the source text", () => {
    expect(variableDisplayText({ value: 100, kind: "length", formula: "10 cm" })).toBe("10 cm");
    expect(variableDisplayText({ value: 42, kind: "number" })).toBe("42");
  });
});
