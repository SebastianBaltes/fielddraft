import { describe, expect, it } from "vitest";
import { buildDraftSketchState } from "../sketchSession";
import { rebuildSubCallsFromSolved } from "./replaySolve";
import { sketchWithOrigin, type SketchState } from "./types";

/** Build a solved state with two endpoints (ids 1, 2) and a line
 *  (id 3), mirroring what the kernel allocates for one sketch_line. */
function solvedLine(
  start: [number, number],
  end: [number, number],
): SketchState {
  const s = sketchWithOrigin();
  s.entities.push({ kind: "point", id: 1, at: { x: start[0], y: start[1] } });
  s.entities.push({ kind: "point", id: 2, at: { x: end[0], y: end[1] } });
  s.entities.push({ kind: "line", id: 3, start: 1, end: 2 });
  s.next_entity_id = 4;
  return s;
}

describe("rebuildSubCallsFromSolved", () => {
  it("writes solved endpoint coordinates back into a sketch_line", () => {
    const original = [
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
    ];
    const solved = solvedLine([1, 1], [11, 1]);
    const rebuilt = rebuildSubCallsFromSolved(original, solved);
    expect(rebuilt[0].params).toMatchObject({ start: [1, 1], end: [11, 1] });
  });

  it("leaves constraint calls untouched", () => {
    const original = [
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
      { type: "sketch_constraint_horizontal", params: { line: 3 } },
    ];
    const solved = solvedLine([0, 0], [10, 2]);
    const rebuilt = rebuildSubCallsFromSolved(original, solved);
    expect(rebuilt[1]).toEqual({
      type: "sketch_constraint_horizontal",
      params: { line: 3 },
    });
  });

  it("preserves the call when the solver dropped an entity", () => {
    const original = [{ type: "sketch_point", params: { at: [5, 5] } }];
    // Solved state without point id 1 → call returned unchanged.
    const solved = sketchWithOrigin();
    const rebuilt = rebuildSubCallsFromSolved(original, solved);
    expect(rebuilt[0].params).toEqual({ at: [5, 5] });
  });

  // ---- F23 + latent id-advancement fixes -----------------------------
  // The state built by buildDraftSketchState has the exact entity-id
  // allocation the kernel uses. We move a known point to a "solved"
  // value, run the writeback, and assert both (a) the moved coords land
  // in the right call AND (b) a FOLLOWING sketch_line keeps its
  // coordinates — proving nextId advanced past every entity, including
  // the spline's endpoint legs.

  /** Build a solved state and overwrite the point at `id` with `to`. */
  function solvedFrom(
    subCalls: { type: string; params: Record<string, unknown> }[],
    id: number,
    to: [number, number],
  ): SketchState {
    const s = buildDraftSketchState(subCalls);
    const e = s.entities.find((x) => x.id === id);
    if (e && e.kind === "point") {
      e.at = { x: to[0], y: to[1] };
    }
    return s;
  }

  it("writes back control points and keeps a following line aligned (spline)", () => {
    const original = [
      {
        type: "sketch_spline",
        params: {
          points: [0, 0, 10, 5, 20, 0, 30, 5],
          closed: false,
          mode: "control",
        },
      },
      { type: "sketch_line", params: { start: [50, 50], end: [60, 60] } },
    ];
    // Spline cp0 = id 1; legs = ids 5,6; following line start = id 7.
    // Move cp0 → (1,1) and the line start → (51,51).
    const solved = solvedFrom(original, 1, [1, 1]);
    const ls = solved.entities.find((x) => x.id === 7);
    if (ls && ls.kind === "point") ls.at = { x: 51, y: 51 };

    const rebuilt = rebuildSubCallsFromSolved(original, solved);
    // Control point 0 written back.
    expect((rebuilt[0].params.points as number[]).slice(0, 2)).toEqual([1, 1]);
    // Following line stays correctly aligned (proves legs were skipped).
    expect(rebuilt[1].params).toMatchObject({ start: [51, 51], end: [60, 60] });
  });

  it("writes back control points and keeps a following line aligned (interpolate spline)", () => {
    // V4: open interpolation splines allocate the SAME legs (ids 5,6) as
    // control mode, so nextId must advance identically.
    const original = [
      {
        type: "sketch_spline",
        params: {
          points: [0, 0, 10, 5, 20, 0, 30, 5],
          closed: false,
          mode: "interpolate",
        },
      },
      { type: "sketch_line", params: { start: [50, 50], end: [60, 60] } },
    ];
    // Spline cp0 = id 1; legs = ids 5,6; following line start = id 7.
    const solved = solvedFrom(original, 1, [1, 1]);
    const ls = solved.entities.find((x) => x.id === 7);
    if (ls && ls.kind === "point") ls.at = { x: 51, y: 51 };

    const rebuilt = rebuildSubCallsFromSolved(original, solved);
    expect((rebuilt[0].params.points as number[]).slice(0, 2)).toEqual([1, 1]);
    // Following line stays aligned (proves legs were skipped just like
    // in control mode).
    expect(rebuilt[1].params).toMatchObject({ start: [51, 51], end: [60, 60] });
  });

  it("writes back the centre and keeps a following line aligned (ellipse)", () => {
    const original = [
      {
        type: "sketch_ellipse",
        params: { center: [0, 0], radius_x: 10, radius_y: 5 },
      },
      { type: "sketch_line", params: { start: [50, 50], end: [60, 60] } },
    ];
    // ellipse centre = id 1; following line start = id 2.
    const solved = solvedFrom(original, 1, [2, 3]);
    const ls = solved.entities.find((x) => x.id === 2);
    if (ls && ls.kind === "point") ls.at = { x: 51, y: 51 };

    const rebuilt = rebuildSubCallsFromSolved(original, solved);
    expect(rebuilt[0].params).toMatchObject({ center: [2, 3] });
    expect(rebuilt[1].params).toMatchObject({ start: [51, 51], end: [60, 60] });
  });

  it("writes back both centres and keeps a following line aligned (slot)", () => {
    const original = [
      {
        type: "sketch_slot",
        params: { center_a: [0, 0], center_b: [20, 0], radius: 5 },
      },
      { type: "sketch_line", params: { start: [50, 50], end: [60, 60] } },
    ];
    // slot centres = ids 1,2; following line start = id 3.
    const solved = solvedFrom(original, 1, [1, 1]);
    const cb = solved.entities.find((x) => x.id === 2);
    if (cb && cb.kind === "point") cb.at = { x: 21, y: 1 };
    const ls = solved.entities.find((x) => x.id === 3);
    if (ls && ls.kind === "point") ls.at = { x: 51, y: 51 };

    const rebuilt = rebuildSubCallsFromSolved(original, solved);
    expect(rebuilt[0].params).toMatchObject({
      center_a: [1, 1],
      center_b: [21, 1],
    });
    expect(rebuilt[1].params).toMatchObject({ start: [51, 51], end: [60, 60] });
  });

  it("writes back contour starts and keeps a following line aligned (path)", () => {
    const original = [
      {
        type: "sketch_path",
        params: {
          contours: [
            {
              start: [0, 0],
              segments: [{ kind: "line", to: [10, 0] }],
              closed: false,
            },
          ],
        },
      },
      { type: "sketch_line", params: { start: [50, 50], end: [60, 60] } },
    ];
    // path contour start = id 1; following line start = id 2.
    const solved = solvedFrom(original, 1, [3, 4]);
    const ls = solved.entities.find((x) => x.id === 2);
    if (ls && ls.kind === "point") ls.at = { x: 51, y: 51 };

    const rebuilt = rebuildSubCallsFromSolved(original, solved);
    const contours = rebuilt[0].params.contours as { start: number[] }[];
    expect(contours[0].start).toEqual([3, 4]);
    expect(rebuilt[1].params).toMatchObject({ start: [51, 51], end: [60, 60] });
  });
});
