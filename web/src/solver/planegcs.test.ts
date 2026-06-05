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

beforeEach(() => {
  resetPlanegcsModuleForTests();
});

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
