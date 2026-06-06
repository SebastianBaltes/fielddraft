import { describe, expect, it } from "vitest";
import { textToSubCalls } from "./sketchText";
import { GLYPHS } from "./sketchTextFont";

/** Pull the four corners of every emitted line into flat x/y lists. */
function coords(calls: ReturnType<typeof textToSubCalls>) {
  const xs: number[] = [];
  const ys: number[] = [];
  for (const c of calls) {
    const s = c.params.start as [number, number];
    const e = c.params.end as [number, number];
    xs.push(s[0], e[0]);
    ys.push(s[1], e[1]);
  }
  return { xs, ys };
}

describe("textToSubCalls", () => {
  it('emits "I" as scaled sketch_line strokes spanning y ~0..height', () => {
    const calls = textToSubCalls("I", { origin: [0, 0], height: 10 });

    // Every emitted call is a sketch_line.
    expect(calls.every((c) => c.type === "sketch_line")).toBe(true);

    // The "I" glyph is one stroke of N points → N-1 segments.
    const glyphSegments = GLYPHS["I"].reduce((n, s) => n + (s.length - 1), 0);
    expect(calls.length).toBe(glyphSegments);

    const { ys } = coords(calls);
    expect(Math.min(...ys)).toBeCloseTo(0, 6);
    expect(Math.max(...ys)).toBeCloseTo(10, 6);
  });

  it('"AB" advances so B is to the right of A', () => {
    const aOnly = textToSubCalls("A", { origin: [0, 0], height: 10 });
    const ab = textToSubCalls("AB", { origin: [0, 0], height: 10 });

    const aMaxX = Math.max(...coords(aOnly).xs);

    // B's segments are the ones beyond A's count.
    const bCalls = ab.slice(aOnly.length);
    expect(bCalls.length).toBeGreaterThan(0);
    const bMinX = Math.min(...coords(bCalls).xs);

    expect(bMinX).toBeGreaterThan(aMaxX - 1e-9);
    // And B starts well past the origin (~advance·height for A = 0.7·10).
    expect(bMinX).toBeGreaterThan(6);
  });

  it("space advances the pen but emits nothing for that char", () => {
    const ab = textToSubCalls("AB", { origin: [0, 0], height: 10 });
    const aSpaceB = textToSubCalls("A B", { origin: [0, 0], height: 10 });

    // Same number of lines (space draws nothing).
    expect(aSpaceB.length).toBe(ab.length);

    // But the B in "A B" is shifted right by one space advance (0.5·10 = 5).
    const aOnly = textToSubCalls("A", { origin: [0, 0], height: 10 });
    const bInAB = Math.min(...coords(ab.slice(aOnly.length)).xs);
    const bInASB = Math.min(...coords(aSpaceB.slice(aOnly.length)).xs);
    expect(bInASB - bInAB).toBeCloseTo(5, 6);
  });

  it("height scaling: height 20 yields exactly double height 10", () => {
    const h10 = textToSubCalls("AB", { origin: [0, 0], height: 10 });
    const h20 = textToSubCalls("AB", { origin: [0, 0], height: 20 });
    expect(h20.length).toBe(h10.length);
    for (let i = 0; i < h10.length; i++) {
      const s10 = h10[i].params.start as [number, number];
      const s20 = h20[i].params.start as [number, number];
      expect(s20[0]).toBeCloseTo(s10[0] * 2, 6);
      expect(s20[1]).toBeCloseTo(s10[1] * 2, 6);
    }
  });

  it("construction flag propagates to every sub-call", () => {
    const solid = textToSubCalls("ABC", { origin: [0, 0], height: 10 });
    expect(solid.every((c) => c.params.construction === false)).toBe(true);

    const constr = textToSubCalls("ABC", {
      origin: [0, 0],
      height: 10,
      construction: true,
    });
    expect(constr.length).toBeGreaterThan(0);
    expect(constr.every((c) => c.params.construction === true)).toBe(true);
  });

  it("origin translates every coordinate", () => {
    const at0 = textToSubCalls("A", { origin: [0, 0], height: 10 });
    const at100 = textToSubCalls("A", { origin: [100, 50], height: 10 });
    for (let i = 0; i < at0.length; i++) {
      const s0 = at0[i].params.start as [number, number];
      const s1 = at100[i].params.start as [number, number];
      expect(s1[0]).toBeCloseTo(s0[0] + 100, 6);
      expect(s1[1]).toBeCloseTo(s0[1] + 50, 6);
    }
  });

  it("letterSpacing widens the gap between glyphs", () => {
    const tight = textToSubCalls("AB", { origin: [0, 0], height: 10 });
    const loose = textToSubCalls("AB", {
      origin: [0, 0],
      height: 10,
      letterSpacing: 5,
    });
    const aLen = textToSubCalls("A", { origin: [0, 0], height: 10 }).length;
    const bTight = Math.min(...coords(tight.slice(aLen)).xs);
    const bLoose = Math.min(...coords(loose.slice(aLen)).xs);
    expect(bLoose - bTight).toBeCloseTo(5, 6);
  });

  // ── edge cases ────────────────────────────────────────────────────

  it("empty string yields []", () => {
    expect(textToSubCalls("", { origin: [0, 0], height: 10 })).toEqual([]);
  });

  it("unknown char is skipped without throwing, pen still advances", () => {
    expect(GLYPHS["@"]).toBeUndefined();
    const calls = textToSubCalls("A@B", { origin: [0, 0], height: 10 });
    // Only A and B drew lines.
    const ab = textToSubCalls("AB", { origin: [0, 0], height: 10 });
    expect(calls.length).toBe(ab.length);
    // But the "@" advanced the pen, so B in "A@B" sits further right than in "AB".
    const aLen = textToSubCalls("A", { origin: [0, 0], height: 10 }).length;
    const bAtB = Math.min(...coords(ab.slice(aLen)).xs);
    const bA_B = Math.min(...coords(calls.slice(aLen)).xs);
    expect(bA_B).toBeGreaterThan(bAtB);
  });

  it("lowercase is folded to uppercase glyphs", () => {
    const lower = textToSubCalls("abc", { origin: [0, 0], height: 10 });
    const upper = textToSubCalls("ABC", { origin: [0, 0], height: 10 });
    expect(lower.length).toBe(upper.length);
    for (let i = 0; i < lower.length; i++) {
      expect(lower[i].params.start).toEqual(upper[i].params.start);
      expect(lower[i].params.end).toEqual(upper[i].params.end);
    }
  });
});

describe("GLYPHS font table is well-formed", () => {
  it("every glyph stroke has >=1 point and stays within the cell (+margin)", () => {
    const MARGIN = 0.1; // small overshoot allowed for curve caps / descenders
    for (const [ch, glyph] of Object.entries(GLYPHS)) {
      for (const stroke of glyph) {
        expect(stroke.length, `${ch}: empty stroke`).toBeGreaterThanOrEqual(1);
        for (const [x, y] of stroke) {
          expect(Number.isFinite(x), `${ch}: x not finite`).toBe(true);
          expect(Number.isFinite(y), `${ch}: y not finite`).toBe(true);
          expect(x, `${ch}: x below cell`).toBeGreaterThanOrEqual(-MARGIN);
          expect(x, `${ch}: x above cell`).toBeLessThanOrEqual(1.2 + MARGIN);
          expect(y, `${ch}: y below cell`).toBeGreaterThanOrEqual(-MARGIN);
          expect(y, `${ch}: y above cell`).toBeLessThanOrEqual(1 + MARGIN);
        }
      }
    }
  });

  it("covers A-Z, 0-9, space and . - _", () => {
    for (let c = 65; c <= 90; c++) {
      expect(GLYPHS[String.fromCharCode(c)], String.fromCharCode(c)).toBeDefined();
    }
    for (let d = 0; d <= 9; d++) {
      expect(GLYPHS[String(d)], String(d)).toBeDefined();
    }
    for (const ch of [" ", ".", "-", "_"]) {
      expect(GLYPHS[ch], ch).toBeDefined();
    }
  });
});
