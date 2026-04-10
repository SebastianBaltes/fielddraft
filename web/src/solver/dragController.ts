/**
 * Drag controller (K5b adapter layer).
 *
 * Wraps the §4.4 "drag without topology kip" story as a
 * stateful controller that the UI drives per mouse gesture.
 * The original SolveSpace-era design inserted a temporary
 * point-to-cursor coincidence and marked it with PlaneGCS'
 * `temporary` flag so the solver would continue toward the
 * nearest valid manifold instead of flipping the sketch's
 * topology — and that's what the §4.4 briefing documents.
 *
 * K5b implements the same user-visible behaviour through a
 * simpler mechanism. Instead of adding a helper point and a
 * soft constraint, the controller **overwrites the dragged
 * point's coordinates directly** in the working copy with each
 * update, then asks PlaneGCS to re-solve starting from that
 * new initial guess. PlaneGCS' numerical-continuation solver
 * does the rest: the hard constraints (horizontal, distance,
 * coincident-to-origin, …) still hold exactly, and the
 * dragged point lands on the closest valid-manifold point to
 * the cursor — which is the Onshape-style continuation
 * behaviour the §4.4 briefing asks for, without needing a
 * helper entity or a secondary wrapper API.
 *
 * ## Lifecycle
 *
 * 1. [`DragController.start`] — the user clicks a point. The
 *    controller snapshots the current sketch and stashes the
 *    dragged entity id.
 * 2. [`DragController.update`] — on every mouse move the
 *    controller overwrites the dragged point's coordinates in
 *    the working copy, re-runs [`solveSketch`], and keeps the
 *    solved state as the basis for the next update. The
 *    solved state is returned to the caller (usually the UI)
 *    so the viewport can re-render.
 * 3. [`DragController.finish`] — on mouse release the
 *    controller returns the latest solved state. No helper
 *    cleanup is needed because no helpers were inserted.
 * 4. [`DragController.cancel`] — if the user presses Escape
 *    or the drag otherwise aborts, the controller reverts to
 *    the pre-drag snapshot.
 *
 * A fresh controller instance per drag is the simplest usage
 * pattern; tests in `dragController.test.ts` cover the normal
 * drag, multi-step drag, and cancel paths.
 */

import { solveSketch } from "./planegcs";
import type { EntityId, SketchState, SolveResult } from "./types";

interface ActiveDrag {
  /** The user-visible point the UI is dragging. */
  draggedEntityId: EntityId;
  /** Snapshot at the start of the drag, returned by
   *  [`cancel`] and never mutated in place. */
  snapshot: SketchState;
  /** The latest solved state. Overwritten on every [`update`]
   *  so the next solve starts from the newest initial guess. */
  working: SketchState;
}

export class DragController {
  private active: ActiveDrag | null = null;

  /** Is a drag currently in progress? */
  isDragging(): boolean {
    return this.active !== null;
  }

  /**
   * Begin a drag. `base` is the sketch at the moment of the
   * click; `draggedEntityId` is the point the user grabbed.
   * `initialCursor` is the mouse position in sketch
   * millimetres — the controller immediately overwrites the
   * dragged point's coordinates with this value so the
   * returned state matches what the UI sees on the first
   * frame, even before the first [`update`] call.
   *
   * Returns the working copy of the sketch with the dragged
   * point already moved to the initial cursor. The UI can
   * render it directly.
   */
  start(
    base: SketchState,
    draggedEntityId: EntityId,
    initialCursor: { x: number; y: number },
  ): SketchState {
    if (this.active) {
      throw new Error(
        "DragController: start() called while a drag is already in progress",
      );
    }

    const snapshot = deepCloneSketch(base);
    const working = deepCloneSketch(base);
    movePoint(working, draggedEntityId, initialCursor);

    this.active = {
      draggedEntityId,
      snapshot,
      working,
    };
    return working;
  }

  /**
   * Update the drag with a new cursor position. Overwrites the
   * dragged point in the working copy, asks PlaneGCS to solve
   * from that initial guess, and keeps the solved state as
   * the next-update starting point so the drag looks like a
   * smooth continuation path through configuration space.
   */
  async update(
    cursor: { x: number; y: number },
  ): Promise<SolveResult> {
    const state = this.requireActive("update");
    movePoint(state.working, state.draggedEntityId, cursor);
    const result = await solveSketch(state.working);
    state.working = result.state;
    return result;
  }

  /**
   * Finish the drag. Returns the latest solved state as a
   * new sketch the caller should store back. No helpers to
   * clean up — the controller's working copy is already in
   * final form.
   */
  finish(): SketchState {
    const state = this.requireActive("finish");
    const final = state.working;
    this.active = null;
    return final;
  }

  /**
   * Abandon the drag. Returns the pre-drag snapshot verbatim —
   * every mutation the drag introduced is discarded. Use this
   * from Escape-key handlers.
   */
  cancel(): SketchState {
    const state = this.requireActive("cancel");
    const snapshot = state.snapshot;
    this.active = null;
    return snapshot;
  }

  private requireActive(operation: string): ActiveDrag {
    if (!this.active) {
      throw new Error(
        `DragController: ${operation}() called with no active drag`,
      );
    }
    return this.active;
  }
}

// ─── helpers ────────────────────────────────────────────────────

function deepCloneSketch(state: SketchState): SketchState {
  // Structured-clone-like deep copy preserving the exact wire
  // shape. Every field here is JSON-serialisable by
  // definition (it's the Rust-side wire form), so the
  // JSON-round-trip is safe and cheap for sketch sizes.
  return JSON.parse(JSON.stringify(state)) as SketchState;
}

function movePoint(
  state: SketchState,
  id: EntityId,
  to: { x: number; y: number },
): void {
  for (const e of state.entities) {
    if (e.id === id && e.kind === "point") {
      e.at = { x: to.x, y: to.y };
      return;
    }
  }
  throw new Error(
    `DragController: cannot move entity id ${id} — not a point or not found`,
  );
}
