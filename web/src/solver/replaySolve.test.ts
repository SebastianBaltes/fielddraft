import { describe, expect, it } from "vitest";
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
    const original = [
      { type: "sketch_point", params: { at: [5, 5] } },
    ];
    // Solved state without point id 1 → call returned unchanged.
    const solved = sketchWithOrigin();
    const rebuilt = rebuildSubCallsFromSolved(original, solved);
    expect(rebuilt[0].params).toEqual({ at: [5, 5] });
  });
});
