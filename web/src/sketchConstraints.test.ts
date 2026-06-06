import { describe, expect, it } from "vitest";
import {
  applicableConstraints,
  buildConstraintCall,
} from "./sketchConstraints";
import { buildDraftSketchState } from "./sketchSession";
import type { SketchState } from "./solver/types";

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
  it("offers horizontal + vertical + length for a single line", () => {
    const state = twoLinesAndCircle();
    const opts = applicableConstraints(state, [3]).map((o) => o.type);
    expect(opts).toEqual([
      "sketch_constraint_horizontal",
      "sketch_constraint_vertical",
      "sketch_constraint_distance",
    ]);
  });

  it("offers parallel + perpendicular + angle for two lines", () => {
    const state = twoLinesAndCircle();
    const opts = applicableConstraints(state, [3, 6]).map((o) => o.type);
    expect(opts).toEqual([
      "sketch_constraint_parallel",
      "sketch_constraint_perpendicular",
      "sketch_constraint_angle",
    ]);
  });

  it("offers coincident + distance for two points", () => {
    const state = twoLinesAndCircle();
    // points: line-1 start=1, line-2 start=4.
    const opts = applicableConstraints(state, [1, 4]).map((o) => o.type);
    expect(opts).toEqual([
      "sketch_constraint_coincident",
      "sketch_constraint_distance",
    ]);
  });

  it("offers coincident + distance against the origin", () => {
    const state = twoLinesAndCircle();
    const opts = applicableConstraints(state, [0, 1]).map((o) => o.type);
    expect(opts).toEqual([
      "sketch_constraint_coincident",
      "sketch_constraint_distance",
    ]);
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

  it("offers midpoint + on-curve for a point + a line, order-independent", () => {
    const state = twoLinesAndCircle();
    // line 3 + point 4 (start of line 2).
    expect(applicableConstraints(state, [3, 4]).map((o) => o.type)).toEqual([
      "sketch_constraint_midpoint",
      "sketch_constraint_point_on_object",
    ]);
    expect(applicableConstraints(state, [4, 3]).map((o) => o.type)).toEqual([
      "sketch_constraint_midpoint",
      "sketch_constraint_point_on_object",
    ]);
  });

  it("offers on-curve (and not midpoint) for a point + a circle/arc", () => {
    const state = twoLinesAndCircle();
    // point 4 + circle 8, order-independent.
    expect(applicableConstraints(state, [4, 8]).map((o) => o.type)).toEqual([
      "sketch_constraint_point_on_object",
    ]);
    expect(applicableConstraints(state, [8, 4]).map((o) => o.type)).toEqual([
      "sketch_constraint_point_on_object",
    ]);
  });

  it("does not offer on-curve for two points or a line + a curve", () => {
    const state = twoLinesAndCircle();
    // two points → coincident/distance, never on-curve.
    expect(
      applicableConstraints(state, [1, 4]).map((o) => o.type),
    ).not.toContain("sketch_constraint_point_on_object");
    // line + circle → tangent only.
    expect(
      applicableConstraints(state, [3, 8]).map((o) => o.type),
    ).not.toContain("sketch_constraint_point_on_object");
  });

  it("offers symmetric for two points + a line axis", () => {
    const state = twoLinesAndCircle();
    // points 1 and 4 mirrored across line 6 (the axis).
    expect(applicableConstraints(state, [1, 4, 6]).map((o) => o.type)).toEqual([
      "sketch_constraint_symmetric",
    ]);
  });

  it("offers radius + diameter for a single circle or arc", () => {
    const state = twoLinesAndCircle();
    // circle id 8.
    expect(applicableConstraints(state, [8]).map((o) => o.type)).toEqual([
      "sketch_constraint_radius",
      "sketch_constraint_diameter",
    ]);
  });

  it("offers equal + concentric for two circles/arcs", () => {
    const s2 = buildDraftSketchState([
      { type: "sketch_circle", params: { center: [0, 0], radius: 3 } }, // circle 2
      { type: "sketch_circle", params: { center: [9, 0], radius: 4 } }, // circle 4
    ]);
    const opts = applicableConstraints(s2, [2, 4]).map((o) => o.type);
    expect(opts).toContain("sketch_constraint_equal");
    expect(opts).toContain("sketch_constraint_concentric");
  });

  it("does not offer concentric for a line + a circle", () => {
    const state = twoLinesAndCircle();
    // line 3 + circle 8 → only tangent, never concentric.
    const opts = applicableConstraints(state, [3, 8]).map((o) => o.type);
    expect(opts).not.toContain("sketch_constraint_concentric");
  });

  it("does not offer concentric for a single circle", () => {
    const state = twoLinesAndCircle();
    const opts = applicableConstraints(state, [8]).map((o) => o.type);
    expect(opts).not.toContain("sketch_constraint_concentric");
  });

  it("offers fix for a single point", () => {
    const state = twoLinesAndCircle();
    // point 1 (start of line 1).
    expect(applicableConstraints(state, [1]).map((o) => o.type)).toEqual([
      "sketch_constraint_fix",
    ]);
    // the origin is point-like too.
    expect(applicableConstraints(state, [0]).map((o) => o.type)).toEqual([
      "sketch_constraint_fix",
    ]);
  });

  it("does not offer fix for a single line or curve", () => {
    const state = twoLinesAndCircle();
    expect(applicableConstraints(state, [3]).map((o) => o.type)).not.toContain(
      "sketch_constraint_fix",
    );
    expect(applicableConstraints(state, [8]).map((o) => o.type)).not.toContain(
      "sketch_constraint_fix",
    );
  });

  it("offers nothing for unsupported / empty selections", () => {
    const state = twoLinesAndCircle();
    expect(applicableConstraints(state, [])).toEqual([]);
    // three lines: nothing.
    expect(applicableConstraints(state, [3, 6, 3])).toEqual([]);
  });

  // F23 endpoint tangent: a control-mode OPEN spline's endpoint legs are
  // plain `line` entities, so every existing line constraint applies to
  // them with no special-casing.
  it("treats a spline endpoint leg as an ordinary line for constraints", () => {
    // control+open N=4: control points 1..=4, start leg id 5,
    // end leg id 6. Then a circle (centre 7, circle 8) and a line
    // (start 9, end 10, line 11).
    const state = buildDraftSketchState([
      {
        type: "sketch_spline",
        params: {
          points: [0, 0, 10, 5, 20, 0, 30, 5],
          closed: false,
          mode: "control",
        },
      },
      { type: "sketch_circle", params: { center: [50, 50], radius: 5 } },
      { type: "sketch_line", params: { start: [0, 20], end: [10, 20] } },
    ]);
    const startLeg = 5;
    const endLeg = 6;
    const circle = 8;
    const line = 11;
    // leg alone → horizontal/vertical/length.
    expect(applicableConstraints(state, [startLeg]).map((o) => o.type)).toEqual([
      "sketch_constraint_horizontal",
      "sketch_constraint_vertical",
      "sketch_constraint_distance",
    ]);
    // leg + circle → tangent (order-independent).
    expect(
      applicableConstraints(state, [endLeg, circle]).map((o) => o.type),
    ).toContain("sketch_constraint_tangent");
    expect(
      applicableConstraints(state, [circle, endLeg]).map((o) => o.type),
    ).toContain("sketch_constraint_tangent");
    // leg + line → parallel/perpendicular/angle.
    expect(applicableConstraints(state, [endLeg, line]).map((o) => o.type)).toEqual([
      "sketch_constraint_parallel",
      "sketch_constraint_perpendicular",
      "sketch_constraint_angle",
    ]);
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

  it("maps two curves to a / b for concentric", () => {
    const s2 = buildDraftSketchState([
      { type: "sketch_circle", params: { center: [0, 0], radius: 3 } }, // circle 2
      { type: "sketch_circle", params: { center: [9, 0], radius: 4 } }, // circle 4
    ]);
    expect(buildConstraintCall("sketch_constraint_concentric", [2, 4], s2)).toEqual({
      type: "sketch_constraint_concentric",
      params: { a: 2, b: 4 },
    });
  });

  it("returns null for concentric with wrong selections", () => {
    const state = twoLinesAndCircle();
    // two points (1, 4): not curves.
    expect(buildConstraintCall("sketch_constraint_concentric", [1, 4], state)).toBeNull();
    // a single curve (circle 8): wrong count.
    expect(buildConstraintCall("sketch_constraint_concentric", [8], state)).toBeNull();
    // mixed line + curve (3, 8): one is not curvish.
    expect(buildConstraintCall("sketch_constraint_concentric", [3, 8], state)).toBeNull();
  });

  it("maps a single point to the fix param name", () => {
    const state = twoLinesAndCircle();
    expect(buildConstraintCall("sketch_constraint_fix", [1], state)).toEqual({
      type: "sketch_constraint_fix",
      params: { point: 1 },
    });
    // the origin (id 0) is point-like and fixable too.
    expect(buildConstraintCall("sketch_constraint_fix", [0], state)).toEqual({
      type: "sketch_constraint_fix",
      params: { point: 0 },
    });
  });

  it("returns null for fix with wrong selections", () => {
    const state = twoLinesAndCircle();
    // a single line (3): not point-like.
    expect(buildConstraintCall("sketch_constraint_fix", [3], state)).toBeNull();
    // a single curve (circle 8): not point-like.
    expect(buildConstraintCall("sketch_constraint_fix", [8], state)).toBeNull();
    // two points: wrong count.
    expect(buildConstraintCall("sketch_constraint_fix", [1, 4], state)).toBeNull();
  });

  it("maps point + line to point/curve for point_on_object, order-independent", () => {
    const state = twoLinesAndCircle();
    // point 4 (start of line 2) onto line 3 — point first then curve first.
    expect(
      buildConstraintCall("sketch_constraint_point_on_object", [4, 3], state),
    ).toEqual({
      type: "sketch_constraint_point_on_object",
      params: { point: 4, curve: 3 },
    });
    expect(
      buildConstraintCall("sketch_constraint_point_on_object", [3, 4], state),
    ).toEqual({
      type: "sketch_constraint_point_on_object",
      params: { point: 4, curve: 3 },
    });
  });

  it("maps point + circle to point/curve for point_on_object, order-independent", () => {
    const state = twoLinesAndCircle();
    // point 1 onto circle 8.
    expect(
      buildConstraintCall("sketch_constraint_point_on_object", [1, 8], state),
    ).toEqual({
      type: "sketch_constraint_point_on_object",
      params: { point: 1, curve: 8 },
    });
    expect(
      buildConstraintCall("sketch_constraint_point_on_object", [8, 1], state),
    ).toEqual({
      type: "sketch_constraint_point_on_object",
      params: { point: 1, curve: 8 },
    });
  });

  it("maps point + arc to point/curve for point_on_object, order-independent", () => {
    // buildDraftSketchState doesn't synthesise full arc entities, so
    // assemble a minimal state with an arc (id 5) and a free point (id 1).
    const state: SketchState = {
      name: "poc-arc",
      entities: [
        { kind: "origin", id: 0 },
        { kind: "point", id: 1, at: { x: 3, y: 4 } },
        { kind: "point", id: 2, at: { x: 0, y: 0 } }, // arc centre
        { kind: "point", id: 3, at: { x: 5, y: 0 } }, // arc start
        { kind: "point", id: 4, at: { x: 0, y: 5 } }, // arc end
        { kind: "arc", id: 5, center: 2, start: 3, end: 4, radius: 5 },
      ],
      constraints: [],
      next_entity_id: 6,
      next_constraint_id: 1,
    };
    expect(
      buildConstraintCall("sketch_constraint_point_on_object", [1, 5], state),
    ).toEqual({
      type: "sketch_constraint_point_on_object",
      params: { point: 1, curve: 5 },
    });
    expect(
      buildConstraintCall("sketch_constraint_point_on_object", [5, 1], state),
    ).toEqual({
      type: "sketch_constraint_point_on_object",
      params: { point: 1, curve: 5 },
    });
  });

  it("returns null for point_on_object with wrong selections", () => {
    const state = twoLinesAndCircle();
    // two points: no curve.
    expect(
      buildConstraintCall("sketch_constraint_point_on_object", [1, 4], state),
    ).toBeNull();
    // line + circle: no point.
    expect(
      buildConstraintCall("sketch_constraint_point_on_object", [3, 8], state),
    ).toBeNull();
    // single entity: wrong count.
    expect(
      buildConstraintCall("sketch_constraint_point_on_object", [1], state),
    ).toBeNull();
  });

  it("normalises tangent so the line maps to `line` and curve to `curve`", () => {
    const state = twoLinesAndCircle();
    // curve-first selection still produces line/curve correctly.
    expect(buildConstraintCall("sketch_constraint_tangent", [8, 3], state)).toEqual({
      type: "sketch_constraint_tangent",
      params: { line: 3, curve: 8 },
    });
  });

  it("measures a single line's length into value_mm", () => {
    // line 3 runs (0,0)→(10,0): length 10. entity_a/_b are its
    // endpoint ids 1 and 2.
    const state = twoLinesAndCircle();
    expect(buildConstraintCall("sketch_constraint_distance", [3], state)).toEqual({
      type: "sketch_constraint_distance",
      params: { entity_a: 1, entity_b: 2, value_mm: 10 },
    });
  });

  it("measures the distance between two picked points", () => {
    // point 1 = (0,0), point 4 = (0,5): distance 5.
    const state = twoLinesAndCircle();
    expect(buildConstraintCall("sketch_constraint_distance", [1, 4], state)).toEqual({
      type: "sketch_constraint_distance",
      params: { entity_a: 1, entity_b: 4, value_mm: 5 },
    });
  });

  it("measures the distance to the origin", () => {
    const state = buildDraftSketchState([
      { type: "sketch_point", params: { at: [3, 4] } }, // point 1
    ]);
    // origin (0) → point 1 (3,4): distance 5.
    expect(buildConstraintCall("sketch_constraint_distance", [0, 1], state)).toEqual({
      type: "sketch_constraint_distance",
      params: { entity_a: 0, entity_b: 1, value_mm: 5 },
    });
  });

  it("measures the signed angle between two lines (l2 − l1) in degrees", () => {
    // line 3 horizontal (angle 0), line 6 at 90° up.
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } }, // line 3
      { type: "sketch_line", params: { start: [0, 0], end: [0, 10] } }, // line 6
    ]);
    const call = buildConstraintCall("sketch_constraint_angle", [3, 6], state)!;
    expect(call.type).toBe("sketch_constraint_angle");
    expect(call.params.line_a).toBe(3);
    expect(call.params.line_b).toBe(6);
    expect(call.params.value_deg).toBeCloseTo(90, 9);
  });

  it("wraps the measured angle into (−180, 180]", () => {
    // l1 points up-left (135°), l2 points down-right (−45°):
    // raw diff −180 wraps to 180.
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [0, 0], end: [-10, 10] } }, // line 3 @135°
      { type: "sketch_line", params: { start: [0, 0], end: [10, -10] } }, // line 6 @−45°
    ]);
    const call = buildConstraintCall("sketch_constraint_angle", [3, 6], state)!;
    expect(Math.abs(call.params.value_deg as number)).toBeCloseTo(180, 9);
  });

  it("normalises midpoint so the point maps to `point` and line to `line`", () => {
    const state = twoLinesAndCircle();
    // line-first and point-first selections both resolve correctly.
    expect(buildConstraintCall("sketch_constraint_midpoint", [3, 4], state)).toEqual({
      type: "sketch_constraint_midpoint",
      params: { point: 4, line: 3 },
    });
    expect(buildConstraintCall("sketch_constraint_midpoint", [4, 3], state)).toEqual({
      type: "sketch_constraint_midpoint",
      params: { point: 4, line: 3 },
    });
  });

  it("maps two points + an axis line to entity_a / entity_b / axis", () => {
    const state = twoLinesAndCircle();
    // Axis line can appear anywhere in the pick; the two points keep
    // their selection order.
    expect(buildConstraintCall("sketch_constraint_symmetric", [1, 4, 6], state)).toEqual({
      type: "sketch_constraint_symmetric",
      params: { entity_a: 1, entity_b: 4, axis: 6 },
    });
    expect(buildConstraintCall("sketch_constraint_symmetric", [6, 1, 4], state)).toEqual({
      type: "sketch_constraint_symmetric",
      params: { entity_a: 1, entity_b: 4, axis: 6 },
    });
  });

  it("seeds radius from a circle's current radius", () => {
    const state = twoLinesAndCircle();
    // circle 8 has radius 5.
    expect(buildConstraintCall("sketch_constraint_radius", [8], state)).toEqual({
      type: "sketch_constraint_radius",
      params: { entity: 8, value_mm: 5 },
    });
  });

  it("seeds diameter as twice a circle's radius", () => {
    const state = twoLinesAndCircle();
    expect(buildConstraintCall("sketch_constraint_diameter", [8], state)).toEqual({
      type: "sketch_constraint_diameter",
      params: { entity: 8, value_mm: 10 },
    });
  });

  it("returns null when the selection doesn't match the constraint", () => {
    const state = twoLinesAndCircle();
    expect(buildConstraintCall("sketch_constraint_parallel", [3], state)).toBeNull();
    expect(buildConstraintCall("sketch_constraint_coincident", [3, 6], state)).toBeNull();
    expect(buildConstraintCall("sketch_constraint_unknown", [3], state)).toBeNull();
    // midpoint needs exactly one point + one line.
    expect(buildConstraintCall("sketch_constraint_midpoint", [3, 6], state)).toBeNull();
    // symmetric needs two points + one line.
    expect(buildConstraintCall("sketch_constraint_symmetric", [1, 4], state)).toBeNull();
    // radius / diameter need a single circle or arc, not a line.
    expect(buildConstraintCall("sketch_constraint_radius", [3], state)).toBeNull();
    expect(buildConstraintCall("sketch_constraint_diameter", [3], state)).toBeNull();
  });
});
