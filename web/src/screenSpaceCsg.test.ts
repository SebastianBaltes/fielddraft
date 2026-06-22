import { describe, it, expect } from "vitest";
import {
  classifySubtractionPixel,
  classifyUnionPixel,
  collectCsgOperands,
  bodyNeedsSubtractionPass,
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
