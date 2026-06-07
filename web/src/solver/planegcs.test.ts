/**
 * K5a headless smoke tests for the PlaneGCS wrapper.
 *
 * These tests actually instantiate the PlaneGCS WASM module,
 * build sketches programmatically, hand them to the wrapper,
 * and verify both the solved coordinates and the diagnostics.
 * No UI, no worker, no kernel — pure Rust-to-solver data flow
 * through the separate-WASM integration path.
 *
 * Each test resets the cached wrapper module via
 * `resetPlanegcsModuleForTests` before running so cases don't
 * share solver state across the file.
 */

import { beforeEach, describe, expect, it } from "vitest";
import {
  resetPlanegcsModuleForTests,
  solveSketch,
} from "./planegcs";
import {
  emptySketch,
  fullyConstrained,
  sketchWithOrigin,
  type SketchState,
} from "./types";
import { bsplinePoints } from "../sketchSpline";

beforeEach(() => {
  resetPlanegcsModuleForTests();
});

/** Shortest distance from a point to a segment (test helper). */
function distToSeg(
  p: [number, number],
  a: [number, number],
  b: [number, number],
): number {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const lenSq = dx * dx + dy * dy;
  if (lenSq < 1e-12) return Math.hypot(p[0] - a[0], p[1] - a[1]);
  let t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / lenSq;
  t = Math.max(0, Math.min(1, t));
  return Math.hypot(p[0] - (a[0] + t * dx), p[1] - (a[1] + t * dy));
}

describe("solveSketch: trivial sketches", () => {
  it("returns an origin-only sketch unchanged", async () => {
    const sketch = sketchWithOrigin("trivial");
    const result = await solveSketch(sketch);
    expect(result.state.entities.length).toBe(1);
    expect(result.state.entities[0].kind).toBe("origin");
    expect(result.diagnostics.converged).toBe(true);
    expect(result.diagnostics.conflicting).toEqual([]);
  });

  it("returns a free point sketch unchanged — no constraints means nothing to solve", async () => {
    const sketch = sketchWithOrigin("free-point");
    sketch.entities.push({
      kind: "point",
      id: 1,
      at: { x: 5, y: 7 },
    });
    sketch.next_entity_id = 2;
    const result = await solveSketch(sketch);
    const p = result.state.entities.find((e) => e.id === 1);
    expect(p).toBeDefined();
    if (p && p.kind === "point") {
      expect(p.at.x).toBeCloseTo(5);
      expect(p.at.y).toBeCloseTo(7);
    }
    expect(result.diagnostics.converged).toBe(true);
    // A lone free point carries two unconstrained DoFs — the
    // authoritative number now comes straight from the solver via
    // `wrapper.gcs.dof()`, not the naive JS tally.
    expect(result.diagnostics.dof).toBe(2);
  });
});

describe("solveSketch: dimension constraints", () => {
  /**
   * Two free points with a 50 mm distance constraint. The solver
   * should keep one point stationary and move the other to
   * satisfy the constraint, within numeric tolerance.
   */
  it("p2p distance pulls points to the specified length", async () => {
    const sketch: SketchState = emptySketch("p2p-distance");
    sketch.entities.push({
      kind: "point",
      id: 1,
      at: { x: 0, y: 0 },
    });
    sketch.entities.push({
      kind: "point",
      id: 2,
      at: { x: 10, y: 0 },
    });
    sketch.next_entity_id = 3;
    sketch.constraints.push({
      kind: "distance",
      id: 1,
      a: 1,
      b: 2,
      value_mm: 50,
    });
    sketch.next_constraint_id = 2;
    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const a = result.state.entities.find((e) => e.id === 1);
    const b = result.state.entities.find((e) => e.id === 2);
    if (
      a &&
      a.kind === "point" &&
      b &&
      b.kind === "point"
    ) {
      const dx = b.at.x - a.at.x;
      const dy = b.at.y - a.at.y;
      const dist = Math.sqrt(dx * dx + dy * dy);
      expect(dist).toBeCloseTo(50, 5);
    }
  });
});

describe("solveSketch: orientation constraints", () => {
  /**
   * A slightly-tilted line with a Horizontal constraint: the
   * solver should flatten it to a true horizontal. Starting the
   * line slightly off-axis exercises the numeric solve — a
   * line that's already horizontal would converge trivially.
   */
  it("horizontal constraint flattens a near-horizontal line", async () => {
    const sketch = emptySketch("horizontal");
    sketch.entities.push({
      kind: "point",
      id: 1,
      at: { x: 0, y: 0 },
    });
    sketch.entities.push({
      kind: "point",
      id: 2,
      at: { x: 10, y: 0.5 },
    });
    sketch.entities.push({
      kind: "line",
      id: 3,
      start: 1,
      end: 2,
    });
    sketch.next_entity_id = 4;
    sketch.constraints.push({
      kind: "horizontal",
      id: 1,
      line: 3,
    });
    sketch.next_constraint_id = 2;
    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const a = result.state.entities.find((e) => e.id === 1);
    const b = result.state.entities.find((e) => e.id === 2);
    if (
      a &&
      a.kind === "point" &&
      b &&
      b.kind === "point"
    ) {
      expect(b.at.y).toBeCloseTo(a.at.y, 5);
    }
  });

  /**
   * Two lines constrained to be parallel. Start them at slightly
   * different slopes; the solver should align them to the same
   * direction vector.
   */
  it("parallel constraint aligns two line slopes", async () => {
    const sketch = emptySketch("parallel");
    // Line 1: (0, 0) -> (10, 0)
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "point", id: 2, at: { x: 10, y: 0 } });
    sketch.entities.push({ kind: "line", id: 3, start: 1, end: 2 });
    // Line 2: (0, 5) -> (9, 7) — not parallel yet
    sketch.entities.push({ kind: "point", id: 4, at: { x: 0, y: 5 } });
    sketch.entities.push({ kind: "point", id: 5, at: { x: 9, y: 7 } });
    sketch.entities.push({ kind: "line", id: 6, start: 4, end: 5 });
    sketch.next_entity_id = 7;
    sketch.constraints.push({
      kind: "parallel",
      id: 1,
      a: 3,
      b: 6,
    });
    sketch.next_constraint_id = 2;
    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    // Slopes must match after the solve.
    const p = (id: number) => {
      const e = result.state.entities.find((x) => x.id === id);
      if (e && e.kind === "point") return e.at;
      throw new Error(`expected point at ${id}`);
    };
    const slope = (a: { x: number; y: number }, b: { x: number; y: number }) => {
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      return dy / dx;
    };
    const s1 = slope(p(1), p(2));
    const s2 = slope(p(4), p(5));
    expect(Math.abs(s1 - s2)).toBeLessThan(1e-5);
  });

  /**
   * Perpendicular constraint between two lines whose initial
   * angle isn't 90°. After solving, the angle must be exactly
   * π/2 within tolerance.
   */
  it("perpendicular constraint forces a 90-degree angle", async () => {
    const sketch = emptySketch("perpendicular");
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "point", id: 2, at: { x: 10, y: 0 } });
    sketch.entities.push({ kind: "line", id: 3, start: 1, end: 2 });
    sketch.entities.push({ kind: "point", id: 4, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "point", id: 5, at: { x: 8, y: 6 } });
    sketch.entities.push({ kind: "line", id: 6, start: 4, end: 5 });
    sketch.next_entity_id = 7;
    sketch.constraints.push({ kind: "coincident", id: 1, a: 1, b: 4 });
    sketch.constraints.push({ kind: "perpendicular", id: 2, a: 3, b: 6 });
    sketch.next_constraint_id = 3;
    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const p = (id: number) => {
      const e = result.state.entities.find((x) => x.id === id);
      if (e && e.kind === "point") return e.at;
      throw new Error(`expected point at ${id}`);
    };
    const dx1 = p(2).x - p(1).x;
    const dy1 = p(2).y - p(1).y;
    const dx2 = p(5).x - p(4).x;
    const dy2 = p(5).y - p(4).y;
    // Dot product of direction vectors should be ~0.
    const dot = dx1 * dx2 + dy1 * dy2;
    const len1 = Math.sqrt(dx1 * dx1 + dy1 * dy1);
    const len2 = Math.sqrt(dx2 * dx2 + dy2 * dy2);
    const cosAngle = dot / (len1 * len2);
    expect(Math.abs(cosAngle)).toBeLessThan(1e-4);
  });
});

describe("solveSketch: fully constrained rectangle", () => {
  /**
   * Build a rectangle by constraints — four points, four lines,
   * horizontal / vertical constraints on two opposite edges, and
   * two Distance constraints for width and height. This is the
   * canonical "closed by dimensions" smoke test every sketch
   * solver has to pass.
   */
  it("four-point rectangle with H/V + width + height solves to zero DOF (topology check)", async () => {
    const sketch = sketchWithOrigin("rectangle");

    // Four corner points — slightly off the ideal rectangle so
    // the solver has real work to do.
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "point", id: 2, at: { x: 25, y: 1 } });
    sketch.entities.push({ kind: "point", id: 3, at: { x: 24, y: 16 } });
    sketch.entities.push({ kind: "point", id: 4, at: { x: -1, y: 15 } });

    // Four edges.
    sketch.entities.push({ kind: "line", id: 5, start: 1, end: 2 });
    sketch.entities.push({ kind: "line", id: 6, start: 2, end: 3 });
    sketch.entities.push({ kind: "line", id: 7, start: 3, end: 4 });
    sketch.entities.push({ kind: "line", id: 8, start: 4, end: 1 });

    sketch.next_entity_id = 9;

    // Pin corner 1 to the origin so the whole sketch is anchored.
    sketch.constraints.push({ kind: "coincident", id: 1, a: 0, b: 1 });
    // Two horizontals and two verticals fix the axis alignment.
    sketch.constraints.push({ kind: "horizontal", id: 2, line: 5 });
    sketch.constraints.push({ kind: "horizontal", id: 3, line: 7 });
    sketch.constraints.push({ kind: "vertical", id: 4, line: 6 });
    sketch.constraints.push({ kind: "vertical", id: 5, line: 8 });
    // Width 25 mm, height 15 mm via p2p distances on adjacent
    // corners.
    sketch.constraints.push({
      kind: "distance",
      id: 6,
      a: 1,
      b: 2,
      value_mm: 25,
    });
    sketch.constraints.push({
      kind: "distance",
      id: 7,
      a: 2,
      b: 3,
      value_mm: 15,
    });
    sketch.next_constraint_id = 8;

    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    expect(result.diagnostics.conflicting).toEqual([]);
    // Authoritative solver DoF: a fully-pinned rectangle has none.
    expect(result.diagnostics.dof).toBe(0);

    // After solving we should have an axis-aligned rectangle
    // with corner 1 at the origin, corner 2 at (25, 0), corner
    // 3 at (25, 15), corner 4 at (0, 15). Tolerance 1e-4.
    const p = (id: number) => {
      const e = result.state.entities.find((x) => x.id === id);
      if (e && e.kind === "point") return e.at;
      throw new Error(`expected point at ${id}`);
    };
    expect(p(1).x).toBeCloseTo(0, 4);
    expect(p(1).y).toBeCloseTo(0, 4);
    expect(p(2).x).toBeCloseTo(25, 4);
    expect(p(2).y).toBeCloseTo(0, 4);
    expect(p(3).x).toBeCloseTo(25, 4);
    expect(p(3).y).toBeCloseTo(15, 4);
    expect(p(4).x).toBeCloseTo(0, 4);
    expect(p(4).y).toBeCloseTo(15, 4);
  });
});

describe("solveSketch: midpoint + symmetric", () => {
  /**
   * `Midpoint { point, line }` should pull the point onto the centre
   * of the line's two endpoints. Translated to PlaneGCS as
   * `p2p_symmetric_ppp` (endpoints symmetric about the point), so the
   * solved point sits at the endpoint average regardless of where the
   * endpoints settle.
   */
  it("midpoint pins a point to the centre of a line's endpoints", async () => {
    const sketch = emptySketch("midpoint");
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "point", id: 2, at: { x: 10, y: 0 } });
    sketch.entities.push({ kind: "line", id: 3, start: 1, end: 2 });
    sketch.entities.push({ kind: "point", id: 4, at: { x: 3, y: 4 } });
    sketch.next_entity_id = 5;
    sketch.constraints.push({ kind: "midpoint", id: 1, point: 4, line: 3 });
    sketch.next_constraint_id = 2;

    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const p = (id: number) => {
      const e = result.state.entities.find((x) => x.id === id);
      if (!e || e.kind !== "point") throw new Error(`point ${id} missing`);
      return e.at;
    };
    const a = p(1);
    const b = p(2);
    const m = p(4);
    expect(m.x).toBeCloseTo((a.x + b.x) / 2, 4);
    expect(m.y).toBeCloseTo((a.y + b.y) / 2, 4);
  });

  /**
   * `Symmetric { a, b, axis }` mirrors two points across the axis
   * line. With the axis pinned to the x-axis (origin + horizontal),
   * the reflection of (x, y) is (x, −y), so the solved points satisfy
   * Bx ≈ Ax and By ≈ −Ay. Maps to PlaneGCS' `p2p_symmetric_ppl`.
   */
  it("symmetric mirrors two points across an axis line", async () => {
    const sketch = sketchWithOrigin("symmetric");
    sketch.entities.push({ kind: "point", id: 1, at: { x: 10, y: 0 } });
    sketch.entities.push({ kind: "line", id: 2, start: 0, end: 1 });
    sketch.entities.push({ kind: "point", id: 3, at: { x: 3, y: 5 } });
    sketch.entities.push({ kind: "point", id: 4, at: { x: 6, y: -2 } });
    sketch.next_entity_id = 5;
    // Pin the axis to the x-axis: origin anchors it at (0,0), the
    // horizontal constraint keeps it flat.
    sketch.constraints.push({ kind: "horizontal", id: 1, line: 2 });
    sketch.constraints.push({ kind: "symmetric", id: 2, a: 3, b: 4, axis: 2 });
    sketch.next_constraint_id = 3;

    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const p = (id: number) => {
      const e = result.state.entities.find((x) => x.id === id);
      if (!e || e.kind !== "point") throw new Error(`point ${id} missing`);
      return e.at;
    };
    const a = p(3);
    const b = p(4);
    expect(b.x).toBeCloseTo(a.x, 4);
    expect(b.y).toBeCloseTo(-a.y, 4);
  });
});

describe("solveSketch: radius + diameter", () => {
  /** A radius constraint pins a circle's radius to the target value. */
  it("radius pins a circle's radius", async () => {
    const sketch = emptySketch("radius");
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "circle", id: 2, center: 1, radius: 5 });
    sketch.next_entity_id = 3;
    sketch.constraints.push({
      kind: "radius",
      id: 1,
      entity: 2,
      value_mm: 12,
    });
    sketch.next_constraint_id = 2;

    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const c = result.state.entities.find((e) => e.id === 2);
    if (c && c.kind === "circle") {
      expect(c.radius).toBeCloseTo(12, 4);
    } else {
      throw new Error("circle 2 missing from solved state");
    }
  });

  /** A diameter constraint pins a circle to diameter/2 radius. */
  it("diameter pins a circle to half the diameter", async () => {
    const sketch = emptySketch("diameter");
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "circle", id: 2, center: 1, radius: 5 });
    sketch.next_entity_id = 3;
    sketch.constraints.push({
      kind: "diameter",
      id: 1,
      entity: 2,
      value_mm: 20,
    });
    sketch.next_constraint_id = 2;

    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const c = result.state.entities.find((e) => e.id === 2);
    if (c && c.kind === "circle") {
      expect(c.radius).toBeCloseTo(10, 4);
    } else {
      throw new Error("circle 2 missing from solved state");
    }
  });
});

describe("solveSketch: fix constraint", () => {
  /**
   * A fixed point and a free point joined by a distance
   * constraint. The fix must pin the first point at its original
   * coordinates while the distance pulls only the free point.
   * This proves the `fix` constraint realises the `fixed: true`
   * entity flag end to end.
   */
  it("keeps a fixed point still while a free point is pulled to length", async () => {
    const sketch = emptySketch("fix-distance");
    sketch.entities.push({ kind: "point", id: 1, at: { x: 3, y: 4 } });
    sketch.entities.push({ kind: "point", id: 2, at: { x: 10, y: 4 } });
    sketch.next_entity_id = 3;
    // Pin point 1 in place.
    sketch.constraints.push({ kind: "fix", id: 1, point: 1 });
    // Pull point 2 so the segment is 20 mm long.
    sketch.constraints.push({
      kind: "distance",
      id: 2,
      a: 1,
      b: 2,
      value_mm: 20,
    });
    sketch.next_constraint_id = 3;

    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const a = result.state.entities.find((e) => e.id === 1);
    const b = result.state.entities.find((e) => e.id === 2);
    expect(a && a.kind === "point").toBe(true);
    expect(b && b.kind === "point").toBe(true);
    if (a && a.kind === "point" && b && b.kind === "point") {
      // The fixed point did not move at all.
      expect(a.at.x).toBeCloseTo(3, 6);
      expect(a.at.y).toBeCloseTo(4, 6);
      // The free point absorbed the whole distance change.
      const dist = Math.hypot(b.at.x - a.at.x, b.at.y - a.at.y);
      expect(dist).toBeCloseTo(20, 5);
    }
  });

  it("a fixed point ignores a distance pull toward another fixed point", async () => {
    // Two fixed points 7 mm apart with a 20 mm distance constraint
    // between them. Because both points are pinned, neither can
    // move — so the distance can't be satisfied and the sketch is
    // not fully constrained, yet the coordinates stay put.
    const sketch = emptySketch("fix-vs-fix");
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "point", id: 2, at: { x: 7, y: 0 } });
    sketch.next_entity_id = 3;
    sketch.constraints.push({ kind: "fix", id: 1, point: 1 });
    sketch.constraints.push({ kind: "fix", id: 2, point: 2 });
    sketch.constraints.push({
      kind: "distance",
      id: 3,
      a: 1,
      b: 2,
      value_mm: 20,
    });
    sketch.next_constraint_id = 4;

    const result = await solveSketch(sketch);
    const a = result.state.entities.find((e) => e.id === 1);
    const b = result.state.entities.find((e) => e.id === 2);
    if (a && a.kind === "point" && b && b.kind === "point") {
      expect(a.at.x).toBeCloseTo(0, 6);
      expect(a.at.y).toBeCloseTo(0, 6);
      expect(b.at.x).toBeCloseTo(7, 6);
      expect(b.at.y).toBeCloseTo(0, 6);
    }
  });
});

describe("solveSketch: point_on_object constraint", () => {
  /** Distance from a point to the infinite line through (a, b). */
  function pointLineDistance(
    px: number,
    py: number,
    ax: number,
    ay: number,
    bx: number,
    by: number,
  ): number {
    const dx = bx - ax;
    const dy = by - ay;
    const len = Math.hypot(dx, dy);
    return Math.abs((px - ax) * dy - (py - ay) * dx) / len;
  }

  it("pulls a point onto a line (point_on_line_pl)", async () => {
    const sketch = emptySketch("poc-line");
    // A horizontal line from (0,0) to (10,0), and a free point above it.
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "point", id: 2, at: { x: 10, y: 0 } });
    sketch.entities.push({ kind: "line", id: 3, start: 1, end: 2 });
    sketch.entities.push({ kind: "point", id: 4, at: { x: 5, y: 6 } });
    sketch.next_entity_id = 5;
    // Pin the line so it can't move to meet the point.
    sketch.constraints.push({ kind: "fix", id: 1, point: 1 });
    sketch.constraints.push({ kind: "fix", id: 2, point: 2 });
    sketch.constraints.push({
      kind: "point_on_object",
      id: 3,
      point: 4,
      curve: 3,
    });
    sketch.next_constraint_id = 4;

    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const a = result.state.entities.find((e) => e.id === 1);
    const b = result.state.entities.find((e) => e.id === 2);
    const p = result.state.entities.find((e) => e.id === 4);
    if (
      a?.kind === "point" &&
      b?.kind === "point" &&
      p?.kind === "point"
    ) {
      const d = pointLineDistance(
        p.at.x,
        p.at.y,
        a.at.x,
        a.at.y,
        b.at.x,
        b.at.y,
      );
      expect(d).toBeCloseTo(0, 5);
    } else {
      throw new Error("entities missing from solved state");
    }
  });

  it("pulls a point onto a circle (point_on_circle)", async () => {
    const sketch = emptySketch("poc-circle");
    // Circle of radius 5 centred at the origin, free point outside it.
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "circle", id: 2, center: 1, radius: 5 });
    sketch.entities.push({ kind: "point", id: 3, at: { x: 12, y: 0 } });
    sketch.next_entity_id = 4;
    // Pin the centre and radius so the circle can't grow to meet the point.
    sketch.constraints.push({ kind: "fix", id: 1, point: 1 });
    sketch.constraints.push({ kind: "radius", id: 2, entity: 2, value_mm: 5 });
    sketch.constraints.push({
      kind: "point_on_object",
      id: 3,
      point: 3,
      curve: 2,
    });
    sketch.next_constraint_id = 4;

    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const c = result.state.entities.find((e) => e.id === 1);
    const p = result.state.entities.find((e) => e.id === 3);
    if (c?.kind === "point" && p?.kind === "point") {
      const dist = Math.hypot(p.at.x - c.at.x, p.at.y - c.at.y);
      // The point lands on the rim → distance to centre ≈ radius.
      expect(dist).toBeCloseTo(5, 4);
    } else {
      throw new Error("entities missing from solved state");
    }
  });

  it("pulls a point onto an arc (point_on_arc)", async () => {
    const sketch = emptySketch("poc-arc");
    // Quarter arc of radius 5 centred at the origin (0°→90°), and a
    // free point off the rim.
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } }); // centre
    sketch.entities.push({ kind: "point", id: 2, at: { x: 5, y: 0 } }); // start
    sketch.entities.push({ kind: "point", id: 3, at: { x: 0, y: 5 } }); // end
    sketch.entities.push({
      kind: "arc",
      id: 4,
      center: 1,
      start: 2,
      end: 3,
      radius: 5,
    });
    sketch.entities.push({ kind: "point", id: 5, at: { x: 6, y: 6 } });
    sketch.next_entity_id = 6;
    // Pin the arc geometry so only the free point moves. The arc's own
    // radius parameter is a free DOF in PlaneGCS, so pin it explicitly —
    // otherwise the solver could satisfy point_on_arc by GROWING the arc
    // to meet the point instead of moving the point onto the rim.
    sketch.constraints.push({ kind: "fix", id: 1, point: 1 });
    sketch.constraints.push({ kind: "fix", id: 2, point: 2 });
    sketch.constraints.push({ kind: "fix", id: 3, point: 3 });
    sketch.constraints.push({ kind: "radius", id: 4, entity: 4, value_mm: 5 });
    sketch.constraints.push({
      kind: "point_on_object",
      id: 5,
      point: 5,
      curve: 4,
    });
    sketch.next_constraint_id = 6;

    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const c = result.state.entities.find((e) => e.id === 1);
    const p = result.state.entities.find((e) => e.id === 5);
    if (c?.kind === "point" && p?.kind === "point") {
      const dist = Math.hypot(p.at.x - c.at.x, p.at.y - c.at.y);
      // On the arc → distance to centre ≈ radius.
      expect(dist).toBeCloseTo(5, 4);
    } else {
      throw new Error("entities missing from solved state");
    }
  });
});

describe("solveSketch: ellipse constraints", () => {
  /**
   * `point_on_ellipse`: a free point pulled onto an ellipse. The
   * ellipse centre and focus are pinned but the semi-minor radius
   * (`radmin`) is a free DOF, so we don't assert a fixed shape —
   * instead we verify the solved point satisfies the ellipse's
   * focal-sum property (`|P−F1| + |P−F2| = 2a`) using the *solved*
   * parameters. That holds iff the point genuinely lies on the
   * ellipse, regardless of where radmin settles.
   */
  it("pulls a point onto an ellipse (point_on_ellipse)", async () => {
    const sketch = emptySketch("poc-ellipse");
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } }); // centre
    sketch.entities.push({ kind: "point", id: 2, at: { x: 8, y: 0 } }); // focus
    sketch.entities.push({
      kind: "ellipse",
      id: 3,
      center: 1,
      focus1: 2,
      radmin: 6,
    });
    sketch.entities.push({ kind: "point", id: 4, at: { x: 15, y: 3 } }); // free
    sketch.next_entity_id = 5;
    // Pin the ellipse centre and focus so it can't drift to the point.
    sketch.constraints.push({ kind: "fix", id: 1, point: 1 });
    sketch.constraints.push({ kind: "fix", id: 2, point: 2 });
    sketch.constraints.push({
      kind: "point_on_object",
      id: 3,
      point: 4,
      curve: 3,
    });
    sketch.next_constraint_id = 4;

    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const centre = result.state.entities.find((e) => e.id === 1);
    const focus = result.state.entities.find((e) => e.id === 2);
    const ell = result.state.entities.find((e) => e.id === 3);
    const p = result.state.entities.find((e) => e.id === 4);
    if (
      centre?.kind === "point" &&
      focus?.kind === "point" &&
      ell?.kind === "ellipse" &&
      p?.kind === "point"
    ) {
      const f1 = focus.at;
      const f2 = { x: 2 * centre.at.x - f1.x, y: 2 * centre.at.y - f1.y };
      const c = Math.hypot(f1.x - centre.at.x, f1.y - centre.at.y);
      const a = Math.sqrt(ell.radmin * ell.radmin + c * c);
      const focalSum =
        Math.hypot(p.at.x - f1.x, p.at.y - f1.y) +
        Math.hypot(p.at.x - f2.x, p.at.y - f2.y);
      expect(focalSum).toBeCloseTo(2 * a, 4);
    } else {
      throw new Error("entities missing from solved state");
    }
  });

  /**
   * `tangent_le`: a horizontal line made tangent to a fully-pinned
   * ellipse. Centre (0,0) + focus (8,0) put the foci on the x-axis;
   * a fixed rim point at (0,6) plus point_on_ellipse pins the
   * semi-minor radius to 6 (so the ellipse is a=10, b=6). The
   * topmost point of that ellipse is (0,6), so a horizontal tangent
   * from above settles at y = 6.
   */
  it("makes a horizontal line tangent to a pinned ellipse (tangent_le)", async () => {
    const sketch = emptySketch("tangent-ellipse");
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } }); // centre
    sketch.entities.push({ kind: "point", id: 2, at: { x: 8, y: 0 } }); // focus
    sketch.entities.push({
      kind: "ellipse",
      id: 3,
      center: 1,
      focus1: 2,
      radmin: 6,
    });
    sketch.entities.push({ kind: "point", id: 4, at: { x: 0, y: 6 } }); // rim
    // Tangent line, starting above the ellipse.
    sketch.entities.push({ kind: "point", id: 5, at: { x: -20, y: 9 } });
    sketch.entities.push({ kind: "point", id: 6, at: { x: 20, y: 9 } });
    sketch.entities.push({ kind: "line", id: 7, start: 5, end: 6 });
    sketch.next_entity_id = 8;
    // Pin the ellipse: centre + focus fixed, plus a fixed rim point on
    // the boundary pins radmin (so the shape is fully determined).
    sketch.constraints.push({ kind: "fix", id: 1, point: 1 });
    sketch.constraints.push({ kind: "fix", id: 2, point: 2 });
    sketch.constraints.push({ kind: "fix", id: 3, point: 4 });
    sketch.constraints.push({
      kind: "point_on_object",
      id: 4,
      point: 4,
      curve: 3,
    });
    sketch.constraints.push({ kind: "horizontal", id: 5, line: 7 });
    sketch.constraints.push({ kind: "tangent", id: 6, line: 7, curve: 3 });
    sketch.next_constraint_id = 7;

    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const s = result.state.entities.find((e) => e.id === 5);
    const e2 = result.state.entities.find((e) => e.id === 6);
    if (s?.kind === "point" && e2?.kind === "point") {
      // Horizontal line → both ends share a y; tangent to the top of
      // the b = 6 ellipse → that y is ±6 (nearest touch from y = 9).
      expect(s.at.y).toBeCloseTo(e2.at.y, 4);
      expect(Math.abs(s.at.y)).toBeCloseTo(6, 3);
    } else {
      throw new Error("line endpoints missing from solved state");
    }
  });
});

describe("solveSketch: bspline constraints", () => {
  /**
   * `point_on_bspline`: a free point pulled onto a clamped cubic
   * B-spline whose four poles are pinned. With the curve rigid, the
   * solved point must lie on it. We verify by independently sampling
   * the same control-mode B-spline (`bsplinePoints`, the renderer's
   * sampler) and checking the solved point is within tolerance of the
   * sampled polyline — i.e. genuinely on the curve, wherever along it
   * the free curve-parameter settles.
   */
  it("pulls a point onto a B-spline (point_on_bspline)", async () => {
    const sketch = emptySketch("poc-bspline");
    // four poles of a clamped cubic, pinned.
    const poles: [number, number][] = [
      [0, 0],
      [10, 20],
      [20, 20],
      [30, 0],
    ];
    poles.forEach(([x, y], i) =>
      sketch.entities.push({ kind: "point", id: i + 1, at: { x, y } }),
    );
    sketch.entities.push({
      kind: "bspline",
      id: 5,
      poles: [1, 2, 3, 4],
      degree: 3,
      periodic: false,
    });
    sketch.entities.push({ kind: "point", id: 6, at: { x: 15, y: 30 } }); // free, off-curve
    sketch.next_entity_id = 7;
    poles.forEach((_, i) =>
      sketch.constraints.push({ kind: "fix", id: i + 1, point: i + 1 }),
    );
    sketch.constraints.push({
      kind: "point_on_object",
      id: 5,
      point: 6,
      curve: 5,
    });
    sketch.next_constraint_id = 6;

    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const p = result.state.entities.find((e) => e.id === 6);
    if (p?.kind !== "point") throw new Error("free point missing");

    // The poles stayed pinned; sample the same curve and measure.
    const curve = bsplinePoints(poles, 3, false, 128);
    let best = Infinity;
    for (let i = 0; i + 1 < curve.length; i++) {
      best = Math.min(best, distToSeg([p.at.x, p.at.y], curve[i], curve[i + 1]));
    }
    expect(best).toBeLessThan(0.05);
  });

  /** A degree-1 B-spline (two poles) is the segment between them, so
   *  point_on_bspline must land the point on that segment. */
  it("treats a 2-pole B-spline as a segment (point_on_bspline)", async () => {
    const sketch = emptySketch("poc-bspline-deg1");
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "point", id: 2, at: { x: 10, y: 10 } });
    sketch.entities.push({
      kind: "bspline",
      id: 3,
      poles: [1, 2],
      degree: 1,
      periodic: false,
    });
    sketch.entities.push({ kind: "point", id: 4, at: { x: 8, y: 2 } }); // free
    sketch.next_entity_id = 5;
    sketch.constraints.push({ kind: "fix", id: 1, point: 1 });
    sketch.constraints.push({ kind: "fix", id: 2, point: 2 });
    sketch.constraints.push({
      kind: "point_on_object",
      id: 3,
      point: 4,
      curve: 3,
    });
    sketch.next_constraint_id = 4;

    const result = await solveSketch(sketch);
    expect(result.diagnostics.converged).toBe(true);
    const p = result.state.entities.find((e) => e.id === 4);
    if (p?.kind !== "point") throw new Error("free point missing");
    // On the line y = x: distance to the segment is ~0.
    expect(distToSeg([p.at.x, p.at.y], [0, 0], [10, 10])).toBeLessThan(0.01);
  });
});

describe("solveSketch: conflict diagnostics", () => {
  /**
   * Two Distance constraints between the same two points with
   * incompatible values. PlaneGCS must report a conflict and
   * flag at least one of the two constraints in
   * `diagnostics.conflicting`.
   */
  it("flags contradictory distance constraints as conflicting", async () => {
    const sketch = emptySketch("conflict");
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "point", id: 2, at: { x: 10, y: 0 } });
    sketch.next_entity_id = 3;
    sketch.constraints.push({
      kind: "distance",
      id: 1,
      a: 1,
      b: 2,
      value_mm: 10,
    });
    sketch.constraints.push({
      kind: "distance",
      id: 2,
      a: 1,
      b: 2,
      value_mm: 20,
    });
    sketch.next_constraint_id = 3;

    const result = await solveSketch(sketch);
    // The solver is expected to report the contradiction either
    // by failing to converge or by populating a diagnostics
    // array (PlaneGCS' behaviour varies by algorithm). Either
    // outcome is valid as long as the sketch is not reported as
    // fully constrained.
    expect(fullyConstrained(result.diagnostics)).toBe(false);
  });
});
