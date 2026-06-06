import { describe, expect, it } from "vitest";
import {
  DimensionVerdict,
  countDegreesOfFreedom,
  preflightDimension,
} from "./dofPreflight";
import { sketchWithOrigin, type SketchState } from "./types";

function rectangleSketch(): SketchState {
  // Four free points + four lines + H/V constraints + two
  // dimensions → the canonical "well-constrained rectangle"
  // that lives at DOF = 0 before the second distance lands.
  const s = sketchWithOrigin();
  s.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
  s.entities.push({ kind: "point", id: 2, at: { x: 25, y: 0 } });
  s.entities.push({ kind: "point", id: 3, at: { x: 25, y: 15 } });
  s.entities.push({ kind: "point", id: 4, at: { x: 0, y: 15 } });
  s.entities.push({ kind: "line", id: 5, start: 1, end: 2 });
  s.entities.push({ kind: "line", id: 6, start: 2, end: 3 });
  s.entities.push({ kind: "line", id: 7, start: 3, end: 4 });
  s.entities.push({ kind: "line", id: 8, start: 4, end: 1 });
  s.next_entity_id = 9;
  s.constraints.push({ kind: "coincident", id: 1, a: 0, b: 1 });
  s.constraints.push({ kind: "horizontal", id: 2, line: 5 });
  s.constraints.push({ kind: "horizontal", id: 3, line: 7 });
  s.constraints.push({ kind: "vertical", id: 4, line: 6 });
  s.constraints.push({ kind: "vertical", id: 5, line: 8 });
  s.constraints.push({
    kind: "distance",
    id: 6,
    a: 1,
    b: 2,
    value_mm: 25,
  });
  s.next_constraint_id = 7;
  return s;
}

describe("countDegreesOfFreedom", () => {
  it("counts zero for an empty sketch", () => {
    const s = sketchWithOrigin();
    expect(countDegreesOfFreedom(s)).toBe(0);
  });

  it("counts 2 for a single free point", () => {
    const s = sketchWithOrigin();
    s.entities.push({ kind: "point", id: 1, at: { x: 5, y: 5 } });
    s.next_entity_id = 2;
    expect(countDegreesOfFreedom(s)).toBe(2);
  });

  it("counts 4 for two free points plus a line connecting them", () => {
    const s = sketchWithOrigin();
    s.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    s.entities.push({ kind: "point", id: 2, at: { x: 10, y: 0 } });
    s.entities.push({ kind: "line", id: 3, start: 1, end: 2 });
    s.next_entity_id = 4;
    expect(countDegreesOfFreedom(s)).toBe(4);
  });

  it("subtracts 1 per simple-equation constraint", () => {
    const s = sketchWithOrigin();
    s.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    s.entities.push({ kind: "point", id: 2, at: { x: 10, y: 0 } });
    s.entities.push({ kind: "line", id: 3, start: 1, end: 2 });
    s.next_entity_id = 4;
    // 4 DOFs - 1 horizontal = 3 DOFs remaining.
    s.constraints.push({ kind: "horizontal", id: 1, line: 3 });
    s.next_constraint_id = 2;
    expect(countDegreesOfFreedom(s)).toBe(3);
  });

  it("subtracts 2 for a coincident constraint", () => {
    const s = sketchWithOrigin();
    s.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    s.entities.push({ kind: "point", id: 2, at: { x: 10, y: 0 } });
    s.next_entity_id = 3;
    // 4 DOFs - 2 coincident = 2 DOFs.
    s.constraints.push({ kind: "coincident", id: 1, a: 1, b: 2 });
    s.next_constraint_id = 2;
    expect(countDegreesOfFreedom(s)).toBe(2);
  });

  it("subtracts 2 for a fix constraint (pins both DOFs of a point)", () => {
    const s = sketchWithOrigin();
    s.entities.push({ kind: "point", id: 1, at: { x: 5, y: 5 } });
    s.next_entity_id = 2;
    // 2 DOFs - 2 fix = 0 DOFs (the point is fully pinned).
    s.constraints.push({ kind: "fix", id: 1, point: 1 });
    s.next_constraint_id = 2;
    expect(countDegreesOfFreedom(s)).toBe(0);
  });

  it("subtracts 1 for a point_on_object constraint (point slides along the curve)", () => {
    const s = sketchWithOrigin();
    // A circle (centre point 1 + circle 2) and a free point 3.
    s.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    s.entities.push({ kind: "circle", id: 2, center: 1, radius: 5 });
    s.entities.push({ kind: "point", id: 3, at: { x: 9, y: 0 } });
    s.next_entity_id = 4;
    // 2 (centre) + 1 (circle) + 2 (free point) = 5 DOF.
    // point_on_object removes 1 → 4.
    s.constraints.push({ kind: "point_on_object", id: 1, point: 3, curve: 2 });
    s.next_constraint_id = 2;
    expect(countDegreesOfFreedom(s)).toBe(4);
  });

  it("goes negative when over-constrained", () => {
    const s = sketchWithOrigin();
    s.entities.push({ kind: "point", id: 1, at: { x: 5, y: 5 } });
    s.next_entity_id = 2;
    // 2 DOF for the point, then pin it twice (fix removes 2 each) →
    // 2 - 2 - 2 = -2: redundantly over-constrained.
    s.constraints.push({ kind: "fix", id: 1, point: 1 });
    s.constraints.push({ kind: "fix", id: 2, point: 1 });
    s.next_constraint_id = 3;
    expect(countDegreesOfFreedom(s)).toBe(-2);
  });

  it("counts the canonical rectangle with width constraint at DOF = 1", () => {
    // The rectangleSketch helper has four points (8 DOF), four
    // lines (0), four H/V constraints (-4), one coincident
    // to origin (-2), and one width distance (-1) → 1 DOF left
    // (the height). Adding a height dimension brings it to 0.
    const s = rectangleSketch();
    expect(countDegreesOfFreedom(s)).toBe(1);
  });
});

describe("preflightDimension", () => {
  it("returns Driving when DOF would stay non-negative", () => {
    const s = rectangleSketch();
    // DOF = 1, adding height dimension → DOF = 0. Driving.
    const verdict = preflightDimension(s, {
      kind: "distance",
      id: -1,
      a: 2,
      b: 3,
      value_mm: 15,
    });
    expect(verdict).toBe(DimensionVerdict.Driving);
  });

  it("returns Driven when DOF would drop below zero", () => {
    const s = rectangleSketch();
    // Add the height dimension first so DOF = 0.
    s.constraints.push({
      kind: "distance",
      id: 7,
      a: 2,
      b: 3,
      value_mm: 15,
    });
    s.next_constraint_id = 8;
    // Attempt a third distance (diagonal) — would over-
    // constrain, so preflight demotes to driven.
    const verdict = preflightDimension(s, {
      kind: "distance",
      id: -1,
      a: 1,
      b: 3,
      value_mm: 29,
    });
    expect(verdict).toBe(DimensionVerdict.Driven);
  });

  it("passes through non-dimensional constraints as Driving without a check", () => {
    const s = rectangleSketch();
    const verdict = preflightDimension(s, {
      kind: "parallel",
      id: -1,
      a: 5,
      b: 7,
    });
    expect(verdict).toBe(DimensionVerdict.Driving);
  });

  it("preflight for angle behaves the same as distance", () => {
    // Zero DOF sketch — adding an angle should demote.
    const s = rectangleSketch();
    s.constraints.push({
      kind: "distance",
      id: 7,
      a: 2,
      b: 3,
      value_mm: 15,
    });
    s.next_constraint_id = 8;
    const verdict = preflightDimension(s, {
      kind: "angle",
      id: -1,
      a: 5,
      b: 6,
      value_deg: 90,
    });
    expect(verdict).toBe(DimensionVerdict.Driven);
  });
});
