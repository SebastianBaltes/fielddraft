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
import { bsplinePoints, catmullRomPoints, type Vec2 } from "./sketchSpline";
import { closedRegionsCached } from "./sketchRegions";

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

/** Committed sketch entity types whose geometry renders in the 3D scene
 *  graph ({@link collectSketchSessionGeometry}) rather than reprojected on
 *  the 2D overlay. Covers every drawable entity, so `drawOverlay` no longer
 *  strokes committed geometry at all. */
export const SESSION_3D_TYPES: ReadonlySet<string> = new Set([
  "sketch_line",
  "sketch_point",
  "sketch_rectangle",
  "sketch_circle",
  "sketch_polygon",
  "sketch_arc",
  "sketch_ellipse",
  "sketch_slot",
  "sketch_spline",
  "sketch_path",
]);

export interface SketchSessionGeometry {
  /** DOF-coloured outline segment pairs (non-construction geometry). */
  solid: number[];
  /** Construction-geometry outline segment pairs (rendered dashed/muted). */
  construction: number[];
  /** Dashed guide segments: spline control polygons, path off-curve
   *  handle connectors. */
  guide: number[];
  /** Spline endpoint tangent-leg segments (accent colour) — selectable
   *  line entities the user can constrain. */
  legs: number[];
  /** Filled on-curve vertex / centre marker positions `[x,y,z, ...]`. */
  points: number[];
  /** Off-curve control-handle marker positions (quad control points). */
  handles: number[];
}

/**
 * Flatten the active sketch session's committed geometry into world-space
 * 3D primitives on its plane frame, one category per material/object the
 * 3D layer renders. Covers EVERY drawable entity (so the 2D overlay no
 * longer strokes committed geometry); the per-type outline/point/guide
 * set mirrors `drawOverlay` in `SketchCanvas2d` so the move to the GPU is
 * visually transparent.
 */
export function collectSketchSessionGeometry(
  subCalls: SketchSubCall[],
  frame: PlaneFrame,
): SketchSessionGeometry {
  const solid: number[] = [];
  const construction: number[] = [];
  const guide: number[] = [];
  const legs: number[] = [];
  const points: number[] = [];
  const handles: number[] = [];
  const w = (a: Vec2): [number, number, number] => planeToWorld(a[0], a[1], frame);
  const seg = (target: number[], a: Vec2, b: Vec2) => {
    target.push(...w(a), ...w(b));
  };
  const arc = (
    target: number[],
    cx: number,
    cy: number,
    r: number,
    a0: number,
    sweep: number,
    steps: number,
  ) => {
    let prev: Vec2 = [cx + r * Math.cos(a0), cy + r * Math.sin(a0)];
    for (let i = 1; i <= steps; i++) {
      const t = a0 + (sweep * i) / steps;
      const next: Vec2 = [cx + r * Math.cos(t), cy + r * Math.sin(t)];
      seg(target, prev, next);
      prev = next;
    }
  };

  const getVec2 = (v: unknown): Vec2 | null => {
    if (Array.isArray(v) && v.length >= 2 && typeof v[0] === "number" && typeof v[1] === "number") {
      return [v[0], v[1]];
    }
    return null;
  };
  const getNum = (v: unknown): number | null => (typeof v === "number" ? v : null);

  for (const call of subCalls) {
    const p = call.params;
    const isC = Boolean(p.construction);
    // Outline strokes route to solid vs construction; affordances/points
    // only render for real (non-construction) geometry, matching drawOverlay.
    const line = isC ? construction : solid;
    const pt = (a: Vec2) => {
      if (!isC) points.push(...w(a));
    };
    switch (call.type) {
      case "sketch_line": {
        const a = getVec2(p.start);
        const b = getVec2(p.end);
        if (a && b) {
          seg(line, a, b);
          pt(a);
          pt(b);
        }
        break;
      }
      case "sketch_point": {
        const a = getVec2(p.at);
        if (a) pt(a);
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
          seg(line, [x0, y0], [x1, y0]);
          seg(line, [x1, y0], [x1, y1]);
          seg(line, [x1, y1], [x0, y1]);
          seg(line, [x0, y1], [x0, y0]);
        }
        break;
      }
      case "sketch_circle": {
        const c = getVec2(p.center);
        const r = getNum(p.radius);
        if (c && r !== null && r > 0) {
          arc(line, c[0], c[1], r, 0, Math.PI * 2, 64);
          pt(c);
        }
        break;
      }
      case "sketch_polygon": {
        const verts = p.vertices;
        if (Array.isArray(verts) && verts.length >= 4) {
          const pts: Vec2[] = [];
          for (let i = 0; i + 1 < verts.length; i += 2) {
            const x = Number(verts[i]);
            const y = Number(verts[i + 1]);
            if (Number.isFinite(x) && Number.isFinite(y)) pts.push([x, y]);
          }
          for (let i = 0; i < pts.length; i++) {
            seg(line, pts[i], pts[(i + 1) % pts.length]);
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
          arc(line, c[0], c[1], r, a0, sweep, steps);
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
          const at = (t: number): Vec2 => {
            const ex = rx * Math.cos(t);
            const ey = ry * Math.sin(t);
            return [c[0] + ex * cosR - ey * sinR, c[1] + ex * sinR + ey * cosR];
          };
          let prev = at(0);
          for (let i = 1; i <= steps; i++) {
            const next = at((i / steps) * Math.PI * 2);
            seg(line, prev, next);
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
            const caps = 16;
            seg(line, o.aPlus, o.bPlus);
            arc(line, o.centerB[0], o.centerB[1], o.radius, o.capBStart, -Math.PI, caps);
            seg(line, o.bMinus, o.aMinus);
            arc(line, o.centerA[0], o.centerA[1], o.radius, o.capAStart, -Math.PI, caps);
            pt(a);
            pt(b);
          }
        }
        break;
      }
      case "sketch_spline": {
        const raw = p.points;
        const closed = p.closed === true;
        const isControl = p.mode === "control";
        if (Array.isArray(raw) && raw.length >= 4) {
          const ctrl: Vec2[] = [];
          for (let i = 0; i + 1 < raw.length; i += 2) {
            ctrl.push([Number(raw[i]), Number(raw[i + 1])]);
          }
          const n = ctrl.length;
          const legSet = new Set<number>();
          if (!closed && n >= 2) {
            legSet.add(0);
            if (n >= 3) legSet.add(n - 2);
          }
          if (!isC) {
            if (isControl) {
              for (let i = 0; i + 1 < n; i++) {
                if (legSet.has(i)) seg(legs, ctrl[i], ctrl[i + 1]);
                else seg(guide, ctrl[i], ctrl[i + 1]);
              }
              if (closed && n >= 2) seg(guide, ctrl[n - 1], ctrl[0]);
            } else {
              for (const i of legSet) seg(legs, ctrl[i], ctrl[i + 1]);
            }
          }
          const sampled = isControl
            ? bsplinePoints(ctrl, 3, closed, 64)
            : catmullRomPoints(ctrl, closed, 16);
          for (let i = 0; i + 1 < sampled.length; i++) {
            seg(line, sampled[i], sampled[i + 1]);
          }
          if (!isControl && closed && sampled.length >= 2) {
            seg(line, sampled[sampled.length - 1], sampled[0]);
          }
          for (const c of ctrl) pt(c);
        }
        break;
      }
      case "sketch_path": {
        const contours = p.contours;
        if (Array.isArray(contours)) {
          for (const c of contours) {
            const cc = c as { start?: unknown; segments?: unknown; closed?: unknown };
            const start = cc.start;
            const segs = cc.segments;
            if (!Array.isArray(start) || start.length !== 2 || !Array.isArray(segs)) continue;
            let prev: Vec2 = [Number(start[0]), Number(start[1])];
            const first = prev;
            if (!isC) pt(first);
            for (const s of segs) {
              const sg = s as { kind?: unknown; to?: unknown; ctrl?: unknown };
              const to = sg.to;
              if (!Array.isArray(to) || to.length !== 2) continue;
              const end: Vec2 = [Number(to[0]), Number(to[1])];
              if (sg.kind === "quad" && Array.isArray(sg.ctrl)) {
                const cp: Vec2 = [Number(sg.ctrl[0]), Number(sg.ctrl[1])];
                const STEPS = 16;
                let a = prev;
                for (let k = 1; k <= STEPS; k++) {
                  const t = k / STEPS;
                  const mt = 1 - t;
                  const b: Vec2 = [
                    mt * mt * prev[0] + 2 * mt * t * cp[0] + t * t * end[0],
                    mt * mt * prev[1] + 2 * mt * t * cp[1] + t * t * end[1],
                  ];
                  seg(line, a, b);
                  a = b;
                }
                if (!isC) {
                  seg(guide, prev, cp);
                  seg(guide, cp, end);
                  handles.push(...w(cp));
                }
              } else {
                seg(line, prev, end);
              }
              if (!isC) pt(end);
              prev = end;
            }
            if (cc.closed === true) seg(line, prev, first);
          }
        }
        break;
      }
      default:
        break;
    }
  }
  return { solid, construction, guide, legs, points, handles };
}

/** Flat world-space triangle positions `[x,y,z, ...]` (3 per triangle) for
 *  the translucent fill of every closed region of the sketch — the 3D
 *  equivalent of `drawOverlay`'s `COLOR_REGION_FILL` canvas fill. Each
 *  region polygon is triangulated independently (ear clipping); overlapping
 *  loops therefore composite their alpha rather than using a single nonzero
 *  fill, an accepted minor difference. */
export function collectSketchRegionFill(
  subCalls: ReadonlyArray<SketchSubCall>,
  frame: PlaneFrame,
  triangulate: (polygon: Vec2[]) => number[][],
): number[] {
  const out: number[] = [];
  const regions = closedRegionsCached(subCalls);
  for (const poly of regions) {
    if (poly.length < 3) continue;
    const tris = triangulate(poly);
    for (const tri of tris) {
      for (const idx of tri) {
        const pCoord = poly[idx];
        if (pCoord) out.push(...planeToWorld(pCoord[0], pCoord[1], frame));
      }
    }
  }
  return out;
}
