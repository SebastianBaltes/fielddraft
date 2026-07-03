/**
 * Arc-tool controller (architecture review, Befund 2): the centre → start →
 * end three-click gesture. The sweep is unwrapped live (`unwrapSweep`) so
 * arcs past 180° stay reachable. 1:1 translation of the old canvas branches.
 */

import { angleAbout, buildArcSubCall, unwrapSweep } from "../sketchArc";
import { noSnap } from "../sketchSnapResolve";
import { radiusLabel } from "../sketchCanvasHud";
import type { SketchToolController, ToolHud } from "./toolTypes";

type Pt = [number, number];

interface ArcDrag {
  center: Pt;
  /** null until the 2nd click fixes the radius + start angle. */
  start: Pt | null;
  current: Pt;
  startAngle: number;
  sweep: number;
}

export const arcTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    const [su, sv] = ev.snap!.snapped;
    const drag = ctx.dragOf<ArcDrag>(arcTool);
    if (!drag) {
      // Click 1 — place the centre.
      ctx.setDrag(arcTool, {
        center: [su, sv],
        start: null,
        current: [su, sv],
        startAngle: 0,
        sweep: 0,
      } satisfies ArcDrag);
    } else if (drag.start === null) {
      // Click 2 — fix radius + start angle.
      ctx.setDrag(arcTool, {
        ...drag,
        start: [su, sv],
        current: [su, sv],
        startAngle: angleAbout(drag.center, [su, sv]),
        sweep: 0,
      });
    } else {
      // Click 3 — fix the sweep and commit (or discard if degenerate).
      const call = buildArcSubCall(
        drag.center,
        drag.start,
        [su, sv],
        drag.sweep,
      );
      ctx.clearDrag();
      if (call) {
        ctx.store.pushSketchSubCalls([call]);
        ctx.scheduleSolverReplay();
      }
    }
    ctx.requestRedraw();
  },
  onMove(ctx, ev) {
    const drag = ctx.dragOf<ArcDrag>(arcTool);
    if (!drag) return false;
    if (drag.start === null) {
      // Stage 1: rubber-band the radius/start point.
      ctx.setDrag(arcTool, { ...drag, current: ev.gridded });
    } else {
      // Stage 2: sweep, unwrapped so it tracks past ±180°.
      const sweep = unwrapSweep(
        drag.startAngle,
        angleAbout(drag.center, ev.gridded),
        drag.sweep,
      );
      ctx.setDrag(arcTool, { ...drag, current: ev.gridded, sweep });
    }
    ctx.setSnap(noSnap(ev.gridded));
    return true;
  },
  onDeactivate(ctx) {
    // Reset the in-progress multi-click draft when the active tool changes,
    // so a stale point can't get committed as the first click of the next
    // gesture.
    ctx.clearDrag();
    ctx.requestRedraw();
  },
  hud(drag) {
    const { center, start, current, startAngle, sweep } = drag as ArcDrag;
    const hud: ToolHud = {};
    let r: number;
    if (start === null) {
      r = Math.hypot(current[0] - center[0], current[1] - center[1]);
      hud.previewCalls = [
        { type: "sketch_circle", params: { center, radius: r } },
      ];
      hud.previewLines = [[center, current]];
      hud.previewPoints = [center];
    } else {
      r = Math.hypot(start[0] - center[0], start[1] - center[1]);
      hud.previewCalls = [
        {
          type: "sketch_arc",
          params: {
            center,
            radius: r,
            start_angle: startAngle,
            sweep_angle: sweep,
          },
        },
      ];
      hud.previewLines = [[center, start]];
      hud.previewPoints = [center, start];
    }
    const l = radiusLabel(center, r);
    if (l) hud.labels = [l];
    return hud;
  },
};
