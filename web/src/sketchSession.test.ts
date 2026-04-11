import { describe, expect, it } from "vitest";
import {
  DRAFT_DRAG_START_ID,
  INACTIVE_SKETCH_SESSION,
  applySketchToolDraft,
  buildDraftSketchState,
  enterSketchSession,
  exitSketchSession,
  selectSketchTool,
  setSketchToolDraft,
} from "./sketchSession";

describe("sketchSession reducers", () => {
  it("starts in the inactive shape", () => {
    expect(INACTIVE_SKETCH_SESSION.active).toBe(false);
    expect(INACTIVE_SKETCH_SESSION.planeQuery).toBeNull();
    expect(INACTIVE_SKETCH_SESSION.selectedTool).toBeNull();
    expect(INACTIVE_SKETCH_SESSION.subCalls).toEqual([]);
  });

  it("enterSketchSession flips the active flag and locks the plane", () => {
    const next = enterSketchSession(INACTIVE_SKETCH_SESSION, "qFaceOf(extrude_1)");
    expect(next.active).toBe(true);
    expect(next.planeQuery).toBe("qFaceOf(extrude_1)");
    expect(next.selectedTool).toBeNull();
  });

  it("enterSketchSession is a no-op when already active", () => {
    const once = enterSketchSession(INACTIVE_SKETCH_SESSION, "qA");
    const twice = enterSketchSession(once, "qB");
    expect(twice).toBe(once);
  });

  it("exitSketchSession returns to the inactive shape", () => {
    const session = enterSketchSession(INACTIVE_SKETCH_SESSION, "qA");
    const exited = exitSketchSession(session);
    expect(exited.active).toBe(false);
    expect(exited.selectedTool).toBeNull();
  });

  it("exitSketchSession is a no-op when inactive", () => {
    const same = exitSketchSession(INACTIVE_SKETCH_SESSION);
    expect(same).toBe(INACTIVE_SKETCH_SESSION);
  });

  it("selectSketchTool stores the tool name and the initial draft", () => {
    const session = enterSketchSession(INACTIVE_SKETCH_SESSION, "qA");
    const picked = selectSketchTool(session, "sketch_point", { at: [1, 2] });
    expect(picked.selectedTool).toBe("sketch_point");
    expect(picked.toolDraft).toEqual({ at: [1, 2] });
  });

  it("selectSketchTool clears the draft when typeName is null", () => {
    const picked = selectSketchTool(
      enterSketchSession(INACTIVE_SKETCH_SESSION, "qA"),
      "sketch_point",
      { at: [1, 2] },
    );
    const cleared = selectSketchTool(picked, null);
    expect(cleared.selectedTool).toBeNull();
    expect(cleared.toolDraft).toEqual({});
  });

  it("selectSketchTool is a no-op on an inactive session", () => {
    const same = selectSketchTool(INACTIVE_SKETCH_SESSION, "sketch_point");
    expect(same).toBe(INACTIVE_SKETCH_SESSION);
  });

  it("setSketchToolDraft replaces the working copy", () => {
    const picked = selectSketchTool(
      enterSketchSession(INACTIVE_SKETCH_SESSION, "qA"),
      "sketch_line",
      { start: [0, 0], end: [10, 0] },
    );
    const edited = setSketchToolDraft(picked, {
      start: [0, 0],
      end: [20, 0],
    });
    expect(edited.toolDraft).toEqual({ start: [0, 0], end: [20, 0] });
  });

  it("setSketchToolDraft is a no-op when no tool is selected", () => {
    const session = enterSketchSession(INACTIVE_SKETCH_SESSION, "qA");
    const same = setSketchToolDraft(session, { at: [1, 2] });
    expect(same).toBe(session);
  });

  it("applySketchToolDraft appends an entry to the sub-spine", () => {
    const picked = selectSketchTool(
      enterSketchSession(INACTIVE_SKETCH_SESSION, "qA"),
      "sketch_point",
      { at: [5, 5] },
    );
    const applied = applySketchToolDraft(picked);
    expect(applied.subCalls).toHaveLength(1);
    expect(applied.subCalls[0]).toEqual({
      type: "sketch_point",
      params: { at: [5, 5] },
    });
  });

  it("applySketchToolDraft accumulates multiple entries in order", () => {
    let session = selectSketchTool(
      enterSketchSession(INACTIVE_SKETCH_SESSION, "qA"),
      "sketch_point",
      { at: [0, 0] },
    );
    session = applySketchToolDraft(session);
    session = setSketchToolDraft(session, { at: [10, 0] });
    session = applySketchToolDraft(session);
    expect(session.subCalls).toEqual([
      { type: "sketch_point", params: { at: [0, 0] } },
      { type: "sketch_point", params: { at: [10, 0] } },
    ]);
  });

  it("applySketchToolDraft is a no-op without a selected tool", () => {
    const session = enterSketchSession(INACTIVE_SKETCH_SESSION, "qA");
    const same = applySketchToolDraft(session);
    expect(same).toBe(session);
  });

  it("buildDraftSketchState emits an origin + one point per sketch_point call", () => {
    const state = buildDraftSketchState([
      { type: "sketch_point", params: { at: [3, 4] } },
      { type: "sketch_point", params: { at: [-1, 7] } },
    ]);
    // origin + two points
    expect(state.entities).toHaveLength(3);
    const points = state.entities.filter((e) => e.kind === "point");
    expect(points).toHaveLength(2);
  });

  it("buildDraftSketchState emits two endpoints + a line per sketch_line call", () => {
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
    ]);
    // origin + two endpoint points + one line entity
    expect(state.entities).toHaveLength(4);
    const lines = state.entities.filter((e) => e.kind === "line");
    expect(lines).toHaveLength(1);
  });

  it("buildDraftSketchState injects the drag-start synthetic point when requested", () => {
    const state = buildDraftSketchState([], [5, 6]);
    const drag = state.entities.find((e) => e.id === DRAFT_DRAG_START_ID);
    expect(drag).toBeDefined();
    expect(drag && drag.kind === "point" && drag.at).toEqual({ x: 5, y: 6 });
  });

  it("exitSketchSession discards any accumulated sub-spine draft", () => {
    let session = selectSketchTool(
      enterSketchSession(INACTIVE_SKETCH_SESSION, "qA"),
      "sketch_point",
      { at: [0, 0] },
    );
    session = applySketchToolDraft(session);
    expect(session.subCalls).toHaveLength(1);
    const exited = exitSketchSession(session);
    expect(exited.subCalls).toEqual([]);
    expect(exited.active).toBe(false);
  });
});
