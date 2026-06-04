import { describe, expect, it } from "vitest";
import { hitTestSketchEntity } from "./sketchHitTest";
import { buildDraftSketchState } from "./sketchSession";

describe("hitTestSketchEntity", () => {
  it("returns null when nothing is within tolerance", () => {
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
    ]);
    expect(hitTestSketchEntity(state, [50, 50], 2)).toBeNull();
  });

  it("picks a line when the cursor is on its midspan", () => {
    // Line entities: start=1, end=2, line=3.
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
    ]);
    const hit = hitTestSketchEntity(state, [5, 0.3], 2);
    expect(hit).toBe(3);
  });

  it("prefers a vertex point over the line that shares it", () => {
    // Line off the origin: start vertex=1 at (3,3), end=2, line=3.
    // Cursor on the start vertex picks the point, not the line.
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [3, 3], end: [10, 0] } },
    ]);
    const hit = hitTestSketchEntity(state, [3.1, 3.1], 2);
    expect(hit).toBe(1);
  });

  it("selects the origin when clicked", () => {
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [3, 3], end: [10, 0] } },
    ]);
    expect(hitTestSketchEntity(state, [0.1, 0.1], 2)).toBe(0);
  });

  it("picks a circle by distance to its circumference", () => {
    // Circle off the origin: center point=1 at (20,20), entity=2, r=5.
    const state = buildDraftSketchState([
      { type: "sketch_circle", params: { center: [20, 20], radius: 5 } },
    ]);
    // Cursor just outside the rim → within tol of the ring → circle.
    expect(hitTestSketchEntity(state, [25.4, 20], 1)).toBe(2);
    // Cursor on the centre → the centre point wins.
    expect(hitTestSketchEntity(state, [20, 20], 1)).toBe(1);
    // Cursor deep inside the disk is on neither rim nor centre.
    expect(hitTestSketchEntity(state, [22, 20], 1)).toBeNull();
  });

  it("returns the nearest of two competing lines", () => {
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } }, // line=3
      { type: "sketch_line", params: { start: [0, 5], end: [10, 5] } }, // line=6
    ]);
    // Cursor at y≈4.7 is closer to the second line (y=5).
    expect(hitTestSketchEntity(state, [5, 4.7], 1)).toBe(6);
  });
});
