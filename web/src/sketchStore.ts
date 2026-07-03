/**
 * SketchStore — the sketch-domain controller.
 *
 * Everything about the ACTIVE sketch session lives here: session
 * lifecycle (enter / edit / exit / cancel / commit), the drawing- and
 * modify-tools (trim / split / extend / fillet / chamfer / offset /
 * mirror / pattern / move / rotate / text), constraints, the JS-side
 * sketch undo stack, and the hookup into the shared solve-and-replace
 * pipeline. The KernelStore facade delegates its public sketch API 1:1
 * to this class — components keep calling `store.enterSketch(...)` etc.
 *
 * Sketch geometry is UI-side state: the session's `subCalls` only reach
 * the kernel on Finish (`commitSketch` → addSketch / replaceSketch).
 */

import type { Kernel } from "./kernel";
import {
  applySketchToolDraft as reduceApply,
  enterSketchSession as reduceEnter,
  exitSketchSession as reduceExit,
  setSketchSessionPlane as reduceSetPlane,
  selectSketchTool as reduceSelect,
  setSketchToolDraft as reduceSetDraft,
  removeSketchSubCallAt as reduceRemoveAt,
  replaceSketchSubCallAt as reduceReplaceAt,
  toggleSketchSelection as reduceToggleSelection,
  setSketchSelection as reduceSetSelection,
  clearSketchSelection as reduceClearSelection,
  buildDraftSketchState,
  subCallsForSelection,
  entityOwnerSubCalls,
  type SketchSessionState,
} from "./sketchSession";
import {
  buildConstraintCall,
  applicableConstraints,
  isConstraintTool,
  resolveCoincidentConstraint,
  CONSTRAINT_DIMENSION_TOOL,
  CONSTRAINT_COINCIDENT_TOOL,
  DIMENSION_TYPES_BY_SPECIFICITY,
} from "./sketchConstraints";
import { logUserAction } from "./userActionLog";
import { resolveCornerFillet } from "./sketchCorner";
import { buildLineLengthEdit } from "./sketchLiveInput";
import {
  computeTrim,
  curveOfSubCall,
  edgesOfSubCall,
  subCallOfCurve,
} from "./sketchTrim";
import type { Vec2 } from "./sketchTrim";
import { mirrorSubCall, isMirrorableSubCall } from "./sketchMirror";
import { offsetCurve, offsetChain, buildChain } from "./sketchOffset";
import { patternLinear, patternCircular, rotateCurve } from "./sketchPattern";
import {
  translateSubCall,
  isTranslatableSubCall,
} from "./sketchSubCallTransform";
import {
  filletLines,
  filletRecipe,
  reeditFilletRadius,
  nextFdId,
  fdIdOf,
  FD_ID_PARAM,
} from "./sketchFillet";
import {
  chamferLines,
  chamferRecipe,
  reeditChamferDistance,
} from "./sketchChamfer";
import { extendLine } from "./sketchExtend";
import { splitCurveAt } from "./sketchSplit";
import { importVectorToSubCalls } from "./importVector";
import { textToSubCalls, reeditTextGroup, removeTextGroup } from "./sketchText";
import {
  DEFAULT_FONT_ID,
  cachedFont,
  loadBundledFont,
  parseFontBuffer,
  registerUploadedFont,
} from "./sketchTextFont";
import { solveAndReplace } from "./solver/replaySolve";
import type { SketchDiagnostics } from "./solver/types";
import {
  StoreCore,
  formatKernelError,
  type StoreState,
} from "./storeCore";

/**
 * Parameters of the interactive sketch tools, consolidated into ONE
 * StoreState object (previously ~12 individual `sketchXxx` fields).
 * Every tool setter is a one-line `patchToolParams` now, and consumers
 * read `state.sketchToolParams.<field>`.
 */
export interface SketchToolParams {
  /** F10: sketch-fillet radius (mm) used by the two-click `sketch_fillet`
   *  tool to round the corner between two picked lines. */
  filletRadius: number;
  /** F11: sketch-chamfer setback distance (mm) used by the two-click
   *  `sketch_chamfer` tool to bevel the corner between two picked lines. */
  chamferDistance: number;
  /** F15: sketch-offset distance (mm) used by the single-click
   *  `sketch_offset` tool to place a parallel copy of the picked curve. */
  offsetDistance: number;
  /** F16: number of instances (original + copies) for a sketch pattern. */
  patternCount: number;
  /** F16: per-step spacing (mm) for a linear sketch pattern. */
  patternSpacing: number;
  /** F16: total span (degrees) for a circular sketch pattern. */
  patternAngle: number;
  /** F17: rotation angle (degrees) used by the `rotateSketchSelection`
   *  transform (rotates the selection about the sketch origin). */
  rotateAngle: number;
  /** F23-full: which spline family the `sketch_spline` tool draws —
   *  `"interpolate"` (Catmull-Rom through the points) or `"control"`
   *  (real cubic B-spline with the points as control polygon). */
  splineMode: "interpolate" | "control";
  /** F24: the string the single-click `sketch_text` tool emits as
   *  filled glyph geometry. */
  textValue: string;
  /** F24: cap/em height (mm) for the `sketch_text` tool. */
  textHeight: number;
  /** F24-full: id of the font the `sketch_text` tool renders with —
   *  a bundled-font id (see sketchTextFont.BUNDLED_FONTS) or a synthetic
   *  `uploaded:…` id for a user-supplied .ttf/.otf. */
  textFontId: string;
  /** F24-full: true once the currently-selected text font has finished
   *  loading + parsing (drives the UI's "loading font…" hint and gates
   *  placement). */
  textFontReady: boolean;
}

/** The defaults for a fresh store. */
export function initialSketchToolParams(): SketchToolParams {
  return {
    filletRadius: 5,
    chamferDistance: 5,
    offsetDistance: 5,
    patternCount: 3,
    patternSpacing: 20,
    patternAngle: 360,
    rotateAngle: 90,
    splineMode: "interpolate",
    textValue: "Text",
    textHeight: 10,
    textFontId: DEFAULT_FONT_ID,
    textFontReady: cachedFont(DEFAULT_FONT_ID) !== undefined,
  };
}

/** Cross-domain pieces the sketch controller needs from the facade. */
export interface SketchStoreDeps {
  /** Feature removal lives in the feature domain — `cancelSketch` drops
   *  the sketch feature that was created on session entry. */
  removeFeature(id: number): boolean;
}

export class SketchStore {
  constructor(
    private readonly core: StoreCore,
    private readonly deps: SketchStoreDeps,
  ) {}

  private get kernel(): Kernel {
    return this.core.kernel;
  }
  private get state(): StoreState {
    return this.core.state;
  }
  private set state(next: StoreState) {
    this.core.state = next;
  }
  private patch(partial: Partial<StoreState>): void {
    this.core.patch(partial);
  }
  private notify(): void {
    this.core.notify();
  }

  /** Also the `SketchSolveHost` surface for {@link solveAndReplace}. */
  getState = (): StoreState => this.core.state;

  /** Consolidated one-liner behind every sketch-tool param setter. */
  private patchToolParams(partial: Partial<SketchToolParams>): void {
    this.core.patch({
      sketchToolParams: { ...this.core.state.sketchToolParams, ...partial },
    });
    this.core.emit();
  }

  // ── sketch-session undo/redo (ticket r3ci) ───────────────────────
  //
  // Sketch geometry lives entirely on the UI side as the session's
  // `subCalls` (it isn't pushed to the kernel until Finish), so the
  // kernel undo stack — what `undo()`/`redo()` drive — knows nothing
  // about a line the user just drew. These two stacks give the active
  // session its own history: each entry is a full `subCalls` snapshot
  // (the arrays are always replaced immutably, so storing the reference
  // is a sound snapshot). Checkpoints are captured at the two structural
  // chokepoints — `updateSketch` (active→active, subCalls changed) and
  // `pushSketchSubCalls`. The solver's re-coordinate pass goes through
  // `replaceSketchSubCalls` (NOT these), so a solve never lands as its
  // own undo step.
  private sketchUndoStack: SketchSessionState["subCalls"][] = [];
  private sketchRedoStack: SketchSessionState["subCalls"][] = [];

  /** Snapshot the current sub-spine onto the undo stack and start a fresh
   *  redo branch. Called just before a user-initiated structural mutation. */
  private pushSketchUndoCheckpoint(): void {
    this.sketchUndoStack.push(this.state.sketchSession.subCalls);
    // Bound the history so a long session can't grow it without limit.
    if (this.sketchUndoStack.length > 200) this.sketchUndoStack.shift();
    this.sketchRedoStack = [];
  }

  /** Clear both sketch history stacks — on session enter/exit, where a
   *  cross-session undo would make no sense. */
  private resetSketchHistory(): void {
    this.sketchUndoStack = [];
    this.sketchRedoStack = [];
  }

  /** Undo the last structural sketch edit (drawn geometry, deletes,
   *  dimension edits). No-op outside a session or with empty history.
   *  Re-solves so the overlay / inference / DoF reflect the restored
   *  spine, exactly like every other mutation. */
  sketchUndo(): boolean {
    if (!this.state.sketchSession.active) return false;
    const prev = this.sketchUndoStack.pop();
    if (prev === undefined) return false;
    this.sketchRedoStack.push(this.state.sketchSession.subCalls);
    this.state = {
      ...this.state,
      sketchSession: {
        ...this.state.sketchSession,
        subCalls: prev,
        selection: [],
      },
    };
    this.notify();
    void solveAndReplace(this);
    return true;
  }

  /** Redo the last undone sketch edit. */
  sketchRedo(): boolean {
    if (!this.state.sketchSession.active) return false;
    const next = this.sketchRedoStack.pop();
    if (next === undefined) return false;
    this.sketchUndoStack.push(this.state.sketchSession.subCalls);
    this.state = {
      ...this.state,
      sketchSession: {
        ...this.state.sketchSession,
        subCalls: next,
        selection: [],
      },
    };
    this.notify();
    void solveAndReplace(this);
    return true;
  }

  /**
   * Enter sketch mode against a given plane query. K6b.2 accepts
   * a plane query as a plain string (placeholder until K6b.3
   * wires the face-pick path); calling this while a session is
   * already active is a no-op so the viewport can't stack
   * sessions on top of each other.
   */
  private updateSketch(next: SketchSessionState): void {
    if (next === this.state.sketchSession) return;
    // r3ci: a structural sub-spine change within an ongoing session is a
    // user edit — checkpoint it for sketch undo. Enter/exit transitions
    // (active flips) reset history separately and must not checkpoint.
    const cur = this.state.sketchSession;
    if (cur.active && next.active && next.subCalls !== cur.subCalls) {
      this.pushSketchUndoCheckpoint();
    }
    this.patch({ sketchSession: next });
    this.notify();
  }

  enterSketch(planeQuery: string): void {
    if (this.state.sketchSession.active) return;
    logUserAction("sketch", "enter", { plane: planeQuery });
    // Ticket d9o9 (Onshape style): materialise the sketch as a real feature
    // immediately so it shows up in the tree the moment you start drawing —
    // not only after Finish. We then edit it in place (the editingFeatureId
    // path); Finish keeps it (even empty) and Cancel deletes it.
    let id: number;
    try {
      id = this.kernel.addSketch(planeQuery, []);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: enterSketch failed to create sketch", err);
      this.core.setTransientError(formatKernelError(err, "Could not start sketch"));
      return;
    }
    this.resetSketchHistory(); // r3ci: fresh session, fresh history
    this.core.commit(new Set([id]), id); // surface + select the new sketch row
    this.updateSketch(
      reduceEnter(this.state.sketchSession, planeQuery, {
        editingFeatureId: id,
        createdThisSession: true,
      }),
    );
  }

  /**
   * Ticket d9o9: Cancel (✗) a sketch session. When the session created its
   * feature on entry, the cancel removes that fresh sketch entirely (it was
   * never confirmed); re-editing an existing sketch just discards the
   * in-session edits and leaves the committed feature untouched.
   */
  cancelSketch(): void {
    const session = this.state.sketchSession;
    const createdId = session.createdThisSession
      ? session.editingFeatureId
      : null;
    this.exitSketch();
    if (createdId !== null) {
      this.deps.removeFeature(createdId);
    }
  }

  /**
   * Ticket yx6a: re-anchor the ACTIVE session's sketch plane. The
   * plane is an ordinary feature property now — the session panel's
   * QueryWidget calls this when the user picks a face (or types a
   * datum query). Committed (u,v) sub-calls keep their coordinates
   * and re-project onto the new plane.
   */
  setSketchPlane(planeQuery: string): void {
    this.updateSketch(reduceSetPlane(this.state.sketchSession, planeQuery));
  }

  /**
   * K6b.3b: reopen a previously committed sketch for editing.
   * Pulls the sketch's sub_calls back out of the kernel and
   * seeds the session with them, remembering the feature id
   * so Finish takes the `replaceSketch` path instead of
   * creating a new outliner row.
   */
  editSketch(featureId: number): void {
    if (this.state.sketchSession.active) return;
    let subCalls: SketchSessionState["subCalls"] = [];
    try {
      const raw = this.kernel.getSketchSubCalls(featureId);
      subCalls = raw.map((c) => ({ type: c.type, params: c.params }));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("fielddraft: editSketch() failed to load sub-calls", err);
      return;
    }
    // Round-trip the committed plane query (ticket yx6a): the real plane
    // lives in the FeatureCall params; falling back to the XY placeholder
    // silently re-planed every edited sketch.
    let planeQuery = "qCreatedBy(origin_plane_xy)";
    try {
      const params = this.kernel.getFeatureCallParams(featureId);
      if (typeof params.plane === "string" && params.plane.length > 0) {
        planeQuery = params.plane;
      }
    } catch {
      // params unavailable — keep the XY fallback
    }
    this.resetSketchHistory(); // r3ci: re-opened sketch starts fresh history
    this.updateSketch(
      reduceEnter(this.state.sketchSession, planeQuery, {
        initialSubCalls: subCalls,
        editingFeatureId: featureId,
      }),
    );
  }

  /**
   * Exit sketch mode, discarding the draft. K6b.2 does not yet
   * persist sub_calls back into the kernel — that's the commit
   * path K6b.3 will implement. Both Finish and Cancel route
   * here for now.
   */
  exitSketch(): void {
    logUserAction("sketch", "exit");
    this.resetSketchHistory(); // r3ci
    this.updateSketch(reduceExit(this.state.sketchSession));
  }

  /**
   * Select a sketch tool by type name. Passing `null` clears
   * the tool selection. Delegates shape changes to the pure
   * reducer in `sketchSession.ts`.
   */
  selectSketchTool(
    typeName: string | null,
    initialDraft: Record<string, unknown> = {},
  ): void {
    logUserAction("sketch", "tool", { tool: typeName });
    this.updateSketch(
      reduceSelect(this.state.sketchSession, typeName, initialDraft),
    );
    // F24-full: preload the text font on Text-tool activation so the first
    // placement click has a parsed font ready (load is async).
    if (typeName === "sketch_text") {
      void this.ensureTextFontLoaded();
    }
  }

  /** Update the active sketch tool's FeatureDialog working copy. */
  setSketchToolDraft(draft: Record<string, unknown>): void {
    this.updateSketch(reduceSetDraft(this.state.sketchSession, draft));
  }

  /**
   * K6b.2: append the current tool draft to the session's
   * in-memory sub_calls buffer. No kernel interaction — K6b.3
   * replaces this with a regeneration path that rebuilds the
   * full `sketch` FeatureCall after every apply.
   */
  applySketchToolDraft(): void {
    this.updateSketch(reduceApply(this.state.sketchSession));
  }

  /**
   * K6b.3d: append multiple sub-spine calls atomically. Used by
   * the 2D canvas commit path when a line drag needs to land
   * both the `sketch_line` call and a constraint call in a
   * single step so the session snapshot is always consistent.
   */
  pushSketchSubCalls(
    calls: ReadonlyArray<{ type: string; params: Record<string, unknown> }>,
  ): void {
    if (!this.state.sketchSession.active) return;
    if (calls.length === 0) return;
    logUserAction("sketch", "draw", { calls: calls.map((c) => c.type) });
    this.pushSketchUndoCheckpoint(); // r3ci
    this.state = {
      ...this.state,
      sketchSession: {
        ...this.state.sketchSession,
        subCalls: [...this.state.sketchSession.subCalls, ...calls],
      },
    };
    this.notify();
  }

  /**
   * F22: parse an SVG / DXF file's text into sketch sub-calls and append
   * them to the active session, then re-solve. Returns the number of
   * sub-calls imported (0 when nothing parsed or no session is active),
   * so the caller can surface a hint on an empty result. SVG coordinates
   * are y-flipped (y-down → y-up); DXF passes through unflipped. Curves
   * are tessellated into `sketch_line` chains.
   */
  importVectorSubCalls(text: string, filename: string): number {
    if (!this.state.sketchSession.active) return 0;
    let calls: ReadonlyArray<{ type: string; params: Record<string, unknown> }>;
    try {
      calls = importVectorToSubCalls(text, filename);
    } catch {
      return 0;
    }
    if (calls.length === 0) return 0;
    this.pushSketchSubCalls(calls);
    void solveAndReplace(this);
    return calls.length;
  }

  /**
   * K6b.3d: wholesale replace the session's sub-spine. After a
   * commit-time solver pass rewrites the coordinates of every
   * call, the canvas overlay calls this to swap the old
   * sub_calls for the solved ones without going through a
   * delta diff.
   */
  replaceSketchSubCalls(
    calls: ReadonlyArray<{ type: string; params: Record<string, unknown> }>,
  ): void {
    if (!this.state.sketchSession.active) return;
    this.state = {
      ...this.state,
      sketchSession: {
        ...this.state.sketchSession,
        subCalls: [...calls],
      },
    };
    this.notify();
  }

  /**
   * F18 (click-to-edit dimension): set the committed line at spine
   * `index` to exactly `lengthMm`. The line keeps its start point and
   * direction; the typed value also lands in a DRIVEN distance
   * constraint between the line's endpoints (replacing an existing one
   * from a prior edit) so the dimension stays attached. Re-solves.
   * Returns false when the edit is degenerate (see buildLineLengthEdit).
   */
  setSketchLineLength(index: number, lengthMm: number): boolean {
    const session = this.state.sketchSession;
    if (!session.active) return false;
    const edit = buildLineLengthEdit(session.subCalls, index, lengthMm);
    if (!edit) return false;
    let next = reduceReplaceAt(session, index, [edit.line]);
    if (edit.existingConstraintIndex !== null) {
      next = reduceReplaceAt(next, edit.existingConstraintIndex, [
        edit.constraint,
      ]);
    } else {
      next = { ...next, subCalls: [...next.subCalls, edit.constraint] };
    }
    this.updateSketch(next);
    void solveAndReplace(this);
    return true;
  }

  /**
   * F20: stash the latest solve diagnostics on the active session so
   * the Outliner constraints panel can flag conflicting / redundant
   * constraint rows. No-op when no session is active. Called by
   * `solveAndReplace` after a successful solve; the snapshot-identity
   * guard there ensures a stale solve can't write outdated flags.
   */
  setSketchDiagnostics(diagnostics: SketchDiagnostics): void {
    if (!this.state.sketchSession.active) return;
    this.state = {
      ...this.state,
      sketchSession: {
        ...this.state.sketchSession,
        diagnostics,
      },
    };
    this.notify();
  }

  /**
   * K6b.3g: drop a single sub-spine entry by index. Downstream
   * constraint calls whose references would dangle after the
   * removal are rewritten by the
   * [`removeSketchSubCallAt`](./sketchSession) reducer — ones
   * with refs into the removed range are also dropped, ones
   * above the range are shifted down by the range size so
   * they still point at the same post-remove entity.
   */
  removeSketchSubCall(index: number): void {
    this.updateSketch(reduceRemoveAt(this.state.sketchSession, index));
  }

  /**
   * Delete the sketch elements currently selected in select mode
   * (Delete/Backspace in a sketch session). Maps the selected entity
   * ids to their owning geometry sub-calls and drops those rows
   * (descending, so earlier indices stay valid); constraints that
   * reference the removed entities are cascaded by the reducer.
   * Returns true when something was deleted.
   */
  deleteSketchSelection(): boolean {
    const session = this.state.sketchSession;
    if (!session.active || session.selection.length === 0) return false;
    const owner = entityOwnerSubCalls(session.subCalls);
    const indices = [
      ...new Set(
        session.selection
          .map((id) => owner.get(id))
          .filter((i): i is number => i !== undefined),
      ),
    ].sort((a, b) => b - a);
    if (indices.length === 0) return false;
    let next = session;
    for (const i of indices) next = reduceRemoveAt(next, i);
    this.updateSketch(next);
    void solveAndReplace(this);
    return true;
  }

  /**
   * Trim: cut the curve under `click` at its nearest intersections with
   * the other sketch geometry and drop the picked piece. The clicked
   * line/arc is replaced in place by the 0–2 surviving pieces (an
   * intersection-free curve is deleted outright); downstream constraint
   * refs are rewritten by [`replaceSketchSubCallAt`] and the sketch is
   * re-solved. `tolMm` is the pick radius in sketch millimetres. No-op
   * when nothing trimmable is under the cursor.
   */
  trimSketchAt(click: [number, number], tolMm: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const result = computeTrim(session.subCalls, click, tolMm);
    if (!result) return;
    this.updateSketch(reduceReplaceAt(session, result.index, result.newCalls));
    void solveAndReplace(this);
  }

  /**
   * F13: Split — the keep-both sibling of Trim. The curve at
   * `targetIndex` is cut at `click` into two sub-curves that meet at the
   * click point; nothing is deleted. The clicked line/arc is replaced in
   * place by its two pieces (each carrying the target's construction
   * flag); downstream constraint refs are rewritten by
   * [`replaceSketchSubCallAt`] and the sketch is re-solved. No-op when
   * the target isn't a simple line/arc, or the click is at an endpoint /
   * on a full circle (see sketchSplit.ts). `click` is in sketch mm.
   */
  splitSketchAt(
    targetIndex: number,
    click: [number, number],
    _tolMm: number,
  ): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const target = session.subCalls[targetIndex];
    if (!target) return;
    const cv = curveOfSubCall(target);
    if (!cv) return; // composites / unsupported kinds: keep v1 simple

    const pieces = splitCurveAt(cv, click);
    if (!pieces) return;

    const carry: Record<string, unknown> =
      target.params.construction !== undefined
        ? { construction: target.params.construction }
        : {};
    this.updateSketch(
      reduceReplaceAt(session, targetIndex, [
        subCallOfCurve(pieces[0], carry),
        subCallOfCurve(pieces[1], carry),
      ]),
    );
    void solveAndReplace(this);
  }

  /**
   * F12: Extend (complement of Trim). Lengthen the line sub-call at
   * `targetIndex` along its own direction until it meets the nearest other
   * sketch curve. The endpoint nearer `click` is the grabbed end that
   * moves outward; the far endpoint is preserved. The lengthened line
   * replaces the target in place (carrying its construction flag);
   * downstream constraint refs are rewritten by [`replaceSketchSubCallAt`]
   * and the sketch is re-solved. No-op when the target isn't a line or
   * nothing lies in the forward extension. `click` is in sketch mm.
   */
  extendSketchAt(
    targetIndex: number,
    click: [number, number],
    _tolMm: number,
  ): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const target = session.subCalls[targetIndex];
    if (!target) return;
    const line = curveOfSubCall(target);
    if (!line || line.kind !== "line") return;

    // Every edge of every OTHER sub-call is a curve to extend into.
    const others = session.subCalls.flatMap((call, i) =>
      i === targetIndex ? [] : edgesOfSubCall(call),
    );

    const extended = extendLine(line, click, others);
    if (!extended) return;

    const carry: Record<string, unknown> =
      target.params.construction !== undefined
        ? { construction: target.params.construction }
        : {};
    this.updateSketch(
      reduceReplaceAt(session, targetIndex, [subCallOfCurve(extended, carry)]),
    );
    void solveAndReplace(this);
  }

  /**
   * F14: interactive Mirror. Reflect the currently-selected source
   * geometry across the axis line at sub-call index `axisLineIndex` and
   * append the mirrored copies as ordinary `sketch_line` /
   * `sketch_circle` / `sketch_arc` sub-calls (so they stay editable —
   * this is a UI-side rewrite, no kernel mirror tool).
   *
   * Selection → sub-call rule: mirror every SIMPLE geometry sub-call
   * (line / circle / arc, i.e. `curveOfSubCall` non-null) that owns at
   * least one selected entity, EXCLUDING the axis line itself. The axis
   * line's two solved endpoints (read from `buildDraftSketchState`)
   * define the infinite reflection axis. Each mirrored copy carries the
   * source's construction flag. After appending, the sketch is re-solved
   * and the selection is cleared.
   *
   * No-op when the axis isn't a line, the selection is empty, or nothing
   * mirrorable remains after excluding the axis.
   */
  mirrorSketchSelection(axisLineIndex: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (session.selection.length === 0) return;

    // Map the entity-id selection back to the simple-geometry sub-calls
    // that produced it, dropping the axis line and any non-simple kinds.
    const sources = subCallsForSelection(
      session.subCalls,
      session.selection,
    ).filter(
      (i) => i !== axisLineIndex && isMirrorableSubCall(session.subCalls[i]),
    );
    const appended = this.mirrorSubCallsAcrossAxis(axisLineIndex, sources);
    if (!appended) return;
    this.updateSketch(reduceClearSelection(this.state.sketchSession));
    void solveAndReplace(this);
  }

  /**
   * Ticket ps21: live "pick-to-apply" Mirror. Reflect the single sketch
   * entity `entityId` across the axis line at `axisLineIndex` and append
   * the mirrored copy immediately — no Enter, no selection step. Each
   * click in the viewport while the Mirror tool's axis is set calls this,
   * so the result appears the instant the user clicks the source object
   * (Onshape style). No-op when the entity maps to no simple curve, is the
   * axis itself, or the axis isn't a line.
   */
  mirrorSketchEntityAt(axisLineIndex: number, entityId: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const sources = subCallsForSelection(session.subCalls, [entityId]).filter(
      (i) => i !== axisLineIndex && isMirrorableSubCall(session.subCalls[i]),
    );
    if (sources.length === 0) return;
    if (this.mirrorSubCallsAcrossAxis(axisLineIndex, sources)) {
      void solveAndReplace(this);
    }
  }

  /**
   * Shared core for both Mirror entry points: reflect every sub-call in
   * `sourceIndices` across the line at `axisLineIndex` and append the
   * mirrored copies (carrying each source's construction flag). Returns
   * `true` when at least one mirrored copy was pushed, `false` on a no-op
   * (axis not a line, no mirrorable sources). Does NOT touch the selection
   * or trigger a re-solve — callers decide.
   */
  private mirrorSubCallsAcrossAxis(
    axisLineIndex: number,
    sourceIndices: ReadonlyArray<number>,
  ): boolean {
    const session = this.state.sketchSession;
    const axisCall = session.subCalls[axisLineIndex];
    if (!axisCall) return false;
    const axisCurve = curveOfSubCall(axisCall);
    if (!axisCurve || axisCurve.kind !== "line") return false;
    const axisA = axisCurve.a;
    const axisB = axisCurve.b;

    const mirrored: Array<{ type: string; params: Record<string, unknown> }> =
      [];
    for (const i of sourceIndices) {
      // Ticket o6hx: points, simple curves and composites (rectangle /
      // polygon) all reflect via the pure `mirrorSubCall` SSOT.
      mirrored.push(...mirrorSubCall(session.subCalls[i], axisA, axisB));
    }
    if (mirrored.length === 0) return false;
    this.pushSketchSubCalls(mirrored);
    return true;
  }

  /** F10: set the radius (mm) the two-click `sketch_fillet` tool uses to
   *  round the next corner. No-op for a non-finite / non-positive value. */
  setSketchFilletRadius(radius: number): void {
    if (!Number.isFinite(radius) || radius <= 0) return;
    this.patchToolParams({ filletRadius: radius });
  }

  /**
   * F10: round the corner between the two line sub-calls at
   * `firstIndex` and `secondIndex` with a tangent arc of `radius` mm.
   * Reads both lines' solved geometry, computes the fillet via
   * [`filletLines`], then rewrites the spine: the first line is replaced
   * by its trimmed remainder in place, the second line is replaced by
   * its trimmed remainder in place, and the rounding arc is appended.
   * Downstream constraint refs are rewritten by [`replaceSketchSubCallAt`]
   * and the sketch is re-solved. No-op when either pick isn't a line,
   * they're the same call, or no fillet is geometrically possible
   * (parallel, radius too large, no shared corner).
   */
  filletSketchAt(
    firstIndex: number,
    secondIndex: number,
    radius: number,
  ): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (firstIndex === secondIndex) return;
    const c1 = session.subCalls[firstIndex];
    const c2 = session.subCalls[secondIndex];
    if (!c1 || !c2) return;
    const cv1 = curveOfSubCall(c1);
    const cv2 = curveOfSubCall(c2);
    if (!cv1 || !cv2 || cv1.kind !== "line" || cv2.kind !== "line") return;

    const fillet = filletLines(cv1, cv2, radius);
    if (!fillet) return;

    // Carry each line's construction flag onto its trimmed remainder; the
    // arc inherits the first line's flag (a fillet of two construction
    // lines stays construction).
    const carry = (params: Record<string, unknown>): Record<string, unknown> =>
      params.construction !== undefined
        ? { construction: params.construction }
        : {};

    // Plan item 4: tag the two source lines with a STABLE id (re-use one
    // already present, else mint a fresh `fd_id`) so the re-edit recipe can
    // resolve them by id after reorders. The arc carries the same two ids
    // plus the ORIGINAL (pre-trim) geometry, so a radius change re-derives
    // from `cv1`/`cv2` rather than the already-trimmed remainders.
    const idA = fdIdOf(c1) ?? nextFdId();
    const idB = fdIdOf(c2) ?? nextFdId();

    // Replace the two lines in place (each a single trimmed line), then
    // append the arc. Apply the higher index first so the lower index
    // stays valid across the first rewrite.
    const lo = Math.min(firstIndex, secondIndex);
    const hi = Math.max(firstIndex, secondIndex);
    const loCurve = lo === firstIndex ? fillet.trimmed1 : fillet.trimmed2;
    const hiCurve = hi === firstIndex ? fillet.trimmed1 : fillet.trimmed2;
    const loParams = lo === firstIndex ? c1.params : c2.params;
    const hiParams = hi === firstIndex ? c1.params : c2.params;
    const loId = lo === firstIndex ? idA : idB;
    const hiId = hi === firstIndex ? idA : idB;

    let next = reduceReplaceAt(session, hi, [
      subCallOfCurve(hiCurve, { ...carry(hiParams), [FD_ID_PARAM]: hiId }),
    ]);
    next = reduceReplaceAt(next, lo, [
      subCallOfCurve(loCurve, { ...carry(loParams), [FD_ID_PARAM]: loId }),
    ]);
    this.updateSketch(next);
    this.pushSketchSubCalls([
      subCallOfCurve(fillet.arc, {
        ...carry(c1.params),
        [FD_ID_PARAM]: nextFdId(),
        fillet: filletRecipe(idA, idB, cv1, cv2, radius),
      }),
    ]);
    void solveAndReplace(this);
  }

  /**
   * Plan item 4: re-edit a previously baked fillet's radius in place.
   * `arcSubCallId` is the stable `fd_id` of the recipe-tagged `sketch_arc`
   * (surfaced by the in-scene corner editor). Re-derives the rounding arc
   * + the two trimmed source lines from the recipe via [`reeditFilletRadius`]
   * and re-solves. No-op (returns false) when the arc isn't a tagged
   * fillet, a source line is gone, or the new radius is impossible.
   *
   * Known limitation (chained fillets): the recipe snapshots the source
   * lines at first creation, so re-editing a corner whose lines a later
   * corner also trimmed restores their first-creation length — see
   * `reeditFilletRadius`.
   */
  setFilletRadiusAt(arcSubCallId: string, newRadius: number): boolean {
    const session = this.state.sketchSession;
    if (!session.active) return false;
    if (!Number.isFinite(newRadius) || newRadius <= 0) return false;
    const rewritten = reeditFilletRadius(
      session.subCalls,
      arcSubCallId,
      newRadius,
    );
    if (!rewritten) return false;
    this.replaceSketchSubCalls(rewritten);
    void solveAndReplace(this);
    return true;
  }

  /** F11: set the setback distance (mm) the two-click `sketch_chamfer`
   *  tool uses to bevel the next corner. No-op for a non-finite /
   *  non-positive value. */
  setSketchChamferDistance(dist: number): void {
    if (!Number.isFinite(dist) || dist <= 0) return;
    this.patchToolParams({ chamferDistance: dist });
  }

  /**
   * F11: bevel the corner between the two line sub-calls at
   * `firstIndex` and `secondIndex` with a straight chamfer at setback
   * `dist` mm. Reads both lines' solved geometry, computes the chamfer
   * via [`chamferLines`], then rewrites the spine: the first line is
   * replaced by its trimmed remainder in place, the second line is
   * replaced by its trimmed remainder in place, and the connecting bevel
   * line is appended. Downstream constraint refs are rewritten by
   * [`replaceSketchSubCallAt`] and the sketch is re-solved. No-op when
   * either pick isn't a line, they're the same call, or no chamfer is
   * geometrically possible (parallel, setback too large, no shared
   * corner).
   */
  chamferSketchAt(firstIndex: number, secondIndex: number, dist: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (firstIndex === secondIndex) return;
    const c1 = session.subCalls[firstIndex];
    const c2 = session.subCalls[secondIndex];
    if (!c1 || !c2) return;
    const cv1 = curveOfSubCall(c1);
    const cv2 = curveOfSubCall(c2);
    if (!cv1 || !cv2 || cv1.kind !== "line" || cv2.kind !== "line") return;

    const chamfer = chamferLines(cv1, cv2, dist);
    if (!chamfer) return;

    // Carry each line's construction flag onto its trimmed remainder; the
    // bevel inherits the first line's flag (a chamfer of two construction
    // lines stays construction).
    const carry = (params: Record<string, unknown>): Record<string, unknown> =>
      params.construction !== undefined
        ? { construction: params.construction }
        : {};

    // Plan item 4: tag the two source lines with a STABLE id and the bevel
    // with the same two ids + ORIGINAL geometry, so the setback distance is
    // re-editable in-scene. Same scheme as `filletSketchAt`.
    const idA = fdIdOf(c1) ?? nextFdId();
    const idB = fdIdOf(c2) ?? nextFdId();

    // Replace the two lines in place (each a single trimmed line), then
    // append the bevel. Apply the higher index first so the lower index
    // stays valid across the first rewrite.
    const lo = Math.min(firstIndex, secondIndex);
    const hi = Math.max(firstIndex, secondIndex);
    const loCurve = lo === firstIndex ? chamfer.trimmed1 : chamfer.trimmed2;
    const hiCurve = hi === firstIndex ? chamfer.trimmed1 : chamfer.trimmed2;
    const loParams = lo === firstIndex ? c1.params : c2.params;
    const hiParams = hi === firstIndex ? c1.params : c2.params;
    const loId = lo === firstIndex ? idA : idB;
    const hiId = hi === firstIndex ? idA : idB;

    let next = reduceReplaceAt(session, hi, [
      subCallOfCurve(hiCurve, { ...carry(hiParams), [FD_ID_PARAM]: hiId }),
    ]);
    next = reduceReplaceAt(next, lo, [
      subCallOfCurve(loCurve, { ...carry(loParams), [FD_ID_PARAM]: loId }),
    ]);
    this.updateSketch(next);
    this.pushSketchSubCalls([
      subCallOfCurve(chamfer.line, {
        ...carry(c1.params),
        [FD_ID_PARAM]: nextFdId(),
        chamfer: chamferRecipe(idA, idB, cv1, cv2, dist),
      }),
    ]);
    void solveAndReplace(this);
  }

  /**
   * Plan item 4: re-edit a previously baked chamfer's setback distance in
   * place. `bevelSubCallId` is the stable `fd_id` of the recipe-tagged
   * `sketch_line` bevel. Re-derives the bevel + the two trimmed source
   * lines via [`reeditChamferDistance`] and re-solves. No-op (returns
   * false) when the line isn't a tagged chamfer, a source line is gone, or
   * the new distance is impossible. Same chained-corner limitation as the
   * fillet path.
   */
  setChamferDistanceAt(bevelSubCallId: string, newDistance: number): boolean {
    const session = this.state.sketchSession;
    if (!session.active) return false;
    if (!Number.isFinite(newDistance) || newDistance <= 0) return false;
    const rewritten = reeditChamferDistance(
      session.subCalls,
      bevelSubCallId,
      newDistance,
    );
    if (!rewritten) return false;
    this.replaceSketchSubCalls(rewritten);
    void solveAndReplace(this);
    return true;
  }

  /**
   * Tickets tqi4 / chfp: single-CLICK corner fillet. Resolve the corner under
   * the cursor — exploding a Polygon / N-gon / Rectangle into lines if needed —
   * then fillet the two line edges that meet there. No-op when no corner is
   * close enough.
   */
  filletCornerAt(point: [number, number], radius: number, tolMm: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const r = resolveCornerFillet(session.subCalls, point, tolMm);
    if (!r) return;
    if (r.exploded) this.replaceSketchSubCalls(r.subCalls);
    this.filletSketchAt(r.i1, r.i2, radius);
  }

  /** Tickets tqi4 / chfp: single-CLICK corner chamfer (see filletCornerAt). */
  chamferCornerAt(point: [number, number], dist: number, tolMm: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const r = resolveCornerFillet(session.subCalls, point, tolMm);
    if (!r) return;
    if (r.exploded) this.replaceSketchSubCalls(r.subCalls);
    this.chamferSketchAt(r.i1, r.i2, dist);
  }

  /** F15: set the distance (mm) the single-click `sketch_offset` tool
   *  uses to place a parallel copy. No-op for a non-finite / non-positive
   *  value. */
  setSketchOffsetDistance(dist: number): void {
    if (!Number.isFinite(dist) || dist <= 0) return;
    this.patchToolParams({ offsetDistance: dist });
  }

  /**
   * F15 / F15-full: offset the whole connected CHAIN the clicked curve
   * belongs to, toward the side the click `clickPoint` lies on.
   *
   * From `targetIndex` the connected component of line/arc curves whose
   * endpoints coincide is walked ([`buildChain`]); every segment is
   * offset on a globally consistent side and the joints are reconciled —
   * concave corners trim to the support intersection, convex corners get
   * a rounding fillet arc of radius = distance, a closed loop also closes
   * the wrap-around joint ([`offsetChain`]). The result is appended as
   * ordinary `sketch_line` / `sketch_arc` sub-calls (the originals stay
   * put), each carrying its source's construction flag, then the sketch
   * is re-solved.
   *
   * A full circle (no endpoints) is never part of a chain and falls
   * through to the concentric single-curve [`offsetCurve`]; a lone
   * line/arc with no coincident neighbour is the degenerate length-1
   * chain. No-op when the target isn't a simple curve or no valid offset
   * exists (collapsed offset, degenerate geometry, bad distance).
   */
  offsetSketchAt(
    targetIndex: number,
    clickPoint: Vec2,
    distance: number,
  ): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const target = session.subCalls[targetIndex];
    if (!target) return;
    const cv = curveOfSubCall(target);
    if (!cv) return;

    const carry = (params: Record<string, unknown>): Record<string, unknown> =>
      params.construction !== undefined
        ? { construction: params.construction }
        : {};

    // A full circle has no endpoints — offset it concentrically.
    if (cv.kind === "circle") {
      const offset = offsetCurve(cv, distance, clickPoint);
      if (!offset) return;
      this.pushSketchSubCalls([subCallOfCurve(offset, carry(target.params))]);
      void solveAndReplace(this);
      return;
    }

    // Build the connected chain of line/arc curves from the sub-spine,
    // keeping each sub-call's curve at its own index so the seed maps in.
    const curves = session.subCalls.map((c) => curveOfSubCall(c));
    const chain = buildChain(curves, targetIndex);
    if (!chain) return;
    const clickedPos = chain.indices.indexOf(targetIndex);
    if (clickedPos < 0) return;

    const offsets = offsetChain(chain, clickedPos, distance, clickPoint);
    if (!offsets || offsets.length === 0) return;

    // The offset copies inherit the clicked source's construction flag.
    const flags = carry(target.params);
    this.pushSketchSubCalls(offsets.map((o) => subCallOfCurve(o, flags)));
    void solveAndReplace(this);
  }

  /** F23-full: choose the spline family the `sketch_spline` tool draws. */
  setSketchSplineMode(mode: "interpolate" | "control"): void {
    this.patchToolParams({ splineMode: mode });
  }

  /** F24: set the string the `sketch_text` tool emits. */
  setSketchTextValue(value: string): void {
    this.patchToolParams({ textValue: value });
  }

  /** F24: set the cap height (mm) for the `sketch_text` tool. No-op for a
   *  non-finite / non-positive value. */
  setSketchTextHeight(height: number): void {
    if (!Number.isFinite(height) || height <= 0) return;
    this.patchToolParams({ textHeight: height });
  }

  /** F24-full: select the font the `sketch_text` tool renders with and
   *  kick off its async load (no-op when already the current font). */
  setSketchTextFontId(id: string): void {
    if (this.state.sketchToolParams.textFontId === id) return;
    this.patchToolParams({
      textFontId: id,
      textFontReady: cachedFont(id) !== undefined,
    });
    void this.ensureTextFontLoaded(id);
  }

  /**
   * F24-full: ensure the currently-selected (or given) text font is
   * fetched + parsed into the cache. Idempotent and safe to call on tool
   * activation / font change. Flips `textFontReady` when the load
   * settles. Uploaded fonts are already cached, so this returns fast.
   */
  async ensureTextFontLoaded(id = this.state.sketchToolParams.textFontId): Promise<void> {
    if (cachedFont(id) !== undefined) {
      if (
        this.state.sketchToolParams.textFontId === id &&
        !this.state.sketchToolParams.textFontReady
      ) {
        this.patchToolParams({ textFontReady: true });
      }
      return;
    }
    try {
      await loadBundledFont(id);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: text font load failed", err);
      return;
    }
    // Only flip ready if the user hasn't switched fonts meanwhile.
    if (this.state.sketchToolParams.textFontId === id) {
      this.patchToolParams({ textFontReady: true });
    }
  }

  /**
   * F24-full: parse an uploaded .ttf/.otf ArrayBuffer, register it under a
   * synthetic id and make it the active text font. Returns the new id, or
   * `null` when the buffer isn't a parseable font.
   */
  loadUploadedTextFont(name: string, buffer: ArrayBuffer): string | null {
    let id: string;
    try {
      const font = parseFontBuffer(buffer);
      // wu2v: retain the raw bytes so this font persists into saved
      // `.fdraft` ZIPs (previously the bytes were lost on reload).
      id = registerUploadedFont(name, font, new Uint8Array(buffer.slice(0)));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: uploaded font parse failed", err);
      return null;
    }
    this.patchToolParams({ textFontId: id, textFontReady: true });
    return id;
  }

  /**
   * F24-full: place the current text value as FILLED glyph sketch geometry
   * with its baseline-left at `origin`, using the selected font. Each glyph
   * becomes one `sketch_path` sub-call carrying its outer ring + holes (the
   * kernel fills with even-odd holes), then the sketch is re-solved. No-op
   * when no session is active, the font isn't loaded yet, or the text
   * produces no geometry (e.g. an empty / all-space value).
   */
  placeSketchText(origin: [number, number]): void {
    if (!this.state.sketchSession.active) return;
    const font = cachedFont(this.state.sketchToolParams.textFontId);
    if (!font) {
      // Font not ready — kick a load so a follow-up click works.
      void this.ensureTextFontLoaded();
      return;
    }
    // Plan item 5: tag every glyph with a `text` recipe (shared group id +
    // value/height/fontId/origin) so the placement is re-editable in-scene.
    // The kernel ignores the param and round-trips it (see sketchText.ts).
    const calls = textToSubCalls(font, this.state.sketchToolParams.textValue, {
      origin,
      height: this.state.sketchToolParams.textHeight,
      fontId: this.state.sketchToolParams.textFontId,
    });
    if (calls.length === 0) return;
    this.pushSketchSubCalls(calls);
    void solveAndReplace(this);
  }

  /**
   * Plan item 5: re-edit a previously committed text group in place.
   * `groupId` is the shared `text.group` id of the placement's glyph
   * sub-calls (surfaced by the in-scene text editor reopened over a baked
   * text). Re-derives the glyphs at the new value / height / font via
   * [`reeditTextGroup`] from the loaded font and re-solves, replacing the
   * old group's glyphs in place. Returns false (a graceful no-op) when no
   * session is active, the requested font isn't loaded yet, no glyph
   * carries the group, or the new text produces no geometry.
   *
   * Font availability: re-deriving needs the parsed font in the cache. If
   * it isn't ready we kick a load (same as `placeSketchText`) and bail —
   * the old geometry is kept; the caller can retry once the font settles.
   */
  updateSketchTextGroup(
    groupId: string,
    next: { value: string; height: number; fontId: string },
  ): boolean {
    const session = this.state.sketchSession;
    if (!session.active) return false;
    if (!Number.isFinite(next.height) || next.height <= 0) return false;
    const font = cachedFont(next.fontId);
    if (!font) {
      // Font not ready — kick a load so a follow-up edit works. Keep the
      // old geometry for now.
      void this.ensureTextFontLoaded(next.fontId);
      return false;
    }
    const rewritten = reeditTextGroup(session.subCalls, font, groupId, next);
    if (!rewritten) return false;
    this.replaceSketchSubCalls(rewritten);
    void solveAndReplace(this);
    return true;
  }

  /**
   * Ticket g91j: delete a previously committed text group in place.
   * `groupId` is the shared `text.group` id of the placement's glyph
   * sub-calls (surfaced by the in-scene text editor reopened over a baked
   * text). Drops every glyph carrying the group via [`removeTextGroup`] and
   * re-solves, replacing the spine in place. Returns false (a graceful no-op)
   * when no session is active or no glyph carries the group. Unlike
   * `updateSketchTextGroup` this needs no font — it only removes geometry — so
   * it deletes even while a font is mid-load. Called when the editor commits
   * an empty value over an existing text (edit mode → delete).
   */
  deleteSketchTextGroup(groupId: string): boolean {
    const session = this.state.sketchSession;
    if (!session.active) return false;
    const rewritten = removeTextGroup(session.subCalls, groupId);
    if (!rewritten) return false;
    this.replaceSketchSubCalls(rewritten);
    void solveAndReplace(this);
    return true;
  }

  /** F16: set the number of pattern instances (original + copies). Must be
   *  an integer ≥ 1; no-op for anything else. A value < 2 makes the
   *  pattern actions no-ops (no copies). */
  setSketchPatternCount(count: number): void {
    if (!Number.isFinite(count) || count < 1) return;
    this.patchToolParams({ patternCount: Math.round(count) });
  }

  /** F16: set the per-step spacing (mm) for a linear pattern. May be
   *  negative (steps the opposite way); no-op for non-finite values. */
  setSketchPatternSpacing(spacing: number): void {
    if (!Number.isFinite(spacing)) return;
    this.patchToolParams({ patternSpacing: spacing });
  }

  /** F16: set the total span (degrees) for a circular pattern. No-op for
   *  non-finite values. */
  setSketchPatternAngle(angleDeg: number): void {
    if (!Number.isFinite(angleDeg)) return;
    this.patchToolParams({ patternAngle: angleDeg });
  }

  /**
   * F16: linear pattern of the current selection along `axis`.
   *
   * Maps the entity-id selection back to its simple-geometry sub-calls
   * (line / circle / arc, i.e. `curveOfSubCall` non-null), produces the
   * `count − 1` translated copies of each via [`patternLinear`] (per-step
   * spacing `(spacing, 0)` for X, `(0, spacing)` for Y), and appends them
   * as ordinary `sketch_line` / `sketch_circle` / `sketch_arc` sub-calls.
   * Each copy carries its source's construction flag. The originals stay
   * in place; the sketch is re-solved and the selection is kept.
   *
   * No-op when the selection is empty, nothing simple maps, or
   * `count < 2`.
   */
  linearPatternSketch(axis: "x" | "y"): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (session.selection.length === 0) return;

    const count = this.state.sketchToolParams.patternCount;
    const spacing = this.state.sketchToolParams.patternSpacing;
    const dx = axis === "x" ? spacing : 0;
    const dy = axis === "x" ? 0 : spacing;

    const sources = subCallsForSelection(
      session.subCalls,
      session.selection,
    ).filter((i) => curveOfSubCall(session.subCalls[i]) !== null);
    if (sources.length === 0) return;

    const carry = (params: Record<string, unknown>): Record<string, unknown> =>
      params.construction !== undefined
        ? { construction: params.construction }
        : {};

    const copies: Array<{ type: string; params: Record<string, unknown> }> = [];
    for (const i of sources) {
      const call = session.subCalls[i];
      const cv = curveOfSubCall(call);
      if (!cv) continue;
      for (const out of patternLinear([cv], count, dx, dy)) {
        copies.push(subCallOfCurve(out, carry(call.params)));
      }
    }
    if (copies.length === 0) return;

    this.pushSketchSubCalls(copies);
    void solveAndReplace(this);
  }

  /**
   * F16: circular pattern of the current selection about the sketch
   * origin `[0, 0]`, spanning `sketchToolParams.patternAngle` degrees.
   *
   * Maps the entity-id selection back to its simple-geometry sub-calls and
   * produces the `count − 1` rotated copies of each via
   * [`patternCircular`] — see that helper for the full-vs-partial step
   * rule (full revolution → 2π/count so copies don't overlap the
   * original; partial → totalAngle/(count−1), inclusive span). Each copy
   * is appended as an ordinary `sketch_line` / `sketch_circle` /
   * `sketch_arc` carrying its source's construction flag. The originals
   * stay in place; the sketch is re-solved and the selection is kept.
   *
   * No-op when the selection is empty, nothing simple maps, or
   * `count < 2`.
   */
  circularPatternSketch(): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (session.selection.length === 0) return;

    const count = this.state.sketchToolParams.patternCount;
    const angleRad = (this.state.sketchToolParams.patternAngle * Math.PI) / 180;

    const sources = subCallsForSelection(
      session.subCalls,
      session.selection,
    ).filter((i) => curveOfSubCall(session.subCalls[i]) !== null);
    if (sources.length === 0) return;

    const carry = (params: Record<string, unknown>): Record<string, unknown> =>
      params.construction !== undefined
        ? { construction: params.construction }
        : {};

    const copies: Array<{ type: string; params: Record<string, unknown> }> = [];
    for (const i of sources) {
      const call = session.subCalls[i];
      const cv = curveOfSubCall(call);
      if (!cv) continue;
      for (const out of patternCircular([cv], count, [0, 0], angleRad)) {
        copies.push(subCallOfCurve(out, carry(call.params)));
      }
    }
    if (copies.length === 0) return;

    this.pushSketchSubCalls(copies);
    void solveAndReplace(this);
  }

  /** F17: set the rotation angle (degrees) the `rotateSketchSelection`
   *  transform applies. No-op for non-finite values. */
  setSketchRotateAngle(angleDeg: number): void {
    if (!Number.isFinite(angleDeg)) return;
    this.patchToolParams({ rotateAngle: angleDeg });
  }

  /**
   * F17: translate the current selection by `(dx, dy)`.
   *
   * Maps the entity-id selection back to its simple-geometry sub-calls
   * (line / circle / arc, i.e. `curveOfSubCall` non-null) and translates
   * each via [`translateCurve`].
   *
   *  - `copy === false` (Move): each selected geometry sub-call is replaced
   *    IN PLACE by its translated version. A translation never changes a
   *    sub-call's entity COUNT (a line stays a line, a circle a circle), so
   *    the replacement allocates exactly as many ids as it freed — all
   *    downstream entity ids stay put and no constraint refs shift. The
   *    target indices are SNAPSHOTTED first and the replacements applied
   *    highest-index-first so each index stays valid across the loop. The
   *    selection is preserved (the same entity ids still own the moved
   *    geometry).
   *  - `copy === true` (Copy): the originals stay; one translated copy of
   *    each selected geometry sub-call is appended as an ordinary
   *    `sketch_line` / `sketch_circle` / `sketch_arc` carrying its source's
   *    construction flag. The selection is preserved (still points at the
   *    originals).
   *
   * Re-solves afterwards. No-op when the session is inactive, the
   * selection is empty, or nothing simple maps.
   */
  moveSketchSelection(dx: number, dy: number, copy: boolean): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (session.selection.length === 0) return;
    this.translateSketchSubCalls(
      subCallsForSelection(session.subCalls, session.selection),
      dx,
      dy,
      copy,
      session.selection,
    );
  }

  /**
   * Ticket matf: rigidly translate the geometry of an explicit set of
   * committed ENTITY ids by `(dx, dy)`. Backs the Select-mode click-drag (a
   * single entity body, or a whole multi-selection grabbed and dragged). The
   * dragged ids map back to their owning geometry sub-calls and translate in
   * place; the solver then settles every constraint (a move forbidden by a
   * constraint is pulled back). No-op when nothing translatable maps.
   */
  translateSketchEntities(
    entityIds: ReadonlyArray<number>,
    dx: number,
    dy: number,
  ): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (entityIds.length === 0) return;
    this.translateSketchSubCalls(
      subCallsForSelection(session.subCalls, entityIds),
      dx,
      dy,
      false,
      entityIds,
    );
  }

  /**
   * Shared core for Move / Copy / Select-drag translation. Translates every
   * TRANSLATABLE geometry sub-call in `sourceIndices` by `(dx, dy)` — lines,
   * circles, arcs, rectangles, polygons, ellipses, splines, paths and free
   * points all move as one rigid body (ticket matf widened this past the
   * line/circle/arc-only `curveOfSubCall` path). `copy` appends translated
   * copies instead of moving in place. `restoreSelection` is re-applied after
   * an in-place move (an equal-count replace keeps entity ids stable).
   */
  private translateSketchSubCalls(
    sourceIndices: ReadonlyArray<number>,
    dx: number,
    dy: number,
    copy: boolean,
    restoreSelection: ReadonlyArray<number>,
  ): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const sources = sourceIndices.filter((i) =>
      isTranslatableSubCall(session.subCalls[i]),
    );
    if (sources.length === 0) return;

    if (copy) {
      const copies: Array<{ type: string; params: Record<string, unknown> }> =
        [];
      for (const i of sources) {
        copies.push(translateSubCall(session.subCalls[i], dx, dy));
      }
      if (copies.length === 0) return;
      this.pushSketchSubCalls(copies);
      void solveAndReplace(this);
      return;
    }

    // In-place: replace each selected geometry sub-call with its translated
    // self. A translation preserves each call's entity count, so ids stay
    // stable. Highest index first keeps the lower indices valid across each
    // rewrite (replaceSketchSubCallAt clears the selection; restored below).
    let next = session;
    for (const i of [...sources].sort((a, b) => b - a)) {
      next = reduceReplaceAt(next, i, [
        translateSubCall(session.subCalls[i], dx, dy),
      ]);
    }
    // Equal-count replace keeps entity ids, so the original selection still
    // points at the moved geometry — restore it.
    this.updateSketch(reduceSetSelection(next, [...restoreSelection]));
    void solveAndReplace(this);
  }

  /**
   * F17: rotate the current selection about the sketch origin `[0, 0]` by
   * `angleDeg` degrees (CCW). Same selection → simple-geometry mapping and
   * id-stability reasoning as [`moveSketchSelection`]: a rotation preserves
   * each sub-call's entity count, so an in-place replace leaves all
   * downstream ids and constraint refs intact.
   *
   *  - `copy === false`: replace each selected geometry sub-call in place
   *    by its rotated version (selection preserved).
   *  - `copy === true`: append one rotated copy of each (originals kept).
   *
   * Re-solves afterwards. No-op when the session is inactive, the
   * selection is empty, or nothing simple maps.
   */
  rotateSketchSelection(angleDeg: number, copy: boolean): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    if (session.selection.length === 0) return;

    const angleRad = (angleDeg * Math.PI) / 180;
    const sources = subCallsForSelection(
      session.subCalls,
      session.selection,
    ).filter((i) => curveOfSubCall(session.subCalls[i]) !== null);
    if (sources.length === 0) return;

    const carry = (params: Record<string, unknown>): Record<string, unknown> =>
      params.construction !== undefined
        ? { construction: params.construction }
        : {};

    if (copy) {
      const copies: Array<{ type: string; params: Record<string, unknown> }> =
        [];
      for (const i of sources) {
        const call = session.subCalls[i];
        const cv = curveOfSubCall(call);
        if (!cv) continue;
        copies.push(
          subCallOfCurve(rotateCurve(cv, [0, 0], angleRad), carry(call.params)),
        );
      }
      if (copies.length === 0) return;
      this.pushSketchSubCalls(copies);
      void solveAndReplace(this);
      return;
    }

    let next = session;
    for (const i of [...sources].sort((a, b) => b - a)) {
      const call = session.subCalls[i];
      const cv = curveOfSubCall(call);
      if (!cv) continue;
      next = reduceReplaceAt(next, i, [
        subCallOfCurve(rotateCurve(cv, [0, 0], angleRad), carry(call.params)),
      ]);
    }
    this.updateSketch(reduceSetSelection(next, session.selection));
    void solveAndReplace(this);
  }

  /** Select mode: toggle a committed-entity id in the sketch
   *  selection set. Ids are resolved against
   *  `buildDraftSketchState(subCalls)` by the canvas. */
  toggleSketchSelection(entityId: number): void {
    this.updateSketch(
      reduceToggleSelection(this.state.sketchSession, entityId),
    );
  }

  /** Replace the sketch selection wholesale. */
  setSketchSelection(ids: ReadonlyArray<number>): void {
    this.updateSketch(reduceSetSelection(this.state.sketchSession, ids));
  }

  /** Clear the sketch selection. */
  clearSketchSelection(): void {
    this.updateSketch(reduceClearSelection(this.state.sketchSession));
  }

  /**
   * P0.1: apply an explicit geometric constraint to the current
   * sketch selection. Maps the selected entity ids onto the
   * constraint's kernel param names, appends the `sketch_constraint_*`
   * sub-call, clears the selection and re-solves so the geometry
   * snaps to satisfy the new relation. No-op when the selection
   * doesn't match the constraint (the palette only offers matching
   * ones, but the guard keeps the call safe).
   */
  applySketchConstraint(type: string): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const state = buildDraftSketchState(session.subCalls);
    const call = buildConstraintCall(type, session.selection, state);
    if (!call) return;
    logUserAction("sketch", "constraint-apply", {
      type,
      entities: session.selection.length,
    });
    this.pushSketchSubCalls([call]);
    this.clearSketchSelection();
    void solveAndReplace(this);
  }

  /**
   * zz1r: enter a constraint TOOL (the tool-first constrain workflow).
   * The constraint buttons are sketch tools now: pick the tool, then
   * click the entities one after another and the constraint applies the
   * moment the running selection satisfies it. Clicking the active
   * constraint tool again toggles back to plain select mode. The current
   * selection is kept on entry so a selection-first pick (select the
   * lines, then click the constraint) still applies at once.
   */
  selectSketchConstraintTool(type: string): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    logUserAction("sketch", "constraint-tool", { type });
    if (session.selectedTool === type) {
      this.selectSketchTool(null);
      return;
    }
    // Keep the current selection (unlike a drawing tool) so an existing
    // pick can apply immediately and partial picks accumulate.
    this.updateSketch({ ...session, selectedTool: type, toolDraft: {} });
    this.tryApplyConstraintTool();
  }

  /**
   * zz1r: route a committed-entity pick while a constraint tool is active
   * — toggle it into the running selection, then apply the constraint the
   * instant the selection satisfies it (tool stays active for the next
   * application).
   */
  pickSketchConstraintEntity(entityId: number): void {
    const session = this.state.sketchSession;
    if (!session.active || !isConstraintTool(session.selectedTool)) return;
    this.toggleSketchSelection(entityId);
    this.tryApplyConstraintTool();
  }

  /**
   * zz1r: apply the active constraint tool if the running selection now
   * satisfies it. The Dimension tool resolves the most-specific
   * dimensional type that fits; the others apply their own type. No-op
   * until the selection composition matches (so partial picks accumulate).
   */
  private tryApplyConstraintTool(): void {
    const session = this.state.sketchSession;
    const tool = session.selectedTool;
    if (!session.active || !isConstraintTool(tool) || tool === null) return;
    if (session.selection.length === 0) return;
    const state = buildDraftSketchState(session.subCalls);
    const options = applicableConstraints(state, session.selection);
    // uf62: the Coincident tool, like Dimension, is a type-dependent
    // dispatcher — it resolves the fitting coincidence constraint
    // (coincident / point-on-object / collinear / concentric) from the
    // two picked entities rather than emitting a fixed type.
    const resolved =
      tool === CONSTRAINT_DIMENSION_TOOL
        ? (DIMENSION_TYPES_BY_SPECIFICITY.find((t) =>
            options.some((o) => o.type === t),
          ) ?? null)
        : tool === CONSTRAINT_COINCIDENT_TOOL
          ? resolveCoincidentConstraint(state, session.selection)
          : options.some((o) => o.type === tool)
            ? tool
            : null;
    if (!resolved) return;
    // Ticket 32az: for a BINARY constraint applied via the tool, keep the
    // FIRST-picked entity as a persistent anchor. The user can then keep
    // clicking entities and they all chain into the constraint — every line
    // parallel to the reference, every point coincident with it, etc. — instead
    // of having to re-pick in pairs (which left consecutive lines unrelated).
    // Dimensional tools open a value editor on apply, so they don't chain.
    const anchor =
      session.selection.length === 2 && tool !== CONSTRAINT_DIMENSION_TOOL
        ? session.selection[0]
        : null;
    this.applySketchConstraint(resolved);
    if (anchor !== null) this.toggleSketchSelection(anchor);
  }

  toggleConstraintDriven(index: number): void {
    const session = this.state.sketchSession;
    const call = session.subCalls[index];
    if (!call) return;
    const isDimensional =
      call.type === "sketch_constraint_distance" ||
      call.type === "sketch_constraint_angle";
    if (!isDimensional) return;
    const next = [...session.subCalls];
    next[index] = {
      ...call,
      params: { ...call.params, driven: !call.params.driven },
    };
    this.updateSketch({ ...session, subCalls: next });
  }

  /**
   * P0.2: change the numeric value of a dimensional constraint
   * (distance/angle) sub-call, then re-solve so the geometry snaps
   * to the new measurement. The param key depends on the kind:
   * `value_mm` for distance, `value_deg` for angle. No-op for
   * non-dimensional calls or a non-finite value.
   */
  setSketchConstraintValue(index: number, value: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const call = session.subCalls[index];
    if (!call || !Number.isFinite(value)) return;
    const key =
      call.type === "sketch_constraint_distance" ||
      call.type === "sketch_constraint_radius" ||
      call.type === "sketch_constraint_diameter"
        ? "value_mm"
        : call.type === "sketch_constraint_angle"
          ? "value_deg"
          : null;
    if (key === null) return;
    if (call.params[key] === value) return;
    const next = [...session.subCalls];
    next[index] = { ...call, params: { ...call.params, [key]: value } };
    this.updateSketch({ ...session, subCalls: next });
    void solveAndReplace(this);
  }

  /**
   * Toggle the construction flag on a drawing sub-call. Construction
   * geometry still solves and can be referenced by constraints but is
   * dropped from the extrudable region (kernel `build_sketch2d`) and
   * drawn dashed. Constraint sub-calls have no construction state, so
   * the toggle is a no-op on them. Re-solves because the region — and
   * thus the DoF colour — changes.
   */
  toggleSketchConstructionAt(index: number): void {
    const session = this.state.sketchSession;
    if (!session.active) return;
    const call = session.subCalls[index];
    if (!call || call.type.startsWith("sketch_constraint")) return;
    const wasConstruction = Boolean(call.params.construction);
    const next = [...session.subCalls];
    next[index] = {
      ...call,
      params: { ...call.params, construction: !wasConstruction },
    };
    this.updateSketch({ ...session, subCalls: next });
    void solveAndReplace(this);
  }

  /**
   * K6b.3a: commit the current sketch session to the kernel as
   * a real `sketch` FeatureCall. Called from the "Finish"
   * button. Pushes the session's accumulated sub_calls through
   * `kernel.addSketch`, lets the kernel regenerate, then exits
   * sketch mode. If the session has no sub_calls yet, or no
   * plane is set, the session is simply dropped — same as a
   * cancel.
   */
  commitSketch(): number | null {
    logUserAction("sketch", "commit", {
      subCalls: this.state.sketchSession.subCalls.length,
    });
    const session = this.state.sketchSession;
    if (!session.active || session.planeQuery === null) {
      this.exitSketch();
      return null;
    }
    // Edit reentry path: if a sketch feature id is attached to
    // the session we always replace in place — even when the
    // sub-spine is empty — so the user can intentionally clear
    // the sketch by opening it and hitting Finish with no
    // further edits.
    if (session.editingFeatureId !== null) {
      try {
        this.kernel.replaceSketch(
          session.editingFeatureId,
          session.planeQuery,
          session.subCalls.map((c) => ({ type: c.type, params: c.params })),
        );
      } catch (err) {
        // eslint-disable-next-line no-console
        console.error("fielddraft: replaceSketch failed", err);
        return null;
      }
      const editedId = session.editingFeatureId;
      this.updateSketch(reduceExit(this.state.sketchSession));
      this.core.commit(new Set([editedId]), editedId);
      return editedId;
    }
    // Fresh sketch draft: drop on cancel / finish-with-empty.
    if (session.subCalls.length === 0) {
      this.exitSketch();
      this.core.setTransientError(
        "Sketch discarded — add geometry before finishing.",
      );
      return null;
    }
    let id: number | null = null;
    try {
      id = this.kernel.addSketch(
        session.planeQuery,
        session.subCalls.map((c) => ({ type: c.type, params: c.params })),
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addSketch failed", err);
      this.core.setTransientError(formatKernelError(err, "Sketch commit failed"));
      this.exitSketch();
      return null;
    }
    // Commit goes through the standard store `commit` path so
    // features, selection, meshes and autosave all refresh.
    this.updateSketch(reduceExit(this.state.sketchSession));
    this.core.commit(id !== null ? new Set([id]) : undefined, id);
    return id;
  }
}
