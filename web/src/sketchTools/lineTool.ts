/**
 * Line-tool controller (architecture review, Befund 2) — the drag-to-draw
 * line, the F18 click-started PENDING line that rubber-bands to the cursor
 * (second click / Enter commits, Esc cancels), the daa8 polyline chaining,
 * and the click-to-edit dimension readout on the most recently placed line.
 * Behaviour is a 1:1 translation of the old `SketchCanvas2d` line branches.
 */

import { sketchToScreen } from "../sketchCameraBridge";
import {
  buildDraftSketchState,
  predictNextLineIds,
} from "../sketchSession";
import { measuredLength } from "../sketchLiveInput";
import {
  anchorConstraints,
  classifyLineAnchor,
} from "../sketchSnapResolve";
import {
  DIM_LABEL_HIT_PX,
  distanceLabel,
  distanceLabelAnchorPx,
} from "../sketchCanvasHud";
import type {
  SketchSubCall,
  SketchToolController,
  ToolContext,
  ToolHud,
} from "./toolTypes";

type Pt = [number, number];

interface LineDrag {
  /** `drag` = button held (down → up commits); `pending` = F18 click-started
   *  rubber-band line (second click / Enter commits, Esc cancels). */
  phase: "drag" | "pending";
  start: Pt;
  current: Pt;
  /** Entity id the start snapped to at pointerdown time, if the snap was a
   *  coincident-to-point hit. Used by the commit path to emit a matching
   *  coincident constraint. */
  startAnchorId: number | null;
}

/**
 * F18: commit the in-flight pending line as a free line. Returns true
 * when something was committed. The placed line becomes the
 * click-to-edit dimension target; chaining (ticket daa8) starts a NEW
 * rubber-band line from the just-placed endpoint.
 */
function commitPendingLine(ctx: ToolContext, drag: LineDrag): boolean {
  const { start, current, startAnchorId } = drag;
  // Reject a degenerate zero-length line.
  if (measuredLength(start, current) < 1e-3) return false;
  const ids = predictNextLineIds(ctx.session.subCalls);
  const state = buildDraftSketchState(ctx.session.subCalls);
  const calls: SketchSubCall[] = [
    {
      type: "sketch_line",
      params: {
        start: [start[0], start[1]] as Pt,
        end: [current[0], current[1]] as Pt,
      },
    },
  ];
  // Start: an explicit coincident captured at pointerdown (the chain / a
  // point snap) wins; otherwise classify where it landed (ticket inej).
  if (startAnchorId !== null) {
    calls.push({
      type: "sketch_constraint_coincident",
      params: { a: ids.start, b: startAnchorId },
    });
  } else {
    calls.push(
      ...anchorConstraints(ids.start, classifyLineAnchor(start, state)),
    );
  }
  // End: bind it to whatever it snapped onto — a vertex (coincident, e.g.
  // closing a loop) or a line interior (point-on-object). Without this the
  // endpoint drifts off the target when the sketch is later dimensioned.
  calls.push(...anchorConstraints(ids.end, classifyLineAnchor(current, state)));
  ctx.ui.setRecentLine(ctx.session.subCalls.length);
  ctx.store.pushSketchSubCalls(calls);
  // Ticket daa8: chain — instead of ending, start a NEW rubber-band line from
  // the just-placed endpoint, coincident with it, so clicking repeatedly draws
  // a connected polyline (Esc / tool switch / re-clicking Line ends it via
  // onDeactivate + the Escape handling in onKey).
  ctx.setDrag(lineTool, {
    phase: "pending",
    start: [current[0], current[1]],
    current: [current[0], current[1]],
    startAnchorId: ids.end,
  } satisfies LineDrag);
  ctx.requestRedraw();
  ctx.scheduleSolverReplay();
  return true;
}

export const lineTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    const snap = ev.snap!;
    const [su, sv] = snap.snapped;
    const drag = ctx.dragOf<LineDrag>(lineTool);
    // F18: a second click while a pending (click-started) line is in
    // flight commits it as a free line to the click point.
    if (drag?.phase === "pending") {
      const pending: LineDrag = { ...drag, current: [su, sv] };
      if (!commitPendingLine(ctx, pending)) {
        // Degenerate (clicked the start again): keep the pending line
        // alive rather than dropping it silently.
        ctx.setDrag(lineTool, pending);
      }
      ctx.requestRedraw();
      return;
    }
    // Click on the recent line's dimension readout → open the editor
    // instead of starting a new line. A click anywhere else while the
    // editor is open only dismisses it.
    if (ctx.ui.dimEditorOpen()) {
      ctx.ui.dismissDimEditor();
      ctx.requestRedraw();
      return;
    }
    const recent = ctx.ui.recentLineGeometry();
    if (recent !== null) {
      const anchor = distanceLabelAnchorPx(recent.start, recent.end, ev.cam);
      if (
        anchor !== null &&
        Math.hypot(ev.clickPx[0] - anchor[0], ev.clickPx[1] - anchor[1]) <=
          DIM_LABEL_HIT_PX
      ) {
        ctx.ui.openLineLengthEditor(
          ctx.ui.recentLine()!,
          anchor,
          measuredLength(recent.start, recent.end),
        );
        return;
      }
    }
    // Starting a new line retires the previous line's readout.
    ctx.ui.setRecentLine(null);
    ctx.setDrag(lineTool, {
      phase: "drag",
      start: [su, sv],
      current: [su, sv],
      startAnchorId: snap.coincidentTargetId,
    } satisfies LineDrag);
    ctx.requestRedraw();
  },
  onMove(ctx, ev) {
    // Rubber-band both phases to the (snapped) cursor — the drag with the
    // button held and the F18 pending line with no button held.
    const drag = ctx.dragOf<LineDrag>(lineTool);
    if (!drag) return false;
    const snap = ctx.resolveSnapAt(ev, ev.gridded, drag.start);
    ctx.setSnap(snap);
    ctx.setDrag(lineTool, { ...drag, current: snap.snapped });
    return true;
  },
  onUp(ctx, ev) {
    const drag = ctx.dragOf<LineDrag>(lineTool);
    // The pending line ignores pointer-up: it commits on the SECOND click
    // (onDown) or Enter, exactly like the old `kind !== "line"` guard.
    if (!drag || drag.phase !== "drag") return;
    ctx.releasePointer(ev);
    const snap = ctx.resolveSnapAt(ev, ev.raw, drag.start);
    ctx.setSnap(snap);
    const { start, startAnchorId } = drag;
    ctx.clearDrag();
    const [su, sv] = snap.snapped;
    // F18: a click-without-drag (the pointer barely moved between down and
    // up) starts a PENDING line that rubber-bands to the cursor; a second
    // click commits, Esc cancels. A real drag (moved past the pixel
    // threshold) commits a free line below, exactly as before.
    const [sx0, sy0] = sketchToScreen(start[0], start[1], ev.cam);
    const [sx1, sy1] = sketchToScreen(su, sv, ev.cam);
    const movedPx = Math.hypot(sx1 - sx0, sy1 - sy0);
    if (movedPx < 4) {
      ctx.setDrag(lineTool, {
        phase: "pending",
        start: [start[0], start[1]],
        current: [start[0], start[1]],
        startAnchorId,
      } satisfies LineDrag);
      ctx.requestRedraw();
      return;
    }

    // K6b.3d: emit the sketch_line call plus any constraint
    // calls that materialise from the snap candidate at the
    // END of the drag (horizontal / vertical / endpoint) and
    // from the anchor match at the START of the drag (another
    // endpoint coincident hit). Entity ids are predicted by
    // replicating the kernel's allocation order so the
    // constraint calls reference the correct line/point.
    const predictedIds = predictNextLineIds(ctx.session.subCalls);
    const priorState = buildDraftSketchState(ctx.session.subCalls);
    const newCalls: SketchSubCall[] = [
      {
        type: "sketch_line",
        params: {
          start: [start[0], start[1]] as Pt,
          end: [su, sv] as Pt,
        },
      },
    ];

    // Start: an explicit coincident captured at pointerdown wins; otherwise
    // classify where the start landed (vertex → coincident, line interior →
    // point-on-object) so it stays put under later edits (ticket inej).
    if (startAnchorId !== null) {
      newCalls.push({
        type: "sketch_constraint_coincident",
        params: { a: predictedIds.start, b: startAnchorId },
      });
    } else {
      newCalls.push(
        ...anchorConstraints(
          predictedIds.start,
          classifyLineAnchor(start, priorState),
        ),
      );
    }

    // End snap: orientation (horizontal / vertical) plus a binding. The live
    // candidate covers H/V and an inferred vertex coincident; anything else
    // (a line-interior snap, or a snap the inference missed) is bound by
    // classifying the landed position — ticket inej.
    let endBound = false;
    if (snap.candidate) {
      const c = snap.candidate.constraint;
      if (c.kind === "horizontal") {
        newCalls.push({
          type: "sketch_constraint_horizontal",
          params: { line: predictedIds.line },
        });
      } else if (c.kind === "vertical") {
        newCalls.push({
          type: "sketch_constraint_vertical",
          params: { line: predictedIds.line },
        });
      } else if (c.kind === "coincident" && snap.coincidentTargetId !== null) {
        newCalls.push({
          type: "sketch_constraint_coincident",
          params: { a: predictedIds.end, b: snap.coincidentTargetId },
        });
        endBound = true;
      }
    }
    if (!endBound) {
      newCalls.push(
        ...anchorConstraints(
          predictedIds.end,
          classifyLineAnchor([su, sv], priorState),
        ),
      );
    }

    // The placed line becomes the click-to-edit dimension target.
    ctx.ui.setRecentLine(ctx.session.subCalls.length);
    ctx.store.pushSketchSubCalls(newCalls);
    ctx.scheduleSolverReplay();
  },
  onKey(ctx, e) {
    // F18: Esc cancels an in-flight pending line (an open value editor was
    // already handled by the canvas before this dispatch).
    if (e.key !== "Escape") return;
    const drag = ctx.dragOf<LineDrag>(lineTool);
    if (drag?.phase === "pending") {
      e.preventDefault();
      ctx.clearDrag();
      ctx.requestRedraw();
    }
  },
  onDeactivate(ctx) {
    // F18: drop an in-flight pending line when leaving the tool. A
    // button-held drag survives the (unreachable mid-gesture) tool change,
    // exactly like the old effect.
    const drag = ctx.dragOf<LineDrag>(lineTool);
    if (drag?.phase === "pending") {
      ctx.clearDrag();
      ctx.requestRedraw();
    }
  },
  hud(drag, _session, cam) {
    const d = drag as LineDrag;
    const hud: ToolHud = {
      previewCalls: [
        { type: "sketch_line", params: { start: d.start, end: d.current } },
      ],
    };
    const l = distanceLabel(d.start, d.current, cam);
    if (l) hud.labels = [l];
    return hud;
  },
};
