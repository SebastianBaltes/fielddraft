/**
 * Live grip-drag solver.
 *
 * Drives PlaneGCS once per pointer frame while the user drags a
 * grip, so constraints (horizontal, distance, coincident, …) hold
 * *visually* during the drag instead of only snapping back at drop.
 * This is the Onshape / SolveSpace continuation behaviour the §4.4
 * briefing asks for.
 *
 * ## Continuation model
 *
 * Each frame the controller takes the session's current sub-calls,
 * moves the dragged grip to the cursor with [`applyGripDrag`], and
 * feeds the result to [`solveSketch`] as the **initial guess**. The
 * solver settles every entity onto the nearest valid manifold: the
 * dragged grip follows the cursor when it's free and sticks to its
 * constraints when it isn't, while the rest of the sketch reflows.
 * The *solved* geometry is written straight back to the session —
 * there is no separate "raw cursor" copy to fight with, so the
 * dragged grip never jitters between a raw and a solved position.
 *
 * ## Coalescing
 *
 * Pointer-move events fire faster than the solver runs. The
 * controller keeps at most one solve in flight plus one *pending*
 * cursor: while a solve runs, newer moves only overwrite the
 * pending target, and the loop picks up the latest when the solve
 * returns. Intermediate cursors are dropped — only the newest
 * position is ever solved, which keeps the drag responsive on large
 * sketches instead of building an ever-growing solve backlog.
 *
 * ## Lifecycle
 *
 * 1. [`start`] — snapshot the sub-calls (for [`cancel`]) and stash
 *    the grip being dragged.
 * 2. [`update`] — set the pending cursor and kick the solve loop.
 *    Coalesced; returns a promise that resolves once the loop has
 *    drained, which tests await to read the settled state.
 * 3. [`finish`] — run one authoritative solve at the final cursor
 *    (coalescing may have dropped it) and end the drag.
 * 4. [`cancel`] — restore the pre-drag snapshot and end the drag.
 *
 * The controller talks to the app through [`SketchSolveHost`] (the
 * same structural surface [`solveAndReplace`] uses), so it stays
 * decoupled from the concrete store and is trivial to stub in tests.
 */

import { applyGripDrag, type Grip } from "../sketchGrips";
import { buildDraftSketchState } from "../sketchSession";
import { solveSketch } from "./planegcs";
import {
  rebuildSubCallsFromSolved,
  type SketchSolveHost,
  type SubCall,
} from "./replaySolve";

type Cursor = [number, number];

export class DragController {
  private host: SketchSolveHost | null = null;
  private grip: Grip | null = null;
  /** Sub-calls at the moment the drag started, for [`cancel`]. */
  private snapshot: ReadonlyArray<SubCall> | null = null;

  /** Latest cursor not yet consumed by the solve loop. `null` means
   *  the loop has nothing new to do. */
  private pending: Cursor | null = null;
  /** Last cursor seen this drag — [`finish`] re-solves at this value
   *  because coalescing may have dropped it from [`pending`]. */
  private lastCursor: Cursor | null = null;
  /** Is the solve loop currently running? */
  private running = false;
  /** Is a drag in progress? Cleared by [`finish`] / [`cancel`] so a
   *  late solve from a finished drag never writes back. */
  private active = false;
  /** Resolvers waiting for the loop to drain (used by [`update`]). */
  private idleWaiters: Array<() => void> = [];

  /** Is a drag currently in progress? */
  isDragging(): boolean {
    return this.active;
  }

  /**
   * Begin a drag. `grip` is the handle the user grabbed; the
   * controller reads geometry from — and writes solved geometry back
   * to — `host` for the rest of the gesture.
   */
  start(host: SketchSolveHost, grip: Grip): void {
    if (this.active) {
      throw new Error(
        "DragController: start() called while a drag is already in progress",
      );
    }
    this.host = host;
    this.grip = grip;
    this.snapshot = host.getState().sketchSession.subCalls;
    this.pending = null;
    this.lastCursor = null;
    this.running = false;
    this.active = true;
  }

  /**
   * Push a new cursor position (sketch millimetres). Coalesced: only
   * the newest target is ever solved. Returns a promise that
   * resolves when the solve loop next goes idle, so the resolved
   * session reflects this cursor or a newer one — handy for tests.
   */
  update(to: Cursor): Promise<void> {
    if (!this.active) return Promise.resolve();
    this.pending = to;
    this.lastCursor = to;
    const drained = new Promise<void>((resolve) => {
      this.idleWaiters.push(resolve);
    });
    if (!this.running) void this.loop();
    return drained;
  }

  private async loop(): Promise<void> {
    if (!this.host || !this.grip) return;
    this.running = true;
    try {
      while (this.active && this.pending) {
        const to = this.pending;
        this.pending = null;
        await this.solveOnce(to);
      }
    } finally {
      this.running = false;
      const waiters = this.idleWaiters;
      this.idleWaiters = [];
      for (const resolve of waiters) resolve();
    }
  }

  /** Solve at one cursor and write the result back, unless the drag
   *  ended or the session closed meanwhile. */
  private async solveOnce(to: Cursor): Promise<void> {
    const host = this.host;
    const grip = this.grip;
    if (!host || !grip) return;
    const session = host.getState().sketchSession;
    if (!session.active) return;
    const input = applyGripDrag(session.subCalls, grip, to);
    try {
      const result = await solveSketch(buildDraftSketchState(input));
      // The drag may have ended (finish/cancel) or the session closed
      // while we awaited — never clobber newer state with a stale solve.
      if (!this.active) return;
      const after = host.getState().sketchSession;
      if (!after.active) return;
      host.replaceSketchSubCalls(rebuildSubCallsFromSolved(input, result.state));
      host.setSketchDiagnostics(result.diagnostics);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: live drag solve failed", err);
    }
  }

  /**
   * End the drag with one authoritative solve at the final cursor.
   * Coalescing can drop the very last move, so we re-solve it here
   * (bypassing the now-inactive loop) to guarantee the committed
   * geometry matches where the user let go. A no-op final solve when
   * the drag never moved.
   */
  async finish(): Promise<void> {
    if (!this.active) {
      throw new Error("DragController: finish() called with no active drag");
    }
    const host = this.host;
    const grip = this.grip;
    const to = this.lastCursor;
    // Deactivate first so any in-flight loop solve discards its write,
    // then run the final solve as the last writer.
    this.active = false;
    this.clear();
    if (host && grip && to) {
      const session = host.getState().sketchSession;
      if (session.active) {
        const input = applyGripDrag(session.subCalls, grip, to);
        try {
          const result = await solveSketch(buildDraftSketchState(input));
          const after = host.getState().sketchSession;
          if (after.active) {
            host.replaceSketchSubCalls(
              rebuildSubCallsFromSolved(input, result.state),
            );
            host.setSketchDiagnostics(result.diagnostics);
          }
        } catch (err) {
          // eslint-disable-next-line no-console
          console.error("fielddraft: final drag solve failed", err);
        }
      }
    }
  }

  /**
   * Abandon the drag and restore the pre-drag sub-calls verbatim —
   * every mutation the drag introduced is discarded. Use from an
   * Escape handler.
   */
  cancel(): void {
    if (!this.active) {
      throw new Error("DragController: cancel() called with no active drag");
    }
    const host = this.host;
    const snapshot = this.snapshot;
    this.active = false;
    this.clear();
    if (host && snapshot) host.replaceSketchSubCalls(snapshot.slice());
  }

  private clear(): void {
    this.host = null;
    this.grip = null;
    this.snapshot = null;
    this.pending = null;
    this.lastCursor = null;
  }
}
