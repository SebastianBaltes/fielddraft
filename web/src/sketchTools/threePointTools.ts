/**
 * Three-point gesture controllers (architecture review, Befund 2): F7's
 * circle-through-3-points and rotated rectangle-from-3-points. Both are
 * click-click-click flows previewing live between clicks. 1:1 translation
 * of the old `SketchCanvas2d` branches.
 */

import { predictNextLineIds } from "../sketchSession";
import { circleThrough3, rect3pCorners } from "../sketchPrimitiveVariants";
import { radiusLabel } from "../sketchCanvasHud";
import type {
  SketchSubCall,
  SketchToolController,
  ToolHud,
} from "./toolTypes";

type Pt = [number, number];

interface ThreePointDrag {
  p1: Pt;
  /** null until the 2nd click. */
  p2: Pt | null;
  current: Pt;
}

// ─── circle through three points ────────────────────────────────

export const circle3pTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    const [su, sv] = ev.snap!.snapped;
    const drag = ctx.dragOf<ThreePointDrag>(circle3pTool);
    if (!drag) {
      // Click 1 — first point on the circumference.
      ctx.setDrag(circle3pTool, {
        p1: [su, sv],
        p2: null,
        current: [su, sv],
      } satisfies ThreePointDrag);
    } else if (drag.p2 === null) {
      // Click 2 — second point.
      ctx.setDrag(circle3pTool, { ...drag, p2: [su, sv], current: [su, sv] });
    } else {
      // Click 3 — solve the circumcircle and commit (or discard if
      // the three points are collinear).
      const res = circleThrough3(drag.p1, drag.p2, [su, sv]);
      ctx.clearDrag();
      if (res) {
        // Ticket ybnl: keep the three defining points as real entities (so
        // they render as draggable dots) and bind the circle through them
        // with point-on-object constraints, instead of collapsing to a bare
        // centre+radius circle. Dragging any point re-fits the circle.
        // Id allocation order (mirrors buildDraftSketchState): three points,
        // then the circle's centre + entity.
        const base = predictNextLineIds(ctx.session.subCalls).start;
        const circleId = base + 4; // base..+2 points, +3 centre, +4 circle
        ctx.store.pushSketchSubCalls([
          { type: "sketch_point", params: { at: drag.p1 } },
          { type: "sketch_point", params: { at: drag.p2 } },
          { type: "sketch_point", params: { at: [su, sv] } },
          {
            type: "sketch_circle",
            params: { center: res.center, radius: res.radius },
          },
          {
            type: "sketch_constraint_point_on_object",
            params: { point: base, curve: circleId },
          },
          {
            type: "sketch_constraint_point_on_object",
            params: { point: base + 1, curve: circleId },
          },
          {
            type: "sketch_constraint_point_on_object",
            params: { point: base + 2, curve: circleId },
          },
        ]);
        ctx.scheduleSolverReplay();
      }
    }
    ctx.requestRedraw();
  },
  onMove(ctx, ev) {
    const drag = ctx.dragOf<ThreePointDrag>(circle3pTool);
    if (!drag) return false;
    ctx.setDrag(circle3pTool, { ...drag, current: ev.gridded });
    return true;
  },
  onDeactivate(ctx) {
    // Reset the in-progress multi-click draft on tool change.
    ctx.clearDrag();
    ctx.requestRedraw();
  },
  hud(drag) {
    const d = drag as ThreePointDrag;
    const hud: ToolHud = { previewPoints: [d.p1] };
    if (d.p2 === null) {
      hud.previewLines = [[d.p1, d.current]];
    } else {
      hud.previewPoints = [d.p1, d.p2, d.current];
      const res = circleThrough3(d.p1, d.p2, d.current);
      if (res) {
        hud.previewCalls = [
          {
            type: "sketch_circle",
            params: { center: res.center, radius: res.radius },
          },
        ];
        const l = radiusLabel(res.center, res.radius);
        if (l) hud.labels = [l];
      }
    }
    return hud;
  },
};

// ─── rotated rectangle from three points ────────────────────────

/**
 * F7: expand a rotated rectangle (four corners) into the sub-calls that
 * realise it: four `sketch_line` edges around the loop, coincident
 * constraints joining adjacent corners so the loop stays closed under
 * solving, and perpendicular constraints on adjacent edges so the right
 * angles are locked (three suffice to fix all four corners). Entity ids
 * are predicted the way the line-commit path does — each `sketch_line`
 * allocates start, end, line in that order — so the constraints reference
 * the lines/points the kernel is about to create.
 */
function buildRect3pSubCalls(
  corners: [Pt, Pt, Pt, Pt],
  subCalls: ReadonlyArray<SketchSubCall>,
): SketchSubCall[] {
  const base = predictNextLineIds(subCalls).start;
  // Per line i: startPt = base + 3i, endPt = base + 3i + 1, line = base + 3i + 2.
  const startPt = (i: number) => base + 3 * i;
  const endPt = (i: number) => base + 3 * i + 1;
  const lineId = (i: number) => base + 3 * i + 2;

  const calls: SketchSubCall[] = [];
  for (let i = 0; i < 4; i++) {
    const a = corners[i];
    const b = corners[(i + 1) % 4];
    calls.push({
      type: "sketch_line",
      params: {
        start: [a[0], a[1]] as Pt,
        end: [b[0], b[1]] as Pt,
      },
    });
  }
  // Close the loop: line i's end coincides with line (i+1)'s start.
  for (let i = 0; i < 4; i++) {
    calls.push({
      type: "sketch_constraint_coincident",
      params: { a: endPt(i), b: startPt((i + 1) % 4) },
    });
  }
  // Lock the right angles: perpendicular between consecutive edges. Three
  // constraints pin all four corners (the fourth angle follows).
  for (let i = 0; i < 3; i++) {
    calls.push({
      type: "sketch_constraint_perpendicular",
      params: { line_a: lineId(i), line_b: lineId(i + 1) },
    });
  }
  return calls;
}

export const rect3pTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    const [su, sv] = ev.snap!.snapped;
    const drag = ctx.dragOf<ThreePointDrag>(rect3pTool);
    if (!drag) {
      // Click 1 — first edge endpoint.
      ctx.setDrag(rect3pTool, {
        p1: [su, sv],
        p2: null,
        current: [su, sv],
      } satisfies ThreePointDrag);
    } else if (drag.p2 === null) {
      // Click 2 — second edge endpoint (fixes the edge direction).
      ctx.setDrag(rect3pTool, { ...drag, p2: [su, sv], current: [su, sv] });
    } else {
      // Click 3 — the perpendicular width; commit four lines + two
      // perpendicular constraints (or discard if degenerate).
      const corners = rect3pCorners(drag.p1, drag.p2, [su, sv]);
      ctx.clearDrag();
      if (corners) {
        ctx.store.pushSketchSubCalls(
          buildRect3pSubCalls(corners, ctx.session.subCalls),
        );
        ctx.scheduleSolverReplay();
      }
    }
    ctx.requestRedraw();
  },
  onMove(ctx, ev) {
    const drag = ctx.dragOf<ThreePointDrag>(rect3pTool);
    if (!drag) return false;
    ctx.setDrag(rect3pTool, { ...drag, current: ev.gridded });
    return true;
  },
  onDeactivate(ctx) {
    // Reset the in-progress multi-click draft on tool change.
    ctx.clearDrag();
    ctx.requestRedraw();
  },
  hud(drag) {
    const d = drag as ThreePointDrag;
    const hud: ToolHud = { previewPoints: [d.p1] };
    if (d.p2 === null) {
      hud.previewLines = [[d.p1, d.current]];
    } else {
      const corners = rect3pCorners(d.p1, d.p2, d.current);
      if (corners) {
        const lines: Array<[Pt, Pt]> = [];
        const points: Pt[] = [d.p1, d.p2];
        for (let i = 0; i < 4; i++) {
          lines.push([corners[i], corners[(i + 1) % 4]]);
          points.push(corners[i]);
        }
        hud.previewLines = lines;
        hud.previewPoints = points;
      } else {
        hud.previewPoints = [d.p1, d.p2];
        hud.previewLines = [[d.p1, d.p2]];
      }
    }
    return hud;
  },
};
