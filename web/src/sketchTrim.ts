/**
 * Interactive trim for the 2D sketcher (UI gap follow-up).
 *
 * "Trim to nearest" — the user clicks a portion of a drawn curve and
 * that portion is removed, bounded by the curve's intersections with
 * every other curve (or its own endpoints). A line clicked on an
 * overhang shortens to the crossing; a line clicked between two
 * crossings splits into two lines; a full circle clicked between two
 * intersections becomes the complementary arc; an arc shortens or
 * splits like a line.
 *
 * This is a pure, DOM-free session-level rewrite: it reads the solved
 * geometry straight from the sub-call params and emits ordinary
 * `sketch_line` / `sketch_arc` sub-calls. The kernel needs no trim
 * tool — the result is just more of the primitives it already
 * understands. The store splices the replacement in via
 * [`replaceSketchSubCallAt`](./sketchSession) (which rewrites any
 * downstream constraint refs) and re-solves.
 *
 * Scope: trims `sketch_line`, `sketch_circle` and `sketch_arc`, plus the
 * edges of the composite primitives `sketch_rectangle` and
 * `sketch_polygon`. A composite is a single sub-call that expands to
 * several edges; trimming one of its edges decomposes the whole composite
 * into individual `sketch_line` sub-calls (the clicked edge trimmed, the
 * siblings carried over verbatim) — a rectangle is no longer a closed
 * region once you cut an edge, so flattening it to lines is the right
 * model. Every edge of every sub-call also acts as a cutter.
 */

export type Vec2 = [number, number];

export interface SubCall {
  type: string;
  params: Record<string, unknown>;
}

/** Angular / parametric tolerance for de-duplicating intersections and
 *  ignoring crossings that sit on a curve's own endpoint. In mm for
 *  positions, radians for angles — both well below any pick tolerance. */
const EPS = 1e-7;
const TAU = Math.PI * 2;

// ── geometry extracted from sub-call params ─────────────────────────

export type Curve =
  | { kind: "line"; a: Vec2; b: Vec2 }
  | { kind: "circle"; c: Vec2; r: number }
  | { kind: "arc"; c: Vec2; r: number; start: number; sweep: number };

function asVec2(v: unknown): Vec2 | null {
  if (Array.isArray(v) && v.length === 2) {
    const x = Number(v[0]);
    const y = Number(v[1]);
    if (Number.isFinite(x) && Number.isFinite(y)) return [x, y];
  }
  return null;
}

/** Parse a trimmable sub-call into a curve, or null if the type isn't
 *  trimmable (or its params are malformed). */
export function curveOfSubCall(call: SubCall): Curve | null {
  if (call.type === "sketch_line") {
    const a = asVec2(call.params.start);
    const b = asVec2(call.params.end);
    return a && b ? { kind: "line", a, b } : null;
  }
  if (call.type === "sketch_circle") {
    const c = asVec2(call.params.center);
    const r = Number(call.params.radius);
    return c && Number.isFinite(r) && r > EPS ? { kind: "circle", c, r } : null;
  }
  if (call.type === "sketch_arc") {
    const c = asVec2(call.params.center);
    const r = Number(call.params.radius);
    const start = Number(call.params.start_angle);
    const sweep = Number(call.params.sweep_angle);
    if (
      c &&
      Number.isFinite(r) &&
      r > EPS &&
      Number.isFinite(start) &&
      Number.isFinite(sweep) &&
      Math.abs(sweep) > EPS
    ) {
      return { kind: "arc", c, r, start, sweep };
    }
  }
  return null;
}

/** The four edges (bottom, right, top, left) of a `sketch_rectangle`,
 *  in the same corner order the kernel allocates them. */
function rectangleEdges(call: SubCall): Curve[] {
  const a = asVec2(call.params.corner_a);
  const b = asVec2(call.params.corner_b);
  if (!a || !b) return [];
  const minX = Math.min(a[0], b[0]);
  const minY = Math.min(a[1], b[1]);
  const maxX = Math.max(a[0], b[0]);
  const maxY = Math.max(a[1], b[1]);
  if (maxX - minX < EPS || maxY - minY < EPS) return [];
  const bl: Vec2 = [minX, minY];
  const br: Vec2 = [maxX, minY];
  const tr: Vec2 = [maxX, maxY];
  const tl: Vec2 = [minX, maxY];
  return [
    { kind: "line", a: bl, b: br },
    { kind: "line", a: br, b: tr },
    { kind: "line", a: tr, b: tl },
    { kind: "line", a: tl, b: bl },
  ];
}

/** The N edges of a `sketch_polygon`, closing the last vertex back to the
 *  first. `vertices` is a flat `[x0,y0,x1,y1,…]` array. */
function polygonEdges(call: SubCall): Curve[] {
  const flat = call.params.vertices;
  if (!Array.isArray(flat) || flat.length < 6) return [];
  const verts: Vec2[] = [];
  for (let i = 0; i + 1 < flat.length; i += 2) {
    const x = Number(flat[i]);
    const y = Number(flat[i + 1]);
    if (!Number.isFinite(x) || !Number.isFinite(y)) return [];
    verts.push([x, y]);
  }
  if (verts.length < 3) return [];
  return verts.map((v, i) => ({
    kind: "line" as const,
    a: v,
    b: verts[(i + 1) % verts.length],
  }));
}

/** Every trimmable edge a sub-call contributes: one for a line/circle/arc,
 *  four for a rectangle, N for a polygon, none for anything else. */
export function edgesOfSubCall(call: SubCall): Curve[] {
  const simple = curveOfSubCall(call);
  if (simple) return [simple];
  if (call.type === "sketch_rectangle") return rectangleEdges(call);
  if (call.type === "sketch_polygon") return polygonEdges(call);
  return [];
}

// ── small vector helpers ────────────────────────────────────────────

const sub = (p: Vec2, q: Vec2): Vec2 => [p[0] - q[0], p[1] - q[1]];
const dot = (p: Vec2, q: Vec2): number => p[0] * q[0] + p[1] * q[1];
const len2 = (p: Vec2): number => p[0] * p[0] + p[1] * p[1];
const distSq = (p: Vec2, q: Vec2): number => len2(sub(p, q));
const dist = (p: Vec2, q: Vec2): number => Math.hypot(p[0] - q[0], p[1] - q[1]);

function pointOnLine(a: Vec2, b: Vec2, t: number): Vec2 {
  return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
}
function pointOnCircle(c: Vec2, r: number, theta: number): Vec2 {
  return [c[0] + r * Math.cos(theta), c[1] + r * Math.sin(theta)];
}

/** Wrap a value into [0, TAU). */
function norm2pi(a: number): number {
  let x = a % TAU;
  if (x < 0) x += TAU;
  return x;
}

/**
 * If `theta` lies on the arc, return its normalized parameter s ∈ [0, 1]
 * (0 = arc start, 1 = arc end); otherwise null. Handles both sweep
 * directions and the ±π seam.
 */
function arcParam(start: number, sweep: number, theta: number): number | null {
  const span = Math.abs(sweep);
  // Distance travelled from start toward the arc end, in the sweep's
  // direction, wrapped to [0, TAU).
  const travelled = sweep >= 0 ? norm2pi(theta - start) : norm2pi(start - theta);
  if (travelled <= span + EPS) return Math.min(1, travelled / span);
  return null;
}

// ── intersections (each returns points that lie on BOTH curves) ─────

function intersectLineLine(
  l1: { a: Vec2; b: Vec2 },
  l2: { a: Vec2; b: Vec2 },
): Vec2[] {
  const r = sub(l1.b, l1.a);
  const s = sub(l2.b, l2.a);
  const denom = r[0] * s[1] - r[1] * s[0];
  if (Math.abs(denom) < EPS) return []; // parallel / collinear
  const qp = sub(l2.a, l1.a);
  const t = (qp[0] * s[1] - qp[1] * s[0]) / denom;
  const u = (qp[0] * r[1] - qp[1] * r[0]) / denom;
  if (t < -EPS || t > 1 + EPS || u < -EPS || u > 1 + EPS) return [];
  return [pointOnLine(l1.a, l1.b, t)];
}

function intersectLineCircle(
  l: { a: Vec2; b: Vec2 },
  c: { c: Vec2; r: number },
): Vec2[] {
  const d = sub(l.b, l.a);
  const f = sub(l.a, c.c);
  const aa = len2(d);
  if (aa < EPS) return [];
  const bb = 2 * dot(f, d);
  const cc = len2(f) - c.r * c.r;
  let disc = bb * bb - 4 * aa * cc;
  if (disc < -EPS) return [];
  disc = Math.max(0, disc);
  const sq = Math.sqrt(disc);
  const out: Vec2[] = [];
  for (const t of [(-bb - sq) / (2 * aa), (-bb + sq) / (2 * aa)]) {
    if (t >= -EPS && t <= 1 + EPS) out.push(pointOnLine(l.a, l.b, t));
  }
  return out;
}

function intersectCircleCircle(
  c1: { c: Vec2; r: number },
  c2: { c: Vec2; r: number },
): Vec2[] {
  const d = dist(c1.c, c2.c);
  if (d < EPS) return []; // concentric
  if (d > c1.r + c2.r + EPS) return []; // too far apart
  if (d < Math.abs(c1.r - c2.r) - EPS) return []; // one inside the other
  const a = (c1.r * c1.r - c2.r * c2.r + d * d) / (2 * d);
  const h2 = c1.r * c1.r - a * a;
  const h = Math.sqrt(Math.max(0, h2));
  const dir = sub(c2.c, c1.c);
  const mid: Vec2 = [c1.c[0] + (a * dir[0]) / d, c1.c[1] + (a * dir[1]) / d];
  if (h < EPS) return [mid]; // tangent
  const ox = (-dir[1] * h) / d;
  const oy = (dir[0] * h) / d;
  return [
    [mid[0] + ox, mid[1] + oy],
    [mid[0] - ox, mid[1] - oy],
  ];
}

/** Keep only points whose angle about the arc centre lies on the arc. */
function clampToArc(pts: Vec2[], arc: Curve & { kind: "arc" }): Vec2[] {
  return pts.filter((p) => {
    const theta = Math.atan2(p[1] - arc.c[1], p[0] - arc.c[0]);
    return arcParam(arc.start, arc.sweep, theta) !== null;
  });
}

/** All intersection points of two curves, each lying on both extents. */
export function intersectCurves(x: Curve, y: Curve): Vec2[] {
  // Normalise to the underlying circle for arc cases, then clamp the
  // results back onto whichever operand(s) are arcs.
  const circleOf = (cv: Curve): { c: Vec2; r: number } | null =>
    cv.kind === "circle" || cv.kind === "arc" ? { c: cv.c, r: cv.r } : null;

  let pts: Vec2[];
  if (x.kind === "line" && y.kind === "line") {
    pts = intersectLineLine(x, y);
  } else if (x.kind === "line") {
    pts = intersectLineCircle(x, circleOf(y)!);
  } else if (y.kind === "line") {
    pts = intersectLineCircle(y, circleOf(x)!);
  } else {
    pts = intersectCircleCircle(circleOf(x)!, circleOf(y)!);
  }
  if (x.kind === "arc") pts = clampToArc(pts, x);
  if (y.kind === "arc") pts = clampToArc(pts, y);
  return dedupePoints(pts);
}

/** Merge points that coincide within a hair (e.g. a tangency that the
 *  quadratic returns as two near-identical roots). */
function dedupePoints(pts: Vec2[]): Vec2[] {
  const out: Vec2[] = [];
  for (const p of pts) {
    if (!out.some((q) => distSq(p, q) < 1e-12)) out.push(p);
  }
  return out;
}

// ── distance from a click to a curve's drawn extent ─────────────────

/** Shortest distance from `p` to a curve, respecting segment / arc
 *  extents (so the empty side of an arc isn't a hit). */
export function distToCurve(p: Vec2, cv: Curve): number {
  if (cv.kind === "line") {
    const d = sub(cv.b, cv.a);
    const ll = len2(d);
    if (ll < EPS) return dist(p, cv.a);
    let t = dot(sub(p, cv.a), d) / ll;
    t = Math.max(0, Math.min(1, t));
    return dist(p, pointOnLine(cv.a, cv.b, t));
  }
  if (cv.kind === "circle") {
    return Math.abs(dist(p, cv.c) - cv.r);
  }
  // arc: radial distance if the click's angle is within the span,
  // otherwise the distance to the nearer endpoint.
  const theta = Math.atan2(p[1] - cv.c[1], p[0] - cv.c[0]);
  if (arcParam(cv.start, cv.sweep, theta) !== null) {
    return Math.abs(dist(p, cv.c) - cv.r);
  }
  const startPt = pointOnCircle(cv.c, cv.r, cv.start);
  const endPt = pointOnCircle(cv.c, cv.r, cv.start + cv.sweep);
  return Math.min(dist(p, startPt), dist(p, endPt));
}

/**
 * Index of the trimmable sub-call nearest the click (within `tolMm`),
 * or null when the click is over empty space.
 */
export function nearestTrimTarget(
  subCalls: ReadonlyArray<SubCall>,
  click: Vec2,
  tolMm: number,
): number | null {
  let bestIdx: number | null = null;
  let bestDist = tolMm;
  subCalls.forEach((call, i) => {
    for (const cv of edgesOfSubCall(call)) {
      const d = distToCurve(click, cv);
      if (d < bestDist) {
        bestDist = d;
        bestIdx = i;
      }
    }
  });
  return bestIdx;
}

// ── the trim itself ─────────────────────────────────────────────────

/** Parameter of the click projected onto a bounded curve (line: t along
 *  the segment, clamped; arc: s along the span, clamped). */
function clickParamBounded(cv: Curve, click: Vec2): number {
  if (cv.kind === "line") {
    const d = sub(cv.b, cv.a);
    const ll = len2(d);
    if (ll < EPS) return 0;
    return Math.max(0, Math.min(1, dot(sub(click, cv.a), d) / ll));
  }
  // arc
  if (cv.kind === "arc") {
    const theta = Math.atan2(click[1] - cv.c[1], click[0] - cv.c[0]);
    const s = arcParam(cv.start, cv.sweep, theta);
    if (s !== null) return s;
    // Click sits just past an end — snap to the nearer end.
    const dStart = norm2pi(
      cv.sweep >= 0 ? theta - cv.start : cv.start - theta,
    );
    return dStart < TAU / 2 ? 0 : 1;
  }
  return 0;
}

/** De-dupe a sorted list of parameters, dropping near-duplicates. */
function dedupeSorted(values: number[]): number[] {
  const out: number[] = [];
  for (const v of values) {
    if (out.length === 0 || Math.abs(v - out[out.length - 1]) > 1e-6) {
      out.push(v);
    }
  }
  return out;
}

/** Carry forward generic, geometry-independent params (the `construction`
 *  flag today) so a trimmed construction edge stays construction. */
function carryFlags(params: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  if (params.construction !== undefined) out.construction = params.construction;
  return out;
}

/** Reproduce a curve as a standalone sub-call — used for the sibling edges
 *  of a composite that survive untrimmed when one of its edges is cut. */
export function subCallOfCurve(
  cv: Curve,
  carry: Record<string, unknown>,
): SubCall {
  if (cv.kind === "line") {
    return {
      type: "sketch_line",
      params: { ...carry, start: [cv.a[0], cv.a[1]], end: [cv.b[0], cv.b[1]] },
    };
  }
  if (cv.kind === "circle") {
    return {
      type: "sketch_circle",
      params: { ...carry, center: [cv.c[0], cv.c[1]], radius: cv.r },
    };
  }
  return {
    type: "sketch_arc",
    params: {
      ...carry,
      center: [cv.c[0], cv.c[1]],
      radius: cv.r,
      start_angle: cv.start,
      sweep_angle: cv.sweep,
    },
  };
}

/** Trim a single edge at `click`, bounded by its intersections with
 *  `cutters`. Returns 0, 1 or 2 replacement sub-calls (0 = delete). */
function trimEdge(
  cv: Curve,
  cutters: ReadonlyArray<Curve>,
  click: Vec2,
  carry: Record<string, unknown>,
): SubCall[] {
  const hits: Vec2[] = [];
  for (const cutter of cutters) hits.push(...intersectCurves(cv, cutter));

  if (cv.kind === "line") {
    const ts = dedupeSorted(
      hits
        .map((p) => {
          const d = sub(cv.b, cv.a);
          return dot(sub(p, cv.a), d) / len2(d);
        })
        .filter((t) => t > EPS && t < 1 - EPS)
        .sort((a, b) => a - b),
    );
    const clickT = clickParamBounded(cv, click);
    const lo = lowerBoundary(ts, clickT); // 0 if none below
    const hi = upperBoundary(ts, clickT); // 1 if none above
    const pieces: Array<[number, number]> = [];
    if (lo > EPS) pieces.push([0, lo]);
    if (hi < 1 - EPS) pieces.push([hi, 1]);
    return pieces.map((seg) => ({
      type: "sketch_line",
      params: {
        ...carry,
        start: pointOnLine(cv.a, cv.b, seg[0]),
        end: pointOnLine(cv.a, cv.b, seg[1]),
      },
    }));
  }

  if (cv.kind === "arc") {
    const ss = dedupeSorted(
      hits
        .map((p) => {
          const theta = Math.atan2(p[1] - cv.c[1], p[0] - cv.c[0]);
          return arcParam(cv.start, cv.sweep, theta);
        })
        .filter((s): s is number => s !== null && s > EPS && s < 1 - EPS)
        .sort((a, b) => a - b),
    );
    const clickS = clickParamBounded(cv, click);
    const lo = lowerBoundary(ss, clickS);
    const hi = upperBoundary(ss, clickS);
    const pieces: Array<[number, number]> = [];
    if (lo > EPS) pieces.push([0, lo]);
    if (hi < 1 - EPS) pieces.push([hi, 1]);
    return pieces.map((seg) => ({
      type: "sketch_arc",
      params: {
        ...carry,
        center: [cv.c[0], cv.c[1]] as Vec2,
        radius: cv.r,
        start_angle: cv.start + seg[0] * cv.sweep,
        sweep_angle: (seg[1] - seg[0]) * cv.sweep,
      },
    }));
  }

  // Full circle: needs at least two intersections to split into removable
  // arcs. With fewer than two it isn't divided, so trimming it is a no-op —
  // we keep the whole circle rather than deleting it. Wiping an entire closed
  // loop on a click that can't bound a segment is never the intent (Onshape
  // leaves an uncut circle alone); the old "return []" here is exactly the
  // rxi2 bug where clicking a circle's rim erased the whole circle.
  const angles = dedupeSorted(
    hits
      .map((p) => norm2pi(Math.atan2(p[1] - cv.c[1], p[0] - cv.c[0])))
      .sort((a, b) => a - b),
  );
  if (angles.length < 2) return [subCallOfCurve(cv, carry)];

  const clickTheta = norm2pi(
    Math.atan2(click[1] - cv.c[1], click[0] - cv.c[0]),
  );
  // The two intersection angles bracketing the click (cyclically).
  let lo = angles[angles.length - 1] - TAU;
  let hi = angles[0];
  for (let i = 0; i < angles.length; i++) {
    if (angles[i] <= clickTheta + EPS) {
      lo = angles[i];
      hi = angles[(i + 1) % angles.length];
      if (i === angles.length - 1) hi = angles[0] + TAU;
    }
  }
  // Remove the arc from lo→hi (through the click); keep the complement,
  // i.e. the arc from hi CCW back round to lo.
  const keepStart = norm2pi(hi);
  const keepSweep = norm2pi(lo - hi); // positive remainder of the circle
  if (keepSweep < EPS) return [];
  return [
    {
      type: "sketch_arc",
      params: {
        ...carry,
        center: [cv.c[0], cv.c[1]] as Vec2,
        radius: cv.r,
        start_angle: keepStart,
        sweep_angle: keepSweep,
      },
    },
  ];
}

/**
 * Compute the replacement sub-calls when the sub-call at `targetIndex` is
 * trimmed at `click`. Returns:
 *  - an array of replacement sub-calls (empty = the target is deleted,
 *    e.g. an isolated line or an intersection-free circle), or
 *  - null when the target isn't trimmable (the caller treats null as
 *    "do nothing").
 *
 * For a simple line/circle/arc this is the trimmed result. For a composite
 * (rectangle/polygon) the clicked edge is trimmed and every sibling edge
 * is carried over as a standalone `sketch_line`, so the composite flattens
 * into individual lines — a cut edge means it's no longer a closed region.
 *
 * Replacement sub-calls inherit the target's `construction` flag.
 */
export function trimCurveAt(
  subCalls: ReadonlyArray<SubCall>,
  targetIndex: number,
  click: Vec2,
): SubCall[] | null {
  const target = subCalls[targetIndex];
  if (!target) return null;
  const edges = edgesOfSubCall(target);
  if (edges.length === 0) return null;

  // Which edge of the target did the click land on?
  let clickedEdge = 0;
  let bestDist = Infinity;
  edges.forEach((cv, k) => {
    const d = distToCurve(click, cv);
    if (d < bestDist) {
      bestDist = d;
      clickedEdge = k;
    }
  });

  // Every other edge in the whole sketch is a potential cutter — including
  // the sibling edges of a composite target (they only meet the clicked
  // edge at shared corners, which the endpoint tolerance filters out).
  const cutters: Curve[] = [];
  subCalls.forEach((call, i) => {
    edgesOfSubCall(call).forEach((cv, k) => {
      if (i === targetIndex && k === clickedEdge) return;
      cutters.push(cv);
    });
  });

  const carry = carryFlags(target.params);

  const out: SubCall[] = [];
  edges.forEach((cv, k) => {
    if (k === clickedEdge) out.push(...trimEdge(cv, cutters, click, carry));
    else out.push(subCallOfCurve(cv, carry));
  });
  return out;
}

/** Largest boundary value strictly below `x`, or 0 if none. */
function lowerBoundary(sorted: number[], x: number): number {
  let lo = 0;
  for (const v of sorted) if (v < x - EPS) lo = v;
  return lo;
}
/** Smallest boundary value strictly above `x`, or 1 if none. */
function upperBoundary(sorted: number[], x: number): number {
  for (const v of sorted) if (v > x + EPS) return v;
  return 1;
}

/**
 * High-level entry: find the curve under `click` and compute its trim.
 * Returns the target index plus replacement sub-calls, or null when
 * nothing trimmable is under the cursor / no change results.
 */
export function computeTrim(
  subCalls: ReadonlyArray<SubCall>,
  click: Vec2,
  tolMm: number,
): { index: number; newCalls: SubCall[] } | null {
  const index = nearestTrimTarget(subCalls, click, tolMm);
  if (index === null) return null;
  const newCalls = trimCurveAt(subCalls, index, click);
  if (newCalls === null) return null;
  return { index, newCalls };
}
