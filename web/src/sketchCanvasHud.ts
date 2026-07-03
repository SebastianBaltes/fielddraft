/**
 * Sketch overlay HUD drawing — extracted verbatim from `SketchCanvas2d.tsx`
 * (architecture review, Befund 2: tool-controller registry).
 *
 * `drawOverlay` publishes the transient HUD GEOMETRY (drag previews,
 * hover/snap markers, projected-edge guides) to `sketchHudBridge` — rendered
 * as real 3D geometry by `SketchHud3D` — and the text labels (constraint
 * glyphs, dimension readouts, snap label) to `sketchLabelBridge`, rendered as
 * 3D billboard sprites by `SketchLabels3D`. The 2D canvas itself keeps only
 * the pure screen-space passes: linear dimensions (Maßlinien) and the
 * box-select marquee.
 *
 * Tool-controller drags (`drag.kind === "owned"`) contribute their preview
 * primitives / labels / screen pass through the owning controller's
 * {@link SketchToolController.hud}; the legacy `DragState` switch covers the
 * not-yet-migrated tools and disappears with the last migration step.
 */

import type { SketchSessionState } from "./sketchSession";
import { buildDraftSketchState } from "./sketchSession";
import { sketchToScreen, type SketchCameraState } from "./sketchCameraBridge";
import {
  formatSketchLength,
  getSketchLengthUnit,
  sketchDisplayLength,
} from "./sketchUnitsBridge";
import { publishSketchHud } from "./sketchHudBridge";
import { publishSketchLabels, type SketchLabel } from "./sketchLabelBridge";
import { displayDegreesOfFreedom } from "./solver/dofPreflight";
import type { ProjectedEdge, SnapResolution } from "./sketchSnapResolve";
import type { DragState } from "./SketchCanvas2d";
import type { SketchSubCall, ToolHud } from "./sketchTools/toolTypes";

/** Visual tokens. Keep in sync with `styles/tokens.css` if we
 *  ever add sketch colours there. */
// K6b.6: 3-color DOF status.
// Blue = underconstrained (default), Green = fully constrained,
// Red = overconstrained.
export const COLOR_UNDER = "#2563eb";
export const COLOR_FULL = "#16a34a";
export const COLOR_OVER = "#dc2626";

export function dofColor(dof: number): string {
  if (dof > 0) return COLOR_UNDER;
  if (dof === 0) return COLOR_FULL;
  return COLOR_OVER;
}

export const COLOR_SNAP = "#f59e0b";
/** Dimension readouts: unobtrusive plain text in the accent blue —
 *  no box, so the numbers never compete with the geometry. */
export const LABEL_DIM = "#3b82f6";
/** Label offset in CSS pixels, along the line's left-normal. */
export const LABEL_OFFSET_PX = 18;
/** Click radius (CSS px) around the dimension readout that opens the
 *  exact-length editor on the most recently placed line. */
export const DIM_LABEL_HIT_PX = 24;
/** Snap indicator radius in CSS pixels, drawn as an open ring
 *  around the snap anchor. */
export const SNAP_RING_PX = 8;

/** F4 marquee: enclosing ("window", left→right) = solid blue border;
 *  crossing (right→left) = dashed green border. Translucent fills. */
const COLOR_MARQUEE_ENCLOSING = "#2563eb";
const COLOR_MARQUEE_CROSSING = "#16a34a";
const FILL_MARQUEE_ENCLOSING = "rgba(37, 99, 235, 0.12)";
const FILL_MARQUEE_CROSSING = "rgba(22, 163, 74, 0.12)";
const MARQUEE_CROSSING_DASH = [6, 4];

/** The committed geometry of the most recently placed line (the
 *  click-to-edit dimension readout target). */
export interface RecentLineGeometry {
  start: [number, number];
  end: [number, number];
}

export function resizeToContainer(canvas: HTMLCanvasElement): void {
  const rect = canvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(rect.width * dpr));
  const h = Math.max(1, Math.round(rect.height * dpr));
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
}

// ─── label builders ─────────────────────────────────────────────

/** Screen-space perpendicular nudge (px, +x right +y up) that pushes a line's
 *  dimension readout off the line — the old canvas left-normal, recomputed in
 *  the same camera so the 3D label lands exactly where the readout used to. */
function lineOffsetPx(
  a: [number, number],
  b: [number, number],
  cam: Readonly<SketchCameraState>,
): [number, number] {
  const [sx, sy] = sketchToScreen(a[0], a[1], cam);
  const [ex, ey] = sketchToScreen(b[0], b[1], cam);
  const dx = ex - sx;
  const dy = ey - sy;
  const len = Math.hypot(dx, dy);
  if (len < 1) return [0, LABEL_OFFSET_PX];
  // Canvas left-normal (y-down); flip y for the sprite's y-up offset.
  return [(dy / len) * LABEL_OFFSET_PX, (dx / len) * LABEL_OFFSET_PX];
}

/** The live distance readout of a line from `a` to `b` (or null when
 *  degenerate) — anchor at the midpoint, pushed off the line. */
export function distanceLabel(
  a: [number, number],
  b: [number, number],
  cam: Readonly<SketchCameraState>,
): SketchLabel | null {
  const dist = Math.hypot(b[0] - a[0], b[1] - a[1]);
  if (dist < 1e-4) return null;
  return {
    anchorUv: [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2],
    text: formatSketchLength(dist),
    style: "dimension",
    color: LABEL_DIM,
    offsetPx: lineOffsetPx(a, b, cam),
  };
}

/** The live "W × H unit" readout of an axis-aligned box spanned by two
 *  corners (or null when degenerate). */
export function sizeLabel(
  a: [number, number],
  b: [number, number],
): SketchLabel | null {
  const w = Math.abs(b[0] - a[0]);
  const h = Math.abs(b[1] - a[1]);
  if (w < 1e-4 && h < 1e-4) return null;
  const u = getSketchLengthUnit();
  return {
    anchorUv: [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2],
    text: `${sketchDisplayLength(w).toFixed(u.decimals)} × ${sketchDisplayLength(h).toFixed(u.decimals)} ${u.unit}`,
    style: "dimension",
    color: LABEL_DIM,
    offsetPx: [0, 0],
  };
}

/** The live radius readout of a circle/arc gesture (or null when
 *  degenerate). */
export function radiusLabel(
  center: [number, number],
  r: number,
): SketchLabel | null {
  if (r < 1e-4) return null;
  return {
    anchorUv: center,
    text: `R ${formatSketchLength(r)}`,
    style: "dimension",
    color: LABEL_DIM,
    offsetPx: [LABEL_OFFSET_PX, LABEL_OFFSET_PX],
  };
}

// ─── drawing ────────────────────────────────────────────────────

/**
 * Transient sketch HUD GEOMETRY (preview shapes, hover/snap markers,
 * projected-edge guides) in sketch (u,v) space, mirroring drawOverlay's
 * drag switch but as primitives instead of canvas strokes — published to
 * `sketchHudBridge` and rendered as real 3D geometry by `SketchHud3D`.
 * Text labels (dimensions, glyphs, snap label) stay on the overlay.
 */
function buildSketchHudUv(
  drag: DragState,
  hover: [number, number] | null,
  snap: SnapResolution,
  projectedEdges: ProjectedEdge[],
  mirrorAxisHover: [[number, number], [number, number]] | null,
  owned: ToolHud | null,
) {
  const previewCalls: SketchSubCall[] = [];
  const previewLines: Array<[[number, number], [number, number]]> = [];
  const previewPoints: Array<[number, number]> = [];
  const previewGuides: Array<[[number, number], [number, number]]> = [];

  switch (drag.kind) {
    case "owned":
      if (owned) {
        previewCalls.push(...(owned.previewCalls ?? []));
        previewLines.push(...(owned.previewLines ?? []));
        previewPoints.push(...(owned.previewPoints ?? []));
        previewGuides.push(...(owned.previewGuides ?? []));
      }
      break;
    default:
      // idle / grip / marquee: no 3D preview shapes.
      break;
  }

  // Ticket czqf: highlight the hovered mirror-axis line in the preview colour.
  if (mirrorAxisHover) previewLines.push(mirrorAxisHover);

  return {
    previewCalls,
    previewLines,
    previewPoints,
    previewGuides,
    hover: drag.kind === "idle" ? hover : null,
    // Show the snap ring whenever the cursor is pulled onto a target — an
    // inference candidate (endpoint/H/V/…) OR a geometry projection snap
    // (line: pgyf/ymbp, circle/arc: fkig), which carry no candidate but do
    // place the cursor exactly on geometry.
    snap: snap.anchor,
    projected: projectedEdges.map(
      (e) => [e.p0, e.p1] as [[number, number], [number, number]],
    ),
  };
}

export function drawOverlay(
  canvas: HTMLCanvasElement,
  session: SketchSessionState,
  cam: Readonly<SketchCameraState>,
  drag: DragState,
  hover: [number, number] | null,
  snap: SnapResolution,
  projectedEdges: ProjectedEdge[] | null = null,
  recentLine: RecentLineGeometry | null = null,
  mirrorAxisHover: [[number, number], [number, number]] | null = null,
): void {
  // A controller-owned drag contributes its HUD through the owning
  // controller — computed once, consumed by both the 3D-geometry publish
  // below and the label / screen-space passes further down.
  const owned: ToolHud | null =
    drag.kind === "owned"
      ? (drag.owner.hud?.(drag.state, session, cam) ?? null)
      : null;

  // Publish the transient HUD GEOMETRY (previews/hover/snap-ring/projected)
  // for the 3D layer (SketchHud3D); this overlay draws only text + marquee.
  publishSketchHud(
    buildSketchHudUv(
      drag,
      hover,
      snap,
      projectedEdges ?? [],
      mirrorAxisHover,
      owned,
    ),
  );
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const dpr = cam.devicePixelRatio || window.devicePixelRatio || 1;

  // Reset to identity and clear. The device-pixel-ratio scale
  // goes on _after_ the clear so we can work in CSS pixels for
  // everything below.
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.scale(dpr, dpr);

  // A non-positive zoom is a degenerate camera: every screen transform
  // divides by zoom and the adaptive-grid loop below would never terminate.
  // Bail with a cleared canvas rather than freezing the tab.
  if (!(cam.zoom > 0)) return;

  // Projected B-Rep edge guides, drag previews, the hover point and the
  // snap RING now render as 3D scene geometry (SketchHud3D); committed
  // geometry + selection are 3D too (SketchSession3D). drawOverlay keeps
  // only the screen-space TEXT/marquee HUD: constraint glyphs, dimension
  // readouts, the snap label and the marquee box.

  // Solved state + DOF colour for the constraint glyphs.
  const skState = buildDraftSketchState(session.subCalls);
  let committedColor = COLOR_UNDER;
  try {
    const over =
      session.diagnostics.conflicting.length > 0 ||
      session.diagnostics.malformed.length > 0;
    const dof = displayDegreesOfFreedom(skState, session.diagnostics);
    committedColor = over ? COLOR_OVER : dofColor(dof);
  } catch {
    // Solver not available — fall back to the underconstrained color.
  }

  // Text labels — constraint glyphs, dimension readouts, the snap label —
  // render as 3D billboard sprites (SketchLabels3D): collected here in plane
  // (u, v) space and published, drawn in-scene so they ride the camera in
  // lockstep with the geometry instead of lagging on this 2D canvas. The
  // marquee (below) stays here — it's a pure screen-space selection box.
  const labels: SketchLabel[] = [];

  const pushDistance = (a: [number, number], b: [number, number]) => {
    const l = distanceLabel(a, b, cam);
    if (l) labels.push(l);
  };

  // Constraint glyphs anchored to the constrained geometry.
  {
    const entityPos = (entityId: number): [number, number] | null => {
      const e = skState.entities.find((x) => x.id === entityId);
      if (!e) return null;
      if (e.kind === "point") return [e.at.x, e.at.y];
      if (e.kind === "origin") return [0, 0];
      return null;
    };
    const lineMidpoint = (lineId: number): [number, number] | null => {
      const e = skState.entities.find((x) => x.id === lineId);
      if (!e || e.kind !== "line") return null;
      const s = entityPos(e.start);
      const en = entityPos(e.end);
      if (!s || !en) return null;
      return [(s[0] + en[0]) / 2, (s[1] + en[1]) / 2];
    };
    for (const c of skState.constraints) {
      let anchor: [number, number] | null = null;
      let label = "C";
      if (c.kind === "horizontal") {
        anchor = lineMidpoint(c.line);
        label = "H";
      } else if (c.kind === "vertical") {
        anchor = lineMidpoint(c.line);
        label = "V";
      } else if (c.kind === "coincident") {
        anchor = entityPos(c.a);
        label = "⊙";
      }
      if (!anchor) continue;
      labels.push({
        anchorUv: anchor,
        text: label,
        style: "constraint",
        color: committedColor,
        offsetPx: [0, 10],
      });
    }

    // Ticket 7lxk: render every distance dimension as a Maßlinie — extension
    // lines, an offset dimension line with arrowheads, and the value — so a
    // dimension placed by the Dimension tool is actually visible (not just a
    // DoF change). Onshape-style. Drawn on the 2D overlay in screen space.
    for (const c of skState.constraints) {
      if (c.kind !== "distance") continue;
      const pa = entityPos(c.a);
      const pb = entityPos(c.b);
      if (!pa || !pb) continue;
      drawLinearDimension(ctx, pa, pb, c.value_mm, cam);
    }
  }

  // Dimension readouts. The recent-line readout is click-to-edit (hit-tested
  // in onPointerDown); the others track the live drag.
  if (recentLine !== null && drag.kind === "idle") {
    pushDistance(recentLine.start, recentLine.end);
  }
  if (drag.kind === "owned" && owned) {
    labels.push(...(owned.labels ?? []));
    owned.drawScreen?.(ctx, cam);
  }

  // Snap label — a small pill up-and-right of the snap ring (the ring renders
  // in 3D via SketchHud3D).
  if (snap.anchor && snap.candidate) {
    labels.push({
      anchorUv: snap.anchor,
      text: snap.candidate.label,
      style: "snap",
      color: COLOR_SNAP,
      offsetPx: [SNAP_RING_PX + 6, SNAP_RING_PX + 6],
    });
  }

  publishSketchLabels(labels);
}

/**
 * F4: draw the box-select marquee. The drag direction decides the
 * style — left→right (start.x ≤ current.x) is an enclosing "window"
 * select drawn with a solid blue border, right→left a crossing select
 * drawn with a dashed green border. Both get a faint matching fill.
 */
export function drawMarquee(
  ctx: CanvasRenderingContext2D,
  start: [number, number],
  current: [number, number],
  cam: Readonly<SketchCameraState>,
): void {
  const enclosing = start[0] <= current[0];
  const [sx, sy] = sketchToScreen(start[0], start[1], cam);
  const [cx, cy] = sketchToScreen(current[0], current[1], cam);
  const x = Math.min(sx, cx);
  const y = Math.min(sy, cy);
  const w = Math.abs(cx - sx);
  const h = Math.abs(cy - sy);
  ctx.save();
  ctx.fillStyle = enclosing ? FILL_MARQUEE_ENCLOSING : FILL_MARQUEE_CROSSING;
  ctx.fillRect(x, y, w, h);
  ctx.strokeStyle = enclosing
    ? COLOR_MARQUEE_ENCLOSING
    : COLOR_MARQUEE_CROSSING;
  ctx.lineWidth = 1.5;
  ctx.setLineDash(enclosing ? [] : MARQUEE_CROSSING_DASH);
  ctx.strokeRect(x, y, w, h);
  ctx.restore();
}

/**
 * Ticket 7lxk: draw a linear dimension (Maßlinie) between two sketch points —
 * extension lines, an offset dimension line with arrowheads, and the value
 * label — directly on the 2D overlay in screen space. Onshape-style.
 */
function drawLinearDimension(
  ctx: CanvasRenderingContext2D,
  aUv: [number, number],
  bUv: [number, number],
  valueMm: number,
  cam: Readonly<SketchCameraState>,
): void {
  const [ax, ay] = sketchToScreen(aUv[0], aUv[1], cam);
  const [bx, by] = sketchToScreen(bUv[0], bUv[1], cam);
  const dx = bx - ax;
  const dy = by - ay;
  const len = Math.hypot(dx, dy);
  if (len < 2) return;
  const ux = dx / len;
  const uy = dy / len;
  // Perpendicular (screen-left of a→b); offset the dimension line outward.
  const nx = -uy;
  const ny = ux;
  const OFF = 24;
  const a2: [number, number] = [ax + nx * OFF, ay + ny * OFF];
  const b2: [number, number] = [bx + nx * OFF, by + ny * OFF];

  ctx.save();
  ctx.strokeStyle = LABEL_DIM;
  ctx.fillStyle = LABEL_DIM;
  ctx.lineWidth = 1;

  // Extension lines from each measured point, slightly past the dim line.
  ctx.beginPath();
  ctx.moveTo(ax + nx * 3, ay + ny * 3);
  ctx.lineTo(a2[0] + nx * 4, a2[1] + ny * 4);
  ctx.moveTo(bx + nx * 3, by + ny * 3);
  ctx.lineTo(b2[0] + nx * 4, b2[1] + ny * 4);
  ctx.stroke();

  // Dimension line + arrowheads pointing outward at each end.
  ctx.beginPath();
  ctx.moveTo(a2[0], a2[1]);
  ctx.lineTo(b2[0], b2[1]);
  ctx.stroke();
  const arrow = (p: [number, number], dirx: number, diry: number) => {
    const s = 7;
    const px = -diry;
    const py = dirx;
    ctx.beginPath();
    ctx.moveTo(p[0], p[1]);
    ctx.lineTo(p[0] + dirx * s + px * 2.6, p[1] + diry * s + py * 2.6);
    ctx.lineTo(p[0] + dirx * s - px * 2.6, p[1] + diry * s - py * 2.6);
    ctx.closePath();
    ctx.fill();
  };
  arrow(a2, ux, uy);
  arrow(b2, -ux, -uy);

  // Value label at the dim-line midpoint, on a small readable background.
  // Kept in lockstep with `linearDimLabelPx` (the click-to-edit hit test).
  const text = formatSketchLength(valueMm);
  const label = linearDimLabelPx(aUv, bUv, cam) ?? [
    (a2[0] + b2[0]) / 2,
    (a2[1] + b2[1]) / 2,
  ];
  const mx = label[0];
  const my = label[1];
  ctx.font = "12px system-ui, sans-serif";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  const w = ctx.measureText(text).width + 8;
  ctx.fillStyle = "rgba(255,255,255,0.85)";
  ctx.fillRect(mx - w / 2, my - 9, w, 18);
  ctx.fillStyle = LABEL_DIM;
  ctx.fillText(text, mx, my);
  ctx.restore();
}

/**
 * Screen-px centre of a linear dimension's value label (the readable number on
 * the Maßlinie). Matches `drawLinearDimension`'s placement exactly — midpoint of
 * the offset dimension line — so the click-to-edit hit test lands on what's
 * drawn. `null` for a (near) zero-length screen segment.
 */
export function linearDimLabelPx(
  aUv: [number, number],
  bUv: [number, number],
  cam: Readonly<SketchCameraState>,
): [number, number] | null {
  const [ax, ay] = sketchToScreen(aUv[0], aUv[1], cam);
  const [bx, by] = sketchToScreen(bUv[0], bUv[1], cam);
  const dx = bx - ax;
  const dy = by - ay;
  const len = Math.hypot(dx, dy);
  if (len < 2) return null;
  const nx = -dy / len;
  const ny = dx / len;
  const OFF = 24;
  return [(ax + bx) / 2 + nx * OFF, (ay + by) / 2 + ny * OFF];
}

/**
 * Anchor (canvas CSS px) of a line's dimension readout: the midpoint,
 * pushed `LABEL_OFFSET_PX` along the line's visual left-normal. Used by the
 * click-to-edit hit test and to place the floating value-input pill (the 3D
 * label sprite mirrors the same offset). `null` for a (near) zero-length
 * screen segment.
 */
export function distanceLabelAnchorPx(
  start: [number, number],
  end: [number, number],
  cam: Readonly<SketchCameraState>,
): [number, number] | null {
  const [sx, sy] = sketchToScreen(start[0], start[1], cam);
  const [ex, ey] = sketchToScreen(end[0], end[1], cam);
  const dxScreen = ex - sx;
  const dyScreen = ey - sy;
  const lenScreen = Math.hypot(dxScreen, dyScreen);
  if (lenScreen < 1) return null;
  // Left-normal in screen space: rotate the direction 90° CCW.
  // Canvas has screen-Y pointing *down*, so "left as you face
  // from start to end" maps to rotating the direction vector
  // by -90° (CW) in canvas coordinates. `left = (-dy, dx)` is
  // the mathematical CCW, which is visually "right" on a
  // Y-down canvas — so we negate to get visual left.
  const nx = dyScreen / lenScreen;
  const ny = -dxScreen / lenScreen;
  return [
    (sx + ex) / 2 + nx * LABEL_OFFSET_PX,
    (sy + ey) / 2 + ny * LABEL_OFFSET_PX,
  ];
}
