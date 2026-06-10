/**
 * Closed-region detection for the 2D sketcher (ticket 7x4v).
 *
 * Onshape shades every closed sketch contour so the user can see which
 * loops will extrude. This module resolves the committed session
 * sub-calls into a list of closed polygons (sketch-space mm, CCW) that
 * the canvas overlay fills.
 *
 * Two sources of closed regions:
 *   1. Intrinsically closed primitives — circle, ellipse, rectangle,
 *      polygon, slot, closed spline, closed path contour — are sampled
 *      directly into one polygon each.
 *   2. Open curves (lines, arcs, open splines, open path contours) are
 *      welded into a planar graph by their endpoints and the bounded
 *      faces are extracted via the standard DCEL face traversal: sort
 *      outgoing half-edges CCW around each vertex, follow
 *      `next(h) = clockwise-most turn after twin(h)`, keep faces with
 *      positive signed area (the unbounded outer face traces negative).
 *
 * Construction geometry never contributes — the kernel drops it from
 * the extruded profile, so shading it would lie.
 *
 * Pure, DOM-free math; rendering happens in SketchCanvas2d.
 */

import { sampleArc, type Vec2 } from "./sketchArc";
import { slotOutline } from "./sketchSlot";
import { bsplinePoints, catmullRomPoints } from "./sketchSpline";
import type { SubCall } from "./sketchTrim";

/** Endpoint weld tolerance (mm). Snapped / solver-coincident endpoints
 *  are bit-identical; this only absorbs float noise, so loops that are
 *  visually-but-not-actually closed stay unshaded (Onshape behaviour). */
const WELD_TOL_MM = 1e-3;

/** Faces below this area (mm²) are tracing artifacts (dangling stubs,
 *  zero-width slivers), not regions. */
const MIN_REGION_AREA_MM2 = 1e-4;

const CIRCLE_SAMPLES = 64;
const TAU = Math.PI * 2;

function asVec2(v: unknown): Vec2 | null {
  if (Array.isArray(v) && v.length === 2) {
    const x = Number(v[0]);
    const y = Number(v[1]);
    if (Number.isFinite(x) && Number.isFinite(y)) return [x, y];
  }
  return null;
}

function asNum(v: unknown): number | null {
  const n = Number(v);
  return Number.isFinite(n) ? n : null;
}

/** Shoelace signed area; positive = CCW in Y-up sketch space. */
export function signedArea(poly: ReadonlyArray<Vec2>): number {
  let acc = 0;
  for (let i = 0; i < poly.length; i++) {
    const [x0, y0] = poly[i];
    const [x1, y1] = poly[(i + 1) % poly.length];
    acc += x0 * y1 - x1 * y0;
  }
  return acc / 2;
}

function ccw(poly: Vec2[]): Vec2[] {
  return signedArea(poly) < 0 ? poly.slice().reverse() : poly;
}

// ── per-sub-call geometry extraction ─────────────────────────────────

interface Extracted {
  /** Intrinsically closed contours (not yet orientation-normalized). */
  polygons: Vec2[][];
  /** Open polyline edges for the loop graph (≥ 2 points each). */
  edges: Vec2[][];
}

function samplePathContour(contour: unknown): { pts: Vec2[]; closed: boolean } | null {
  const cc = contour as { start?: unknown; segments?: unknown; closed?: unknown };
  const start = asVec2(cc.start);
  const segs = cc.segments;
  if (!start || !Array.isArray(segs)) return null;
  const pts: Vec2[] = [start];
  for (const s of segs) {
    const seg = s as { kind?: unknown; to?: unknown; ctrl?: unknown };
    const to = asVec2(seg.to);
    if (!to) continue;
    const prev = pts[pts.length - 1];
    const cp = seg.kind === "quad" ? asVec2(seg.ctrl) : null;
    if (cp) {
      const STEPS = 16;
      for (let k = 1; k <= STEPS; k++) {
        const t = k / STEPS;
        const mt = 1 - t;
        pts.push([
          mt * mt * prev[0] + 2 * mt * t * cp[0] + t * t * to[0],
          mt * mt * prev[1] + 2 * mt * t * cp[1] + t * t * to[1],
        ]);
      }
    } else {
      pts.push(to);
    }
  }
  return pts.length >= 2 ? { pts, closed: cc.closed === true } : null;
}

function extract(subCalls: ReadonlyArray<SubCall>): Extracted {
  const polygons: Vec2[][] = [];
  const edges: Vec2[][] = [];
  for (const call of subCalls) {
    if (call.params.construction === true) continue;
    if (call.type === "sketch_line") {
      const a = asVec2(call.params.start);
      const b = asVec2(call.params.end);
      if (a && b) edges.push([a, b]);
    } else if (call.type === "sketch_arc") {
      const c = asVec2(call.params.center);
      const r = asNum(call.params.radius);
      const a0 = asNum(call.params.start_angle);
      const sweep = asNum(call.params.sweep_angle);
      if (c && r !== null && r > 0 && a0 !== null && sweep !== null) {
        const pts = sampleArc(c, r, a0, sweep);
        if (Math.abs(sweep) >= TAU - 1e-6) polygons.push(pts.slice(0, -1));
        else edges.push(pts);
      }
    } else if (call.type === "sketch_circle") {
      const c = asVec2(call.params.center);
      const r = asNum(call.params.radius);
      if (c && r !== null && r > 0) {
        polygons.push(sampleArc(c, r, 0, TAU, TAU / CIRCLE_SAMPLES).slice(0, -1));
      }
    } else if (call.type === "sketch_ellipse") {
      const c = asVec2(call.params.center);
      const rx = asNum(call.params.radius_x);
      const ry = asNum(call.params.radius_y);
      const rot = asNum(call.params.rotation) ?? 0;
      if (c && rx !== null && ry !== null && rx > 0 && ry > 0) {
        const cosR = Math.cos(rot);
        const sinR = Math.sin(rot);
        const poly: Vec2[] = [];
        for (let i = 0; i < CIRCLE_SAMPLES; i++) {
          const t = (TAU * i) / CIRCLE_SAMPLES;
          const ex = rx * Math.cos(t);
          const ey = ry * Math.sin(t);
          poly.push([c[0] + ex * cosR - ey * sinR, c[1] + ex * sinR + ey * cosR]);
        }
        polygons.push(poly);
      }
    } else if (call.type === "sketch_rectangle") {
      const a = asVec2(call.params.corner_a);
      const b = asVec2(call.params.corner_b);
      if (a && b) {
        polygons.push([
          [a[0], a[1]],
          [b[0], a[1]],
          [b[0], b[1]],
          [a[0], b[1]],
        ]);
      }
    } else if (call.type === "sketch_polygon") {
      const flat = call.params.vertices;
      if (Array.isArray(flat) && flat.length >= 6) {
        const poly: Vec2[] = [];
        for (let i = 0; i + 1 < flat.length; i += 2) {
          const x = Number(flat[i]);
          const y = Number(flat[i + 1]);
          if (Number.isFinite(x) && Number.isFinite(y)) poly.push([x, y]);
        }
        if (poly.length >= 3) polygons.push(poly);
      }
    } else if (call.type === "sketch_slot") {
      const a = asVec2(call.params.center_a);
      const b = asVec2(call.params.center_b);
      const r = asNum(call.params.radius);
      if (a && b && r !== null) {
        const o = slotOutline(a, b, r);
        if (o) {
          polygons.push([
            o.aPlus,
            o.bPlus,
            ...sampleArc(o.centerB, o.radius, o.capBStart, -Math.PI).slice(1, -1),
            o.bMinus,
            o.aMinus,
            ...sampleArc(o.centerA, o.radius, o.capAStart, -Math.PI).slice(1, -1),
          ]);
        }
      }
    } else if (call.type === "sketch_spline") {
      const raw = call.params.points;
      const closed = call.params.closed === true;
      const isControl = call.params.mode === "control";
      if (Array.isArray(raw) && raw.length >= 4) {
        const ctrl: Vec2[] = [];
        for (let i = 0; i + 1 < raw.length; i += 2) {
          ctrl.push([Number(raw[i]), Number(raw[i + 1])]);
        }
        const sampled = isControl
          ? bsplinePoints(ctrl, 3, closed, 64)
          : catmullRomPoints(ctrl, closed, 16);
        if (sampled.length >= 2) {
          if (closed) polygons.push(sampled);
          else edges.push(sampled);
        }
      }
    } else if (call.type === "sketch_path") {
      const contours = call.params.contours;
      if (Array.isArray(contours)) {
        for (const c of contours) {
          const res = samplePathContour(c);
          if (!res) continue;
          if (res.closed && res.pts.length >= 3) polygons.push(res.pts);
          else edges.push(res.pts);
        }
      }
    }
  }
  return { polygons, edges };
}

// ── endpoint welding ─────────────────────────────────────────────────

/** Tolerance-welding vertex pool: spatial hash with 3×3-cell lookup so
 *  near-identical endpoints land on one vertex id regardless of which
 *  side of a cell boundary they fall. */
class VertexPool {
  positions: Vec2[] = [];
  private cells = new Map<string, number[]>();

  idFor(p: Vec2): number {
    const cx = Math.round(p[0] / WELD_TOL_MM);
    const cy = Math.round(p[1] / WELD_TOL_MM);
    for (let dx = -1; dx <= 1; dx++) {
      for (let dy = -1; dy <= 1; dy++) {
        const bucket = this.cells.get(`${cx + dx},${cy + dy}`);
        if (!bucket) continue;
        for (const id of bucket) {
          const q = this.positions[id];
          if (Math.hypot(q[0] - p[0], q[1] - p[1]) <= WELD_TOL_MM) return id;
        }
      }
    }
    const id = this.positions.length;
    this.positions.push(p);
    const key = `${cx},${cy}`;
    const bucket = this.cells.get(key);
    if (bucket) bucket.push(id);
    else this.cells.set(key, [id]);
    return id;
  }
}

// ── planar face traversal ────────────────────────────────────────────

/**
 * Extract the bounded faces of the open-edge graph. Each face is a
 * closed polygon in sketch space. Self-loop edges (a sampled curve
 * returning to its start) become faces directly.
 */
export function loopsFromEdges(edges: ReadonlyArray<Vec2[]>): Vec2[][] {
  const pool = new VertexPool();
  interface GraphEdge {
    pts: Vec2[];
    u: number;
    v: number;
  }
  const graph: GraphEdge[] = [];
  const faces: Vec2[][] = [];

  for (const pts of edges) {
    if (pts.length < 2) continue;
    const u = pool.idFor(pts[0]);
    const v = pool.idFor(pts[pts.length - 1]);
    if (u === v) {
      // Curve closes onto itself — already a loop.
      const loop = pts.slice(0, -1);
      if (loop.length >= 3 && Math.abs(signedArea(loop)) > MIN_REGION_AREA_MM2) {
        faces.push(loop);
      }
      continue;
    }
    graph.push({ pts, u, v });
  }

  // Half-edge h: edge graph[h >> 1], direction h & 1 (0 = u→v, 1 = v→u).
  const tail = (h: number) => (h & 1 ? graph[h >> 1].v : graph[h >> 1].u);
  const head = (h: number) => (h & 1 ? graph[h >> 1].u : graph[h >> 1].v);
  /** Departure angle at the tail: direction toward the first interior
   *  sample, so curved edges sort by their true tangent. */
  const angle = (h: number): number => {
    const e = graph[h >> 1];
    const p = h & 1 ? e.pts[e.pts.length - 1] : e.pts[0];
    const q = h & 1 ? e.pts[e.pts.length - 2] : e.pts[1];
    return Math.atan2(q[1] - p[1], q[0] - p[0]);
  };

  // CCW-sorted outgoing half-edges per vertex.
  const outgoing = new Map<number, number[]>();
  for (let h = 0; h < graph.length * 2; h++) {
    const t = tail(h);
    const list = outgoing.get(t);
    if (list) list.push(h);
    else outgoing.set(t, [h]);
  }
  for (const list of outgoing.values()) {
    list.sort((a, b) => angle(a) - angle(b));
  }

  const visited = new Uint8Array(graph.length * 2);
  for (let start = 0; start < graph.length * 2; start++) {
    if (visited[start]) continue;
    const poly: Vec2[] = [];
    let h = start;
    // Bounded by the half-edge count — every step marks one as visited.
    for (let guard = 0; guard <= graph.length * 2; guard++) {
      visited[h] = 1;
      const e = graph[h >> 1];
      const pts = h & 1 ? e.pts.slice().reverse() : e.pts;
      // Drop the joint point shared with the previous edge.
      for (let i = 0; i + 1 < pts.length; i++) poly.push(pts[i]);
      // next(h): at head(h), take the clockwise-most turn after twin(h).
      const twin = h ^ 1;
      const list = outgoing.get(head(h));
      if (!list || list.length === 0) break;
      const idx = list.indexOf(twin);
      h = list[(idx - 1 + list.length) % list.length];
      if (h === start) break;
      if (visited[h]) break; // open chain — no face here
    }
    if (h !== start || poly.length < 3) continue;
    if (signedArea(poly) > MIN_REGION_AREA_MM2) faces.push(poly);
  }
  return faces;
}

/**
 * All closed regions of the committed sketch geometry, as CCW polygons
 * in sketch-space mm. Fill them as ONE canvas path with the nonzero
 * rule: overlapping loops (a circle inside a rectangle extrudes as
 * their union) must shade uniformly, not punch holes or double-darken.
 */
export function closedRegionPolygons(
  subCalls: ReadonlyArray<SubCall>,
): Vec2[][] {
  const { polygons, edges } = extract(subCalls);
  const out: Vec2[][] = [];
  for (const p of polygons) {
    if (p.length >= 3 && Math.abs(signedArea(p)) > MIN_REGION_AREA_MM2) {
      out.push(ccw(p));
    }
  }
  for (const f of loopsFromEdges(edges)) out.push(ccw(f));
  return out;
}

const regionCache = new WeakMap<object, Vec2[][]>();

/** Memoized [`closedRegionPolygons`] — the session sub-call array is
 *  immutable, so its identity keys one analysis. */
export function closedRegionsCached(
  subCalls: ReadonlyArray<SubCall>,
): Vec2[][] {
  const hit = regionCache.get(subCalls as object);
  if (hit) return hit;
  const result = closedRegionPolygons(subCalls);
  regionCache.set(subCalls as object, result);
  return result;
}
