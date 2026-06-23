import { describe, it, expect } from "vitest";
import {
  classifySubtractionPixel,
  classifyUnionPixel,
  classifyIntersectionPixel,
  collectCsgOperands,
  collectIntersectionOperands,
  bodyNeedsSubtractionPass,
  bodyNeedsIntersectionPass,
  viewZToNdcDepth,
  type DepthInterval,
} from "./screenSpaceCsg";
import type { HybridLeafInfo } from "./kernel";

/**
 * Tests für die REINE Screen-Space-CSG-Logik (Stage 1). Das ist der einzige
 * headless verifizierbare Teil: die GPU-Tiefen + Shader-Optik sind hier NICHT
 * abgedeckt (kein Browser/GPU) — sie brauchen einen Browser-Test des Users.
 */

const interval = (front: number, back: number): DepthInterval => ({
  front,
  back,
});

/** Minimaler Leaf-Stub — nur das `role`-Feld zählt für die Aufteilung. */
function leaf(role: "mesh" | "skip", kind = "box"): HybridLeafInfo {
  return {
    kind,
    aabb: [0, 0, 0, 0, 0, 0],
    transform: [0, 0, 0, 0, 0, 0, 1, 1, 1, 1],
    params: [],
    role,
  };
}

describe("classifySubtractionPixel — A − B Intervall-Klassifikation", () => {
  it("keep: kein Subtrahend an diesem Pixel", () => {
    const r = classifySubtractionPixel(10, null);
    expect(r.action).toBe("keep");
    expect(r.replacementDepth).toBe(10);
  });

  it("keep: Minuend-Fläche VOR dem Subtrahend-Eintritt", () => {
    // aDepth < bFront → der Schnittkörper liegt hinter der sichtbaren Fläche.
    const r = classifySubtractionPixel(5, interval(8, 12));
    expect(r.action).toBe("keep");
    expect(r.replacementDepth).toBe(5);
  });

  it("keep: Minuend-Fläche HINTER dem Subtrahend-Austritt", () => {
    // aDepth > bBack → der Schnittkörper liegt komplett vor der Fläche.
    const r = classifySubtractionPixel(20, interval(8, 12));
    expect(r.action).toBe("keep");
    expect(r.replacementDepth).toBe(20);
  });

  it("cut: Minuend-Fläche IM Subtrahend-Intervall → Backface wird sichtbar", () => {
    // aDepth ∈ (bFront, bBack); kein aBack angegeben → reiner Cut auf bBack.
    const r = classifySubtractionPixel(10, interval(8, 14));
    expect(r.action).toBe("cut");
    expect(r.replacementDepth).toBe(14);
  });

  it("cut: bBack liegt noch VOR dem Minuend-Austritt → gültige Schnittfläche", () => {
    // Subtrahend-Austritt (14) < Minuend-Austritt (20) → Material bleibt.
    const r = classifySubtractionPixel(10, interval(8, 14), 20);
    expect(r.action).toBe("cut");
    expect(r.replacementDepth).toBe(14);
  });

  it("discard: bBack HINTER dem Minuend-Austritt → kein Material mehr", () => {
    // Subtrahend frisst durch den ganzen Körper (bBack 25 >= aBack 20).
    const r = classifySubtractionPixel(10, interval(8, 25), 20);
    expect(r.action).toBe("discard");
  });

  it("Randfall: aDepth exakt auf bFront bleibt keep (Epsilon-tolerant)", () => {
    const r = classifySubtractionPixel(8, interval(8, 12));
    expect(r.action).toBe("keep");
  });

  it("Randfall: aDepth exakt auf bBack bleibt keep (Epsilon-tolerant)", () => {
    const r = classifySubtractionPixel(12, interval(8, 12));
    expect(r.action).toBe("keep");
  });
});

describe("classifyUnionPixel — Min-Tiefe (Stage-1 Union-Logik)", () => {
  it("nimmt die nächste (kleinste) Operandentiefe", () => {
    expect(classifyUnionPixel([12, 7, 20])).toBe(7);
  });

  it("ignoriert fehlende Treffer (null)", () => {
    expect(classifyUnionPixel([null, 9, null])).toBe(9);
  });

  it("liefert null, wenn kein Operand trifft", () => {
    expect(classifyUnionPixel([null, null])).toBeNull();
  });
});

describe("collectCsgOperands — Aufteilung nach role", () => {
  it("trennt Minuend (mesh) von Subtrahend (skip)", () => {
    const leaves = [leaf("mesh"), leaf("skip"), leaf("mesh")];
    const { minuend, subtrahend } = collectCsgOperands(leaves);
    expect(minuend).toHaveLength(2);
    expect(subtrahend).toHaveLength(1);
  });

  it("reiner Union-Body (nur mesh) → leerer Subtrahend", () => {
    const { minuend, subtrahend } = collectCsgOperands([
      leaf("mesh"),
      leaf("mesh"),
    ]);
    expect(minuend).toHaveLength(2);
    expect(subtrahend).toHaveLength(0);
  });
});

describe("bodyNeedsSubtractionPass — lohnt sich ein A−B-Pass?", () => {
  it("true bei Minuend + Subtrahend (echte Subtraktion)", () => {
    expect(bodyNeedsSubtractionPass([leaf("mesh"), leaf("skip")])).toBe(true);
  });

  it("false ohne Subtrahend (nichts wegzuschneiden)", () => {
    expect(bodyNeedsSubtractionPass([leaf("mesh"), leaf("mesh")])).toBe(false);
  });

  it("false ohne Minuend (nichts zu schneiden)", () => {
    expect(bodyNeedsSubtractionPass([leaf("skip")])).toBe(false);
  });

  it("false bei leerem Body", () => {
    expect(bodyNeedsSubtractionPass([])).toBe(false);
  });
});

describe("classifyIntersectionPixel — A ∩ B Intervall-Klassifikation", () => {
  it("show: überlappende Intervalle → Eintritt = max(Front), useA wenn A-Front weiter hinten", () => {
    // A [5,15], B [8,20] → entry=8 (B-Front), exit=15. B-Front > A-Front → useA=false.
    const r = classifyIntersectionPixel(5, 15, 8, 20);
    expect(r.action).toBe("show");
    expect(r.depth).toBe(8);
    expect(r.useA).toBe(false);
  });

  it("show: A-Front weiter hinten → useA = true", () => {
    // A [9,20], B [5,15] → entry=9 (A-Front), exit=15. A-Front >= B-Front → useA=true.
    const r = classifyIntersectionPixel(9, 20, 5, 15);
    expect(r.action).toBe("show");
    expect(r.depth).toBe(9);
    expect(r.useA).toBe(true);
  });

  it("discard: kein Treffer in A (aFront <= 0)", () => {
    expect(classifyIntersectionPixel(0, 0, 8, 20).action).toBe("discard");
  });

  it("discard: kein Treffer in B (bFront <= 0)", () => {
    expect(classifyIntersectionPixel(5, 15, 0, 0).action).toBe("discard");
  });

  it("discard: disjunkte Intervalle (leerer Schnitt)", () => {
    // A [2,5], B [10,14] → entry=10, exit=5 → entry >= exit → leer.
    expect(classifyIntersectionPixel(2, 5, 10, 14).action).toBe("discard");
  });

  it("discard: berührende Intervalle (entry == exit, Epsilon-tolerant)", () => {
    // A [2,8], B [8,14] → entry=8, exit=8 → entry >= exit - EPS → leer.
    expect(classifyIntersectionPixel(2, 8, 8, 14).action).toBe("discard");
  });

  it("show: A komplett in B → Eintritt = A-Front, useA", () => {
    // A [6,9] ⊂ B [3,20] → entry=6 (A-Front), exit=9. A-Front >= B-Front → useA=true.
    const r = classifyIntersectionPixel(6, 9, 3, 20);
    expect(r.action).toBe("show");
    expect(r.depth).toBe(6);
    expect(r.useA).toBe(true);
  });
});

describe("collectIntersectionOperands — Aufteilung der zwei skip-Leaves", () => {
  it("a = [leaf0], b = [leaf1]", () => {
    const l0 = leaf("skip", "box");
    const l1 = leaf("skip", "cylinder");
    const { a, b } = collectIntersectionOperands([l0, l1]);
    expect(a).toEqual([l0]);
    expect(b).toEqual([l1]);
  });
});

describe("bodyNeedsIntersectionPass — binärer Schnitt erkannt?", () => {
  it("true bei genau zwei skip-Leaves (Intersection-Fingerabdruck)", () => {
    expect(bodyNeedsIntersectionPass([leaf("skip"), leaf("skip")])).toBe(true);
  });

  it("false bei Difference (ein mesh-Leaf vorhanden)", () => {
    expect(bodyNeedsIntersectionPass([leaf("mesh"), leaf("skip")])).toBe(false);
  });

  it("false bei nur einem skip-Leaf", () => {
    expect(bodyNeedsIntersectionPass([leaf("skip")])).toBe(false);
  });

  it("false bei drei skip-Leaves (nicht binär)", () => {
    expect(
      bodyNeedsIntersectionPass([leaf("skip"), leaf("skip"), leaf("skip")]),
    ).toBe(false);
  });

  it("false bei leerem Body", () => {
    expect(bodyNeedsIntersectionPass([])).toBe(false);
  });

  it("false bei zwei mesh-Leaves (reine Union)", () => {
    expect(bodyNeedsIntersectionPass([leaf("mesh"), leaf("mesh")])).toBe(false);
  });
});

describe("viewZToNdcDepth — lineare Sichttiefe → gl_FragDepth", () => {
  // Standard-Perspektiv-Projektion (rechtshändig, NDC z ∈ [-1, 1]):
  //   m10 = -(far + near) / (far - near)
  //   m14 = -2 * far * near / (far - near)
  const near = 1;
  const far = 100;
  const m10 = -(far + near) / (far - near);
  const m14 = (-2 * far * near) / (far - near);

  it("Near-Plane (viewZ == near) → window depth 0", () => {
    expect(viewZToNdcDepth(near, m10, m14)).toBeCloseTo(0, 6);
  });

  it("Far-Plane (viewZ == far) → window depth 1", () => {
    expect(viewZToNdcDepth(far, m10, m14)).toBeCloseTo(1, 6);
  });

  it("monoton wachsend mit der Tiefe (näher = kleinere Depth)", () => {
    const dNear = viewZToNdcDepth(2, m10, m14);
    const dMid = viewZToNdcDepth(20, m10, m14);
    const dFar = viewZToNdcDepth(80, m10, m14);
    expect(dNear).toBeLessThan(dMid);
    expect(dMid).toBeLessThan(dFar);
  });

  it("alle Tiefen im gültigen Window-Bereich [0, 1]", () => {
    // Toleranz für Float-Rundung an den Clip-Plane-Rändern (z.B. near → -1e-16).
    const tol = 1e-9;
    for (const z of [1, 5, 25, 50, 100]) {
      const d = viewZToNdcDepth(z, m10, m14);
      expect(d).toBeGreaterThanOrEqual(-tol);
      expect(d).toBeLessThanOrEqual(1 + tol);
    }
  });
});
