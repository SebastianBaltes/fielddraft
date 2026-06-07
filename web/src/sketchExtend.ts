/**
 * Interactive line extend for the 2D sketcher (UI gap follow-up,
 * sibling of [`sketchTrim`](./sketchTrim) and [`sketchFillet`](./sketchFillet)).
 *
 * "Extend to nearest" — the complement of Trim. The user clicks a line
 * near one of its ends; that end is lengthened ALONG the line's own
 * direction until it meets the nearest other curve (line, circle or arc).
 * Like trim and fillet, this is a pure, DOM-free rewrite that reads the
 * solved geometry from the sub-call params and emits an ordinary
 * `sketch_line` — the kernel needs no extend tool. The store splices the
 * lengthened line in via [`replaceSketchSubCallAt`](./sketchSession) and
 * re-solves.
 *
 * Scope (v1): extends `sketch_line` targets only. Extending an arc or a
 * circle (grow the sweep until it crosses something) is out of scope and
 * returns null — the caller treats null as "do nothing". Other curves
 * (lines, circles, arcs) all act as targets to extend INTO.
 *
 * Geometry — the moving endpoint is the one CLOSER to the click (the user
 * grabs the end they want to push out). The extension ray starts at that
 * moving endpoint and points OUTWARD, away from the segment:
 *   dir = unit(movingEnd − otherEnd).
 * For every other curve we intersect the line's INFINITE support with that
 * curve (the target span stays bounded — a circle/arc is clamped to its
 * own extent, a line to its drawn segment), keep only hits that lie in the
 * FORWARD direction (dot(hit − movingEnd, dir) > eps), and pick the
 * NEAREST one. The lengthened line keeps the other endpoint and moves the
 * grabbed endpoint onto that hit.
 */

import type { Curve, Vec2 } from "./sketchTrim";

/** Re-export the shared geometry vocabulary so callers (tests, store)
 *  don't reach into two modules for the same `Curve` / `Vec2` types. */
export type { Curve, Vec2 } from "./sketchTrim";

const EPS = 1e-9;
const TAU = Math.PI * 2;

// ── small vector helpers ────────────────────────────────────────────

const sub = (p: Vec2, q: Vec2): Vec2 => [p[0] - q[0], p[1] - q[1]];
const add = (p: Vec2, q: Vec2): Vec2 => [p[0] + q[0], p[1] + q[1]];
const scale = (p: Vec2, s: number): Vec2 => [p[0] * s, p[1] * s];
const dot = (p: Vec2, q: Vec2): number => p[0] * q[0] + p[1] * q[1];
const cross = (p: Vec2, q: Vec2): number => p[0] * q[1] - p[1] * q[0];
const len2 = (p: Vec2): number => p[0] * p[0] + p[1] * p[1];
const norm = (p: Vec2): number => Math.hypot(p[0], p[1]);
const dist = (p: Vec2, q: Vec2): number => Math.hypot(p[0] - q[0], p[1] - q[1]);

function unit(p: Vec2): Vec2 | null {
  const n = norm(p);
  if (n < EPS) return null;
  return [p[0] / n, p[1] / n];
}

/** Wrap a value into [0, TAU). */
function norm2pi(a: number): number {
  let x = a % TAU;
  if (x < 0) x += TAU;
  return x;
}

/** True iff `theta` lies on the arc spanning `start` → `start + sweep`,
 *  handling both sweep directions and the ±π seam. */
function thetaOnArc(start: number, sweep: number, theta: number): boolean {
  const span = Math.abs(sweep);
  const travelled = sweep >= 0 ? norm2pi(theta - start) : norm2pi(start - theta);
  return travelled <= span + 1e-9;
}

// ── ray (infinite line support) vs bounded target intersections ─────

/**
 * Intersection of an infinite line through `o` with direction `d` and a
 * bounded segment `[a, b]`. Returns the single crossing point (clamped to
 * the segment's extent) or null. The ray is infinite — we only bound the
 * OTHER curve, exactly like trim clamps cutters to their drawn span.
 */
function rayVsSegment(o: Vec2, d: Vec2, a: Vec2, b: Vec2): Vec2 | null {
  const s = sub(b, a);
  const denom = cross(d, s);
  if (Math.abs(denom) < EPS) return null; // parallel / collinear
  const w = sub(a, o);
  // o + t·d = a + u·s
  const u = cross(w, d) / denom;
  if (u < -1e-9 || u > 1 + 1e-9) return null; // off the target segment
  return add(a, scale(s, Math.max(0, Math.min(1, u))));
}

/**
 * Intersections of an infinite line through `o` with direction `d` and a
 * circle of centre `c`, radius `r`. Returns 0–2 points. The line is the
 * full support (parameter t unbounded), the circle is unbounded by nature;
 * an optional `onArc` predicate clamps the hits onto an arc's drawn span.
 */
function rayVsCircle(
  o: Vec2,
  d: Vec2,
  c: Vec2,
  r: number,
  onArc?: (theta: number) => boolean,
): Vec2[] {
  const f = sub(o, c);
  const aa = len2(d);
  if (aa < EPS) return [];
  const bb = 2 * dot(f, d);
  const cc = len2(f) - r * r;
  let disc = bb * bb - 4 * aa * cc;
  if (disc < -1e-9) return [];
  disc = Math.max(0, disc);
  const sq = Math.sqrt(disc);
  const out: Vec2[] = [];
  for (const t of [(-bb - sq) / (2 * aa), (-bb + sq) / (2 * aa)]) {
    const p = add(o, scale(d, t));
    if (onArc) {
      const theta = Math.atan2(p[1] - c[1], p[0] - c[0]);
      if (!onArc(theta)) continue;
    }
    out.push(p);
  }
  return out;
}

/** All crossings of the infinite ray (origin `o`, direction `d`) with a
 *  bounded other curve. Lines clamp to their segment, arcs to their span,
 *  circles are whole. */
function rayVsCurve(o: Vec2, d: Vec2, other: Curve): Vec2[] {
  if (other.kind === "line") {
    const p = rayVsSegment(o, d, other.a, other.b);
    return p ? [p] : [];
  }
  if (other.kind === "circle") {
    return rayVsCircle(o, d, other.c, other.r);
  }
  // arc: clamp the circle hits to the drawn span
  return rayVsCircle(o, d, other.c, other.r, (theta) =>
    thetaOnArc(other.start, other.sweep, theta),
  );
}

// ── the extend itself ───────────────────────────────────────────────

/**
 * Lengthen the line `line` toward the nearest of `others`. The endpoint
 * CLOSER to `clickPoint` is the one moved (the grabbed end); it travels
 * OUTWARD along the line until it meets the nearest other curve that lies
 * in that forward direction and within that curve's own extent. Returns
 * the lengthened line (other endpoint preserved), or null when:
 *  - `line` isn't a line (arcs/circles are out of scope for v1),
 *  - the line has zero length (no direction to extend),
 *  - no other curve crosses the forward extension (nothing to reach).
 */
export function extendLine(
  line: Curve,
  clickPoint: Vec2,
  others: ReadonlyArray<Curve>,
): Curve | null {
  if (line.kind !== "line") return null;

  // Grabbed endpoint = the one closer to the click; the other stays put.
  const dA = dist(clickPoint, line.a);
  const dB = dist(clickPoint, line.b);
  const moving = dA <= dB ? line.a : line.b;
  const anchor = dA <= dB ? line.b : line.a;

  // Outward direction: away from the anchor, past the moving end.
  const dir = unit(sub(moving, anchor));
  if (!dir) return null; // zero-length line

  let best: Vec2 | null = null;
  let bestT = Infinity;
  for (const other of others) {
    for (const hit of rayVsCurve(moving, dir, other)) {
      // Forward only: the hit must be ahead of the moving end along dir.
      // (Strictly positive — a crossing AT the moving end is not an
      // extension, and a crossing BEHIND it is the wrong direction.)
      const t = dot(sub(hit, moving), dir);
      if (t <= 1e-7) continue;
      if (t < bestT) {
        bestT = t;
        best = hit;
      }
    }
  }
  if (!best) return null;

  // Move the grabbed endpoint onto the hit, keep the anchor.
  return dA <= dB
    ? { kind: "line", a: best, b: line.b }
    : { kind: "line", a: line.a, b: best };
}
