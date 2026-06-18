import { describe, it, expect } from "vitest";
import {
  shapeVertices,
  explodeShapeToLines,
  resolveCornerFillet,
  type SubCall,
} from "./sketchCorner";

const polygon = (verts: number[]): SubCall => ({
  type: "sketch_polygon",
  params: { vertices: verts },
});

describe("sketchCorner (single-click corner fillet, tickets tqi4/chfp)", () => {
  it("explodes a polygon into a closed loop of lines", () => {
    const lines = explodeShapeToLines(polygon([0, 0, 10, 0, 10, 10, 0, 10]));
    expect(lines).not.toBeNull();
    expect(lines).toHaveLength(4);
    expect(lines!.every((l) => l.type === "sketch_line")).toBe(true);
    // Closed: last line returns to the first vertex.
    expect(lines![3].params.start).toEqual([0, 10]);
    expect(lines![3].params.end).toEqual([0, 0]);
  });

  it("explodes a rectangle into 4 lines", () => {
    const rect: SubCall = {
      type: "sketch_rectangle",
      params: { corner_a: [0, 0], corner_b: [20, 10] },
    };
    expect(shapeVertices(rect)).toEqual([
      [0, 0],
      [20, 0],
      [20, 10],
      [0, 10],
    ]);
    expect(explodeShapeToLines(rect)).toHaveLength(4);
  });

  it("resolves a polygon corner to the two edges meeting there (exploded)", () => {
    // Square; click near the (10,0) corner.
    const r = resolveCornerFillet(
      [polygon([0, 0, 10, 0, 10, 10, 0, 10])],
      [10.2, 0.1],
      2,
    );
    expect(r).not.toBeNull();
    expect(r!.exploded).toBe(true);
    expect(r!.subCalls).toHaveLength(4); // polygon exploded into 4 lines
    // Vertex index 1 = (10,0) is shared by edge 0 (v0→v1) and edge 1 (v1→v2).
    expect([r!.i1, r!.i2].sort()).toEqual([0, 1]);
  });

  it("resolves a line↔line junction without exploding", () => {
    const subCalls: SubCall[] = [
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
      { type: "sketch_line", params: { start: [10, 0], end: [10, 10] } },
    ];
    const r = resolveCornerFillet(subCalls, [10.1, 0.1], 2);
    expect(r).not.toBeNull();
    expect(r!.exploded).toBe(false);
    expect([r!.i1, r!.i2].sort()).toEqual([0, 1]);
  });

  it("returns null when no corner is within tolerance", () => {
    expect(
      resolveCornerFillet([polygon([0, 0, 10, 0, 10, 10, 0, 10])], [50, 50], 2),
    ).toBeNull();
  });
});
