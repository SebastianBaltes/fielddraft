/**
 * Sketch outline extraction (DOM-free): flatten a sketch's sub-calls
 * into world-space line segments on its plane frame. Shared by the
 * viewport's sketch overlay rendering and the extrude gizmo's
 * pending-sketch-extrude anchor (the suppressed ghost contributes no
 * B-Rep, so its preview prism derives from the raw sketch geometry).
 *
 * Curve coverage mirrors what the overlay has always drawn: lines,
 * rectangles, circles, polygons, arcs. Splines/ellipses fall back to
 * nothing here — the preview outline is approximate by design; the
 * kernel boolean produces the exact geometry on confirm.
 */

import { planeToWorld, type PlaneFrame } from "./sketchPlaneFrame";
import { slotOutline } from "./sketchSlot";

export interface SketchSubCall {
  type: string;
  params: Record<string, unknown>;
}

/** Flat `[ax,ay,az, bx,by,bz, ...]` world-space segment pairs of the
 *  sketch's drawable sub-calls, projected through `frame`. */
export function collectSketchLineSegments(
  subCalls: SketchSubCall[],
  frame: PlaneFrame,
): number[] {
  const out: number[] = [];
  const emit = (a: [number, number], b: [number, number]) => {
    out.push(...planeToWorld(a[0], a[1], frame), ...planeToWorld(b[0], b[1], frame));
  };

  const getVec2 = (v: unknown): [number, number] | null => {
    if (Array.isArray(v) && v.length >= 2 && typeof v[0] === "number" && typeof v[1] === "number") {
      return [v[0], v[1]];
    }
    return null;
  };
  const getNum = (v: unknown): number | null =>
    typeof v === "number" ? v : null;

  for (const call of subCalls) {
    const p = call.params;
    switch (call.type) {
      case "sketch_line": {
        const a = getVec2(p.start);
        const b = getVec2(p.end);
        if (a && b) emit(a, b);
        break;
      }
      case "sketch_rectangle": {
        const a = getVec2(p.corner_a);
        const b = getVec2(p.corner_b);
        if (a && b) {
          const x0 = Math.min(a[0], b[0]);
          const x1 = Math.max(a[0], b[0]);
          const y0 = Math.min(a[1], b[1]);
          const y1 = Math.max(a[1], b[1]);
          emit([x0, y0], [x1, y0]);
          emit([x1, y0], [x1, y1]);
          emit([x1, y1], [x0, y1]);
          emit([x0, y1], [x0, y0]);
        }
        break;
      }
      case "sketch_circle": {
        const c = getVec2(p.center);
        const r = getNum(p.radius);
        if (c && r !== null && r > 0) {
          const steps = 64;
          let prev: [number, number] = [c[0] + r, c[1]];
          for (let i = 1; i <= steps; i++) {
            const t = (i / steps) * Math.PI * 2;
            const next: [number, number] = [
              c[0] + r * Math.cos(t),
              c[1] + r * Math.sin(t),
            ];
            emit(prev, next);
            prev = next;
          }
        }
        break;
      }
      case "sketch_polygon": {
        const verts = p.vertices;
        if (Array.isArray(verts) && verts.length >= 4) {
          const points: [number, number][] = [];
          for (let i = 0; i + 1 < verts.length; i += 2) {
            const x = Number(verts[i]);
            const y = Number(verts[i + 1]);
            if (Number.isFinite(x) && Number.isFinite(y)) {
              points.push([x, y]);
            }
          }
          for (let i = 0; i < points.length; i++) {
            const a = points[i];
            const b = points[(i + 1) % points.length];
            emit(a, b);
          }
        }
        break;
      }
      case "sketch_arc": {
        const c = getVec2(p.center);
        const r = getNum(p.radius);
        const a0 = getNum(p.start_angle);
        const sweep = getNum(p.sweep_angle);
        if (c && r !== null && r > 0 && a0 !== null && sweep !== null) {
          const a1 = a0 + sweep;
          const steps = Math.max(8, Math.ceil((Math.abs(a1 - a0) / Math.PI) * 32));
          let prev: [number, number] = [
            c[0] + r * Math.cos(a0),
            c[1] + r * Math.sin(a0),
          ];
          for (let i = 1; i <= steps; i++) {
            const t = a0 + ((a1 - a0) * i) / steps;
            const next: [number, number] = [
              c[0] + r * Math.cos(t),
              c[1] + r * Math.sin(t),
            ];
            emit(prev, next);
            prev = next;
          }
        }
        break;
      }
      default:
        break;
    }
  }
  return out;
}

/** Entity types whose committed outline is rendered as real 3D geometry
 *  in the scene graph (fat lines + vertex points) rather than reprojected
 *  on the 2D overlay each frame. Construction geometry and the richer
 *  editing affordances (spline control polygons, path handles) stay on
 *  the overlay for now. */
export const SESSION_3D_TYPES: ReadonlySet<string> = new Set([
  "sketch_line",
  "sketch_rectangle",
  "sketch_circle",
  "sketch_polygon",
  "sketch_arc",
  "sketch_ellipse",
  "sketch_slot",
]);

export interface SketchSessionGeometry {
  /** Flat world-space segment pairs `[ax,ay,az, bx,by,bz, ...]` for the
   *  committed outline strokes. */
  outline: number[];
  /** Flat world-space marker positions `[x,y,z, ...]` for committed
   *  vertex/centre dots (matches what the overlay drew per entity). */
  points: number[];
}

/**
 * Flatten the active sketch session's committed geometry into world-space
 * 3D primitives on its plane frame: line/curve outlines as segment pairs
 * and vertex/centre markers as points. Covers exactly {@link SESSION_3D_TYPES}
 * and skips construction geometry (which the overlay still renders dashed).
 *
 * The per-type outline + point set mirrors `drawOverlay` in `SketchCanvas2d`
 * one-for-one so moving these to the GPU is visually transparent.
 */
export function collectSketchSessionGeometry(
  subCalls: SketchSubCall[],
  frame: PlaneFrame,
): SketchSessionGeometry {
  const outline: number[] = [];
  const points: number[] = [];
  const seg = (a: [number, number], b: [number, number]) => {
    outline.push(...planeToWorld(a[0], a[1], frame), ...planeToWorld(b[0], b[1], frame));
  };
  const pt = (a: [number, number]) => {
    points.push(...planeToWorld(a[0], a[1], frame));
  };
  const arc = (
    cx: number,
    cy: number,
    r: number,
    a0: number,
    sweep: number,
    steps: number,
  ) => {
    let prev: [number, number] = [cx + r * Math.cos(a0), cy + r * Math.sin(a0)];
    for (let i = 1; i <= steps; i++) {
      const t = a0 + (sweep * i) / steps;
      const next: [number, number] = [cx + r * Math.cos(t), cy + r * Math.sin(t)];
      seg(prev, next);
      prev = next;
    }
  };

  const getVec2 = (v: unknown): [number, number] | null => {
    if (Array.isArray(v) && v.length >= 2 && typeof v[0] === "number" && typeof v[1] === "number") {
      return [v[0], v[1]];
    }
    return null;
  };
  const getNum = (v: unknown): number | null => (typeof v === "number" ? v : null);

  for (const call of subCalls) {
    if (call.params.construction) continue;
    if (!SESSION_3D_TYPES.has(call.type)) continue;
    const p = call.params;
    switch (call.type) {
      case "sketch_line": {
        const a = getVec2(p.start);
        const b = getVec2(p.end);
        if (a && b) {
          seg(a, b);
          pt(a);
          pt(b);
        }
        break;
      }
      case "sketch_rectangle": {
        const a = getVec2(p.corner_a);
        const b = getVec2(p.corner_b);
        if (a && b) {
          const x0 = Math.min(a[0], b[0]);
          const x1 = Math.max(a[0], b[0]);
          const y0 = Math.min(a[1], b[1]);
          const y1 = Math.max(a[1], b[1]);
          seg([x0, y0], [x1, y0]);
          seg([x1, y0], [x1, y1]);
          seg([x1, y1], [x0, y1]);
          seg([x0, y1], [x0, y0]);
        }
        break;
      }
      case "sketch_circle": {
        const c = getVec2(p.center);
        const r = getNum(p.radius);
        if (c && r !== null && r > 0) {
          arc(c[0], c[1], r, 0, Math.PI * 2, 64);
          pt(c);
        }
        break;
      }
      case "sketch_polygon": {
        const verts = p.vertices;
        if (Array.isArray(verts) && verts.length >= 4) {
          const pts: [number, number][] = [];
          for (let i = 0; i + 1 < verts.length; i += 2) {
            const x = Number(verts[i]);
            const y = Number(verts[i + 1]);
            if (Number.isFinite(x) && Number.isFinite(y)) pts.push([x, y]);
          }
          for (let i = 0; i < pts.length; i++) {
            seg(pts[i], pts[(i + 1) % pts.length]);
            pt(pts[i]);
          }
        }
        break;
      }
      case "sketch_arc": {
        const c = getVec2(p.center);
        const r = getNum(p.radius);
        const a0 = getNum(p.start_angle);
        const sweep = getNum(p.sweep_angle);
        if (c && r !== null && r > 0 && a0 !== null && sweep !== null) {
          const steps = Math.max(8, Math.ceil((Math.abs(sweep) / Math.PI) * 32));
          arc(c[0], c[1], r, a0, sweep, steps);
          pt(c);
        }
        break;
      }
      case "sketch_ellipse": {
        const c = getVec2(p.center);
        const rx = getNum(p.radius_x);
        const ry = getNum(p.radius_y);
        const rot = getNum(p.rotation) ?? 0;
        if (c && rx !== null && ry !== null && rx > 0 && ry > 0) {
          const cosR = Math.cos(rot);
          const sinR = Math.sin(rot);
          const steps = 64;
          const at = (t: number): [number, number] => {
            const ex = rx * Math.cos(t);
            const ey = ry * Math.sin(t);
            return [c[0] + ex * cosR - ey * sinR, c[1] + ex * sinR + ey * cosR];
          };
          let prev = at(0);
          for (let i = 1; i <= steps; i++) {
            const next = at((i / steps) * Math.PI * 2);
            seg(prev, next);
            prev = next;
          }
          pt(c);
        }
        break;
      }
      case "sketch_slot": {
        const a = getVec2(p.center_a);
        const b = getVec2(p.center_b);
        const r = getNum(p.radius);
        if (a && b && r !== null && r > 0) {
          const o = slotOutline(a, b, r);
          if (o) {
            const caps = Math.max(8, Math.ceil(0.5 * 32));
            seg(o.aPlus, o.bPlus);
            arc(o.centerB[0], o.centerB[1], o.radius, o.capBStart, -Math.PI, caps);
            seg(o.bMinus, o.aMinus);
            arc(o.centerA[0], o.centerA[1], o.radius, o.capAStart, -Math.PI, caps);
            pt(a);
            pt(b);
          }
        }
        break;
      }
      default:
        break;
    }
  }
  return { outline, points };
}
