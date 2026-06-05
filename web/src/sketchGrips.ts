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
  | "arc-center"
  | "arc-rim";

export interface Grip {
  /** Index into `subCalls` of the geometry call this grip edits. */
  subCallIndex: number;
  role: GripRole;
  /** Current handle position in sketch-plane millimetres. */
  pos: [number, number];
  /** For `polygon-vertex`: which vertex (index into the flat
   *  `[x0,y0,x1,y1,…]` array, divided by two) this grip edits. */
  vertexIndex?: number;
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
        const startGrip: Grip = { subCallIndex: i, role: "line-start", pos: start };
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
        const bl: Grip = { subCallIndex: i, role: "rect-bl", pos: [minX, minY] };
        const br: Grip = { subCallIndex: i, role: "rect-br", pos: [maxX, minY] };
        const tr: Grip = { subCallIndex: i, role: "rect-tr", pos: [maxX, maxY] };
        const tl: Grip = { subCallIndex: i, role: "rect-tl", pos: [minX, maxY] };
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
            { subCallIndex: i, role: "polygon-vertex", pos: [x, y], vertexIndex: v },
          ]);
        }
        break;
      }
      case "sketch_arc": {
        // Only the centre point is modelled as an entity. Map it to
        // both a centre grip and a radius grip placed at the arc's
        // mid-sweep angle (analogous to a circle's rim).
        const center = asXy(call.params.center);
        if (!center) break;
        const radius = Number(call.params.radius);
        const start = Number(call.params.start_angle);
        const sweep = Number(call.params.sweep_angle);
        const centerId = id++;
        const centerGrip: Grip = {
          subCallIndex: i,
          role: "arc-center",
          pos: center,
        };
        const grips = [centerGrip];
        if (Number.isFinite(radius) && Number.isFinite(start) && Number.isFinite(sweep)) {
          const mid = start + sweep / 2;
          grips.push({
            subCallIndex: i,
            role: "arc-rim",
            pos: [center[0] + radius * Math.cos(mid), center[1] + radius * Math.sin(mid)],
          });
        }
        byEntity.set(centerId, grips);
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
    next[grip.subCallIndex] = { ...call, params: { ...call.params, ...params } };
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
  }
  return next;
}
