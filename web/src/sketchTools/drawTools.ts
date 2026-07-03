/**
 * Simple draw-tool controllers (architecture review, Befund 2): the
 * click-to-place Point tool and the drag-to-commit primitives — rectangle
 * (corner-corner and centre), circle, ellipse, slot, regular N-gon — plus
 * the multi-click Polygon tool. Behaviour is a 1:1 translation of the old
 * `SketchCanvas2d` if-chains; each controller owns its private drag type.
 */

import { sketchToScreen } from "../sketchCameraBridge";
import { regularPolygonVertices } from "../sketchSession";
import { rectCenterCorners } from "../sketchPrimitiveVariants";
import { noSnap } from "../sketchSnapResolve";
import { radiusLabel, sizeLabel } from "../sketchCanvasHud";
import type {
  SketchToolController,
  ToolContext,
  ToolHud,
  ToolPointerEvent,
} from "./toolTypes";

type Pt = [number, number];

/** Shared shape of the two-point drag gestures (anchor + live cursor). */
interface TwoPointDrag {
  a: Pt;
  current: Pt;
}

/** Arm a two-point drag at the snapped down-position. */
function armTwoPointDrag(
  owner: SketchToolController,
  ctx: ToolContext,
  ev: ToolPointerEvent,
): void {
  const p = ev.snap!.snapped;
  ctx.setDrag(owner, { a: p, current: p } satisfies TwoPointDrag);
  ctx.requestRedraw();
}

/** Track the cursor on an owned two-point drag; `resetSnap` mirrors which
 *  legacy branches cleared the snap indicator while dragging. */
function trackTwoPointDrag(
  owner: SketchToolController,
  ctx: ToolContext,
  ev: ToolPointerEvent,
  resetSnap: boolean,
): boolean {
  const drag = ctx.dragOf<TwoPointDrag>(owner);
  if (!drag) return false;
  ctx.setDrag(owner, { ...drag, current: ev.gridded });
  if (resetSnap) ctx.setSnap(noSnap(ev.gridded));
  return true;
}

/** Release + clear on pointer-up, returning the finished drag (or null when
 *  this controller owned no drag — the dispatch guards that anyway). */
function finishDrag<T>(
  owner: SketchToolController,
  ctx: ToolContext,
  ev: ToolPointerEvent,
): T | null {
  const drag = ctx.dragOf<T>(owner);
  if (drag === null) return null;
  ctx.releasePointer(ev);
  ctx.clearDrag();
  return drag;
}

// ─── point ──────────────────────────────────────────────────────

export const pointTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    const [su, sv] = ev.snap!.snapped;
    ctx.store.setSketchToolDraft({ at: [su, sv] });
    ctx.store.applySketchToolDraft();
  },
};

// ─── rectangle (corner → corner) ────────────────────────────────

export const rectangleTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    armTwoPointDrag(rectangleTool, ctx, ev);
  },
  onMove(ctx, ev) {
    // Rectangles don't run inference at the drag endpoint —
    // the H/V constraints are implicit in the tool itself, so
    // an orientation snap would be redundant. Shift still
    // suppresses; hover snap is also disabled while dragging.
    return trackTwoPointDrag(rectangleTool, ctx, ev, true);
  },
  onUp(ctx, ev) {
    const drag = finishDrag<TwoPointDrag>(rectangleTool, ctx, ev);
    if (!drag) return;
    const { a: start, current } = drag;
    const [cx, cy] = current;
    // Reject near-zero-area rectangles.
    const dx = cx - start[0];
    const dy = cy - start[1];
    if (dx * dx + dy * dy < 1e-4) {
      ctx.redrawNow();
      return;
    }
    ctx.store.pushSketchSubCalls([
      {
        type: "sketch_rectangle",
        params: {
          corner_a: [start[0], start[1]] as Pt,
          corner_b: [cx, cy] as Pt,
        },
      },
    ]);
    ctx.scheduleSolverReplay();
  },
  hud(drag) {
    const d = drag as TwoPointDrag;
    const hud: ToolHud = {
      previewCalls: [
        {
          type: "sketch_rectangle",
          params: { corner_a: d.a, corner_b: d.current },
        },
      ],
    };
    const l = sizeLabel(d.a, d.current);
    if (l) hud.labels = [l];
    return hud;
  },
};

// ─── rectangle from centre (F7) ─────────────────────────────────

export const rectCenterTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    armTwoPointDrag(rectCenterTool, ctx, ev);
  },
  onMove(ctx, ev) {
    return trackTwoPointDrag(rectCenterTool, ctx, ev, true);
  },
  onUp(ctx, ev) {
    const drag = finishDrag<TwoPointDrag>(rectCenterTool, ctx, ev);
    if (!drag) return;
    const res = rectCenterCorners(drag.a, drag.current);
    if (!res) {
      ctx.redrawNow();
      return;
    }
    ctx.store.pushSketchSubCalls([
      {
        type: "sketch_rectangle",
        params: { corner_a: res.cornerA, corner_b: res.cornerB },
      },
    ]);
    ctx.scheduleSolverReplay();
  },
  hud(drag) {
    const d = drag as TwoPointDrag;
    const opp: Pt = [2 * d.a[0] - d.current[0], 2 * d.a[1] - d.current[1]];
    const hud: ToolHud = {
      previewCalls: [
        {
          type: "sketch_rectangle",
          params: { corner_a: opp, corner_b: d.current },
        },
      ],
      previewPoints: [d.a],
    };
    const l = sizeLabel(opp, d.current);
    if (l) hud.labels = [l];
    return hud;
  },
};

// ─── circle (centre → rim) ──────────────────────────────────────

export const circleTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    armTwoPointDrag(circleTool, ctx, ev);
  },
  onMove(ctx, ev) {
    return trackTwoPointDrag(circleTool, ctx, ev, false);
  },
  onUp(ctx, ev) {
    const drag = finishDrag<TwoPointDrag>(circleTool, ctx, ev);
    if (!drag) return;
    const { a: center, current } = drag;
    const dx = current[0] - center[0];
    const dy = current[1] - center[1];
    const radius = Math.hypot(dx, dy);
    if (radius < 1e-2) {
      ctx.redrawNow();
      return;
    }
    ctx.store.pushSketchSubCalls([
      {
        type: "sketch_circle",
        params: {
          center: [center[0], center[1]] as Pt,
          radius,
        },
      },
    ]);
    ctx.scheduleSolverReplay();
  },
  hud(drag) {
    const d = drag as TwoPointDrag;
    const r = Math.hypot(d.current[0] - d.a[0], d.current[1] - d.a[1]);
    const hud: ToolHud = {
      previewCalls: [
        { type: "sketch_circle", params: { center: d.a, radius: r } },
      ],
    };
    const l = radiusLabel(d.a, r);
    if (l) hud.labels = [l];
    return hud;
  },
};

// ─── ellipse (F8, axis-aligned centre → corner) ─────────────────

export const ellipseTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    armTwoPointDrag(ellipseTool, ctx, ev);
  },
  onMove(ctx, ev) {
    return trackTwoPointDrag(ellipseTool, ctx, ev, false);
  },
  onUp(ctx, ev) {
    const drag = finishDrag<TwoPointDrag>(ellipseTool, ctx, ev);
    if (!drag) return;
    const { a: center, current } = drag;
    const radiusX = Math.abs(current[0] - center[0]);
    const radiusY = Math.abs(current[1] - center[1]);
    if (radiusX < 1e-2 || radiusY < 1e-2) {
      ctx.redrawNow();
      return;
    }
    ctx.store.pushSketchSubCalls([
      {
        type: "sketch_ellipse",
        params: {
          center: [center[0], center[1]] as Pt,
          radius_x: radiusX,
          radius_y: radiusY,
          rotation: Number(ctx.session.toolDraft?.rotation) || 0,
          segments: Number(ctx.session.toolDraft?.segments) || 0,
        },
      },
    ]);
    ctx.scheduleSolverReplay();
  },
  hud(drag, session) {
    const d = drag as TwoPointDrag;
    const rx = Math.abs(d.current[0] - d.a[0]);
    const ry = Math.abs(d.current[1] - d.a[1]);
    return {
      previewCalls: [
        {
          type: "sketch_ellipse",
          params: {
            center: d.a,
            radius_x: rx,
            radius_y: ry,
            rotation: Number(session.toolDraft?.rotation) || 0,
          },
        },
      ],
    };
  },
};

// ─── slot (F9, centre → centre stadium) ─────────────────────────

export const slotTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    armTwoPointDrag(slotTool, ctx, ev);
  },
  onMove(ctx, ev) {
    return trackTwoPointDrag(slotTool, ctx, ev, false);
  },
  onUp(ctx, ev) {
    const drag = finishDrag<TwoPointDrag>(slotTool, ctx, ev);
    if (!drag) return;
    const { a: centerA, current } = drag;
    const len = Math.hypot(current[0] - centerA[0], current[1] - centerA[1]);
    if (len < 1e-2) {
      ctx.redrawNow();
      return;
    }
    const radius = Number(ctx.session.toolDraft?.radius) || 8;
    ctx.store.pushSketchSubCalls([
      {
        type: "sketch_slot",
        params: {
          center_a: [centerA[0], centerA[1]] as Pt,
          center_b: [current[0], current[1]] as Pt,
          radius,
        },
      },
    ]);
    ctx.scheduleSolverReplay();
  },
  hud(drag, session) {
    const d = drag as TwoPointDrag;
    const radius = Number(session.toolDraft?.radius) || 8;
    return {
      previewCalls: [
        {
          type: "sketch_slot",
          params: { center_a: d.a, center_b: d.current, radius },
        },
      ],
    };
  },
};

// ─── regular N-gon (F6, centre → radius) ────────────────────────

export const regularPolygonTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    armTwoPointDrag(regularPolygonTool, ctx, ev);
  },
  onMove(ctx, ev) {
    return trackTwoPointDrag(regularPolygonTool, ctx, ev, true);
  },
  onUp(ctx, ev) {
    const drag = finishDrag<TwoPointDrag>(regularPolygonTool, ctx, ev);
    if (!drag) return;
    const { a: center, current } = drag;
    const radius = Math.hypot(current[0] - center[0], current[1] - center[1]);
    if (radius < 1e-2) {
      ctx.redrawNow();
      return;
    }
    ctx.store.pushSketchSubCalls([
      {
        type: "sketch_regular_polygon",
        params: {
          center: [center[0], center[1]] as Pt,
          radius,
          sides: Number(ctx.session.toolDraft?.sides) || 6,
          inscribed: ctx.session.toolDraft?.inscribed ?? true,
        },
      },
    ]);
    ctx.scheduleSolverReplay();
  },
  hud(drag, session) {
    const d = drag as TwoPointDrag;
    const radius = Math.hypot(d.current[0] - d.a[0], d.current[1] - d.a[1]);
    const verts = regularPolygonVertices(
      d.a,
      radius,
      Number(session.toolDraft?.sides) || 6,
      session.toolDraft?.inscribed !== false,
    );
    const hud: ToolHud = {};
    if (verts.length >= 3) {
      hud.previewCalls = [
        {
          type: "sketch_polygon",
          params: { vertices: verts.flatMap((v) => [v[0], v[1]]) },
        },
      ];
      hud.previewPoints = [d.a];
    }
    const l = radiusLabel(d.a, radius);
    if (l) hud.labels = [l];
    return hud;
  },
};

// ─── polygon (multi-click, close on first vertex) ───────────────

interface PolygonDrag {
  vertices: Pt[];
  current: Pt;
}

export const polygonTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    const [su, sv] = ev.snap!.snapped;
    // If the previous polygon was applied (draft cleared),
    // start fresh rather than appending to the old one.
    const draftVerts = ctx.session.toolDraft?.vertices;
    const draftEmpty =
      !draftVerts || (Array.isArray(draftVerts) && draftVerts.length === 0);
    const inFlight = ctx.dragOf<PolygonDrag>(polygonTool);
    const existing = inFlight && !draftEmpty ? inFlight.vertices : [];

    // Close-on-first-vertex: if we have ≥ 3 vertices and
    // the click is within 8 px of the first vertex, auto-
    // close and commit instead of adding another vertex.
    if (existing.length >= 3) {
      const [fpx, fpy] = sketchToScreen(existing[0][0], existing[0][1], ev.cam);
      const [cpx, cpy] = sketchToScreen(su, sv, ev.cam);
      const dist = Math.hypot(cpx - fpx, cpy - fpy);
      if (dist < 8) {
        // Commit the polygon as-is (without the closing
        // duplicate vertex).
        ctx.store.setSketchToolDraft({ vertices: existing.flat() });
        ctx.store.applySketchToolDraft();
        ctx.clearDrag();
        ctx.requestRedraw();
        return;
      }
    }

    const verts: Pt[] = [...existing, [su, sv]];
    ctx.setDrag(polygonTool, {
      vertices: verts,
      current: [su, sv],
    } satisfies PolygonDrag);
    ctx.store.setSketchToolDraft({
      vertices: verts.flat(),
    });
    ctx.requestRedraw();
  },
  onMove(ctx, ev) {
    const drag = ctx.dragOf<PolygonDrag>(polygonTool);
    if (!drag) return false;
    ctx.setDrag(polygonTool, { ...drag, current: ev.gridded });
    ctx.setSnap(noSnap(ev.gridded));
    return true;
  },
  hud(drag) {
    const { vertices, current } = drag as PolygonDrag;
    const hud: ToolHud = {};
    if (vertices.length > 0) {
      const lines: Array<[Pt, Pt]> = [];
      for (let i = 0; i + 1 < vertices.length; i++) {
        lines.push([vertices[i], vertices[i + 1]]);
      }
      lines.push([vertices[vertices.length - 1], current]);
      hud.previewLines = lines;
      hud.previewGuides = [[current, vertices[0]]];
      hud.previewPoints = [...vertices];
    }
    return hud;
  },
};

/** The draw-tool registry entries, keyed by `selectedTool` name. */
export const DRAW_TOOL_CONTROLLERS: Record<string, SketchToolController> = {
  sketch_point: pointTool,
  sketch_rectangle: rectangleTool,
  sketch_rectangle_center: rectCenterTool,
  sketch_circle: circleTool,
  sketch_ellipse: ellipseTool,
  sketch_slot: slotTool,
  sketch_regular_polygon: regularPolygonTool,
  sketch_polygon: polygonTool,
};
