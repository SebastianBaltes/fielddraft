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
    const subCalls: SubCall[] = [{ type: "sketch_point", params: { at: [5, 7] } }];
    const grips = gripsForSelection(subCalls, [1]);
    expect(grips).toHaveLength(1);
    expect(grips[0]).toMatchObject({ role: "point", pos: [5, 7], subCallIndex: 0 });
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

  it("keeps ids aligned across a rectangle (no grips, 8 ids)", () => {
    const subCalls: SubCall[] = [
      { type: "sketch_rectangle", params: { corner_a: [0, 0], corner_b: [10, 10] } },
      { type: "sketch_point", params: { at: [3, 3] } },
    ];
    // rectangle consumes ids 1..8, point = 9
    expect(gripsForSelection(subCalls, [9]).map((g) => g.role)).toEqual([
      "point",
    ]);
    // The rectangle itself exposes no grips yet.
    expect(gripsForSelection(subCalls, [1, 2, 5, 8])).toEqual([]);
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
    const next = applyGripDrag([line], { subCallIndex: 0, role: "line-end", pos: [40, 0] }, [9, 9]);
    expect(next[0].params.end).toEqual([9, 9]);
    expect(next[0].params.start).toEqual([0, 0]);
  });

  it("moves a point", () => {
    const pt: SubCall = { type: "sketch_point", params: { at: [1, 1] } };
    const next = applyGripDrag([pt], { subCallIndex: 0, role: "point", pos: [1, 1] }, [3, 4]);
    expect(next[0].params.at).toEqual([3, 4]);
  });

  it("circle-rim sets a new radius from the cursor distance", () => {
    const circle: SubCall = {
      type: "sketch_circle",
      params: { center: [0, 0], radius: 5 },
    };
    const next = applyGripDrag(circle ? [circle] : [], { subCallIndex: 0, role: "circle-rim", pos: [5, 0] }, [10, 0]);
    expect(next[0].params.radius).toBeCloseTo(10, 6);
    expect(next[0].params.center).toEqual([0, 0]);
  });

  it("circle-center moves the centre, keeps the radius", () => {
    const circle: SubCall = {
      type: "sketch_circle",
      params: { center: [0, 0], radius: 5 },
    };
    const next = applyGripDrag([circle], { subCallIndex: 0, role: "circle-center", pos: [0, 0] }, [7, 8]);
    expect(next[0].params.center).toEqual([7, 8]);
    expect(next[0].params.radius).toBe(5);
  });

  it("does not mutate the input array", () => {
    const subCalls: SubCall[] = [{ ...line, params: { ...line.params } }];
    const before = JSON.stringify(subCalls);
    applyGripDrag(subCalls, { subCallIndex: 0, role: "line-end", pos: [40, 0] }, [1, 2]);
    expect(JSON.stringify(subCalls)).toBe(before);
  });
});
