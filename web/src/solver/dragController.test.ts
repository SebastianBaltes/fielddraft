import { beforeEach, describe, expect, it } from "vitest";
import { DragController } from "./dragController";
import { resetPlanegcsModuleForTests } from "./planegcs";
import { sketchWithOrigin, type SketchState } from "./types";

beforeEach(() => {
  resetPlanegcsModuleForTests();
});

function twoPointLineSketch(): SketchState {
  const s = sketchWithOrigin();
  s.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
  s.entities.push({ kind: "point", id: 2, at: { x: 10, y: 0 } });
  s.entities.push({ kind: "line", id: 3, start: 1, end: 2 });
  s.next_entity_id = 4;
  // Horizontal + fix one endpoint to origin. Dragging the
  // other endpoint should keep the line horizontal under the
  // drag controller's coincident-to-cursor hack.
  s.constraints.push({ kind: "coincident", id: 1, a: 0, b: 1 });
  s.constraints.push({ kind: "horizontal", id: 2, line: 3 });
  s.next_constraint_id = 3;
  return s;
}

describe("DragController lifecycle", () => {
  it("start() moves the dragged point to the initial cursor and returns the working state", () => {
    const sketch = twoPointLineSketch();
    const controller = new DragController();
    const working = controller.start(sketch, 2, { x: 20, y: 5 });
    expect(controller.isDragging()).toBe(true);
    // No helper entities or constraints — the controller
    // overwrites the dragged point's position directly.
    expect(working.entities.length).toBe(sketch.entities.length);
    expect(working.constraints.length).toBe(sketch.constraints.length);
    const moved = working.entities.find((e) => e.id === 2);
    if (moved && moved.kind === "point") {
      expect(moved.at.x).toBe(20);
      expect(moved.at.y).toBe(5);
    } else {
      throw new Error("expected point entity");
    }
  });

  it("start() throws when called twice without finish / cancel", () => {
    const controller = new DragController();
    controller.start(twoPointLineSketch(), 2, { x: 5, y: 0 });
    expect(() =>
      controller.start(twoPointLineSketch(), 2, { x: 5, y: 0 }),
    ).toThrow(/already in progress/);
  });

  it("finish() without start() throws", () => {
    const controller = new DragController();
    expect(() => controller.finish()).toThrow(/no active drag/);
  });

  it("update() without start() throws", async () => {
    const controller = new DragController();
    await expect(controller.update({ x: 0, y: 0 })).rejects.toThrow(
      /no active drag/,
    );
  });

  it("cancel() restores the pre-drag snapshot and clears active state", () => {
    const sketch = twoPointLineSketch();
    const controller = new DragController();
    controller.start(sketch, 2, { x: 20, y: 5 });
    const rolled = controller.cancel();
    expect(controller.isDragging()).toBe(false);
    expect(rolled).toEqual(sketch);
  });
});

describe("DragController solver integration", () => {
  it("update() moves the dragged point toward the cursor while respecting constraints", async () => {
    // Horizontal line pinned at origin on end 1; dragging
    // end 2 upwards shouldn't break horizontality — the solver
    // should keep end 2 on the x-axis in the y component.
    const sketch = twoPointLineSketch();
    const controller = new DragController();
    controller.start(sketch, 2, { x: 10, y: 0 });
    const result = await controller.update({ x: 15, y: 8 });
    expect(result.diagnostics.converged).toBe(true);

    const end = result.state.entities.find((e) => e.id === 2);
    if (end && end.kind === "point") {
      // End x should have moved forward; y should stay at 0
      // within tolerance because the horizontal constraint
      // wins over the temporary coincidence.
      expect(end.at.x).toBeGreaterThan(10);
      expect(Math.abs(end.at.y)).toBeLessThan(1e-4);
    } else {
      throw new Error("expected point entity for id 2");
    }
  });

  it("finish() strips the synthetic cursor and returns a clean sketch", async () => {
    const sketch = twoPointLineSketch();
    const controller = new DragController();
    controller.start(sketch, 2, { x: 10, y: 0 });
    await controller.update({ x: 15, y: 0 });
    const final = controller.finish();
    expect(controller.isDragging()).toBe(false);
    // Same entity and constraint counts as before the drag.
    expect(final.entities.length).toBe(sketch.entities.length);
    expect(final.constraints.length).toBe(sketch.constraints.length);
    // The dragged endpoint should reflect the moved position.
    const end = final.entities.find((e) => e.id === 2);
    if (end && end.kind === "point") {
      expect(end.at.x).toBeGreaterThan(10);
    } else {
      throw new Error("expected point entity");
    }
  });

  it("multi-step update continues from the solved state each step", async () => {
    const sketch = twoPointLineSketch();
    const controller = new DragController();
    controller.start(sketch, 2, { x: 10, y: 0 });
    await controller.update({ x: 12, y: 0 });
    await controller.update({ x: 14, y: 0 });
    const third = await controller.update({ x: 16, y: 0 });
    const end = third.state.entities.find((e) => e.id === 2);
    if (end && end.kind === "point") {
      expect(end.at.x).toBeCloseTo(16, 4);
    }
  });
});
