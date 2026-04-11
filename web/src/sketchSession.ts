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
};

/** Enter a new sketch session. If a session is already active
 *  the previous state is returned unchanged — sessions don't
 *  stack. */
export function enterSketchSession(
  state: SketchSessionState,
  planeQuery: string,
): SketchSessionState {
  if (state.active) return state;
  return {
    active: true,
    planeQuery,
    selectedTool: null,
    toolDraft: {},
    subCalls: [],
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
