import { describe, expect, it } from "vitest";
import {
  DRAFT_DRAG_START_ID,
  INACTIVE_SKETCH_SESSION,
  applySketchToolDraft,
  buildDraftSketchState,
  enterSketchSession,
  exitSketchSession,
  removeSketchSubCallAt,
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

  it("removeSketchSubCallAt drops a geometric call", () => {
    let session = enterSketchSession(INACTIVE_SKETCH_SESSION, "qA");
    session = selectSketchTool(session, "sketch_point", { at: [1, 1] });
    session = applySketchToolDraft(session);
    session = setSketchToolDraft(session, { at: [2, 2] });
    session = applySketchToolDraft(session);
    expect(session.subCalls).toHaveLength(2);
    const after = removeSketchSubCallAt(session, 0);
    expect(after.subCalls).toHaveLength(1);
    expect((after.subCalls[0].params as { at: [number, number] }).at).toEqual([2, 2]);
  });

  it("removeSketchSubCallAt drops dangling constraint refs and shifts survivors", () => {
    // Build a session that mimics a line-with-horizontal-constraint
    // followed by a free sketch_point referenced by nothing.
    const session: Parameters<typeof removeSketchSubCallAt>[0] = {
      active: true,
      planeQuery: "qA",
      selectedTool: null,
      toolDraft: {},
      editingFeatureId: null,
      subCalls: [
        { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
        { type: "sketch_constraint_horizontal", params: { line: 3 } },
        { type: "sketch_point", params: { at: [5, 5] } },
      ],
    };
    const after = removeSketchSubCallAt(session, 0);
    // The line call at index 0 allocated entities 1-3. The
    // horizontal constraint referenced line=3 which is now
    // invalid → drop. The trailing point had no refs → stays.
    expect(after.subCalls).toHaveLength(1);
    expect(after.subCalls[0].type).toBe("sketch_point");
  });

  it("removeSketchSubCallAt rewrites constraint refs above the removed range", () => {
    const session: Parameters<typeof removeSketchSubCallAt>[0] = {
      active: true,
      planeQuery: "qA",
      selectedTool: null,
      toolDraft: {},
      editingFeatureId: null,
      subCalls: [
        // Call 0: sketch_point — allocates entity 1.
        { type: "sketch_point", params: { at: [0, 0] } },
        // Call 1: sketch_line — allocates entities 2, 3, 4 (line=4).
        { type: "sketch_line", params: { start: [1, 1], end: [9, 1] } },
        // Call 2: horizontal constraint on line 4.
        { type: "sketch_constraint_horizontal", params: { line: 4 } },
      ],
    };
    // Remove the leading sketch_point — the line shifts down by
    // one entity (2→1, 3→2, 4→3), so the constraint ref becomes 3.
    const after = removeSketchSubCallAt(session, 0);
    expect(after.subCalls).toHaveLength(2);
    const cstr = after.subCalls[1];
    expect(cstr.type).toBe("sketch_constraint_horizontal");
    expect((cstr.params as { line: number }).line).toBe(3);
  });

  it("buildDraftSketchState translates every constraint sub-call into the solver state", () => {
    // Two lines (entities 1-3 and 4-6) plus one constraint of each
    // multi-entity kind referencing them, so the solver actually
    // sees them rather than silently dropping the relation.
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
      { type: "sketch_line", params: { start: [0, 5], end: [10, 5] } },
      { type: "sketch_constraint_parallel", params: { line_a: 3, line_b: 6 } },
      { type: "sketch_constraint_perpendicular", params: { line_a: 3, line_b: 6 } },
      { type: "sketch_constraint_equal", params: { entity_a: 3, entity_b: 6 } },
      { type: "sketch_constraint_angle", params: { line_a: 3, line_b: 6, value_deg: 45 } },
      { type: "sketch_constraint_distance", params: { entity_a: 1, entity_b: 4, value_mm: 7 } },
    ]);
    const kinds = state.constraints.map((c) => c.kind).sort();
    expect(kinds).toEqual(
      ["angle", "distance", "equal", "parallel", "perpendicular"].sort(),
    );
    const angle = state.constraints.find((c) => c.kind === "angle");
    expect(angle && angle.kind === "angle" && angle.value_deg).toBe(45);
    const dist = state.constraints.find((c) => c.kind === "distance");
    expect(dist && dist.kind === "distance" && dist.value_mm).toBe(7);
  });

  it("buildDraftSketchState carries the driven flag on dimensional constraints", () => {
    const state = buildDraftSketchState([
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
      { type: "sketch_line", params: { start: [0, 5], end: [10, 5] } },
      {
        type: "sketch_constraint_angle",
        params: { line_a: 3, line_b: 6, value_deg: 30, driven: true },
      },
    ]);
    const angle = state.constraints.find((c) => c.kind === "angle");
    expect(angle && angle.kind === "angle" && angle.driven).toBe(true);
  });

  it("removeSketchSubCallAt drops a parallel constraint whose line is removed", () => {
    const session: Parameters<typeof removeSketchSubCallAt>[0] = {
      active: true,
      planeQuery: "qA",
      selectedTool: null,
      toolDraft: {},
      editingFeatureId: null,
      subCalls: [
        { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } }, // 1-3
        { type: "sketch_line", params: { start: [0, 5], end: [10, 5] } }, // 4-6
        { type: "sketch_constraint_parallel", params: { line_a: 3, line_b: 6 } },
      ],
    };
    // Remove the first line (entities 1-3). The parallel ref line_a=3
    // is now invalid → the whole constraint is dropped.
    const after = removeSketchSubCallAt(session, 0);
    expect(after.subCalls).toHaveLength(1);
    expect(after.subCalls[0].type).toBe("sketch_line");
  });

  it("removeSketchSubCallAt shifts a parallel constraint's refs past the removed range", () => {
    const session: Parameters<typeof removeSketchSubCallAt>[0] = {
      active: true,
      planeQuery: "qA",
      selectedTool: null,
      toolDraft: {},
      editingFeatureId: null,
      subCalls: [
        { type: "sketch_point", params: { at: [0, 0] } }, // entity 1
        { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } }, // 2-4 (line=4)
        { type: "sketch_line", params: { start: [0, 5], end: [10, 5] } }, // 5-7 (line=7)
        { type: "sketch_constraint_parallel", params: { line_a: 4, line_b: 7 } },
      ],
    };
    // Remove the leading point (entity 1). Both lines shift down by
    // one, so the parallel refs become 3 and 6.
    const after = removeSketchSubCallAt(session, 0);
    expect(after.subCalls).toHaveLength(3);
    const cstr = after.subCalls[2];
    expect(cstr.type).toBe("sketch_constraint_parallel");
    expect(cstr.params).toMatchObject({ line_a: 3, line_b: 6 });
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
