/**
 * F15 / F15-full: interactive offset for the 2D sketcher.
 *
 * Two entry points:
 *
 *  - [`offsetCurve`] — the degenerate single-curve case. A pure parallel
 *    copy of ONE sketch curve toward the clicked side (line: parallel;
 *    circle/arc: concentric, grown if the click is outside the rim, shrunk
 *    if inside). Kept for the lone-circle / lone-arc / lone-line pick.
 *
 *  - [`offsetChain`] — a full CAD chain/contour offset. Given a CONNECTED
 *    chain of line/arc curves (open polyline or closed loop), offset the
 *    whole chain by `distance` toward the clicked side and reconcile the
 *    joints between consecutive offset segments, exactly like Onshape /
 *    FreeCAD "Offset":
 *      * each segment offsets on a globally consistent side (derived from
 *        the clicked segment's clicked side and propagated around the chain),
 *      * concave / inner corners: the two offset segments are trimmed /
 *        extended to their supporting-geometry intersection,
 *      * convex / outer corners (the offset opens a gap): a rounding fillet
 *        arc of radius = `distance`, centred at the original shared vertex
 *        and tangent to both offset segments, bridges the gap — the standard
 *        rounded-offset behaviour,
 *      * a closed loop also reconciles the last↔first joint.
 *    Self-collapsing segments (distance ≥ local feature size, the offset
 *    would invert) are dropped and the neighbours reconnected; if the whole
 *    result would be invalid, returns null with no partial output.
 *
 * Neither path touches the kernel: the result is just more of the
 * `sketch_line` / `sketch_circle` / `sketch_arc` primitives the kernel
 * already lowers, individually editable afterwards (like trim / fillet /
 * mirror). Line/arc intersection math is reused from [`sketchTrim`].
 */

import type { Curve, Vec2 } from "./sketchTrim";
import { intersectCurves } from "./sketchTrim";

/** Squared length below which a line is treated as degenerate. */
const LEN_EPS_SQ = 1e-18;
/** Generic position epsilon (mm). */
const EPS = 1e-7;
/** Epsilon for endpoint-coincidence when walking a chain. */
const JOIN_EPS = 1e-6;
const TAU = Math.PI * 2;

// ── small vector helpers ────────────────────────────────────────────

const sub = (p: Vec2, q: Vec2): Vec2 => [p[0] - q[0], p[1] - q[1]];
const add = (p: Vec2, q: Vec2): Vec2 => [p[0] + q[0], p[1] + q[1]];
const scale = (p: Vec2, s: number): Vec2 => [p[0] * s, p[1] * s];
const dot = (p: Vec2, q: Vec2): number => p[0] * q[0] + p[1] * q[1];
const cross = (p: Vec2, q: Vec2): number => p[0] * q[1] - p[1] * q[0];
const norm = (p: Vec2): number => Math.hypot(p[0], p[1]);
const dist = (p: Vec2, q: Vec2): number => Math.hypot(p[0] - q[0], p[1] - q[1]);
const finite2 = (p: Vec2): boolean => Number.isFinite(p[0]) && Number.isFinite(p[1]);

function unit(p: Vec2): Vec2 | null {
  const n = norm(p);
  if (n < EPS) return null;
  return [p[0] / n, p[1] / n];
}

/** Curves that can take part in a chain: line / arc (a circle has no
 *  endpoints and is handled by the single-curve concentric path). */
export type LineOrArc =
  | { kind: "line"; a: Vec2; b: Vec2 }
  | { kind: "arc"; c: Vec2; r: number; start: number; sweep: number };

/** Endpoints of a chainable curve in drawn order (start, end). */
function curveStart(cv: LineOrArc): Vec2 {
  if (cv.kind === "line") return cv.a;
  return [cv.c[0] + cv.r * Math.cos(cv.start), cv.c[1] + cv.r * Math.sin(cv.start)];
}
function curveEnd(cv: LineOrArc): Vec2 {
  if (cv.kind === "line") return cv.b;
  return [
    cv.c[0] + cv.r * Math.cos(cv.start + cv.sweep),
    cv.c[1] + cv.r * Math.sin(cv.start + cv.sweep),
  ];
}

// ════════════════════════════════════════════════════════════════════
// Single-curve offset (degenerate chain of length 1)
// ════════════════════════════════════════════════════════════════════

/**
 * Offset `cv` by `distance` mm toward the side the click `sidePoint`
 * lies on. Returns the offset curve, or null when no valid offset exists
 * (non-positive / non-finite distance, degenerate line, or an inner
 * circle/arc offset that collapses the radius).
 */
export function offsetCurve(
  cv: Curve,
  distance: number,
  sidePoint: Vec2,
): Curve | null {
  if (!Number.isFinite(distance) || distance <= 0) return null;

  if (cv.kind === "line") {
    const dx = cv.b[0] - cv.a[0];
    const dy = cv.b[1] - cv.a[1];
    const lenSq = dx * dx + dy * dy;
    if (lenSq < LEN_EPS_SQ) return null;
    const len = Math.sqrt(lenSq);
    // Unit normal n = perp(unit(b − a)) = (−dy, dx)/len.
    const nx = -dy / len;
    const ny = dx / len;
    const sideDot = (sidePoint[0] - cv.a[0]) * nx + (sidePoint[1] - cv.a[1]) * ny;
    const s = sideDot < 0 ? -1 : 1;
    const ox = s * distance * nx;
    const oy = s * distance * ny;
    return {
      kind: "line",
      a: [cv.a[0] + ox, cv.a[1] + oy],
      b: [cv.b[0] + ox, cv.b[1] + oy],
    };
  }

  if (cv.kind === "circle") {
    const distToCentre = Math.hypot(sidePoint[0] - cv.c[0], sidePoint[1] - cv.c[1]);
    const outside = distToCentre > cv.r;
    const r = outside ? cv.r + distance : cv.r - distance;
    if (r <= 0) return null;
    return { kind: "circle", c: cv.c, r };
  }

  // arc: same radius rule as circle, same centre / start / sweep.
  const distToCentre = Math.hypot(sidePoint[0] - cv.c[0], sidePoint[1] - cv.c[1]);
  const outside = distToCentre > cv.r;
  const r = outside ? cv.r + distance : cv.r - distance;
  if (r <= 0) return null;
  return { kind: "arc", c: cv.c, r, start: cv.start, sweep: cv.sweep };
}

// ════════════════════════════════════════════════════════════════════
// Chain detection
// ════════════════════════════════════════════════════════════════════

/** An ordered, connected chain of curves plus whether it closes on
 *  itself. `order` lists indices into the original curve array in walk
 *  order; each entry's drawn direction is normalised so consecutive
 *  curves share `end(prev) ≈ start(next)`. */
export interface Chain {
  /** Curves in walk order, each oriented so end→start chains up. */
  curves: LineOrArc[];
  /** Original indices (parallel to `curves`), for sub-call replacement. */
  indices: number[];
  /** True when the last curve's end coincides with the first's start. */
  closed: boolean;
}

/** Reverse a curve's drawn direction (so its endpoints swap). */
function reverseCurve(cv: LineOrArc): LineOrArc {
  if (cv.kind === "line") return { kind: "line", a: cv.b, b: cv.a };
  // arc: walk it the other way — start at the old end, negate the sweep.
  return {
    kind: "arc",
    c: cv.c,
    r: cv.r,
    start: cv.start + cv.sweep,
    sweep: -cv.sweep,
  };
}

/**
 * Walk the connected component of `curves` that contains `seedIndex`,
 * following shared endpoints (within `JOIN_EPS`). Returns the chain in
 * walk order with each curve oriented head-to-tail, plus whether it
 * forms a closed loop. A curve with no coincident neighbours yields a
 * length-1 open chain.
 *
 * Only line/arc curves participate; a full circle is never part of a
 * chain (it has no endpoints) and is handled by [`offsetCurve`].
 */
export function buildChain(
  curves: ReadonlyArray<Curve | null>,
  seedIndex: number,
): Chain | null {
  const seed = curves[seedIndex];
  if (!seed || seed.kind === "circle") return null;

  // Adjacency by endpoint coincidence. We grow from the seed in both
  // directions, picking at each step the unused curve whose nearer
  // endpoint coincides with the current free end.
  const used = new Set<number>([seedIndex]);

  type Node = { index: number; cv: LineOrArc };
  const chain: Node[] = [{ index: seedIndex, cv: seed }];

  const matchAt = (pt: Vec2, skip: Set<number>): Node | null => {
    for (let i = 0; i < curves.length; i++) {
      if (skip.has(i)) continue;
      const c = curves[i];
      if (!c || c.kind === "circle") continue;
      const s = curveStart(c);
      const e = curveEnd(c);
      if (dist(pt, s) <= JOIN_EPS) return { index: i, cv: c };
      if (dist(pt, e) <= JOIN_EPS) return { index: i, cv: reverseCurve(c) };
    }
    return null;
  };

  // Extend forward from the tail's end.
  for (;;) {
    const tail = chain[chain.length - 1];
    const next = matchAt(curveEnd(tail.cv), used);
    if (!next) break;
    used.add(next.index);
    chain.push(next);
  }

  // Extend backward from the head's start (prepend, oriented so its end
  // meets the head's start).
  for (;;) {
    const head = chain[0];
    const prev = matchAt(curveStart(head.cv), used);
    if (!prev) break;
    used.add(prev.index);
    // `prev.cv` is oriented start→(coincident with head.start); we need
    // its END at head.start, so reverse it.
    chain.unshift({ index: prev.index, cv: reverseCurve(prev.cv) });
  }

  const closed =
    chain.length >= 2 &&
    dist(curveEnd(chain[chain.length - 1].cv), curveStart(chain[0].cv)) <= JOIN_EPS;

  return {
    curves: chain.map((n) => n.cv),
    indices: chain.map((n) => n.index),
    closed,
  };
}

// ════════════════════════════════════════════════════════════════════
// Chain offset
// ════════════════════════════════════════════════════════════════════

/**
 * Offset one oriented curve to its LEFT (+1) or RIGHT (−1) side by
 * `distance`. "Left" is the +90° normal of the drawn direction at every
 * point. For a line that's the constant normal `(−dy, dx)`; for an arc
 * it grows or shrinks the radius depending on whether left points away
 * from or toward the centre. Returns null if an arc collapses.
 */
function offsetSegment(cv: LineOrArc, distance: number, side: 1 | -1): LineOrArc | null {
  if (cv.kind === "line") {
    const d = unit(sub(cv.b, cv.a));
    if (!d) return null;
    const n: Vec2 = [-d[1] * side, d[0] * side];
    const off = scale(n, distance);
    return { kind: "line", a: add(cv.a, off), b: add(cv.b, off) };
  }
  // arc: the left normal of an arc travelled with sweep s points:
  //   sweep > 0 (CCW): toward the centre (inward) → radius shrinks for +left.
  //   sweep < 0 (CW):  away from centre (outward) → radius grows for +left.
  // So delta = −side·sign(sweep)·distance.
  const sgn = cv.sweep >= 0 ? 1 : -1;
  const r = cv.r - side * sgn * distance;
  if (r <= EPS) return null;
  return { kind: "arc", c: cv.c, r, start: cv.start, sweep: cv.sweep };
}

/**
 * Which side of the clicked segment does `sidePoint` lie on, in the
 * segment's oriented frame? +1 = left (the +90° normal side), −1 = right.
 * Deterministic +1 when the click sits on the line.
 */
function clickedSide(cv: LineOrArc, sidePoint: Vec2): 1 | -1 {
  if (cv.kind === "line") {
    const d = unit(sub(cv.b, cv.a));
    if (!d) return 1;
    const n: Vec2 = [-d[1], d[0]];
    const s = dot(sub(sidePoint, cv.a), n);
    return s < 0 ? -1 : 1;
  }
  // arc: outside the rim (farther from centre) is the side away from the
  // centre. Left of a CCW arc is inward (toward centre); left of a CW arc
  // is outward. Map "outside" → which oriented side that is.
  const outside = dist(sidePoint, cv.c) > cv.r;
  const sgn = cv.sweep >= 0 ? 1 : -1;
  // outside means we want radius to GROW; offsetSegment grows when
  // −side·sgn > 0 → side = −sgn. inside (shrink) → side = +sgn.
  if (outside) return (-sgn) as 1 | -1;
  return sgn as 1 | -1;
}

/** Wrap into [0, TAU). */
function norm2pi(a: number): number {
  let x = a % TAU;
  if (x < 0) x += TAU;
  return x;
}

/** Replace a curve's start endpoint (keeping the supporting geometry). */
function setStart(cv: LineOrArc, p: Vec2): LineOrArc {
  if (cv.kind === "line") return { kind: "line", a: p, b: cv.b };
  // arc: move the start angle to p's angle, keep the same end point.
  const end = curveEnd(cv);
  const newStart = Math.atan2(p[1] - cv.c[1], p[0] - cv.c[0]);
  const endAng = Math.atan2(end[1] - cv.c[1], end[0] - cv.c[0]);
  let sweep = cv.sweep >= 0 ? norm2pi(endAng - newStart) : -norm2pi(newStart - endAng);
  if (Math.abs(sweep) < EPS) sweep = cv.sweep; // guard against zero collapse
  return { kind: "arc", c: cv.c, r: cv.r, start: newStart, sweep };
}

/** Replace a curve's end endpoint (keeping the supporting geometry). */
function setEnd(cv: LineOrArc, p: Vec2): LineOrArc {
  if (cv.kind === "line") return { kind: "line", a: cv.a, b: p };
  const start = curveStart(cv);
  const startAng = Math.atan2(start[1] - cv.c[1], start[0] - cv.c[0]);
  const newEnd = Math.atan2(p[1] - cv.c[1], p[0] - cv.c[0]);
  let sweep = cv.sweep >= 0 ? norm2pi(newEnd - startAng) : -norm2pi(startAng - newEnd);
  if (Math.abs(sweep) < EPS) sweep = cv.sweep;
  return { kind: "arc", c: cv.c, r: cv.r, start: startAng, sweep };
}

/**
 * Intersection points of the two offset segments' SUPPORTING geometry
 * (the full infinite line / full circle, not the bounded extents), so a
 * joint can be reconciled even when the trimmed-back ends don't reach.
 * Reuses [`intersectCurves`] by promoting each operand to its unbounded
 * form (line → a long line through both its points; arc → its circle).
 */
function supportIntersections(a: LineOrArc, b: LineOrArc): Vec2[] {
  const promote = (cv: LineOrArc): Curve => {
    if (cv.kind === "arc") return { kind: "circle", c: cv.c, r: cv.r };
    // line: extend far past both ends so the bounded line∩ test in
    // sketchTrim still finds an intersection of the support lines.
    const d = unit(sub(cv.b, cv.a));
    if (!d) return cv;
    const mid: Vec2 = [(cv.a[0] + cv.b[0]) / 2, (cv.a[1] + cv.b[1]) / 2];
    const L = 1e6;
    return { kind: "line", a: add(mid, scale(d, -L)), b: add(mid, scale(d, L)) };
  };
  return intersectCurves(promote(a), promote(b)).filter(finite2);
}

/** The connection point the two ORIGINAL segments share (end of prev =
 *  start of next), used to disambiguate which support intersection is the
 *  real corner and to centre the convex fillet. */
function jointVertex(prev: LineOrArc, next: LineOrArc): Vec2 {
  const e = curveEnd(prev);
  const s = curveStart(next);
  return [(e[0] + s[0]) / 2, (e[1] + s[1]) / 2];
}

/** Unit tangent at a curve's END point, in its drawn direction. */
function tangentAtEnd(cv: LineOrArc): Vec2 | null {
  if (cv.kind === "line") return unit(sub(cv.b, cv.a));
  // arc: tangent = perp(radius) rotated in the sweep direction.
  const e = curveEnd(cv);
  const rad = unit(sub(e, cv.c));
  if (!rad) return null;
  const sgn = cv.sweep >= 0 ? 1 : -1;
  return [-rad[1] * sgn, rad[0] * sgn];
}

/** Unit tangent at a curve's START point, in its drawn direction. */
function tangentAtStart(cv: LineOrArc): Vec2 | null {
  if (cv.kind === "line") return unit(sub(cv.b, cv.a));
  const s = curveStart(cv);
  const rad = unit(sub(s, cv.c));
  if (!rad) return null;
  const sgn = cv.sweep >= 0 ? 1 : -1;
  return [-rad[1] * sgn, rad[0] * sgn];
}

/**
 * Is the joint between original `prev` (entering V) and `next` (leaving V)
 * a CONVEX (outer) corner for an offset on `side`? The corner turns by
 * `turn = cross(tIn, tOut)` (>0 left turn, <0 right turn). Offsetting to a
 * given side opens an outward gap (convex) when the offset moves AWAY from
 * the turn centre — i.e. when `side` and `turn` have OPPOSITE signs:
 * offsetting left (side +1) at a right turn, or right (side −1) at a left
 * turn. A straight continuation (turn ≈ 0) is neither — non-convex (a
 * plain trim / extend). For a CCW convex loop every corner is a left turn,
 * so an inward (left) offset is concave everywhere and an outward (right)
 * offset is convex everywhere — the expected rounded-offset behaviour.
 */
function isConvexJoint(prev: LineOrArc, next: LineOrArc, side: 1 | -1): boolean {
  const tIn = tangentAtEnd(prev);
  const tOut = tangentAtStart(next);
  if (!tIn || !tOut) return false;
  const turn = cross(tIn, tOut);
  return side * turn < -EPS;
}

/**
 * Build the rounding fillet arc for a convex corner: radius = `distance`,
 * centred at the original shared vertex `V`, running from the offset
 * prev-segment's end `p` to the offset next-segment's start `q`, the short
 * way (it spans the gap the outward corner opened). Returns null if the
 * endpoints don't sit on the radius-`distance` circle about V (shouldn't
 * happen for a true offset, but guards NaN).
 */
function convexFillet(V: Vec2, p: Vec2, q: Vec2, distance: number): LineOrArc | null {
  const startAng = Math.atan2(p[1] - V[1], p[0] - V[0]);
  const endAng = Math.atan2(q[1] - V[1], q[0] - V[0]);
  let sweep = endAng - startAng;
  while (sweep <= -Math.PI) sweep += TAU;
  while (sweep > Math.PI) sweep -= TAU;
  if (!Number.isFinite(sweep) || Math.abs(sweep) < EPS) return null;
  return { kind: "arc", c: V, r: distance, start: startAng, sweep };
}

/**
 * Reconcile the joint between two consecutive OFFSET segments `prev` and
 * `next` (sharing original vertex `V`). Mutates nothing; returns the
 * trimmed prev, an optional bridging fillet, and the trimmed next:
 *
 *  - If the supporting geometries intersect near the joint (concave /
 *    inner corner), trim both segments to that intersection — no fillet.
 *  - Otherwise (convex / outer corner) keep both ends and insert a
 *    rounding arc of radius `distance` centred at `V` between them.
 *
 * `convex` (computed from the ORIGINAL corner's turn direction vs the
 * offset side) decides the branch: a convex corner always rounds; a
 * concave / collinear corner trims to the support intersection.
 */
function reconcileJoint(
  prev: LineOrArc,
  next: LineOrArc,
  V: Vec2,
  distance: number,
  convex: boolean,
): { prev: LineOrArc; fillet: LineOrArc | null; next: LineOrArc } {
  const pEnd = curveEnd(prev);
  const nStart = curveStart(next);

  // Already meeting (e.g. collinear continuation) → nothing to do.
  if (dist(pEnd, nStart) <= JOIN_EPS) {
    return { prev, fillet: null, next };
  }

  // Concave / collinear corner: trim or extend both offset segments to
  // their support-geometry intersection.
  if (!convex) {
    const hits = supportIntersections(prev, next);
    if (hits.length > 0) {
      // Choose the intersection nearest the (offset) joint mid-point — the
      // genuine corner, not a far-away second crossing of two circles.
      const jm: Vec2 = [(pEnd[0] + nStart[0]) / 2, (pEnd[1] + nStart[1]) / 2];
      let best = hits[0];
      let bestD = dist(best, jm);
      for (const h of hits) {
        const dd = dist(h, jm);
        if (dd < bestD) {
          bestD = dd;
          best = h;
        }
      }
      const tp = setEnd(prev, best);
      const tn = setStart(next, best);
      if (validCurve(tp) && validCurve(tn)) {
        return { prev: tp, fillet: null, next: tn };
      }
    }
    // No usable intersection (e.g. an offset segment collapsed past the
    // corner) → fall through to a rounding bridge rather than leave a gap.
  }

  // Convex (outer) corner: bridge the opened gap with a rounding fillet
  // arc of radius `distance` centred at the original shared vertex V.
  const fillet = convexFillet(V, pEnd, nStart, distance);
  return { prev, fillet, next };
}

/** A curve is valid when its coordinates are finite and it has extent. */
function validCurve(cv: Curve): boolean {
  if (cv.kind === "line") {
    return finite2(cv.a) && finite2(cv.b) && dist(cv.a, cv.b) > EPS;
  }
  if (cv.kind === "circle") {
    return finite2(cv.c) && Number.isFinite(cv.r) && cv.r > EPS;
  }
  return (
    finite2(cv.c) &&
    Number.isFinite(cv.r) &&
    cv.r > EPS &&
    Number.isFinite(cv.start) &&
    Number.isFinite(cv.sweep) &&
    Math.abs(cv.sweep) > EPS
  );
}

/**
 * Offset a CONNECTED chain of curves by `distance` toward the clicked
 * side. `clickedPos` is the index (into `chain.curves`) of the segment the
 * user clicked, and `sidePoint` the click point used to pick the side.
 *
 * Returns the reconciled offset curves (lines + arcs, including any convex
 * rounding arcs), in chain order, or null when the distance is invalid or
 * the whole result would collapse (no surviving valid segment).
 *
 * Degenerate per-segment offsets (an arc whose radius would invert) are
 * dropped and their neighbours reconciled directly, so a too-large
 * distance thins the chain rather than producing NaN geometry.
 */
export function offsetChain(
  chain: Chain,
  clickedPos: number,
  distance: number,
  sidePoint: Vec2,
): Curve[] | null {
  if (!Number.isFinite(distance) || distance <= 0) return null;
  const n = chain.curves.length;
  if (n === 0) return null;
  const clicked = chain.curves[clickedPos];
  if (!clicked) return null;

  // Global side, derived once from the clicked segment, applied to all.
  const side = clickedSide(clicked, sidePoint);

  // Offset each segment; a null (collapsed) offset drops that segment.
  const offsets: Array<LineOrArc | null> = chain.curves.map((cv) =>
    offsetSegment(cv, distance, side),
  );

  // Surviving segments keep their original index so we can find the
  // shared vertex V between two (possibly non-adjacent after drops)
  // neighbours.
  const kept: Array<{ off: LineOrArc; orig: number }> = [];
  for (let i = 0; i < n; i++) {
    const off = offsets[i];
    if (off && validCurve(off)) kept.push({ off, orig: i });
  }
  if (kept.length === 0) return null;

  // Single surviving segment: nothing to reconcile.
  if (kept.length === 1) return [kept[0].off];

  // Reconcile consecutive joints. Result is interleaved: seg, [fillet],
  // seg, [fillet], … . We accumulate into `out`, mutating the rolling
  // "prev" as joints trim it back.
  const out: LineOrArc[] = [];
  let prev = kept[0].off;
  for (let k = 1; k < kept.length; k++) {
    const next = kept[k].off;
    // The original vertex shared by the two ORIGINAL segments that these
    // offsets came from (use the original chain to find it; with drops we
    // bridge the dropped span by taking prev's original end vertex).
    const oPrev = chain.curves[kept[k - 1].orig];
    const oNext = chain.curves[kept[k].orig];
    const V = jointVertex(oPrev, oNext);
    const convex = isConvexJoint(oPrev, oNext, side);
    const r = reconcileJoint(prev, next, V, distance, convex);
    out.push(r.prev);
    if (r.fillet) out.push(r.fillet);
    prev = r.next;
  }
  out.push(prev);

  // Closed loop: reconcile the wrap-around joint (last → first) and fold
  // the trim back into the first/last emitted segments.
  if (chain.closed && kept.length >= 2) {
    const first = out[0];
    const last = out[out.length - 1];
    const oPrev = chain.curves[kept[kept.length - 1].orig];
    const oNext = chain.curves[kept[0].orig];
    const V = jointVertex(oPrev, oNext);
    const convex = isConvexJoint(oPrev, oNext, side);
    const r = reconcileJoint(last, first, V, distance, convex);
    out[out.length - 1] = r.prev;
    out[0] = r.next;
    if (r.fillet) out.push(r.fillet);
  }

  // Final guard: every emitted curve must be finite + valid.
  const clean = out.filter(validCurve);
  if (clean.length === 0) return null;
  return clean;
}
