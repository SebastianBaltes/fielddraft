import { describe, expect, it } from "vitest";
import { entitiesInBox, hitTestSketchEntity } from "./sketchHitTest";
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

  it("picks an ellipse by distance to its outline", () => {
    // Ellipse: centre point=1 at (20,20), focus=2, ellipse=3.
    // radius_x=10 (major, x), radius_y=5 (minor) → outline reaches
    // x=30 on the major axis.
    const state = buildDraftSketchState([
      {
        type: "sketch_ellipse",
        params: { center: [20, 20], radius_x: 10, radius_y: 5 },
      },
    ]);
    // Cursor on the major-axis vertex → on the outline → ellipse.
    expect(hitTestSketchEntity(state, [30, 20], 1)).toBe(3);
    // Cursor on the centre → the centre point wins.
    expect(hitTestSketchEntity(state, [20, 20], 1)).toBe(1);
  });

  it("never picks the internal ellipse focus point", () => {
    // The focus sits at (20 + sqrt(75), 20) ≈ (28.66, 20) — inside the
    // ellipse. Clicking right on it must not select the focus (id 2);
    // it's internal construction geometry.
    const state = buildDraftSketchState([
      {
        type: "sketch_ellipse",
        params: { center: [20, 20], radius_x: 10, radius_y: 5 },
      },
    ]);
    const focus = state.entities.find((e) => e.id === 2);
    expect(focus?.kind).toBe("point"); // it IS allocated…
    if (focus?.kind === "point") {
      // …but a click on it never returns its id.
      expect(hitTestSketchEntity(state, [focus.at.x, focus.at.y], 0.5)).not.toBe(
        2,
      );
    }
  });

  it("picks a control-mode B-spline by distance to its drawn curve", () => {
    // control+open N=4: control points 1..=4, legs 5,6, B-spline id 7.
    // The clamped cubic over (0,0),(10,20),(20,20),(30,0) passes through
    // its midpoint (15,15) — clicking there picks the B-spline entity.
    const state = buildDraftSketchState([
      {
        type: "sketch_spline",
        params: {
          points: [0, 0, 10, 20, 20, 20, 30, 0],
          closed: false,
          mode: "control",
        },
      },
    ]);
    expect(hitTestSketchEntity(state, [15, 15], 1)).toBe(7);
    // A control point still wins when clicked directly (pass 1 priority).
    expect(hitTestSketchEntity(state, [10, 20], 1)).toBe(2);
    // Well off the curve → nothing.
    expect(hitTestSketchEntity(state, [15, 25], 1)).toBeNull();
  });

  it("box-selects a control-mode B-spline when its curve is enclosed", () => {
    const state = buildDraftSketchState([
      {
        type: "sketch_spline",
        params: {
          points: [0, 0, 10, 20, 20, 20, 30, 0],
          closed: false,
          mode: "control",
        },
      },
    ]);
    // A box enclosing the whole control polygon contains the B-spline id 7.
    expect(entitiesInBox(state, [-1, -1], [31, 21], "enclosing")).toContain(7);
    // A box touching only the curve's middle crosses it.
    expect(entitiesInBox(state, [14, 14], [16, 16], "crossing")).toContain(7);
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

describe("entitiesInBox", () => {
  it("enclosing selects only fully-contained lines", () => {
    // Line A (id 3, points 1+2) fully inside; line B (id 6, points
    // 4+5) has one endpoint outside → enclosed must NOT pick line B,
    // crossing must. (Endpoint POINTS are independent entities and may
    // also be picked; we assert on the line-curve ids specifically.)
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [2, 2], end: [8, 8] } }, // 3
      { type: "sketch_line", params: { start: [5, 5], end: [50, 5] } }, // 6
    ]);
    const min: [number, number] = [1, 1];
    const max: [number, number] = [20, 20];
    const enclosed = entitiesInBox(state, min, max, "enclosing");
    expect(enclosed).toContain(3);
    expect(enclosed).not.toContain(6);
    const crossing = entitiesInBox(state, min, max, "crossing");
    expect(crossing).toContain(3);
    expect(crossing).toContain(6);
  });

  it("crossing selects a line passing through with both endpoints outside", () => {
    // Horizontal line from x=-10 to x=30 at y=5 passes through the box
    // [0,20]×[0,20] but neither endpoint is inside.
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [-10, 5], end: [30, 5] } }, // 3
    ]);
    // Box kept off the origin so id 0 doesn't enter the result.
    const min: [number, number] = [1, 1];
    const max: [number, number] = [20, 20];
    expect(entitiesInBox(state, min, max, "crossing")).toEqual([3]);
    // Neither endpoint inside → not enclosed.
    expect(entitiesInBox(state, min, max, "enclosing")).toEqual([]);
  });

  it("circle fully inside → enclosing; box clipping the edge → crossing only", () => {
    // Circle centre (10,10) r=5 → bbox [5,15]×[5,15], entity id 2.
    const state = buildDraftSketchState([
      { type: "sketch_circle", params: { center: [10, 10], radius: 5 } },
    ]);
    // Box [1,1]-[20,20] fully contains the bbox → enclosing. (The
    // centre point id 1 is also inside; we assert on the circle id 2.)
    expect(entitiesInBox(state, [1, 1], [20, 20], "enclosing")).toContain(2);
    // Box that only clips the lower-left corner of the bbox: overlaps
    // but does not contain → crossing only. The centre (10,10) is
    // outside this box, so the circle is the only match.
    expect(entitiesInBox(state, [3, 3], [7, 7], "enclosing")).toEqual([]);
    expect(entitiesInBox(state, [3, 3], [7, 7], "crossing")).toEqual([2]);
  });

  it("selects points inside the box in both modes; skips points outside", () => {
    const state = buildDraftSketchState([
      { type: "sketch_point", params: { at: [5, 5] } }, // id 1
      { type: "sketch_point", params: { at: [50, 50] } }, // id 2
    ]);
    // Box kept off the origin so only the inside point (id 1) matches.
    const min: [number, number] = [1, 1];
    const max: [number, number] = [10, 10];
    expect(entitiesInBox(state, min, max, "enclosing")).toEqual([1]);
    expect(entitiesInBox(state, min, max, "crossing")).toEqual([1]);
  });

  it("EDGE: the origin is never box-selected, even when inside the box", () => {
    // A point at (3,3) is real geometry; the origin (id 0) sits at (0,0).
    const state = buildDraftSketchState([
      { type: "sketch_point", params: { at: [3, 3] } }, // id 1
    ]);
    const sel = entitiesInBox(state, [-5, -5], [10, 10], "crossing");
    expect(sel).toContain(1);
    expect(sel).not.toContain(0); // origin excluded by design
  });

  it("EDGE: zero-area / inverted box selects nothing", () => {
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [2, 2], end: [8, 8] } },
    ]);
    // Zero-width.
    expect(entitiesInBox(state, [5, 0], [5, 20], "crossing")).toEqual([]);
    // Zero-height.
    expect(entitiesInBox(state, [0, 5], [20, 5], "crossing")).toEqual([]);
    // Inverted (max < min).
    expect(entitiesInBox(state, [20, 20], [0, 0], "crossing")).toEqual([]);
  });

  it("EDGE: box entirely outside all geometry selects nothing", () => {
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
      { type: "sketch_circle", params: { center: [0, 0], radius: 2 } },
    ]);
    expect(entitiesInBox(state, [100, 100], [120, 120], "enclosing")).toEqual(
      [],
    );
    expect(entitiesInBox(state, [100, 100], [120, 120], "crossing")).toEqual(
      [],
    );
  });

  it("EDGE: point exactly on the box border is inclusive", () => {
    const state = buildDraftSketchState([
      { type: "sketch_point", params: { at: [10, 5] } }, // id 1, on x=max
    ]);
    // Border is inclusive by design. Box kept off the origin.
    expect(entitiesInBox(state, [1, 1], [10, 10], "enclosing")).toEqual([1]);
  });

  it("EDGE: degenerate (zero-length) line behaves like its point", () => {
    // Both endpoints equal at (5,5): start=1, end=2 coincident, line=3.
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [5, 5], end: [5, 5] } },
    ]);
    const min: [number, number] = [0, 0];
    const max: [number, number] = [10, 10];
    // Both endpoints inside → enclosed and crossing.
    expect(entitiesInBox(state, min, max, "enclosing")).toContain(3);
    expect(entitiesInBox(state, min, max, "crossing")).toContain(3);
    // Box far away → neither.
    expect(entitiesInBox(state, [20, 20], [30, 30], "crossing")).not.toContain(
      3,
    );
  });

  it("EDGE: excludes the origin even when it falls inside the box", () => {
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [3, 3], end: [8, 8] } },
    ]);
    // Origin (id 0) at (0,0) is inside [-1,-1]-[10,10] but is fixed
    // reference geometry, so a marquee never sweeps it up.
    expect(entitiesInBox(state, [-1, -1], [10, 10], "enclosing")).not.toContain(0);
  });
});
