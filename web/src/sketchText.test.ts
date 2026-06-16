import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import opentype, { type Font } from "opentype.js";
import {
  pathBBox,
  reeditTextGroup,
  textRecipeOf,
  textToSubCalls,
} from "./sketchText";
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

// ── Plan item 5: re-edit recipe (web-only hybrid) ───────────────────

describe("text re-edit recipe", () => {
  it("no fontId → glyphs carry no recipe (untagged)", () => {
    const calls = textToSubCalls(font, "AB", { origin: [0, 0], height: 10 });
    expect(calls.every((c) => textRecipeOf(c) === null)).toBe(true);
  });

  it("fontId → every glyph carries the SAME group + the recipe", () => {
    const calls = textToSubCalls(font, "AB", {
      origin: [3, 7],
      height: 12,
      fontId: "dejavu-sans",
    });
    expect(calls.length).toBe(2);
    const recipes = calls.map((c) => textRecipeOf(c));
    expect(recipes.every((r) => r !== null)).toBe(true);
    const groups = new Set(recipes.map((r) => r!.group));
    expect(groups.size).toBe(1); // all glyphs share one group
    for (const r of recipes) {
      expect(r!.value).toBe("AB");
      expect(r!.height).toBe(12);
      expect(r!.fontId).toBe("dejavu-sans");
      expect(r!.origin).toEqual([3, 7]);
    }
  });

  it("distinct placements get distinct group ids", () => {
    const a = textToSubCalls(font, "A", { origin: [0, 0], height: 10, fontId: "f" });
    const b = textToSubCalls(font, "A", { origin: [0, 0], height: 10, fontId: "f" });
    expect(textRecipeOf(a[0])!.group).not.toBe(textRecipeOf(b[0])!.group);
  });

  it("place → read recipe → reeditTextGroup changes glyphs + new value", () => {
    const calls = textToSubCalls(font, "AB", {
      origin: [0, 0],
      height: 10,
      fontId: "dejavu-sans",
    });
    const group = textRecipeOf(calls[0])!.group;

    const next = reeditTextGroup(calls, font, group, {
      value: "CDE",
      height: 14,
      fontId: "dejavu-sans",
    });
    expect(next).not.toBeNull();

    // The old "AB" glyphs are gone; "CDE" has three glyphs.
    const tagged = next!.filter((c) => textRecipeOf(c) !== null);
    expect(tagged.length).toBe(3);
    // All re-derived glyphs share the ORIGINAL group id ...
    expect(tagged.every((c) => textRecipeOf(c)!.group === group)).toBe(true);
    // ... and carry the new value + height.
    for (const c of tagged) {
      const r = textRecipeOf(c)!;
      expect(r.value).toBe("CDE");
      expect(r.height).toBe(14);
    }
    // The contours actually changed (new geometry, taller).
    const oldMaxY = Math.max(...ysOf(calls[0]));
    const newMaxY = Math.max(...ysOf(tagged[0]));
    expect(newMaxY).toBeGreaterThan(oldMaxY);
  });

  it("reeditTextGroup re-derives from the recipe origin (text doesn't move)", () => {
    const calls = textToSubCalls(font, "A", {
      origin: [20, 5],
      height: 10,
      fontId: "f",
    });
    const group = textRecipeOf(calls[0])!.group;
    const next = reeditTextGroup(calls, font, group, {
      value: "A",
      height: 10,
      fontId: "f",
    });
    expect(textRecipeOf(next![0])!.origin).toEqual([20, 5]);
  });

  it("reeditTextGroup splices the new glyphs at the old group's position", () => {
    // A line sub-call, then a tagged "A", then a circle. Editing "A" → "AB"
    // keeps the surrounding sub-calls and inserts both glyphs in place.
    const text = textToSubCalls(font, "A", { origin: [0, 0], height: 10, fontId: "f" });
    const line: SubCall = { type: "sketch_line", params: { start: [0, 0], end: [1, 1] } };
    const circle: SubCall = { type: "sketch_circle", params: { center: [5, 5], radius: 2 } };
    const spine = [line, ...text, circle];
    const group = textRecipeOf(text[0])!.group;

    const next = reeditTextGroup(spine, font, group, {
      value: "AB",
      height: 10,
      fontId: "f",
    });
    expect(next).not.toBeNull();
    expect(next![0]).toBe(line); // surroundings preserved by reference
    expect(next![next!.length - 1]).toBe(circle);
    const tagged = next!.filter((c) => textRecipeOf(c) !== null);
    expect(tagged.length).toBe(2); // "AB"
  });

  it("reeditTextGroup returns null for an unknown group", () => {
    const calls = textToSubCalls(font, "A", { origin: [0, 0], height: 10, fontId: "f" });
    expect(reeditTextGroup(calls, font, "nope", { value: "B", height: 10, fontId: "f" })).toBeNull();
  });

  it("reeditTextGroup returns null when the new value has no geometry", () => {
    const calls = textToSubCalls(font, "A", { origin: [0, 0], height: 10, fontId: "f" });
    const group = textRecipeOf(calls[0])!.group;
    expect(reeditTextGroup(calls, font, group, { value: "   ", height: 10, fontId: "f" })).toBeNull();
  });

  it("pathBBox bounds a glyph's contours; covers its drawn extent", () => {
    const calls = textToSubCalls(font, "A", { origin: [10, 0], height: 10 });
    const bb = pathBBox(calls[0]);
    expect(bb).not.toBeNull();
    const xs = xsOf(calls[0]);
    const ys = ysOf(calls[0]);
    // The endpoint extent lies within the bbox (ctrl points only grow it).
    expect(bb!.min[0]).toBeLessThanOrEqual(Math.min(...xs) + 1e-6);
    expect(bb!.max[0]).toBeGreaterThanOrEqual(Math.max(...xs) - 1e-6);
    expect(bb!.min[1]).toBeLessThanOrEqual(Math.min(...ys) + 1e-6);
    expect(bb!.max[1]).toBeGreaterThanOrEqual(Math.max(...ys) - 1e-6);
  });

  it("pathBBox returns null for a non-path sub-call", () => {
    expect(pathBBox({ type: "sketch_line", params: { start: [0, 0], end: [1, 1] } })).toBeNull();
  });
});
