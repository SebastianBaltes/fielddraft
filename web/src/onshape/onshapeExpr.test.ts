import { describe, expect, it } from "vitest";

import {
  evalOnshapeExpression,
  quantityKind,
  quantityToDeg,
  quantityToMm,
  toFieldDraftFormula,
  tryEvalOnshapeExpression,
  type Quantity,
} from "./onshapeExpr";

/** mm of an expression, evaluated against `vars` (given in mm / degree). */
function mm(src: string, vars: Record<string, Quantity> = {}): number {
  const q = evalOnshapeExpression(src, new Map(Object.entries(vars)));
  const v = quantityToMm(q);
  expect(v, `"${src}" is not a length`).not.toBeNull();
  return v as number;
}

function deg(src: string, vars: Record<string, Quantity> = {}): number {
  const q = evalOnshapeExpression(src, new Map(Object.entries(vars)));
  const v = quantityToDeg(q);
  expect(v, `"${src}" is not an angle`).not.toBeNull();
  return v as number;
}

const len = (mmValue: number): Quantity => ({ v: mmValue, len: 1, ang: 0 });
const num = (value: number): Quantity => ({ v: value, len: 0, ang: 0 });

describe("onshapeExpr — unit literals", () => {
  it("converts every length unit Onshape writes", () => {
    expect(mm("25 cm")).toBe(250);
    expect(mm("2.5 mm")).toBe(2.5);
    expect(mm("0.31 cm")).toBeCloseTo(3.1, 10);
    expect(mm("1 m")).toBe(1000);
    expect(mm("1 in")).toBe(25.4);
    expect(mm("1 ft")).toBe(304.8);
    expect(mm("3 millimeters")).toBe(3);
  });

  it("reads a unit glued to its number and a `*`-separated one", () => {
    // Both forms occur verbatim in the ticket corpus (`2.3mm`, `0.0*m`).
    expect(mm("2.3mm")).toBeCloseTo(2.3, 10);
    expect(mm("8.1mm")).toBeCloseTo(8.1, 10);
    expect(mm("0.0*m")).toBe(0);
    expect(mm("1.5*cm")).toBe(15);
  });

  it("converts angles to degrees", () => {
    expect(deg("45 deg")).toBeCloseTo(45, 10);
    expect(deg("1.5 deg")).toBeCloseTo(1.5, 10);
    expect(deg("0.5 rad")).toBeCloseTo(28.6478897565, 8);
    expect(deg("90 degrees")).toBeCloseTo(90, 10);
  });

  it("keeps a length and an angle apart", () => {
    expect(quantityToDeg(evalOnshapeExpression("5 mm"))).toBeNull();
    expect(quantityToMm(evalOnshapeExpression("5 deg"))).toBeNull();
    // A bare zero is dimension-agnostic — Onshape writes it for unset params.
    expect(mm("0")).toBe(0);
    expect(deg("0")).toBe(0);
  });
});

describe("onshapeExpr — arithmetic", () => {
  it("honours precedence and parentheses", () => {
    expect(mm("2 mm + 3 mm * 4")).toBe(14);
    expect(mm("(2 + 3) * 4 mm")).toBe(20);
    expect(mm("12 mm / 4 + 1 mm")).toBe(4);
    expect(mm("2 * (3 + 4) mm")).toBe(14);
  });

  it("applies unary minus at any depth", () => {
    expect(mm("-5 mm")).toBe(-5);
    expect(mm("-(2 cm)")).toBe(-20);
    expect(mm("-#TubeOuterDiameter/2", { TubeOuterDiameter: len(12) })).toBe(-6);
    expect(mm("-(#a+2*#b)/2", { a: len(10), b: len(5) })).toBe(-10);
  });

  it("treats juxtaposition as multiplication (a trailing unit on a group)", () => {
    expect(mm("(2) cm")).toBe(20);
    expect(mm("(1 + 2) cm")).toBe(30);
  });

  it("tracks dimensions through multiplication and division", () => {
    // `1/cm` is an inverse length — the corpus uses it to make a ratio.
    expect(evalOnshapeExpression("1 cm * 1/cm")).toEqual({ v: 1, len: 0, ang: 0 });
    expect(evalOnshapeExpression("2 mm * 3 mm").len).toBe(2);
    expect(quantityToMm(evalOnshapeExpression("2 mm * 3 mm"))).toBeNull();
    expect(quantityKind(evalOnshapeExpression("4 mm / 2 mm"))).toBe("number");
  });

  it("refuses to add values of different dimension", () => {
    const bad = tryEvalOnshapeExpression("5 mm + 3");
    expect(bad.ok).toBe(false);
    expect(bad.ok === false && bad.error).toMatch(/Einheiten passen nicht zusammen/);
    // …but a literal zero may be added to anything.
    expect(mm("5 mm + 0")).toBe(5);
  });

  it("reports division by zero instead of returning Infinity", () => {
    const bad = tryEvalOnshapeExpression("5 mm / 0");
    expect(bad.ok).toBe(false);
    expect(bad.ok === false && bad.error).toMatch(/Division durch 0/);
  });
});

describe("onshapeExpr — variables", () => {
  it("resolves a bare reference and keeps its dimension", () => {
    expect(mm("#Brettstaerke", { Brettstaerke: len(15) })).toBe(15);
    expect(deg("#Winkel", { Winkel: { v: 45, len: 0, ang: 1 } })).toBeCloseTo(45, 10);
  });

  it("mixes literals with variables (the corpus's most common form)", () => {
    const vars = { HoleExtraGap: len(0.15) };
    expect(mm("3 mm+#HoleExtraGap", vars)).toBeCloseTo(3.15, 10);
    expect(mm("10 mm+#HoleExtraGap", vars)).toBeCloseTo(10.15, 10);
    expect(mm("#TubeOuterDiameter-2mm", { TubeOuterDiameter: len(12) })).toBeCloseTo(10, 10);
    expect(mm("2mm+#Z", { Z: len(4) })).toBe(6);
  });

  it("evaluates a chain of variables that reference each other", () => {
    // Exactly how the walk builds the table: each declaration sees the ones
    // before it (#Klingenlaenge → #Gesamtlaenge → …).
    const vars = new Map<string, Quantity>();
    vars.set("Klingenlaenge", evalOnshapeExpression("89 cm", vars));
    vars.set("Grifflaenge", evalOnshapeExpression("295 mm", vars));
    vars.set("Gesamtlaenge", evalOnshapeExpression("#Klingenlaenge+#Grifflaenge", vars));
    expect(quantityToMm(vars.get("Gesamtlaenge") as Quantity)).toBeCloseTo(1185, 10);
    expect(
      quantityToMm(evalOnshapeExpression("#Gesamtlaenge/2 + 1 cm", vars) as Quantity),
    ).toBeCloseTo(602.5, 10);
  });

  it("names an unknown variable instead of guessing", () => {
    const bad = tryEvalOnshapeExpression("3 mm+#Gibtsnicht");
    expect(bad.ok).toBe(false);
    expect(bad.ok === false && bad.error).toBe('unbekannte Variable "#Gibtsnicht"');
  });

  it("names Onshape's MEASURED placeholder for what it is", () => {
    const bad = tryEvalOnshapeExpression('"?"');
    expect(bad.ok).toBe(false);
    expect(bad.ok === false && bad.error).toMatch(/gemessener Wert/);
  });
});

describe("onshapeExpr — functions and constants", () => {
  it("evaluates trigonometry on angle arguments", () => {
    expect(evalOnshapeExpression("sin(30 deg)").v).toBeCloseTo(0.5, 10);
    expect(evalOnshapeExpression("cos(60 deg)").v).toBeCloseTo(0.5, 10);
    expect(quantityToDeg(evalOnshapeExpression("atan(1)") as Quantity)).toBeCloseTo(45, 10);
  });

  it("rejects a length where an angle belongs", () => {
    const bad = tryEvalOnshapeExpression("sin(2 mm)");
    expect(bad.ok).toBe(false);
    expect(bad.ok === false && bad.error).toMatch(/erwartet einen Winkel/);
  });

  it("knows pi, sqrt, abs, min/max and pow", () => {
    expect(evalOnshapeExpression("pi").v).toBeCloseTo(Math.PI, 12);
    expect(evalOnshapeExpression("sqrt(9)").v).toBe(3);
    expect(mm("abs(-4 mm)")).toBe(4);
    expect(mm("min(3 mm, 2 cm)")).toBe(3);
    expect(mm("max(3 mm, 2 cm)")).toBe(20);
    expect(mm("pow(2, 3) mm")).toBe(8);
    expect(mm("(2 ^ 3) mm")).toBe(8);
    // `^` binds a bare exponent; a UNIT after it is a dimension error, not a
    // silent reinterpretation (`2 ^ 3 mm` would be "two to the three
    // millimetres"). No corpus expression uses `^` at all.
    expect(tryEvalOnshapeExpression("2 ^ 3 mm").ok).toBe(false);
  });

  it("rounds to an integer (1 arg) and to a precision (2 args)", () => {
    expect(evalOnshapeExpression("round(2.6)").v).toBe(3);
    expect(evalOnshapeExpression("round(0.3574, .1)").v).toBeCloseTo(0.4, 10);
    expect(mm("round(3.57 mm, 0.5 mm)")).toBeCloseTo(3.5, 10);
  });

  it("names an unknown function rather than ignoring it", () => {
    const bad = tryEvalOnshapeExpression("wobble(2)");
    expect(bad.ok).toBe(false);
    expect(bad.ok === false && bad.error).toMatch(/unbekannte Funktion "wobble\(\)"/);
  });
});

describe("onshapeExpr — the corpus's hardest expressions (#417)", () => {
  it("evaluates the mirror-adjustment round/sin/inverse-unit expression", () => {
    // #ZDeltaMirrorAdjustement, verbatim from the #417 document:
    // (19 cm + 254 mm)/2 · sin(1.5°) = 3.573 mm → /cm = 0.3573 → round to 0.1
    // → 0.4 → · cm = 4 mm.
    const vars = { BoxMirrorGap: len(19), MirrorDiameter: len(254) };
    expect(
      mm("(round((#BoxMirrorGap+#MirrorDiameter)/2 * sin(1.5 deg) * 1/cm, .1)) cm", vars),
    ).toBeCloseTo(4, 10);
  });

  it("evaluates the teflon-pad expression (sqrt, pi, trailing unit)", () => {
    // #TeflonPadDiameter = (2·√8 / √(3π)) cm = 18.4264 mm.
    expect(mm("(2 * sqrt(#TelescopeWeight) / sqrt(3*pi)) cm", { TelescopeWeight: num(8) })).toBeCloseTo(
      18.4263546,
      6,
    );
  });

  it("evaluates the remaining corpus shapes", () => {
    const vars = {
      MirrorDiameter: len(254),
      CellHolderOuterOffset: len(18),
      TopInnerDiameter: len(280),
      TopTubeOuterDiameter: len(310),
      MirrorBoxLength: len(400),
      BearingThickness: len(12),
      MirrorSideWhipCellDistanceZ: len(2),
    };
    expect(mm("#MirrorDiameter+#CellHolderOuterOffset+0.6cm", vars)).toBeCloseTo(278, 10);
    expect(mm("#TopInnerDiameter/2+3cm", vars)).toBeCloseTo(170, 10);
    expect(mm("#TopTubeOuterDiameter*2/3", vars)).toBeCloseTo(206.6666667, 6);
    expect(mm("#MirrorBoxLength+2*#BearingThickness+1cm", vars)).toBeCloseTo(434, 10);
    expect(mm("-(#MirrorBoxLength+2*#BearingThickness)/2", vars)).toBeCloseTo(-212, 10);
    expect(mm("1.04cm-1mm+#MirrorSideWhipCellDistanceZ", vars)).toBeCloseTo(11.4, 10);
  });
});

describe("onshapeExpr — projection onto a FieldDraft formula", () => {
  const fd = (n: string) => (n === "HoleExtraGap" ? "HoleExtraGap" : n === "d" ? "Durchmesser" : undefined);

  it("renders a length expression in millimetres", () => {
    expect(toFieldDraftFormula("3 mm+#HoleExtraGap", fd)).toBe("((3 * 1) + HoleExtraGap)");
    expect(toFieldDraftFormula("#d/2+3cm", fd)).toBe("((Durchmesser / 2) + (3 * 10))");
  });

  it("refuses what FieldDraft's dimensionless evaluator cannot mirror", () => {
    // Trigonometry: FieldDraft's angle base is DEGREES, sin() wants radians.
    expect(toFieldDraftFormula("sin(1.5 deg) * #d", fd)).toBeNull();
    // An angle literal has no faithful millimetre projection.
    expect(toFieldDraftFormula("#d + 45 deg", fd)).toBeNull();
    // A variable the caller does not want projected vetoes the render.
    expect(toFieldDraftFormula("#Unbekannt + 1 mm", fd)).toBeNull();
    // Nonsense stays null rather than throwing.
    expect(toFieldDraftFormula("3 mm +", fd)).toBeNull();
  });
});
