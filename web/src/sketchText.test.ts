import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import opentype, { type Font } from "opentype.js";
import { textToSubCalls } from "./sketchText";
import type { PathContour, SubCall } from "./importVector";

/**
 * F24-full unit tests. We parse the bundled DejaVu Sans from disk (the same
 * file shipped in public/fonts/) and assert the glyph→sketch_path layout:
 * holes, lowercase/punctuation coverage, pen advance, cubic→quad, upright
 * orientation, and the empty/space/unsupported edge cases.
 */

let font: Font;

beforeAll(() => {
  // Vitest runs with cwd at the web/ package root.
  const fontPath = resolve(process.cwd(), "public/fonts/DejaVuSans.ttf");
  const buf = readFileSync(fontPath);
  // Hand opentype a tight ArrayBuffer slice of the file's bytes.
  const ab = buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength);
  font = opentype.parse(ab);
});

/** Narrow a SubCall to its contour list (all our text calls are sketch_path). */
function contoursOf(call: SubCall): PathContour[] {
  expect(call.type).toBe("sketch_path");
  return call.params.contours as PathContour[];
}

/** Every endpoint x of a sketch_path call (start + each segment `to`). */
function xsOf(call: SubCall): number[] {
  const xs: number[] = [];
  for (const c of contoursOf(call)) {
    xs.push(c.start[0]);
    for (const s of c.segments) xs.push(s.to[0]);
  }
  return xs;
}

/** Every endpoint y of a sketch_path call. */
function ysOf(call: SubCall): number[] {
  const ys: number[] = [];
  for (const c of contoursOf(call)) {
    ys.push(c.start[1]);
    for (const s of c.segments) ys.push(s.to[1]);
  }
  return ys;
}

describe("textToSubCalls (filled glyphs)", () => {
  it('"O" yields a sketch_path with a hole (>=2 contours)', () => {
    const calls = textToSubCalls(font, "O", { origin: [0, 0], height: 10 });
    expect(calls.length).toBe(1);
    expect(calls[0].type).toBe("sketch_path");
    expect(contoursOf(calls[0]).length).toBeGreaterThanOrEqual(2);
  });

  it('"A" yields a sketch_path with a counter (>=2 contours)', () => {
    const calls = textToSubCalls(font, "A", { origin: [0, 0], height: 10 });
    expect(calls.length).toBe(1);
    expect(contoursOf(calls[0]).length).toBeGreaterThanOrEqual(2);
  });

  it("lowercase l and i produce geometry (no uppercase fold)", () => {
    for (const ch of ["l", "i"]) {
      const calls = textToSubCalls(font, ch, { origin: [0, 0], height: 10 });
      expect(calls.length, ch).toBe(1);
      expect(contoursOf(calls[0]).length, ch).toBeGreaterThanOrEqual(1);
    }
  });

  it("lowercase differs from uppercase (verbatim layout)", () => {
    const lower = textToSubCalls(font, "a", { origin: [0, 0], height: 10 });
    const upper = textToSubCalls(font, "A", { origin: [0, 0], height: 10 });
    // Different glyph outlines → different contour signatures.
    const sig = (c: SubCall) =>
      contoursOf(c)
        .map((x) => x.segments.length)
        .join(",");
    expect(sig(lower[0])).not.toBe(sig(upper[0]));
  });

  it("punctuation '.' produces geometry", () => {
    const calls = textToSubCalls(font, ".", { origin: [0, 0], height: 10 });
    expect(calls.length).toBe(1);
    expect(contoursOf(calls[0]).length).toBeGreaterThanOrEqual(1);
  });

  it('"AB" lays B to the right of A (pen advance)', () => {
    const calls = textToSubCalls(font, "AB", { origin: [0, 0], height: 10 });
    expect(calls.length).toBe(2);
    const aMaxX = Math.max(...xsOf(calls[0]));
    const bMinX = Math.min(...xsOf(calls[1]));
    // B's left edge sits at/after A's right edge (glyphs don't overlap).
    expect(bMinX).toBeGreaterThan(aMaxX - 1e-6);
    // And B clearly starts past the origin.
    expect(bMinX).toBeGreaterThan(3);
  });

  it("a glyph's cubic/curved outline becomes quad segments", () => {
    // "O" is built from curves → its outline must carry quad segments.
    const calls = textToSubCalls(font, "O", { origin: [0, 0], height: 10 });
    const segs = contoursOf(calls[0]).flatMap((c) => c.segments);
    const quads = segs.filter((s) => s.kind === "quad");
    expect(quads.length).toBeGreaterThan(0);
    // Every emitted segment is either a line or a quad (no cubics leak).
    expect(segs.every((s) => s.kind === "line" || s.kind === "quad")).toBe(true);
  });

  it("a capital letter is right-side-up (top above baseline)", () => {
    const calls = textToSubCalls(font, "T", { origin: [0, 0], height: 10 });
    const ys = ysOf(calls[0]);
    // The baseline sits at y≈0 (origin y); the glyph's top must be well
    // above it (positive y), proving y-up orientation (not flipped).
    expect(Math.max(...ys)).toBeGreaterThan(5);
    expect(Math.min(...ys)).toBeGreaterThanOrEqual(-0.5);
  });

  it("origin translates every coordinate", () => {
    const at0 = textToSubCalls(font, "A", { origin: [0, 0], height: 10 });
    const at100 = textToSubCalls(font, "A", { origin: [100, 50], height: 10 });
    const s0 = contoursOf(at0[0])[0].start;
    const s1 = contoursOf(at100[0])[0].start;
    expect(s1[0]).toBeCloseTo(s0[0] + 100, 6);
    expect(s1[1]).toBeCloseTo(s0[1] + 50, 6);
  });

  it("height scaling: height 20 yields exactly double height 10", () => {
    const h10 = textToSubCalls(font, "A", { origin: [0, 0], height: 10 });
    const h20 = textToSubCalls(font, "A", { origin: [0, 0], height: 20 });
    const s10 = contoursOf(h10[0])[0].start;
    const s20 = contoursOf(h20[0])[0].start;
    expect(s20[0]).toBeCloseTo(s10[0] * 2, 6);
    expect(s20[1]).toBeCloseTo(s10[1] * 2, 6);
  });

  // ── edge cases ────────────────────────────────────────────────────

  it("empty string yields []", () => {
    expect(textToSubCalls(font, "", { origin: [0, 0], height: 10 })).toEqual([]);
  });

  it("a space advances but emits nothing", () => {
    // "A B" emits exactly two glyphs (the space draws nothing) ...
    const aSpaceB = textToSubCalls(font, "A B", { origin: [0, 0], height: 10 });
    expect(aSpaceB.length).toBe(2);
    // ... and the space pushes B further right than in "AB".
    const ab = textToSubCalls(font, "AB", { origin: [0, 0], height: 10 });
    const bInAB = Math.min(...xsOf(ab[1]));
    const bInASB = Math.min(...xsOf(aSpaceB[1]));
    expect(bInASB).toBeGreaterThan(bInAB);
  });

  it("only-space string yields []", () => {
    expect(textToSubCalls(font, "   ", { origin: [0, 0], height: 10 })).toEqual(
      [],
    );
  });

  it("an unsupported control char is skipped (no crash)", () => {
    // \x01 maps to .notdef which has no outline in DejaVu → emits nothing.
    const calls = textToSubCalls(font, "A\x01B", { origin: [0, 0], height: 10 });
    // Only A and B drew geometry.
    expect(calls.length).toBe(2);
  });

  it("letterSpacing widens the gap between glyphs", () => {
    const tight = textToSubCalls(font, "AB", { origin: [0, 0], height: 10 });
    const loose = textToSubCalls(font, "AB", {
      origin: [0, 0],
      height: 10,
      letterSpacing: 5,
    });
    const bTight = Math.min(...xsOf(tight[1]));
    const bLoose = Math.min(...xsOf(loose[1]));
    expect(bLoose - bTight).toBeCloseTo(5, 6);
  });
});
