import { describe, expect, it } from "vitest";
import {
  translateSubCall,
  isTranslatableSubCall,
  type SubCall,
} from "./sketchSubCallTransform";

/**
 * Ticket matf: a rigid (dx, dy) translation of a whole geometry sub-call.
 * The Move/Copy/Select-drag path goes through this, so every primitive kind
 * must move all its coordinate fields while leaving radii / angles / the
 * construction flag invariant.
 */

describe("translateSubCall", () => {
  it("moves a free point", () => {
    const out = translateSubCall(
      { type: "sketch_point", params: { at: [1, 2] } },
      10,
      -3,
    );
    expect(out.params.at).toEqual([11, -1]);
  });

  it("moves both line endpoints", () => {
    const out = translateSubCall(
      { type: "sketch_line", params: { start: [0, 0], end: [4, 0] } },
      5,
      5,
    );
    expect(out.params.start).toEqual([5, 5]);
    expect(out.params.end).toEqual([9, 5]);
  });

  it("moves a circle centre, keeping radius", () => {
    const out = translateSubCall(
      { type: "sketch_circle", params: { center: [0, 0], radius: 7 } },
      2,
      3,
    );
    expect(out.params.center).toEqual([2, 3]);
    expect(out.params.radius).toBe(7);
  });

  it("moves an arc centre, keeping radius and angles", () => {
    const out = translateSubCall(
      {
        type: "sketch_arc",
        params: { center: [1, 1], radius: 5, start_angle: 0.2, sweep_angle: 1 },
      },
      -1,
      -1,
    );
    expect(out.params.center).toEqual([0, 0]);
    expect(out.params.radius).toBe(5);
    expect(out.params.start_angle).toBe(0.2);
    expect(out.params.sweep_angle).toBe(1);
  });

  it("moves both rectangle corners", () => {
    const out = translateSubCall(
      { type: "sketch_rectangle", params: { corner_a: [0, 0], corner_b: [4, 2] } },
      1,
      1,
    );
    expect(out.params.corner_a).toEqual([1, 1]);
    expect(out.params.corner_b).toEqual([5, 3]);
  });

  it("moves every polygon vertex in the flat array", () => {
    const out = translateSubCall(
      { type: "sketch_polygon", params: { vertices: [0, 0, 2, 0, 1, 2] } },
      10,
      20,
    );
    expect(out.params.vertices).toEqual([10, 20, 12, 20, 11, 22]);
  });

  it("moves every spline point", () => {
    const out = translateSubCall(
      { type: "sketch_spline", params: { points: [0, 0, 5, 5], closed: false } },
      1,
      2,
    );
    expect(out.params.points).toEqual([1, 2, 6, 7]);
    expect(out.params.closed).toBe(false);
  });

  it("moves path contour starts and segment vertices", () => {
    const out = translateSubCall(
      {
        type: "sketch_path",
        params: {
          contours: [
            {
              start: [0, 0],
              segments: [
                { kind: "line", to: [4, 0] },
                { kind: "quad", ctrl: [6, 1], to: [4, 4] },
              ],
            },
          ],
        },
      },
      2,
      3,
    );
    const c = (out.params.contours as any[])[0];
    expect(c.start).toEqual([2, 3]);
    expect(c.segments[0].to).toEqual([6, 3]);
    expect(c.segments[1].ctrl).toEqual([8, 4]);
    expect(c.segments[1].to).toEqual([6, 7]);
  });

  it("carries the construction flag untouched", () => {
    const out = translateSubCall(
      {
        type: "sketch_line",
        params: { start: [0, 0], end: [1, 0], construction: true },
      },
      1,
      0,
    );
    expect(out.params.construction).toBe(true);
  });

  it("returns constraint sub-calls unchanged", () => {
    const c: SubCall = {
      type: "sketch_constraint_horizontal",
      params: { entity: 3 },
    };
    expect(translateSubCall(c, 5, 5)).toEqual(c);
  });
});

describe("isTranslatableSubCall", () => {
  it("accepts geometry kinds", () => {
    for (const t of [
      "sketch_point",
      "sketch_line",
      "sketch_circle",
      "sketch_arc",
      "sketch_rectangle",
      "sketch_polygon",
      "sketch_spline",
      "sketch_path",
    ]) {
      expect(isTranslatableSubCall({ type: t, params: {} })).toBe(true);
    }
  });

  it("rejects constraints / unknown kinds", () => {
    expect(
      isTranslatableSubCall({ type: "sketch_constraint_coincident", params: {} }),
    ).toBe(false);
  });
});
