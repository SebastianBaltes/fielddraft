/**
 * Single-click corner fillet / chamfer support (tickets tqi4, chfp).
 *
 * The fillet/chamfer kernel ops (`filletLines`/`chamferLines`) operate on two
 * `sketch_line` sub-calls. But a user draws a Polygon / N-gon / Rectangle as a
 * SINGLE sub-call, so its corners have no line pair to fillet — clicking a
 * polygon corner did nothing.
 *
 * This module resolves the corner under the cursor and, when it belongs to a
 * closed-shape sub-call (polygon / regular polygon / rectangle), EXPLODES that
 * shape into individual `sketch_line` sub-calls so the corner becomes an
 * ordinary line↔line junction the existing fillet/chamfer path handles. After
 * explosion all corners are uniform; the store just calls `filletSketchAt`.
 *
 * Pure + DOM-free so the resolution rules are unit-tested directly.
 */

import { regularPolygonVertices } from "./sketchSession";

export type Vec2 = [number, number];

export interface SubCall {
  type: string;
  params: Record<string, unknown>;
}

function asVec2(v: unknown): Vec2 | null {
  if (Array.isArray(v) && v.length >= 2) {
    const x = Number(v[0]);
    const y = Number(v[1]);
    if (Number.isFinite(x) && Number.isFinite(y)) return [x, y];
  }
  return null;
}

const dist = (a: Vec2, b: Vec2): number => Math.hypot(a[0] - b[0], a[1] - b[1]);

/** The ordered boundary vertices of a closed-shape sub-call, or null if the
 *  call isn't a closed shape this module can explode. */
export function shapeVertices(call: SubCall): Vec2[] | null {
  const p = call.params;
  if (call.type === "sketch_polygon") {
    const verts = p.vertices;
    if (!Array.isArray(verts) || verts.length < 6) return null;
    const out: Vec2[] = [];
    for (let i = 0; i + 1 < verts.length; i += 2) {
      out.push([Number(verts[i]), Number(verts[i + 1])]);
    }
    return out.length >= 3 ? out : null;
  }
  if (call.type === "sketch_regular_polygon") {
    const c = asVec2(p.center);
    const r = Number(p.radius);
    if (!c || !(r > 0)) return null;
    const vs = regularPolygonVertices(
      c,
      r,
      Number(p.sides),
      p.inscribed !== false,
    );
    return vs.length >= 3 ? vs : null;
  }
  if (call.type === "sketch_rectangle") {
    const a = asVec2(p.corner_a);
    const b = asVec2(p.corner_b);
    if (!a || !b) return null;
    const x0 = Math.min(a[0], b[0]);
    const x1 = Math.max(a[0], b[0]);
    const y0 = Math.min(a[1], b[1]);
    const y1 = Math.max(a[1], b[1]);
    return [
      [x0, y0],
      [x1, y0],
      [x1, y1],
      [x0, y1],
    ];
  }
  return null;
}

/** Explode a closed-shape sub-call into its closed loop of `sketch_line`
 *  sub-calls (carrying the shape's construction flag), or null if it isn't an
 *  explodable shape. */
export function explodeShapeToLines(call: SubCall): SubCall[] | null {
  const vs = shapeVertices(call);
  if (!vs || vs.length < 3) return null;
  const carry =
    call.params.construction !== undefined
      ? { construction: call.params.construction }
      : {};
  const lines: SubCall[] = [];
  for (let i = 0; i < vs.length; i++) {
    lines.push({
      type: "sketch_line",
      params: { start: vs[i], end: vs[(i + 1) % vs.length], ...carry },
    });
  }
  return lines;
}

export interface CornerFillet {
  /** Sub-calls after any required explosion (identical array reference-wise to
   *  the input only when nothing was exploded). */
  subCalls: SubCall[];
  /** The two `sketch_line` indices in {@link subCalls} that meet at the corner. */
  i1: number;
  i2: number;
  /** True when a closed shape was exploded to produce the pair. */
  exploded: boolean;
}

/**
 * Resolve the corner under `point` (within `tolMm`) into a pair of line
 * sub-calls to fillet/chamfer. A corner on a closed shape explodes that shape
 * to lines first. Returns null when no corner (a vertex shared by two edges)
 * is close enough.
 */
export function resolveCornerFillet(
  subCalls: ReadonlyArray<SubCall>,
  point: Vec2,
  tolMm: number,
): CornerFillet | null {
  // Candidate corners: every closed shape's vertices (shape index + vertex
  // index), plus every line endpoint (line index). Collected into an array,
  // then min-by-distance — avoids closure-narrowing pitfalls.
  type Candidate =
    | { kind: "shape"; index: number; vertex: number; pos: Vec2 }
    | { kind: "line"; index: number; pos: Vec2 };
  const candidates: Candidate[] = [];
  subCalls.forEach((call, index) => {
    if (call.type === "sketch_line") {
      const a = asVec2(call.params.start);
      const b = asVec2(call.params.end);
      if (a) candidates.push({ kind: "line", index, pos: a });
      if (b) candidates.push({ kind: "line", index, pos: b });
      return;
    }
    const vs = shapeVertices(call);
    if (!vs) return;
    vs.forEach((p, vertex) =>
      candidates.push({ kind: "shape", index, vertex, pos: p }),
    );
  });

  let best: Candidate | null = null;
  let bestD = tolMm;
  for (const c of candidates) {
    const d = dist(c.pos, point);
    if (d < bestD) {
      bestD = d;
      best = c;
    }
  }
  if (best === null) return null;

  // Snap epsilon for "two edges share this vertex": tight, scaled to tol.
  const eps = Math.max(tolMm * 0.5, 1e-3);

  if (best.kind === "shape") {
    // Explode the owning shape in place; vertex k is shared by exploded edges
    // (k-1) and (k) (the loop edge i connects vs[i]→vs[i+1]).
    const lines = explodeShapeToLines(subCalls[best.index]);
    if (!lines) return null;
    const n = lines.length;
    const out: SubCall[] = [
      ...subCalls.slice(0, best.index),
      ...lines,
      ...subCalls.slice(best.index + 1),
    ];
    const edgePrev = best.index + ((best.vertex - 1 + n) % n);
    const edgeCur = best.index + (best.vertex % n);
    return { subCalls: out, i1: edgePrev, i2: edgeCur, exploded: true };
  }

  // Line endpoint: find ANOTHER line whose endpoint coincides with this corner.
  const corner = best.pos;
  const ownIndex = best.index;
  let other: number | null = null;
  subCalls.forEach((call, index) => {
    if (index === ownIndex || other !== null) return;
    if (call.type !== "sketch_line") return;
    const a = asVec2(call.params.start);
    const b = asVec2(call.params.end);
    if ((a && dist(a, corner) < eps) || (b && dist(b, corner) < eps)) {
      other = index;
    }
  });
  if (other === null) return null;
  return {
    subCalls: subCalls.slice(),
    i1: ownIndex,
    i2: other,
    exploded: false,
  };
}
