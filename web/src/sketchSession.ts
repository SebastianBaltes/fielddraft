/**
 * K6b.2: pure state reducers for the sketch session slice of the
 * store. Extracting them here keeps the `KernelStore` methods
 * small and gives the Vitest suite a way to exercise the session
 * transitions without booting a kernel / worker / autosave chain.
 *
 * The types live in `store.ts` to avoid an import cycle — this
 * module re-exports them for test convenience.
 */

import type { SketchSessionState } from "./store";
import {
  sketchWithOrigin,
  type EntityId,
  type SketchState,
} from "./solver/types";

export const INACTIVE_SKETCH_SESSION: SketchSessionState = {
  active: false,
  planeQuery: null,
  selectedTool: null,
  toolDraft: {},
  subCalls: [],
  editingFeatureId: null,
};

/** Options for the `enterSketchSession` reducer. K6b.3b adds the
 *  edit-reentry path which preloads the session with an
 *  existing sketch's committed sub_calls and remembers the
 *  feature id so Finish can replace it in place. */
export interface EnterSketchOpts {
  initialSubCalls?: SketchSessionState["subCalls"];
  editingFeatureId?: number | null;
}

/** Enter a new sketch session. If a session is already active
 *  the previous state is returned unchanged — sessions don't
 *  stack. */
export function enterSketchSession(
  state: SketchSessionState,
  planeQuery: string,
  opts: EnterSketchOpts = {},
): SketchSessionState {
  if (state.active) return state;
  return {
    active: true,
    planeQuery,
    selectedTool: null,
    toolDraft: {},
    subCalls: opts.initialSubCalls ?? [],
    editingFeatureId: opts.editingFeatureId ?? null,
  };
}

/** Drop the session and return to the inactive shape. */
export function exitSketchSession(
  state: SketchSessionState,
): SketchSessionState {
  if (!state.active) return state;
  return INACTIVE_SKETCH_SESSION;
}

/** Replace the active tool selection. `null` clears it. Resets
 *  the tool draft to the supplied `initialDraft` (typically
 *  `buildDefaultDefinition(schema)`). */
export function selectSketchTool(
  state: SketchSessionState,
  typeName: string | null,
  initialDraft: Record<string, unknown> = {},
): SketchSessionState {
  if (!state.active) return state;
  if (state.selectedTool === typeName) return state;
  return {
    ...state,
    selectedTool: typeName,
    toolDraft: typeName === null ? {} : initialDraft,
  };
}

/** Update the FeatureDialog working copy for the active tool. */
export function setSketchToolDraft(
  state: SketchSessionState,
  draft: Record<string, unknown>,
): SketchSessionState {
  if (!state.active || state.selectedTool === null) return state;
  return { ...state, toolDraft: draft };
}

/** Append the current draft to the session's sub-spine list. */
export function applySketchToolDraft(
  state: SketchSessionState,
): SketchSessionState {
  if (!state.active || state.selectedTool === null) return state;
  const entry = { type: state.selectedTool, params: state.toolDraft };
  return { ...state, subCalls: [...state.subCalls, entry] };
}

/**
 * For each `sketch_constraint_*` sub-call type, the param keys that
 * hold sketch-local entity ids. Used by both the draft-state
 * rebuild (to translate sub-calls into solver constraints) and the
 * delete path (to rewrite / drop dangling references). Keeping a
 * single table here means a new constraint type is wired in exactly
 * one place. Param names mirror the Rust kernel tool defs in
 * `crates/kernel/src/sketch_features.rs` one-to-one.
 */
export const CONSTRAINT_ENTITY_PARAMS: Record<string, readonly string[]> = {
  sketch_constraint_horizontal: ["line"],
  sketch_constraint_vertical: ["line"],
  sketch_constraint_coincident: ["a", "b"],
  sketch_constraint_distance: ["entity_a", "entity_b"],
  sketch_constraint_angle: ["line_a", "line_b"],
  sketch_constraint_equal: ["entity_a", "entity_b"],
  sketch_constraint_parallel: ["line_a", "line_b"],
  sketch_constraint_perpendicular: ["line_a", "line_b"],
  sketch_constraint_tangent: ["line", "curve"],
  sketch_constraint_midpoint: ["point", "line"],
  sketch_constraint_symmetric: ["entity_a", "entity_b", "axis"],
};

// ─── K6b.3c: draft sketch-state rebuild for live inference ──────

/** ID the [`buildDraftSketchState`] helper assigns to an in-flight
 *  drag-line start point when the caller requests it. Reserved in
 *  a range that can't collide with real kernel-side entity ids. */
export const DRAFT_DRAG_START_ID: EntityId = 0x7fff_ffff;

/**
 * Walk a session's accumulated `subCalls` and rebuild a
 * [`SketchState`] mirroring what the kernel's `sketch` feature
 * would produce at regeneration time. Used by the 2D canvas
 * overlay to feed the K5b `InferenceEngine` with the existing
 * entity set while the user is drawing — the engine needs a
 * `SketchState`, not a flat list of FeatureCall JSON blobs.
 *
 * The resulting state is **not** pushed back into the kernel; it
 * lives entirely on the UI side and gets rebuilt on every
 * pointer move. Cheap: a few object allocations per call.
 *
 * When `drawingLineFrom` is supplied, a synthetic point entity
 * is injected at that sketch coordinate with the reserved
 * [`DRAFT_DRAG_START_ID`] so the inference engine can reference
 * it as the `drawingLineFrom` parameter of its query.
 */
export function buildDraftSketchState(
  subCalls: SketchSessionState["subCalls"],
  drawingLineFrom?: [number, number],
): SketchState {
  const state = sketchWithOrigin();
  for (const call of subCalls) {
    if (call.type === "sketch_point") {
      const at = call.params.at;
      if (Array.isArray(at) && at.length === 2) {
        state.entities.push({
          kind: "point",
          id: state.next_entity_id++,
          at: { x: Number(at[0]), y: Number(at[1]) },
        });
      }
    } else if (call.type === "sketch_line") {
      const start = call.params.start;
      const end = call.params.end;
      if (
        Array.isArray(start) &&
        start.length === 2 &&
        Array.isArray(end) &&
        end.length === 2
      ) {
        const startId = state.next_entity_id++;
        state.entities.push({
          kind: "point",
          id: startId,
          at: { x: Number(start[0]), y: Number(start[1]) },
        });
        const endId = state.next_entity_id++;
        state.entities.push({
          kind: "point",
          id: endId,
          at: { x: Number(end[0]), y: Number(end[1]) },
        });
        state.entities.push({
          kind: "line",
          id: state.next_entity_id++,
          start: startId,
          end: endId,
        });
      }
    } else if (call.type === "sketch_circle") {
      const center = call.params.center;
      const radius = Number(call.params.radius);
      if (
        Array.isArray(center) &&
        center.length === 2 &&
        Number.isFinite(radius)
      ) {
        const centerId = state.next_entity_id++;
        state.entities.push({
          kind: "point",
          id: centerId,
          at: { x: Number(center[0]), y: Number(center[1]) },
        });
        state.entities.push({
          kind: "circle",
          id: state.next_entity_id++,
          center: centerId,
          radius,
        });
      }
    } else if (call.type === "sketch_rectangle") {
      // Mirror the kernel `SketchRectangleTool` allocation
      // order: bl, br, tr, tl, bottom, right, top, left,
      // horizontal × 2, vertical × 2. Any drift between the
      // two sides shifts entity ids and silently corrupts
      // later constraint references.
      const a = call.params.corner_a;
      const b = call.params.corner_b;
      if (
        Array.isArray(a) &&
        a.length === 2 &&
        Array.isArray(b) &&
        b.length === 2
      ) {
        const ax = Number(a[0]);
        const ay = Number(a[1]);
        const bx = Number(b[0]);
        const by = Number(b[1]);
        const minX = Math.min(ax, bx);
        const minY = Math.min(ay, by);
        const maxX = Math.max(ax, bx);
        const maxY = Math.max(ay, by);
        const blId = state.next_entity_id++;
        state.entities.push({ kind: "point", id: blId, at: { x: minX, y: minY } });
        const brId = state.next_entity_id++;
        state.entities.push({ kind: "point", id: brId, at: { x: maxX, y: minY } });
        const trId = state.next_entity_id++;
        state.entities.push({ kind: "point", id: trId, at: { x: maxX, y: maxY } });
        const tlId = state.next_entity_id++;
        state.entities.push({ kind: "point", id: tlId, at: { x: minX, y: maxY } });
        const bottomId = state.next_entity_id++;
        state.entities.push({ kind: "line", id: bottomId, start: blId, end: brId });
        const rightId = state.next_entity_id++;
        state.entities.push({ kind: "line", id: rightId, start: brId, end: trId });
        const topId = state.next_entity_id++;
        state.entities.push({ kind: "line", id: topId, start: trId, end: tlId });
        const leftId = state.next_entity_id++;
        state.entities.push({ kind: "line", id: leftId, start: tlId, end: blId });
        state.constraints.push({
          kind: "horizontal",
          id: state.next_constraint_id++,
          line: bottomId,
        });
        state.constraints.push({
          kind: "horizontal",
          id: state.next_constraint_id++,
          line: topId,
        });
        state.constraints.push({
          kind: "vertical",
          id: state.next_constraint_id++,
          line: rightId,
        });
        state.constraints.push({
          kind: "vertical",
          id: state.next_constraint_id++,
          line: leftId,
        });
      }
    } else if (call.type === "sketch_polygon") {
      const verts = call.params.vertices;
      if (Array.isArray(verts)) {
        for (let i = 0; i + 1 < verts.length; i += 2) {
          state.entities.push({
            kind: "point",
            id: state.next_entity_id++,
            at: { x: Number(verts[i]), y: Number(verts[i + 1]) },
          });
        }
      }
    } else if (call.type === "sketch_arc") {
      const center = call.params.center;
      if (Array.isArray(center) && center.length === 2) {
        state.entities.push({
          kind: "point",
          id: state.next_entity_id++,
          at: { x: Number(center[0]), y: Number(center[1]) },
        });
      }
    } else if (call.type === "sketch_constraint_horizontal") {
      const line = Number(call.params.line);
      if (Number.isFinite(line)) {
        state.constraints.push({
          kind: "horizontal",
          id: state.next_constraint_id++,
          line,
        });
      }
    } else if (call.type === "sketch_constraint_vertical") {
      const line = Number(call.params.line);
      if (Number.isFinite(line)) {
        state.constraints.push({
          kind: "vertical",
          id: state.next_constraint_id++,
          line,
        });
      }
    } else if (call.type === "sketch_constraint_coincident") {
      const a = Number(call.params.a);
      const b = Number(call.params.b);
      if (Number.isFinite(a) && Number.isFinite(b)) {
        state.constraints.push({
          kind: "coincident",
          id: state.next_constraint_id++,
          a,
          b,
        });
      }
    } else if (call.type === "sketch_constraint_parallel") {
      const a = Number(call.params.line_a);
      const b = Number(call.params.line_b);
      if (Number.isFinite(a) && Number.isFinite(b)) {
        state.constraints.push({
          kind: "parallel",
          id: state.next_constraint_id++,
          a,
          b,
        });
      }
    } else if (call.type === "sketch_constraint_perpendicular") {
      const a = Number(call.params.line_a);
      const b = Number(call.params.line_b);
      if (Number.isFinite(a) && Number.isFinite(b)) {
        state.constraints.push({
          kind: "perpendicular",
          id: state.next_constraint_id++,
          a,
          b,
        });
      }
    } else if (call.type === "sketch_constraint_equal") {
      const a = Number(call.params.entity_a);
      const b = Number(call.params.entity_b);
      if (Number.isFinite(a) && Number.isFinite(b)) {
        state.constraints.push({
          kind: "equal",
          id: state.next_constraint_id++,
          a,
          b,
        });
      }
    } else if (call.type === "sketch_constraint_tangent") {
      const line = Number(call.params.line);
      const curve = Number(call.params.curve);
      if (Number.isFinite(line) && Number.isFinite(curve)) {
        state.constraints.push({
          kind: "tangent",
          id: state.next_constraint_id++,
          line,
          curve,
        });
      }
    } else if (call.type === "sketch_constraint_midpoint") {
      const point = Number(call.params.point);
      const line = Number(call.params.line);
      if (Number.isFinite(point) && Number.isFinite(line)) {
        state.constraints.push({
          kind: "midpoint",
          id: state.next_constraint_id++,
          point,
          line,
        });
      }
    } else if (call.type === "sketch_constraint_distance") {
      const a = Number(call.params.entity_a);
      const b = Number(call.params.entity_b);
      const value_mm = Number(call.params.value_mm);
      if (Number.isFinite(a) && Number.isFinite(b) && Number.isFinite(value_mm)) {
        state.constraints.push({
          kind: "distance",
          id: state.next_constraint_id++,
          a,
          b,
          value_mm,
          driven: Boolean(call.params.driven),
        });
      }
    } else if (call.type === "sketch_constraint_angle") {
      const a = Number(call.params.line_a);
      const b = Number(call.params.line_b);
      const value_deg = Number(call.params.value_deg);
      if (Number.isFinite(a) && Number.isFinite(b) && Number.isFinite(value_deg)) {
        state.constraints.push({
          kind: "angle",
          id: state.next_constraint_id++,
          a,
          b,
          value_deg,
          driven: Boolean(call.params.driven),
        });
      }
    } else if (call.type === "sketch_constraint_symmetric") {
      const a = Number(call.params.entity_a);
      const b = Number(call.params.entity_b);
      const axis = Number(call.params.axis);
      if (Number.isFinite(a) && Number.isFinite(b) && Number.isFinite(axis)) {
        state.constraints.push({
          kind: "symmetric",
          id: state.next_constraint_id++,
          a,
          b,
          axis,
        });
      }
    }
  }
  if (drawingLineFrom) {
    state.entities.push({
      kind: "point",
      id: DRAFT_DRAG_START_ID,
      at: { x: drawingLineFrom[0], y: drawingLineFrom[1] },
    });
  }
  return state;
}

/**
 * K6b.3g: remove one sub-call at `index` and rewrite downstream
 * constraint calls so their entity-id references stay valid.
 *
 * Removing a geometric call (sketch_point, sketch_line,
 * sketch_rectangle, sketch_circle) frees a range of entity ids
 * that every subsequent sketch_tool would have been allocated.
 * Any constraint call that references an id inside the removed
 * range is dropped; any constraint call that references an id
 * *above* the removed range has its reference shifted down by
 * the range size so it still points at the same entity after
 * re-allocation.
 *
 * This keeps the sub-spine internally consistent without
 * forcing the user to manually clean up dangling constraints.
 */
export function removeSketchSubCallAt(
  state: SketchSessionState,
  index: number,
): SketchSessionState {
  if (!state.active) return state;
  if (index < 0 || index >= state.subCalls.length) return state;
  const before = state.subCalls.slice(0, index);
  const removed = state.subCalls[index];
  const tail = state.subCalls.slice(index + 1);

  // Delta = entities allocated by the removed call.
  const preState = buildDraftSketchState(before);
  const preId = preState.next_entity_id;
  const postState = buildDraftSketchState([...before, removed]);
  const postId = postState.next_entity_id;
  const rangeLo = preId;
  const rangeHi = postId - 1;
  const delta = postId - preId;

  const shift = (n: number): number => (n > rangeHi ? n - delta : n);
  const inRange = (n: number): boolean => n >= rangeLo && n <= rangeHi;

  const rewritten: SketchSessionState["subCalls"][number][] = [];
  for (const call of tail) {
    const entityParams = CONSTRAINT_ENTITY_PARAMS[call.type];
    if (!entityParams) {
      // Geometric calls allocate fresh ids — no shift needed.
      rewritten.push(call);
      continue;
    }
    // Read every entity-id reference this constraint carries. Drop
    // the whole constraint if any reference points into the removed
    // range (the referenced entity is gone); otherwise shift each
    // reference above the range down so it still targets the same
    // post-remove entity.
    const refs = entityParams.map((key) => Number(call.params[key]));
    if (refs.some((n) => !Number.isFinite(n) || inRange(n))) continue;
    const nextParams = { ...call.params };
    entityParams.forEach((key, i) => {
      nextParams[key] = shift(refs[i]);
    });
    rewritten.push({ ...call, params: nextParams });
  }

  return { ...state, subCalls: [...before, ...rewritten] };
}

/**
 * K6b.3d helper: predict the next sketch_line entity IDs the
 * kernel will allocate when the given sub_calls are applied.
 * Used by the UI commit path to emit constraint calls that
 * reference the line we're about to push.
 *
 * Returns the IDs a fresh `sketch_line` call *would* occupy at
 * the current end of the sub-spine. The caller is responsible
 * for actually appending the line + any constraints that
 * reference these IDs in the same commit.
 */
export function predictNextLineIds(
  subCalls: SketchSessionState["subCalls"],
): { start: number; end: number; line: number } {
  const state = buildDraftSketchState(subCalls);
  const startId = state.next_entity_id;
  const endId = startId + 1;
  const lineId = endId + 1;
  return { start: startId, end: endId, line: lineId };
}
