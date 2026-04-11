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
