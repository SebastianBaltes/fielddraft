import { describe, expect, it } from "vitest";
import {
  applicableConstraints,
  buildConstraintCall,
} from "./sketchConstraints";
import { buildDraftSketchState } from "./sketchSession";

// Two lines (ids 3 and 6) plus a circle (centre 7, circle 8) for the
// fixtures below.
function twoLinesAndCircle() {
  return buildDraftSketchState([
    { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } }, // line 3
    { type: "sketch_line", params: { start: [0, 5], end: [10, 5] } }, // line 6
    { type: "sketch_circle", params: { center: [20, 20], radius: 5 } }, // circle 8
  ]);
}

describe("applicableConstraints", () => {
  it("offers horizontal + vertical for a single line", () => {
    const state = twoLinesAndCircle();
    const opts = applicableConstraints(state, [3]).map((o) => o.type);
    expect(opts).toEqual([
      "sketch_constraint_horizontal",
      "sketch_constraint_vertical",
    ]);
  });

  it("offers parallel + perpendicular for two lines", () => {
    const state = twoLinesAndCircle();
    const opts = applicableConstraints(state, [3, 6]).map((o) => o.type);
    expect(opts).toEqual([
      "sketch_constraint_parallel",
      "sketch_constraint_perpendicular",
    ]);
  });

  it("offers coincident for two points", () => {
    const state = twoLinesAndCircle();
    // points: line-1 start=1, line-2 start=4.
    const opts = applicableConstraints(state, [1, 4]).map((o) => o.type);
    expect(opts).toEqual(["sketch_constraint_coincident"]);
  });

  it("offers coincident against the origin", () => {
    const state = twoLinesAndCircle();
    const opts = applicableConstraints(state, [0, 1]).map((o) => o.type);
    expect(opts).toEqual(["sketch_constraint_coincident"]);
  });

  it("offers tangent for a line + a circle, order-independent", () => {
    const state = twoLinesAndCircle();
    expect(applicableConstraints(state, [3, 8]).map((o) => o.type)).toEqual([
      "sketch_constraint_tangent",
    ]);
    expect(applicableConstraints(state, [8, 3]).map((o) => o.type)).toEqual([
      "sketch_constraint_tangent",
    ]);
  });

  it("offers nothing for unsupported / empty selections", () => {
    const state = twoLinesAndCircle();
    expect(applicableConstraints(state, [])).toEqual([]);
    // line + point: no supported geometric constraint.
    expect(applicableConstraints(state, [3, 1])).toEqual([]);
    // three lines: nothing.
    expect(applicableConstraints(state, [3, 6, 3])).toEqual([]);
  });
});

describe("buildConstraintCall", () => {
  it("maps a single line to the horizontal param name", () => {
    const state = twoLinesAndCircle();
    expect(buildConstraintCall("sketch_constraint_horizontal", [3], state)).toEqual({
      type: "sketch_constraint_horizontal",
      params: { line: 3 },
    });
  });

  it("maps two lines to line_a / line_b for parallel", () => {
    const state = twoLinesAndCircle();
    expect(buildConstraintCall("sketch_constraint_parallel", [3, 6], state)).toEqual({
      type: "sketch_constraint_parallel",
      params: { line_a: 3, line_b: 6 },
    });
  });

  it("maps two points to a / b for coincident", () => {
    const state = twoLinesAndCircle();
    expect(buildConstraintCall("sketch_constraint_coincident", [1, 4], state)).toEqual({
      type: "sketch_constraint_coincident",
      params: { a: 1, b: 4 },
    });
  });

  it("maps two curves to entity_a / entity_b for equal", () => {
    const state = twoLinesAndCircle();
    // Need two curves — reuse the circle id twice is invalid; build a
    // second circle instead.
    const s2 = buildDraftSketchState([
      { type: "sketch_circle", params: { center: [0, 0], radius: 3 } }, // circle 2
      { type: "sketch_circle", params: { center: [9, 0], radius: 4 } }, // circle 4
    ]);
    expect(buildConstraintCall("sketch_constraint_equal", [2, 4], s2)).toEqual({
      type: "sketch_constraint_equal",
      params: { entity_a: 2, entity_b: 4 },
    });
    void state;
  });

  it("normalises tangent so the line maps to `line` and curve to `curve`", () => {
    const state = twoLinesAndCircle();
    // curve-first selection still produces line/curve correctly.
    expect(buildConstraintCall("sketch_constraint_tangent", [8, 3], state)).toEqual({
      type: "sketch_constraint_tangent",
      params: { line: 3, curve: 8 },
    });
  });

  it("returns null when the selection doesn't match the constraint", () => {
    const state = twoLinesAndCircle();
    expect(buildConstraintCall("sketch_constraint_parallel", [3], state)).toBeNull();
    expect(buildConstraintCall("sketch_constraint_coincident", [3, 6], state)).toBeNull();
    expect(buildConstraintCall("sketch_constraint_unknown", [3], state)).toBeNull();
  });
});
