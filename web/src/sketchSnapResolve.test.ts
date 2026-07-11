import { describe, expect, it } from "vitest";
import { classifyLineAnchor, resolveSnap } from "./sketchSnapResolve";
import { buildDraftSketchState } from "./sketchSession";
import type { SketchSessionState } from "./sketchSession";

/** Minimal session wrapper — resolveSnap only reads `subCalls`. */
function sessionOf(
  subCalls: Array<{ type: string; params: Record<string, unknown> }>,
): SketchSessionState {
  return { subCalls } as unknown as SketchSessionState;
}

describe("resolveSnap — curves beat orientation hints (ticket 3095b041)", () => {
  it("snaps a drawn endpoint onto a circle rim even when near-horizontal", () => {
    // Circle of radius 10 at (30, 0.5). Drawing from (0, 0) toward the
    // rim: the cursor at (20.2, 0.3) is near-horizontal (a H candidate
    // applies) AND within snap radius of the rim (~x=20.01 at y=0.3).
    // Real geometry must win: the snap lands ON the rim.
    const session = sessionOf([
      { type: "sketch_circle", params: { center: [30, 0.5], radius: 10 } },
    ]);
    const snap = resolveSnap([20.2, 0.3], session, [0, 0], 2);
    const d = Math.hypot(snap.snapped[0] - 30, snap.snapped[1] - 0.5);
    expect(d).toBeCloseTo(10, 6);
  });

  it("snaps onto an ARC rim (arcs are first-class snap targets now)", () => {
    const session = sessionOf([
      {
        type: "sketch_arc",
        params: {
          center: [0, 0],
          radius: 10,
          start_angle: 0,
          sweep_angle: Math.PI / 2,
        },
      },
    ]);
    // Cursor near the arc's 45° point (7.07, 7.07).
    const snap = resolveSnap([7.5, 7.5], session, null, 2);
    expect(Math.hypot(snap.snapped[0], snap.snapped[1])).toBeCloseTo(10, 6);
  });

  it("a vertex (coincident) snap still beats the curve projection", () => {
    const session = sessionOf([
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
    ]);
    // Cursor within radius of BOTH the line interior and its endpoint —
    // the endpoint (real vertex) wins.
    const snap = resolveSnap([9.5, 0.4], session, null, 2);
    expect(snap.snapped).toEqual([10, 0]);
    expect(snap.coincidentTargetId).toBe(2);
  });
});

describe("resolveSnap — excludeIds (grip drag, ticket b26062eb)", () => {
  const line = { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } };
  const circle = {
    type: "sketch_circle",
    params: { center: [20, 0], radius: 5 },
  };

  it("never snaps a dragged endpoint onto its own line", () => {
    const session = sessionOf([line]);
    // Cursor over the line's interior. Excluding the line's own ids
    // (start=1, end=2, line=3) must yield NO snap.
    const snap = resolveSnap(
      [5, 0.3],
      session,
      null,
      2,
      false,
      [],
      new Set([1, 2, 3]),
    );
    expect(snap.snapped).toEqual([5, 0.3]);
    expect(snap.anchor).toBeNull();
  });

  it("still snaps the dragged endpoint onto OTHER geometry", () => {
    const session = sessionOf([line, circle]);
    // Cursor near the circle rim (x = 15 side). Own line excluded.
    const snap = resolveSnap(
      [15.3, 0.2],
      session,
      null,
      2,
      false,
      [],
      new Set([1, 2, 3]),
    );
    const d = Math.hypot(snap.snapped[0] - 20, snap.snapped[1]);
    expect(d).toBeCloseTo(5, 6);
  });
});

describe("classifyLineAnchor — arcs and exclusions", () => {
  it("binds a point on an arc rim to the arc entity (point-on-object)", () => {
    const state = buildDraftSketchState([
      {
        type: "sketch_arc",
        params: {
          center: [0, 0],
          radius: 10,
          start_angle: 0,
          sweep_angle: Math.PI / 2,
        },
      },
    ]);
    const p45: [number, number] = [
      10 * Math.cos(Math.PI / 4),
      10 * Math.sin(Math.PI / 4),
    ];
    // ids: centre=1, start=2, end=3, arc=4
    expect(classifyLineAnchor(p45, state)).toEqual({ kind: "object", id: 4 });
  });

  it("does not bind outside the arc's live span", () => {
    const state = buildDraftSketchState([
      {
        type: "sketch_arc",
        params: {
          center: [0, 0],
          radius: 10,
          start_angle: 0,
          sweep_angle: Math.PI / 2,
        },
      },
    ]);
    // On the circle but at -90° — outside the 0..90° span.
    expect(classifyLineAnchor([0, -10], state)).toBeNull();
  });

  it("skips excluded entities (a dragged point can't bind to itself)", () => {
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
    ]);
    // Point sits exactly on the line's start vertex; excluding the
    // line's own ids must not classify point OR object.
    expect(classifyLineAnchor([0, 0], state, new Set([1, 2, 3]))).toBeNull();
  });
});

describe("spline tangent legs are not snap targets (ticket 7ad7d649)", () => {
  const spline = {
    type: "sketch_spline",
    params: {
      points: [0, 0, 10, 10, 20, 0, 30, 10],
      mode: "interpolate",
      closed: false,
    },
  };

  it("does not project a drawn point onto a leg line", () => {
    // Cursor near the middle of the start leg (0,0)→(10,10), but away
    // from the curve and both endpoints: no snap at all.
    const snap = resolveSnap([5.4, 4.6], sessionOf([spline]), null, 0.5);
    expect(snap.anchor).toBeNull();
    expect(snap.snapped).toEqual([5.4, 4.6]);
  });

  it("does not bind a dropped endpoint to a leg line", () => {
    const state = buildDraftSketchState([spline]);
    // Exactly on the start leg's interior: legs are guides — no anchor.
    expect(classifyLineAnchor([5, 5], state)).toBeNull();
  });

  it("still binds to the leg's endpoints (real control points)", () => {
    const state = buildDraftSketchState([spline]);
    expect(classifyLineAnchor([10, 10], state)).toEqual({
      kind: "point",
      id: 2,
    });
  });
});
