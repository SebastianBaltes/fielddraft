import { describe, expect, it } from "vitest";
import { applyGripDrag, gripsForSelection, type SubCall } from "./sketchGrips";

/**
 * Entity ids mirror `buildDraftSketchState`: origin = 0, then one
 * bump per allocated point/line/circle entity in sub-call order.
 */

describe("gripsForSelection", () => {
  it("maps a selected line to both endpoint grips", () => {
    const subCalls: SubCall[] = [
      { type: "sketch_line", params: { start: [0, 0], end: [40, 0] } },
    ];
    // ids: start=1, end=2, line=3
    const grips = gripsForSelection(subCalls, [3]);
    expect(grips.map((g) => g.role).sort()).toEqual(["line-end", "line-start"]);
    const start = grips.find((g) => g.role === "line-start")!;
    const end = grips.find((g) => g.role === "line-end")!;
    expect(start.pos).toEqual([0, 0]);
    expect(end.pos).toEqual([40, 0]);
    expect(start.subCallIndex).toBe(0);
  });

  it("maps a selected line endpoint to a single grip", () => {
    const subCalls: SubCall[] = [
      { type: "sketch_line", params: { start: [0, 0], end: [40, 0] } },
    ];
    expect(gripsForSelection(subCalls, [1]).map((g) => g.role)).toEqual([
      "line-start",
    ]);
    expect(gripsForSelection(subCalls, [2]).map((g) => g.role)).toEqual([
      "line-end",
    ]);
  });

  it("maps a free point to its grip", () => {
    const subCalls: SubCall[] = [
      { type: "sketch_point", params: { at: [5, 7] } },
    ];
    const grips = gripsForSelection(subCalls, [1]);
    expect(grips).toHaveLength(1);
    expect(grips[0]).toMatchObject({
      role: "point",
      pos: [5, 7],
      subCallIndex: 0,
    });
  });

  it("maps a selected circle to centre + rim grips", () => {
    const subCalls: SubCall[] = [
      { type: "sketch_circle", params: { center: [10, 20], radius: 5 } },
    ];
    // ids: center=1, circle=2
    const grips = gripsForSelection(subCalls, [2]);
    expect(grips.map((g) => g.role).sort()).toEqual([
      "circle-center",
      "circle-rim",
    ]);
    const rim = grips.find((g) => g.role === "circle-rim")!;
    expect(rim.pos).toEqual([15, 20]); // center.x + radius
    // Selecting just the centre point gives only the centre grip.
    expect(gripsForSelection(subCalls, [1]).map((g) => g.role)).toEqual([
      "circle-center",
    ]);
  });

  it("keeps ids aligned across a rectangle (8 ids)", () => {
    const subCalls: SubCall[] = [
      {
        type: "sketch_rectangle",
        params: { corner_a: [0, 0], corner_b: [10, 10] },
      },
      { type: "sketch_point", params: { at: [3, 3] } },
    ];
    // rectangle consumes ids 1..8, point = 9
    expect(gripsForSelection(subCalls, [9]).map((g) => g.role)).toEqual([
      "point",
    ]);
  });

  it("maps each rectangle corner point to its own corner grip", () => {
    const subCalls: SubCall[] = [
      {
        type: "sketch_rectangle",
        params: { corner_a: [0, 0], corner_b: [10, 20] },
      },
    ];
    // ids: bl=1, br=2, tr=3, tl=4, then 4 edge lines 5..8
    expect(gripsForSelection(subCalls, [1])[0]).toMatchObject({
      role: "rect-bl",
      pos: [0, 0],
    });
    expect(gripsForSelection(subCalls, [2])[0]).toMatchObject({
      role: "rect-br",
      pos: [10, 0],
    });
    expect(gripsForSelection(subCalls, [3])[0]).toMatchObject({
      role: "rect-tr",
      pos: [10, 20],
    });
    expect(gripsForSelection(subCalls, [4])[0]).toMatchObject({
      role: "rect-tl",
      pos: [0, 20],
    });
  });

  it("maps a rectangle edge to all four corner grips", () => {
    const subCalls: SubCall[] = [
      {
        type: "sketch_rectangle",
        params: { corner_a: [0, 0], corner_b: [10, 20] },
      },
    ];
    // selecting the bottom edge (id 5) reveals every corner
    const grips = gripsForSelection(subCalls, [5]);
    expect(grips.map((g) => g.role).sort()).toEqual([
      "rect-bl",
      "rect-br",
      "rect-tl",
      "rect-tr",
    ]);
  });

  it("derives rectangle corners from min/max of stored corners", () => {
    // corners stored out of order — grips must still be the box corners
    const subCalls: SubCall[] = [
      {
        type: "sketch_rectangle",
        params: { corner_a: [10, 20], corner_b: [0, 0] },
      },
    ];
    expect(gripsForSelection(subCalls, [1])[0]).toMatchObject({
      role: "rect-bl",
      pos: [0, 0],
    });
    expect(gripsForSelection(subCalls, [3])[0]).toMatchObject({
      role: "rect-tr",
      pos: [10, 20],
    });
  });

  it("maps each polygon vertex to its own grip", () => {
    const subCalls: SubCall[] = [
      { type: "sketch_polygon", params: { vertices: [0, 0, 10, 0, 5, 8] } },
    ];
    // ids: v0=1, v1=2, v2=3
    expect(gripsForSelection(subCalls, [2])[0]).toMatchObject({
      role: "polygon-vertex",
      pos: [10, 0],
      vertexIndex: 1,
    });
    expect(gripsForSelection(subCalls, [3])[0]).toMatchObject({
      role: "polygon-vertex",
      pos: [5, 8],
      vertexIndex: 2,
    });
  });

  it("maps an arc centre to centre + rim grips at the mid-sweep angle", () => {
    const subCalls: SubCall[] = [
      {
        type: "sketch_arc",
        params: {
          center: [0, 0],
          radius: 10,
          start_angle: 0,
          sweep_angle: Math.PI,
        },
      },
    ];
    // ids: center=1
    const grips = gripsForSelection(subCalls, [1]);
    expect(grips.map((g) => g.role).sort()).toEqual(["arc-center", "arc-rim"]);
    const rim = grips.find((g) => g.role === "arc-rim")!;
    // mid angle = π/2 → rim at (0, 10)
    expect(rim.pos[0]).toBeCloseTo(0, 6);
    expect(rim.pos[1]).toBeCloseTo(10, 6);
  });

  it("dedupes when a line and its endpoint are both selected", () => {
    const subCalls: SubCall[] = [
      { type: "sketch_line", params: { start: [0, 0], end: [40, 0] } },
    ];
    const grips = gripsForSelection(subCalls, [3, 1]);
    expect(grips).toHaveLength(2); // line-start counted once
  });

  it("returns nothing for an empty selection", () => {
    expect(gripsForSelection([], [])).toEqual([]);
  });
});

describe("applyGripDrag", () => {
  const line: SubCall = {
    type: "sketch_line",
    params: { start: [0, 0], end: [40, 0] },
  };

  it("moves a line endpoint and leaves the start put", () => {
    const next = applyGripDrag(
      [line],
      { subCallIndex: 0, role: "line-end", pos: [40, 0] },
      [9, 9],
    );
    expect(next[0].params.end).toEqual([9, 9]);
    expect(next[0].params.start).toEqual([0, 0]);
  });

  it("moves a point", () => {
    const pt: SubCall = { type: "sketch_point", params: { at: [1, 1] } };
    const next = applyGripDrag(
      [pt],
      { subCallIndex: 0, role: "point", pos: [1, 1] },
      [3, 4],
    );
    expect(next[0].params.at).toEqual([3, 4]);
  });

  it("circle-rim sets a new radius from the cursor distance", () => {
    const circle: SubCall = {
      type: "sketch_circle",
      params: { center: [0, 0], radius: 5 },
    };
    const next = applyGripDrag(
      circle ? [circle] : [],
      { subCallIndex: 0, role: "circle-rim", pos: [5, 0] },
      [10, 0],
    );
    expect(next[0].params.radius).toBeCloseTo(10, 6);
    expect(next[0].params.center).toEqual([0, 0]);
  });

  it("circle-center moves the centre, keeps the radius", () => {
    const circle: SubCall = {
      type: "sketch_circle",
      params: { center: [0, 0], radius: 5 },
    };
    const next = applyGripDrag(
      [circle],
      { subCallIndex: 0, role: "circle-center", pos: [0, 0] },
      [7, 8],
    );
    expect(next[0].params.center).toEqual([7, 8]);
    expect(next[0].params.radius).toBe(5);
  });

  it("rect corner drag keeps the diagonally opposite corner put", () => {
    const rect: SubCall = {
      type: "sketch_rectangle",
      params: { corner_a: [0, 0], corner_b: [10, 20] },
    };
    // drag bl (0,0) to (-5,-5): opposite tr (10,20) stays
    const next = applyGripDrag(
      [rect],
      { subCallIndex: 0, role: "rect-bl", pos: [0, 0] },
      [-5, -5],
    );
    const a = next[0].params.corner_a as number[];
    const b = next[0].params.corner_b as number[];
    const minX = Math.min(a[0], b[0]);
    const minY = Math.min(a[1], b[1]);
    const maxX = Math.max(a[0], b[0]);
    const maxY = Math.max(a[1], b[1]);
    expect([minX, minY]).toEqual([-5, -5]);
    expect([maxX, maxY]).toEqual([10, 20]);
  });

  it("rect corner drag stays axis-aligned when the cursor crosses over", () => {
    const rect: SubCall = {
      type: "sketch_rectangle",
      params: { corner_a: [0, 0], corner_b: [10, 20] },
    };
    // drag tr (10,20) past bl, to (-3,-4): opposite bl (0,0) stays
    const next = applyGripDrag(
      [rect],
      { subCallIndex: 0, role: "rect-tr", pos: [10, 20] },
      [-3, -4],
    );
    const a = next[0].params.corner_a as number[];
    const b = next[0].params.corner_b as number[];
    const minX = Math.min(a[0], b[0]);
    const minY = Math.min(a[1], b[1]);
    const maxX = Math.max(a[0], b[0]);
    const maxY = Math.max(a[1], b[1]);
    expect([minX, minY]).toEqual([-3, -4]);
    expect([maxX, maxY]).toEqual([0, 0]);
  });

  it("polygon vertex drag moves only that vertex", () => {
    const poly: SubCall = {
      type: "sketch_polygon",
      params: { vertices: [0, 0, 10, 0, 5, 8] },
    };
    const next = applyGripDrag(
      [poly],
      { subCallIndex: 0, role: "polygon-vertex", pos: [10, 0], vertexIndex: 1 },
      [12, 3],
    );
    expect(next[0].params.vertices).toEqual([0, 0, 12, 3, 5, 8]);
  });

  it("arc-rim sets a new radius, arc-center moves the centre", () => {
    const arc: SubCall = {
      type: "sketch_arc",
      params: {
        center: [0, 0],
        radius: 10,
        start_angle: 0,
        sweep_angle: Math.PI,
      },
    };
    const rimDrag = applyGripDrag(
      [arc],
      { subCallIndex: 0, role: "arc-rim", pos: [0, 10] },
      [0, 25],
    );
    expect(rimDrag[0].params.radius).toBeCloseTo(25, 6);
    expect(rimDrag[0].params.center).toEqual([0, 0]);

    const centerDrag = applyGripDrag(
      [arc],
      { subCallIndex: 0, role: "arc-center", pos: [0, 0] },
      [4, 5],
    );
    expect(centerDrag[0].params.center).toEqual([4, 5]);
    expect(centerDrag[0].params.radius).toBe(10);
  });

  it("does not mutate the input array", () => {
    const subCalls: SubCall[] = [{ ...line, params: { ...line.params } }];
    const before = JSON.stringify(subCalls);
    applyGripDrag(
      subCalls,
      { subCallIndex: 0, role: "line-end", pos: [40, 0] },
      [1, 2],
    );
    expect(JSON.stringify(subCalls)).toBe(before);
  });

  // sketch_path vertices (V2). CANONICAL ids per contour: start, then
  // per segment (Quad → ctrl, to; Line → to).
  const path: SubCall = {
    type: "sketch_path",
    params: {
      contours: [
        {
          start: [0, 0],
          segments: [
            { kind: "line", to: [10, 0] },
            { kind: "quad", ctrl: [15, 5], to: [20, 10] },
          ],
          closed: false,
        },
      ],
    },
  };

  it("path-vertex start drag moves the contour start only", () => {
    // id 1 = start, 2 = line.to, 3 = quad.ctrl, 4 = quad.to.
    const grip = gripsForSelection([path], [1])[0];
    expect(grip).toMatchObject({
      role: "path-vertex",
      pos: [0, 0],
      pathTarget: { contourIndex: 0, kind: "start" },
    });
    const next = applyGripDrag([path], grip, [3, 4]);
    const c = (next[0].params.contours as { start: number[] }[])[0];
    expect(c.start).toEqual([3, 4]);
  });

  it("path-vertex segment-to drag writes back the segment endpoint", () => {
    const grip = gripsForSelection([path], [2])[0];
    expect(grip).toMatchObject({
      role: "path-vertex",
      pathTarget: { contourIndex: 0, kind: "to", segIndex: 0 },
    });
    const next = applyGripDrag([path], grip, [11, 1]);
    const segs = (
      next[0].params.contours as { segments: { to: number[] }[] }[]
    )[0].segments;
    expect(segs[0].to).toEqual([11, 1]);
    expect(segs[1].to).toEqual([20, 10]); // untouched
  });

  it("path-vertex quad-ctrl drag writes back the quad control handle", () => {
    const grip = gripsForSelection([path], [3])[0];
    expect(grip).toMatchObject({
      role: "path-vertex",
      pos: [15, 5],
      pathTarget: { contourIndex: 0, kind: "ctrl", segIndex: 1 },
    });
    const next = applyGripDrag([path], grip, [16, 6]);
    const segs = (
      next[0].params.contours as {
        segments: { ctrl?: number[]; to: number[] }[];
      }[]
    )[0].segments;
    expect(segs[1].ctrl).toEqual([16, 6]);
    expect(segs[1].to).toEqual([20, 10]); // endpoint untouched
  });
});
