import { beforeEach, describe, expect, it } from "vitest";
import { DragController } from "./dragController";
import { resetPlanegcsModuleForTests } from "./planegcs";
import type { SketchSolveHost, SubCall } from "./replaySolve";
import type { Grip } from "../sketchGrips";
import type { SketchDiagnostics } from "./types";

beforeEach(() => {
  resetPlanegcsModuleForTests();
});

/**
 * A horizontal line whose start point is coincident with the origin
 * and whose end point is free. Entity-id allocation (mirrored by
 * `buildDraftSketchState`): origin = 0, then the line's start = 1,
 * end = 2, line = 3. Dragging the end grip must keep the line
 * horizontal (end.y stays 0) while the end x tracks the cursor.
 */
function horizontalLineSubCalls(): SubCall[] {
  return [
    { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
    { type: "sketch_constraint_coincident", params: { a: 0, b: 1 } },
    { type: "sketch_constraint_horizontal", params: { line: 3 } },
  ];
}

/** The grip on the line's end point (sub-call index 0). */
function endGrip(): Grip {
  return { subCallIndex: 0, role: "line-end", pos: [10, 0] };
}

/** In-memory host stub. Exposes the live sub-calls / diagnostics the
 *  controller writes so tests can read the settled state. */
function makeHost(initial: SubCall[]) {
  let calls: ReadonlyArray<SubCall> = initial;
  let diagnostics: SketchDiagnostics | null = null;
  let active = true;
  const host: SketchSolveHost = {
    getState: () => ({ sketchSession: { active, subCalls: calls } }),
    replaceSketchSubCalls: (c) => {
      calls = c;
    },
    setSketchDiagnostics: (d) => {
      diagnostics = d;
    },
  };
  return {
    host,
    calls: () => calls,
    diagnostics: () => diagnostics,
    close: () => {
      active = false;
    },
  };
}

/** Read the solved end point back out of the line sub-call. */
function endPoint(calls: ReadonlyArray<SubCall>): [number, number] {
  const end = calls[0].params.end as [number, number];
  return [Number(end[0]), Number(end[1])];
}

describe("DragController lifecycle", () => {
  it("start() marks the drag active", () => {
    const { host } = makeHost(horizontalLineSubCalls());
    const controller = new DragController();
    expect(controller.isDragging()).toBe(false);
    controller.start(host, endGrip());
    expect(controller.isDragging()).toBe(true);
  });

  it("start() throws when called twice without finish / cancel", () => {
    const { host } = makeHost(horizontalLineSubCalls());
    const controller = new DragController();
    controller.start(host, endGrip());
    expect(() => controller.start(host, endGrip())).toThrow(
      /already in progress/,
    );
  });

  it("finish() without start() rejects", async () => {
    const controller = new DragController();
    await expect(controller.finish()).rejects.toThrow(/no active drag/);
  });

  it("cancel() without start() throws", () => {
    const controller = new DragController();
    expect(() => controller.cancel()).toThrow(/no active drag/);
  });

  it("update() before start() is a no-op", async () => {
    const controller = new DragController();
    await expect(controller.update([5, 5])).resolves.toBeUndefined();
    expect(controller.isDragging()).toBe(false);
  });

  it("cancel() restores the pre-drag sub-calls and clears active state", async () => {
    const initial = horizontalLineSubCalls();
    const h = makeHost(initial);
    const controller = new DragController();
    controller.start(h.host, endGrip());
    await controller.update([15, 8]);
    // The live solve moved the end; cancel must roll it back.
    controller.cancel();
    expect(controller.isDragging()).toBe(false);
    expect(endPoint(h.calls())).toEqual([10, 0]);
  });
});

describe("DragController live solving", () => {
  it("update() solves so the dragged end respects the horizontal constraint", async () => {
    const h = makeHost(horizontalLineSubCalls());
    const controller = new DragController();
    controller.start(h.host, endGrip());
    await controller.update([15, 8]);

    const [x, y] = endPoint(h.calls());
    // x follows the cursor (free), y is pinned to 0 by horizontality.
    expect(x).toBeGreaterThan(10);
    expect(Math.abs(y)).toBeLessThan(1e-4);
    expect(h.diagnostics()?.converged).toBe(true);
  });

  it("multi-step drag continues from the solved state each step", async () => {
    const h = makeHost(horizontalLineSubCalls());
    const controller = new DragController();
    controller.start(h.host, endGrip());
    await controller.update([12, 0]);
    await controller.update([14, 0]);
    await controller.update([16, 0]);
    const [x, y] = endPoint(h.calls());
    expect(x).toBeCloseTo(16, 3);
    expect(Math.abs(y)).toBeLessThan(1e-4);
  });

  it("coalesces rapid moves to the latest cursor", async () => {
    const h = makeHost(horizontalLineSubCalls());
    const controller = new DragController();
    controller.start(h.host, endGrip());
    // Fire several moves without awaiting; only the newest must win.
    void controller.update([12, 0]);
    void controller.update([18, 0]);
    await controller.update([25, 0]);
    const [x] = endPoint(h.calls());
    expect(x).toBeCloseTo(25, 3);
  });

  it("finish() runs an authoritative final solve at the last cursor", async () => {
    const h = makeHost(horizontalLineSubCalls());
    const controller = new DragController();
    controller.start(h.host, endGrip());
    // No await: coalescing may drop this frame — finish() must re-solve it.
    void controller.update([20, 6]);
    await controller.finish();
    expect(controller.isDragging()).toBe(false);
    const [x, y] = endPoint(h.calls());
    expect(x).toBeGreaterThan(10);
    expect(Math.abs(y)).toBeLessThan(1e-4);
  });

  it("a solve that lands after finish() does not clobber the session", async () => {
    const h = makeHost(horizontalLineSubCalls());
    const controller = new DragController();
    controller.start(h.host, endGrip());
    const inflight = controller.update([15, 8]);
    // End the drag immediately; the in-flight solve must discard its write.
    await controller.finish();
    await inflight;
    // finish()'s own final solve is the authoritative last writer.
    const [x, y] = endPoint(h.calls());
    expect(x).toBeGreaterThan(10);
    expect(Math.abs(y)).toBeLessThan(1e-4);
  });
});
