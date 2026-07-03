/**
 * Spline-tool controller (architecture review, Befund 2) — F23 multi-click
 * control-point placement with a live Catmull-Rom / B-spline preview.
 * Finish gestures: click near the FIRST point closes, double-click or a
 * click near the LAST point finishes open, Enter/Escape finish open
 * (ticket egia), leaving the tool commits the placed points (onDeactivate).
 */

import { sketchToScreen } from "../sketchCameraBridge";
import { bsplinePoints, catmullRomPoints, type Vec2 } from "../sketchSpline";
import type {
  SketchToolController,
  ToolContext,
  ToolHud,
} from "./toolTypes";

type Pt = [number, number];

interface SplineDrag {
  points: Pt[];
  current: Pt;
  /** F23-full: `interpolate` (Catmull-Rom) or `control` (cubic B-spline
   *  control polygon). Snapshotted when the gesture starts so the live
   *  preview + commit stay consistent. */
  mode: "interpolate" | "control";
}

function commitSpline(
  ctx: ToolContext,
  points: Pt[],
  closed: boolean,
  mode: SplineDrag["mode"],
): void {
  ctx.store.pushSketchSubCalls([
    {
      type: "sketch_spline",
      params: { points: points.flat(), closed, mode },
    },
  ]);
  ctx.scheduleSolverReplay();
  ctx.clearDrag();
  ctx.requestRedraw();
}

export const splineTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    const [su, sv] = ev.snap!.snapped;
    // Multi-click control-point placement (mirrors the polygon
    // gesture). Finish gestures:
    //   • click within 8 px of the FIRST point (≥ 3 pts) → CLOSE
    //   • double-click, or click within 8 px of the LAST point
    //     (≥ 2 pts) → finish OPEN
    const drag = ctx.dragOf<SplineDrag>(splineTool);
    const existing = drag ? drag.points : [];
    // Mode is locked in when the gesture begins (first click); a
    // mid-gesture toggle would desync the preview, so reuse the
    // in-flight drag's mode if one exists.
    const splineMode = drag
      ? drag.mode
      : ctx.store.getState().sketchToolParams.splineMode;
    const [cpx, cpy] = sketchToScreen(su, sv, ev.cam);
    // Close on first point.
    if (existing.length >= 3) {
      const [fpx, fpy] = sketchToScreen(existing[0][0], existing[0][1], ev.cam);
      if (Math.hypot(cpx - fpx, cpy - fpy) < 8) {
        commitSpline(ctx, existing, true, splineMode);
        return;
      }
    }
    // Finish open: double-click, or click near the last point.
    if (existing.length >= 2) {
      const last = existing[existing.length - 1];
      const [lpx, lpy] = sketchToScreen(last[0], last[1], ev.cam);
      if (ev.e.detail >= 2 || Math.hypot(cpx - lpx, cpy - lpy) < 8) {
        commitSpline(ctx, existing, false, splineMode);
        return;
      }
    }
    const points: Pt[] = [...existing, [su, sv]];
    ctx.setDrag(splineTool, {
      points,
      current: [su, sv],
      mode: splineMode,
    } satisfies SplineDrag);
    ctx.requestRedraw();
  },
  onMove(ctx, ev) {
    // Ticket 8vpe: rubber-band the spline preview to the cursor on EVERY
    // move — including while dragging (button held) after the first point —
    // so the curve continuously follows the pointer, not only between clicks.
    const drag = ctx.dragOf<SplineDrag>(splineTool);
    if (!drag) return false;
    const snap = ctx.resolveSnapAt(ev, ev.gridded, null);
    ctx.setSnap(snap);
    ctx.setDrag(splineTool, { ...drag, current: snap.snapped });
    return true;
  },
  onKey(ctx, e) {
    // F23: keyboard finish for the spline gesture. Ticket egia: BOTH Enter
    // and Escape FINISH the spline — they commit the control points placed
    // so far as an open spline instead of discarding them. Only an Escape
    // with too few points (< 2) cancels outright.
    const drag = ctx.dragOf<SplineDrag>(splineTool);
    if (!drag) return;
    if ((e.key === "Enter" || e.key === "Escape") && drag.points.length >= 2) {
      e.preventDefault();
      commitSpline(ctx, drag.points, false, drag.mode);
    } else if (e.key === "Escape") {
      e.preventDefault();
      ctx.clearDrag();
      ctx.requestRedraw();
    }
  },
  onDeactivate(ctx) {
    // Ticket egia: leaving the Spline tool (switching tool, Esc-to-select)
    // FINISHES the in-progress spline — its placed control points commit as
    // an open spline instead of vanishing. < 2 points has nothing to commit.
    const drag = ctx.dragOf<SplineDrag>(splineTool);
    if (drag && drag.points.length >= 2) {
      commitSpline(ctx, drag.points, false, drag.mode);
    }
  },
  hud(drag) {
    const { points, current, mode } = drag as SplineDrag;
    const hud: ToolHud = {};
    if (points.length > 0) {
      const ctrl: Vec2[] = [...points, current];
      const isControl = mode === "control";
      if (isControl) {
        const guides: Array<[Pt, Pt]> = [];
        for (let i = 0; i + 1 < ctrl.length; i++) {
          guides.push([ctrl[i], ctrl[i + 1]]);
        }
        hud.previewGuides = guides;
      }
      const sampled = isControl
        ? bsplinePoints(ctrl, 3, false, 48)
        : catmullRomPoints(ctrl, false, 16);
      const lines: Array<[Pt, Pt]> = [];
      for (let i = 0; i + 1 < sampled.length; i++) {
        lines.push([sampled[i], sampled[i + 1]]);
      }
      hud.previewLines = lines;
      hud.previewPoints = [...points];
    }
    return hud;
  },
};
