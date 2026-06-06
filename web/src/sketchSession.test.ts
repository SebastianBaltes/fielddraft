import { describe, expect, it } from "vitest";
import {
  DRAFT_DRAG_START_ID,
  INACTIVE_SKETCH_SESSION,
  applySketchToolDraft,
  buildDraftSketchState,
  clearSketchSelection,
  enterSketchSession,
  exitSketchSession,
  removeSketchSubCallAt,
  regularPolygonVertices,
  selectSketchTool,
  setSketchSelection,
  setSketchToolDraft,
  toggleSketchSelection,
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

  it("buildDraftSketchState still emits a solver entity for construction geometry", () => {
    // Construction geometry is reference-only for the region, but it must
    // remain a real solver entity so constraints can reference it.
    const state = buildDraftSketchState([
      {
        type: "sketch_circle",
        params: { center: [0, 0], radius: 5, construction: true },
      },
    ]);
    const circles = state.entities.filter((e) => e.kind === "circle");
    expect(circles).toHaveLength(1);
    expect(circles[0].kind === "circle" && circles[0].radius).toBe(5);
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
      selection: [],
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
      selection: [],
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

  it("buildDraftSketchState translates a concentric sub-call with a fresh constraint id", () => {
    // Two circles: circle 1 = center 1 / curve 2, circle 2 = center 3 /
    // curve 4. The concentric sub-call references the two curve ids and
    // must get a fresh constraint id allocated after the geometry.
    const state = buildDraftSketchState([
      { type: "sketch_circle", params: { center: [0, 0], radius: 3 } },
      { type: "sketch_circle", params: { center: [9, 0], radius: 4 } },
      { type: "sketch_constraint_concentric", params: { a: 2, b: 4 } },
    ]);
    const concentric = state.constraints.find((c) => c.kind === "concentric");
    expect(concentric).toBeDefined();
    if (concentric && concentric.kind === "concentric") {
      expect(concentric.a).toBe(2);
      expect(concentric.b).toBe(4);
      // First constraint allocated → id 1 (the counter starts at 1), and
      // the counter advanced past it.
      expect(concentric.id).toBe(1);
    }
    expect(state.next_constraint_id).toBe(2);
  });

  it("buildDraftSketchState translates a fix sub-call with a fresh constraint id", () => {
    // A single point (id 1) pinned in place. The fix sub-call must get a
    // fresh constraint id allocated after the geometry.
    const state = buildDraftSketchState([
      { type: "sketch_point", params: { at: [3, 4] } },
      { type: "sketch_constraint_fix", params: { point: 1 } },
    ]);
    const fix = state.constraints.find((c) => c.kind === "fix");
    expect(fix).toBeDefined();
    if (fix && fix.kind === "fix") {
      expect(fix.point).toBe(1);
      // First constraint allocated → id 1 (the counter starts at 1).
      expect(fix.id).toBe(1);
    }
    expect(state.next_constraint_id).toBe(2);
  });

  it("buildDraftSketchState translates a point_on_object sub-call with a fresh constraint id", () => {
    // A circle (centre 1, circle 2) and a free point (id 3) constrained
    // onto the circle. The constraint must carry the right point/curve
    // and a fresh id allocated after the geometry.
    const state = buildDraftSketchState([
      { type: "sketch_circle", params: { center: [0, 0], radius: 5 } },
      { type: "sketch_point", params: { at: [3, 4] } },
      {
        type: "sketch_constraint_point_on_object",
        params: { point: 3, curve: 2 },
      },
    ]);
    const poc = state.constraints.find((c) => c.kind === "point_on_object");
    expect(poc).toBeDefined();
    if (poc && poc.kind === "point_on_object") {
      expect(poc.point).toBe(3);
      expect(poc.curve).toBe(2);
      // First constraint allocated → id 1 (the counter starts at 1).
      expect(poc.id).toBe(1);
    }
    expect(state.next_constraint_id).toBe(2);
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
      selection: [],
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
      selection: [],
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

  it("toggleSketchSelection adds then removes an entity id", () => {
    const session = enterSketchSession(INACTIVE_SKETCH_SESSION, "qA");
    expect(session.selection).toEqual([]);
    const one = toggleSketchSelection(session, 3);
    expect(one.selection).toEqual([3]);
    const two = toggleSketchSelection(one, 6);
    expect(two.selection).toEqual([3, 6]);
    const back = toggleSketchSelection(two, 3);
    expect(back.selection).toEqual([6]);
  });

  it("setSketchSelection replaces the whole set, clearSketchSelection empties it", () => {
    const session = enterSketchSession(INACTIVE_SKETCH_SESSION, "qA");
    const set = setSketchSelection(session, [1, 2, 4]);
    expect(set.selection).toEqual([1, 2, 4]);
    const cleared = clearSketchSelection(set);
    expect(cleared.selection).toEqual([]);
  });

  it("clearSketchSelection is identity when already empty", () => {
    const session = enterSketchSession(INACTIVE_SKETCH_SESSION, "qA");
    expect(clearSketchSelection(session)).toBe(session);
  });

  it("selection reducers are no-ops on an inactive session", () => {
    expect(toggleSketchSelection(INACTIVE_SKETCH_SESSION, 1)).toBe(
      INACTIVE_SKETCH_SESSION,
    );
    expect(setSketchSelection(INACTIVE_SKETCH_SESSION, [1])).toBe(
      INACTIVE_SKETCH_SESSION,
    );
  });

  it("selecting a drawing tool clears a stale selection", () => {
    let session = enterSketchSession(INACTIVE_SKETCH_SESSION, "qA");
    session = setSketchSelection(session, [3, 6]);
    const drawing = selectSketchTool(session, "sketch_line", {});
    expect(drawing.selection).toEqual([]);
  });

  it("removeSketchSubCallAt clears the selection (ids shift)", () => {
    let session = enterSketchSession(INACTIVE_SKETCH_SESSION, "qA");
    session = selectSketchTool(session, "sketch_point", { at: [1, 1] });
    session = applySketchToolDraft(session);
    session = selectSketchTool(session, null);
    session = setSketchSelection(session, [1]);
    const after = removeSketchSubCallAt(session, 0);
    expect(after.selection).toEqual([]);
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

describe("regularPolygonVertices + sketch_regular_polygon draft", () => {
  const close = (a: number, b: number) => Math.abs(a - b) < 1e-4;

  it("square (sides=4, inscribed, r=10) yields the expected 4 vertices", () => {
    const v = regularPolygonVertices([0, 0], 10, 4, true);
    expect(v).toHaveLength(4);
    // angle = π/2 + k·π/2 → top, left, bottom, right (CCW).
    const expected = [
      [0, 10],
      [-10, 0],
      [0, -10],
      [10, 0],
    ];
    v.forEach((p, i) => {
      expect(close(p[0], expected[i][0])).toBe(true);
      expect(close(p[1], expected[i][1])).toBe(true);
    });
  });

  it("buildDraftSketchState emits exactly 4 points for a square n-gon", () => {
    const state = buildDraftSketchState([
      {
        type: "sketch_regular_polygon",
        params: { center: [0, 0], radius: 10, sides: 4, inscribed: true },
      },
    ]);
    // The sketch origin is a distinct `kind: "origin"` entity, so the
    // only `"point"` entities are the 4 polygon vertices.
    const points = state.entities.filter((e) => e.kind === "point");
    expect(points).toHaveLength(4);
    const ngon = points.map((p) =>
      p.kind === "point" ? [p.at.x, p.at.y] : [NaN, NaN],
    );
    const expected = [
      [0, 10],
      [-10, 0],
      [0, -10],
      [10, 0],
    ];
    ngon.forEach((p, i) => {
      expect(close(p[0], expected[i][0])).toBe(true);
      expect(close(p[1], expected[i][1])).toBe(true);
    });
  });

  it("triangle (sides=3) yields 3 vertices, first at the top", () => {
    const v = regularPolygonVertices([0, 0], 5, 3, true);
    expect(v).toHaveLength(3);
    expect(close(v[0][0], 0)).toBe(true);
    expect(close(v[0][1], 5)).toBe(true);
  });

  it("hexagon (sides=6) yields 6 vertices", () => {
    expect(regularPolygonVertices([0, 0], 7, 6, true)).toHaveLength(6);
  });

  it("clamps sides < 3 up to 3", () => {
    expect(regularPolygonVertices([0, 0], 5, 2, true)).toHaveLength(3);
    expect(regularPolygonVertices([0, 0], 5, 0, true)).toHaveLength(3);
  });

  it("rounds non-integer sides (5.6 → 6, 5.4 → 5)", () => {
    expect(regularPolygonVertices([0, 0], 5, 5.6, true)).toHaveLength(6);
    expect(regularPolygonVertices([0, 0], 5, 5.4, true)).toHaveLength(5);
  });

  it("circumscribed produces a larger circumradius than inscribed", () => {
    const inscribed = regularPolygonVertices([0, 0], 10, 6, true);
    const circ = regularPolygonVertices([0, 0], 10, 6, false);
    const rIn = Math.hypot(inscribed[0][0], inscribed[0][1]);
    const rCirc = Math.hypot(circ[0][0], circ[0][1]);
    expect(rCirc).toBeGreaterThan(rIn + 1e-3);
  });

  it("radius 0 → no vertices (degenerate reject)", () => {
    expect(regularPolygonVertices([0, 0], 0, 6, true)).toHaveLength(0);
    const state = buildDraftSketchState([
      {
        type: "sketch_regular_polygon",
        params: { center: [0, 0], radius: 0, sides: 6, inscribed: true },
      },
    ]);
    // no polygon vertices added (origin is kind "origin", not "point").
    expect(state.entities.filter((e) => e.kind === "point")).toHaveLength(0);
  });
});

describe("sketch_ellipse draft state", () => {
  it("pushes exactly one centre point with the next id", () => {
    // Origin is id 0, so next_entity_id starts at 1. The kernel's
    // SketchEllipseTool::apply registers ONLY the centre point, so the
    // TS mirror must allocate exactly one entity (id 1) and advance the
    // counter by exactly 1 — to 2.
    const state = buildDraftSketchState([
      { type: "sketch_ellipse", params: { center: [1, 2], radius_x: 5, radius_y: 3 } },
    ]);
    const points = state.entities.filter((e) => e.kind === "point");
    expect(points).toHaveLength(1);
    const centre = points[0];
    expect(centre.kind === "point" && centre.at).toEqual({ x: 1, y: 2 });
    expect(centre.id).toBe(1);
    expect(state.next_entity_id).toBe(2);
  });

  it("keeps entity ids aligned for a following sub-call", () => {
    // After the ellipse consumes id 1, a following circle must take the
    // next contiguous ids (centre 2, circle 3) — proving the ellipse
    // branch advanced the counter by exactly the kernel's one point.
    const state = buildDraftSketchState([
      { type: "sketch_ellipse", params: { center: [0, 0], radius_x: 4, radius_y: 2 } },
      { type: "sketch_circle", params: { center: [10, 10], radius: 5 } },
    ]);
    const circleCentre = state.entities.find(
      (e) => e.kind === "point" && e.at.x === 10 && e.at.y === 10,
    );
    const circle = state.entities.find((e) => e.kind === "circle");
    expect(circleCentre?.id).toBe(2);
    expect(circle && circle.kind === "circle" && circle.center).toBe(2);
    expect(circle?.id).toBe(3);
    expect(state.next_entity_id).toBe(4);
  });

  it("ignores a malformed centre (no entity added)", () => {
    const state = buildDraftSketchState([
      { type: "sketch_ellipse", params: { center: [1], radius_x: 5, radius_y: 3 } },
    ]);
    expect(state.entities.filter((e) => e.kind === "point")).toHaveLength(0);
    expect(state.next_entity_id).toBe(1);
  });
});

describe("sketch_slot draft state", () => {
  it("pushes exactly two centre points (A then B) with consecutive ids", () => {
    // The kernel's SketchSlotTool::apply registers the two end-cap
    // centres in order (center_a then center_b). The TS mirror must
    // allocate exactly two entities (ids 1, 2) and advance the counter
    // by exactly 2 — to 3.
    const state = buildDraftSketchState([
      {
        type: "sketch_slot",
        params: { center_a: [1, 2], center_b: [9, 2], radius: 5 },
      },
    ]);
    const points = state.entities.filter((e) => e.kind === "point");
    expect(points).toHaveLength(2);
    expect(points[0].kind === "point" && points[0].at).toEqual({ x: 1, y: 2 });
    expect(points[0].id).toBe(1);
    expect(points[1].kind === "point" && points[1].at).toEqual({ x: 9, y: 2 });
    expect(points[1].id).toBe(2);
    expect(state.next_entity_id).toBe(3);
  });

  it("keeps entity ids aligned for a following sub-call", () => {
    // After the slot consumes ids 1 and 2, a following circle must take
    // the next contiguous ids (centre 3, circle 4) — proving the slot
    // branch advanced the counter by exactly the kernel's two points.
    const state = buildDraftSketchState([
      {
        type: "sketch_slot",
        params: { center_a: [0, 0], center_b: [20, 0], radius: 4 },
      },
      { type: "sketch_circle", params: { center: [50, 50], radius: 5 } },
    ]);
    const circleCentre = state.entities.find(
      (e) => e.kind === "point" && e.at.x === 50 && e.at.y === 50,
    );
    const circle = state.entities.find((e) => e.kind === "circle");
    expect(circleCentre?.id).toBe(3);
    expect(circle && circle.kind === "circle" && circle.center).toBe(3);
    expect(circle?.id).toBe(4);
    expect(state.next_entity_id).toBe(5);
  });

  it("ignores a malformed slot (no entity added)", () => {
    const state = buildDraftSketchState([
      {
        type: "sketch_slot",
        params: { center_a: [1], center_b: [9, 2], radius: 5 },
      },
    ]);
    expect(state.entities.filter((e) => e.kind === "point")).toHaveLength(0);
    expect(state.next_entity_id).toBe(1);
  });
});
