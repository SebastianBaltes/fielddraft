/**
 * P0.3 — drag-edit grips for committed sketch geometry.
 *
 * A *grip* is a draggable handle on a committed entity: a line's
 * two endpoints, a free point, a circle's centre and its radius
 * rim. Dragging a grip rewrites the underlying sub-call coordinate
 * and the live solver then pulls the rest of the sketch back onto
 * its constraints (see `solveAndReplace`).
 *
 * The grip set is derived from the current selection. Entity ids
 * are allocated in the exact same order as
 * [`buildDraftSketchState`](./sketchSession) — origin = 0, then one
 * bump per point/line/circle/… sub-call — so a selected id maps
 * deterministically onto the sub-call that owns it and the role it
 * plays there. This module mirrors that allocation; if the kernel's
 * allocation order ever changes, both must move together.
 *
 * Scope: lines, points, circles, rectangles (axis-aligned corner
 * resize), polygons (per-vertex drag) and arcs (centre + radius).
 */

export type SubCall = { type: string; params: Record<string, unknown> };

export type GripRole =
  | "point"
  | "line-start"
  | "line-end"
  | "circle-center"
  | "circle-rim"
  // Rectangle corners. Dragging one keeps the diagonally opposite
  // corner put so the box stays axis-aligned.
  | "rect-bl"
  | "rect-br"
  | "rect-tr"
  | "rect-tl"
  | "polygon-vertex"
  | "spline-point"
  // sketch_path on-/off-curve vertex. The exact field is carried by
  // `pathTarget` (contour start, a segment endpoint, or a quad
  // control handle).
  | "path-vertex"
  | "arc-center"
  | "arc-start"
  | "arc-end"
  | "arc-rim";

/** Which coordinate of a sketch_path a `path-vertex` grip edits. */
export type PathGripTarget =
  | { contourIndex: number; kind: "start" }
  | { contourIndex: number; kind: "to"; segIndex: number }
  | { contourIndex: number; kind: "ctrl"; segIndex: number };

export interface Grip {
  /** Index into `subCalls` of the geometry call this grip edits. */
  subCallIndex: number;
  role: GripRole;
  /** Current handle position in sketch-plane millimetres. */
  pos: [number, number];
  /** For `polygon-vertex`: which vertex (index into the flat
   *  `[x0,y0,x1,y1,…]` array, divided by two) this grip edits. */
  vertexIndex?: number;
  /** For `path-vertex`: which contour coordinate this grip edits. */
  pathTarget?: PathGripTarget;
}

function asXy(value: unknown): [number, number] | null {
  if (!Array.isArray(value) || value.length !== 2) return null;
  const a = Number(value[0]);
  const b = Number(value[1]);
  if (!Number.isFinite(a) || !Number.isFinite(b)) return null;
  return [a, b];
}

/**
 * Walk the sub-calls in allocation order and register, for every
 * entity id, the grips that editing it should expose. A line's
 * curve id maps to *both* endpoint grips; each endpoint point id
 * maps to its single grip; a circle's curve id maps to centre +
 * rim, its centre point id to just the centre.
 */
function buildGripIndex(subCalls: ReadonlyArray<SubCall>): Map<number, Grip[]> {
  const byEntity = new Map<number, Grip[]>();
  // Origin is id 0 and owns no editable grip.
  let id = 1;

  subCalls.forEach((call, i) => {
    switch (call.type) {
      case "sketch_point": {
        const at = asXy(call.params.at);
        if (!at) return;
        const pid = id++;
        byEntity.set(pid, [{ subCallIndex: i, role: "point", pos: at }]);
        break;
      }
      case "sketch_line": {
        const start = asXy(call.params.start);
        const end = asXy(call.params.end);
        if (!start || !end) return;
        const startId = id++;
        const endId = id++;
        const lineId = id++;
        const startGrip: Grip = {
          subCallIndex: i,
          role: "line-start",
          pos: start,
        };
        const endGrip: Grip = { subCallIndex: i, role: "line-end", pos: end };
        byEntity.set(startId, [startGrip]);
        byEntity.set(endId, [endGrip]);
        byEntity.set(lineId, [startGrip, endGrip]);
        break;
      }
      case "sketch_circle": {
        const center = asXy(call.params.center);
        const radius = Number(call.params.radius);
        if (!center || !Number.isFinite(radius)) return;
        const centerId = id++;
        const circleId = id++;
        const centerGrip: Grip = {
          subCallIndex: i,
          role: "circle-center",
          pos: center,
        };
        const rimGrip: Grip = {
          subCallIndex: i,
          role: "circle-rim",
          pos: [center[0] + radius, center[1]],
        };
        byEntity.set(centerId, [centerGrip]);
        byEntity.set(circleId, [centerGrip, rimGrip]);
        break;
      }
      case "sketch_rectangle": {
        // Kernel allocates: bl, br, tr, tl (points) then bottom,
        // right, top, left (lines) — 8 ids. The box is axis-aligned,
        // derived from min/max of the two stored corners.
        const a = asXy(call.params.corner_a);
        const b = asXy(call.params.corner_b);
        if (!a || !b) {
          id += 8;
          break;
        }
        const minX = Math.min(a[0], b[0]);
        const minY = Math.min(a[1], b[1]);
        const maxX = Math.max(a[0], b[0]);
        const maxY = Math.max(a[1], b[1]);
        const bl: Grip = {
          subCallIndex: i,
          role: "rect-bl",
          pos: [minX, minY],
        };
        const br: Grip = {
          subCallIndex: i,
          role: "rect-br",
          pos: [maxX, minY],
        };
        const tr: Grip = {
          subCallIndex: i,
          role: "rect-tr",
          pos: [maxX, maxY],
        };
        const tl: Grip = {
          subCallIndex: i,
          role: "rect-tl",
          pos: [minX, maxY],
        };
        const blId = id++;
        const brId = id++;
        const trId = id++;
        const tlId = id++;
        const bottomId = id++;
        const rightId = id++;
        const topId = id++;
        const leftId = id++;
        // A corner point selects its own grip; an edge line (or the
        // whole rectangle) selects all four so any pick resizes it.
        byEntity.set(blId, [bl]);
        byEntity.set(brId, [br]);
        byEntity.set(trId, [tr]);
        byEntity.set(tlId, [tl]);
        const all = [bl, br, tr, tl];
        byEntity.set(bottomId, all);
        byEntity.set(rightId, all);
        byEntity.set(topId, all);
        byEntity.set(leftId, all);
        break;
      }
      case "sketch_polygon": {
        const verts = call.params.vertices;
        if (!Array.isArray(verts)) break;
        const n = Math.floor(verts.length / 2);
        for (let v = 0; v < n; v++) {
          const x = Number(verts[v * 2]);
          const y = Number(verts[v * 2 + 1]);
          const vid = id++;
          if (!Number.isFinite(x) || !Number.isFinite(y)) continue;
          byEntity.set(vid, [
            {
              subCallIndex: i,
              role: "polygon-vertex",
              pos: [x, y],
              vertexIndex: v,
            },
          ]);
        }
        break;
      }
      case "sketch_spline": {
        // One point entity per control point (mirrors the kernel
        // `SketchSplineTool::apply`). Each control point gets a
        // draggable vertex grip; editing it re-fits the smooth curve.
        const pts = call.params.points;
        if (!Array.isArray(pts)) break;
        const n = Math.floor(pts.length / 2);
        for (let v = 0; v < n; v++) {
          const x = Number(pts[v * 2]);
          const y = Number(pts[v * 2 + 1]);
          const vid = id++;
          if (!Number.isFinite(x) || !Number.isFinite(y)) continue;
          byEntity.set(vid, [
            {
              subCallIndex: i,
              role: "spline-point",
              pos: [x, y],
              vertexIndex: v,
            },
          ]);
        }
        // Advance past the entities buildDraftSketchState allocates after
        // the control points but that carry no own grip, so later
        // sub-calls stay id-aligned: the endpoint leg line(s) (any open
        // spline) and the B-spline entity (control mode + open only).
        if (call.params.closed !== true && n >= 2) {
          id += 1; // start leg
          if (n >= 3) id += 1; // end leg
        }
        if (
          call.params.mode === "control" &&
          call.params.closed !== true &&
          n >= 2
        ) {
          id += 1; // B-spline entity
        }
        break;
      }
      case "sketch_path": {
        // One point entity per on-/off-curve vertex (mirrors the kernel
        // `SketchPathTool::apply`). CANONICAL order per contour: start,
        // then per segment (Quad → ctrl, to; Line → to). Each vertex
        // gets a draggable grip writing back into its contour field.
        const contours = call.params.contours;
        if (!Array.isArray(contours)) break;
        contours.forEach((c, contourIndex) => {
          const cc = c as { start?: unknown; segments?: unknown };
          const start = asXy(cc?.start);
          const startId = id++;
          if (start) {
            byEntity.set(startId, [
              {
                subCallIndex: i,
                role: "path-vertex",
                pos: start,
                pathTarget: { contourIndex, kind: "start" },
              },
            ]);
          }
          const segments = cc?.segments;
          if (!Array.isArray(segments)) return;
          segments.forEach((seg, segIndex) => {
            const s = seg as { kind?: unknown; ctrl?: unknown; to?: unknown };
            if (s?.kind === "quad") {
              const ctrl = asXy(s.ctrl);
              const ctrlId = id++;
              if (ctrl) {
                byEntity.set(ctrlId, [
                  {
                    subCallIndex: i,
                    role: "path-vertex",
                    pos: ctrl,
                    pathTarget: { contourIndex, kind: "ctrl", segIndex },
                  },
                ]);
              }
            }
            const to = asXy(s?.to);
            const toId = id++;
            if (to) {
              byEntity.set(toId, [
                {
                  subCallIndex: i,
                  role: "path-vertex",
                  pos: to,
                  pathTarget: { contourIndex, kind: "to", segIndex },
                },
              ]);
            }
          });
        });
        break;
      }
      case "sketch_arc": {
        // Full arc entity model (mirrors buildDraftSketchState):
        // centre + start + end points, then the arc entity. Endpoints
        // are draggable grips (ticket a243c50f); the mid-sweep rim
        // grip drags the radius (analogous to a circle's rim).
        const center = asXy(call.params.center);
        if (!center) break;
        const radius = Number(call.params.radius);
        const start = Number(call.params.start_angle);
        const sweep = Number(call.params.sweep_angle);
        if (
          !Number.isFinite(radius) ||
          !Number.isFinite(start) ||
          !Number.isFinite(sweep)
        )
          break;
        const centerId = id++;
        const startId = id++;
        const endId = id++;
        const arcId = id++;
        const at = (angle: number): [number, number] => [
          center[0] + radius * Math.cos(angle),
          center[1] + radius * Math.sin(angle),
        ];
        const centerGrip: Grip = {
          subCallIndex: i,
          role: "arc-center",
          pos: center,
        };
        const startGrip: Grip = {
          subCallIndex: i,
          role: "arc-start",
          pos: at(start),
        };
        const endGrip: Grip = {
          subCallIndex: i,
          role: "arc-end",
          pos: at(start + sweep),
        };
        const rimGrip: Grip = {
          subCallIndex: i,
          role: "arc-rim",
          pos: at(start + sweep / 2),
        };
        byEntity.set(centerId, [centerGrip]);
        byEntity.set(startId, [startGrip]);
        byEntity.set(endId, [endGrip]);
        byEntity.set(arcId, [centerGrip, startGrip, endGrip, rimGrip]);
        break;
      }
      default:
        // Constraint sub-calls allocate no entities.
        break;
    }
  });

  return byEntity;
}

/**
 * Grips to show (and allow dragging) for the current selection.
 * Deduplicated by (subCallIndex, role) so selecting both a line and
 * one of its endpoints doesn't double up.
 */
export function gripsForSelection(
  subCalls: ReadonlyArray<SubCall>,
  selection: ReadonlyArray<number>,
): Grip[] {
  if (selection.length === 0) return [];
  const index = buildGripIndex(subCalls);
  const out: Grip[] = [];
  const seen = new Set<string>();
  for (const id of selection) {
    const grips = index.get(id);
    if (!grips) continue;
    for (const g of grips) {
      const key = `${g.subCallIndex}:${g.role}`;
      if (seen.has(key)) continue;
      seen.add(key);
      out.push(g);
    }
  }
  return out;
}

/** Every editable grip of every committed entity, in allocation order.
 *  Unlike {@link gripsForSelection} this ignores the current selection —
 *  it's the set a *direct* pointer-down can grab (ticket t11x: mouse-down
 *  on a point/handle starts the drag immediately, without a prior click to
 *  select). Deduplicated by (subCallIndex, role) so a line's endpoint isn't
 *  listed twice (once for the point id, once for the line id). */
export function allGrips(subCalls: ReadonlyArray<SubCall>): Grip[] {
  const index = buildGripIndex(subCalls);
  const out: Grip[] = [];
  const seen = new Set<string>();
  for (const grips of index.values()) {
    for (const g of grips) {
      const key = `${g.subCallIndex}:${g.role}:${g.vertexIndex ?? ""}`;
      if (seen.has(key)) continue;
      seen.add(key);
      out.push(g);
    }
  }
  return out;
}

/**
 * Ticket t11x: pick the grip nearest the pointer within `tolPx` screen
 * pixels, searching *all* committed entities (not just the selection).
 * `project` maps a grip's sketch-mm position to canvas pixels so the
 * tolerance is zoom-independent and the function stays free of camera
 * internals (and therefore unit-testable). Returns the closest grip, or
 * `null` if none is within tolerance.
 */
export function pickGripAt(
  grips: ReadonlyArray<Grip>,
  pointerPx: readonly [number, number],
  project: (pos: readonly [number, number]) => [number, number],
  tolPx: number,
): Grip | null {
  let best: Grip | null = null;
  let bestD = tolPx;
  for (const g of grips) {
    const [gx, gy] = project(g.pos);
    const d = Math.hypot(pointerPx[0] - gx, pointerPx[1] - gy);
    if (d <= bestD) {
      bestD = d;
      best = g;
    }
  }
  return best;
}

/** Value-equality for two grips (the picked grip and one rebuilt by the
 *  index may be distinct object instances). */
function sameGrip(a: Grip, b: Grip): boolean {
  return (
    a.subCallIndex === b.subCallIndex &&
    a.role === b.role &&
    (a.vertexIndex ?? -1) === (b.vertexIndex ?? -1) &&
    JSON.stringify(a.pathTarget ?? null) === JSON.stringify(b.pathTarget ?? null)
  );
}

/** The entity id that owns `grip` — i.e. the id whose pick should select
 *  the dragged geometry. Mirrors the allocation in {@link buildGripIndex}:
 *  it's the id mapped to a single-grip role (a free point, a line endpoint,
 *  a polygon/spline vertex, a rectangle corner, an arc/circle centre). For
 *  whole-curve grips (line/circle/arc body) we return the most specific
 *  owning id. Returns `null` if no owner can be resolved. */
export function entityIdForGrip(
  subCalls: ReadonlyArray<SubCall>,
  grip: Grip,
): number | null {
  const index = buildGripIndex(subCalls);
  // Prefer a single-grip entry — that's the grip's own handle id (point,
  // endpoint, vertex, corner, centre), the most specific selectable target.
  for (const [id, grips] of index.entries()) {
    if (grips.length === 1 && sameGrip(grips[0], grip)) return id;
  }
  // Fall back: the first id whose grip list contains this grip (whole-curve
  // ids list several grips; selecting any is acceptable for the drag).
  for (const [id, grips] of index.entries()) {
    if (grips.some((g) => sameGrip(g, grip))) return id;
  }
  return null;
}

/**
 * Return a new sub-calls array with `grip`'s coordinate moved to
 * `to`. The dragged geometry call is replaced immutably; every
 * other call (including constraints) is shared unchanged. The
 * circle rim maps the cursor distance to a new radius and leaves
 * the centre put.
 */
export function applyGripDrag(
  subCalls: ReadonlyArray<SubCall>,
  grip: Grip,
  to: [number, number],
): SubCall[] {
  const next = subCalls.slice();
  const call = next[grip.subCallIndex];
  if (!call) return next;

  const replace = (params: Record<string, unknown>) => {
    next[grip.subCallIndex] = {
      ...call,
      params: { ...call.params, ...params },
    };
  };

  switch (grip.role) {
    case "point":
      replace({ at: [to[0], to[1]] });
      break;
    case "line-start":
      replace({ start: [to[0], to[1]] });
      break;
    case "line-end":
      replace({ end: [to[0], to[1]] });
      break;
    case "circle-center":
      replace({ center: [to[0], to[1]] });
      break;
    case "circle-rim": {
      const center = asXy(call.params.center);
      if (!center) break;
      const radius = Math.hypot(to[0] - center[0], to[1] - center[1]);
      if (radius > 1e-6) replace({ radius });
      break;
    }
    case "rect-bl":
    case "rect-br":
    case "rect-tr":
    case "rect-tl": {
      const a = asXy(call.params.corner_a);
      const b = asXy(call.params.corner_b);
      if (!a || !b) break;
      const minX = Math.min(a[0], b[0]);
      const minY = Math.min(a[1], b[1]);
      const maxX = Math.max(a[0], b[0]);
      const maxY = Math.max(a[1], b[1]);
      // The corner diagonally opposite the dragged one stays put; the
      // box re-derives from {dragged, opposite} so it stays
      // axis-aligned even if the cursor crosses over.
      const opposite: [number, number] =
        grip.role === "rect-bl"
          ? [maxX, maxY]
          : grip.role === "rect-br"
            ? [minX, maxY]
            : grip.role === "rect-tr"
              ? [minX, minY]
              : [maxX, minY];
      replace({ corner_a: opposite, corner_b: [to[0], to[1]] });
      break;
    }
    case "polygon-vertex": {
      const verts = call.params.vertices;
      if (!Array.isArray(verts) || grip.vertexIndex === undefined) break;
      const next2 = verts.slice();
      next2[grip.vertexIndex * 2] = to[0];
      next2[grip.vertexIndex * 2 + 1] = to[1];
      replace({ vertices: next2 });
      break;
    }
    case "spline-point": {
      const pts = call.params.points;
      if (!Array.isArray(pts) || grip.vertexIndex === undefined) break;
      const next2 = pts.slice();
      next2[grip.vertexIndex * 2] = to[0];
      next2[grip.vertexIndex * 2 + 1] = to[1];
      replace({ points: next2 });
      break;
    }
    case "path-vertex": {
      const target = grip.pathTarget;
      const contours = call.params.contours;
      if (!target || !Array.isArray(contours)) break;
      const newContours = contours.map((c, ci) => {
        if (ci !== target.contourIndex) return c;
        const cc = c as { start?: unknown; segments?: unknown };
        if (target.kind === "start") {
          return { ...(cc as object), start: [to[0], to[1]] };
        }
        const segments = Array.isArray(cc?.segments) ? cc.segments : [];
        const newSegments = segments.map((seg, si) => {
          if (si !== target.segIndex) return seg;
          const s = seg as Record<string, unknown>;
          if (target.kind === "ctrl") {
            return { ...s, ctrl: [to[0], to[1]] };
          }
          return { ...s, to: [to[0], to[1]] };
        });
        return { ...(cc as object), segments: newSegments };
      });
      replace({ contours: newContours });
      break;
    }
    case "arc-center":
      replace({ center: [to[0], to[1]] });
      break;
    case "arc-rim": {
      const center = asXy(call.params.center);
      if (!center) break;
      const radius = Math.hypot(to[0] - center[0], to[1] - center[1]);
      if (radius > 1e-6) replace({ radius });
      break;
    }
    case "arc-start":
    case "arc-end": {
      // Dragging an endpoint re-aims its angle AND the radius (the
      // centre stays put — Onshape behaviour). The other endpoint
      // keeps its angle; the sweep is re-derived so the arc still
      // ends at the same angular position, unwrapped to stay near
      // the previous sweep (so a CW arc doesn't flip CCW mid-drag).
      const center = asXy(call.params.center);
      const start = Number(call.params.start_angle);
      const sweep = Number(call.params.sweep_angle);
      if (!center || !Number.isFinite(start) || !Number.isFinite(sweep)) break;
      const radius = Math.hypot(to[0] - center[0], to[1] - center[1]);
      if (radius <= 1e-6) break;
      const angle = Math.atan2(to[1] - center[1], to[0] - center[0]);
      if (grip.role === "arc-start") {
        const endAngle = start + sweep;
        let newSweep = endAngle - angle;
        while (newSweep - sweep > Math.PI) newSweep -= Math.PI * 2;
        while (newSweep - sweep < -Math.PI) newSweep += Math.PI * 2;
        replace({ radius, start_angle: angle, sweep_angle: newSweep });
      } else {
        let newSweep = angle - start;
        while (newSweep - sweep > Math.PI) newSweep -= Math.PI * 2;
        while (newSweep - sweep < -Math.PI) newSweep += Math.PI * 2;
        replace({ radius, sweep_angle: newSweep });
      }
      break;
    }
  }
  return next;
}
