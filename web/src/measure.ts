/**
 * Geometric measurement of picked B-Rep elements (ticket stv8).
 *
 * The Measure tool resolves a picked edge to its sampled world-space polyline
 * (via `kernel.bodyBrepEdges`) and this module turns that polyline into the
 * numbers Onshape's Measure dialog shows: a straight edge's LENGTH, a circular
 * rim's RADIUS / DIAMETER / centre, and the DISTANCE between two edges.
 *
 * All math is pure (flat `[x,y,z, …]` arrays in mm) so it is unit-tested
 * without a kernel or a canvas. The edge geometry the kernel exposes is already
 * a dense sampled polyline (Hermite/Nurbs/Polyline → `sample_edge`), so a
 * polyline integral here is the same length the kernel would compute — no
 * analytic Line/Circle to defer to. Circularity is detected from the sample
 * cloud (closed loop + constant radius about the centroid).
 */

export type Vec3 = [number, number, number];

export interface CircleInfo {
  center: Vec3;
  radius: number;
  /** Unit plane normal of the circle (orientation only; sign arbitrary). */
  normal: Vec3;
}

export interface EdgeMeasure {
  /** Arc length of the polyline, in mm. */
  length: number;
  /** True when the polyline returns to its start (a rim / full loop). */
  closed: boolean;
  /** Set when the (closed) edge is a circle within tolerance. */
  circle: CircleInfo | null;
}

export interface DistanceMeasure {
  /** Closest approach between the two polylines, in mm. */
  min: number;
  /** The closest points, one on each polyline (world mm). */
  a: Vec3;
  b: Vec3;
  /** Component deltas b − a of the closest-point pair. */
  delta: Vec3;
}

function sub(a: Vec3, b: Vec3): Vec3 {
  return [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
}
function dist(a: Vec3, b: Vec3): number {
  return Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
}
function at(pts: number[], i: number): Vec3 {
  return [pts[i * 3], pts[i * 3 + 1], pts[i * 3 + 2]];
}
function cross(a: Vec3, b: Vec3): Vec3 {
  return [
    a[1] * b[2] - a[2] * b[1],
    a[2] * b[0] - a[0] * b[2],
    a[0] * b[1] - a[1] * b[0],
  ];
}
function norm(a: Vec3): Vec3 {
  const l = Math.hypot(a[0], a[1], a[2]);
  return l < 1e-12 ? [0, 0, 0] : [a[0] / l, a[1] / l, a[2] / l];
}

/** Total arc length of a flat-xyz polyline, in mm. */
export function polylineLength(pts: number[]): number {
  let L = 0;
  for (let i = 3; i < pts.length; i += 3) {
    L += Math.hypot(
      pts[i] - pts[i - 3],
      pts[i + 1] - pts[i - 2],
      pts[i + 2] - pts[i - 1],
    );
  }
  return L;
}

/**
 * Classify a sampled edge: its length, whether it is closed, and — when closed
 * and round within 2 % radial spread — its circle (centre/radius/normal).
 */
export function analyzeEdge(pts: number[]): EdgeMeasure | null {
  const n = Math.floor(pts.length / 3);
  if (n < 2) return null;
  const length = polylineLength(pts);
  const first = at(pts, 0);
  const last = at(pts, n - 1);
  const gap = dist(first, last);
  const closed = gap < 1e-6 || gap < length * 0.02;

  let circle: CircleInfo | null = null;
  // Drop a duplicated closing sample so the centroid is not double-weighted.
  const m = gap < 1e-6 ? n - 1 : n;
  if (closed && m >= 8) {
    let cx = 0,
      cy = 0,
      cz = 0;
    for (let i = 0; i < m; i++) {
      cx += pts[i * 3];
      cy += pts[i * 3 + 1];
      cz += pts[i * 3 + 2];
    }
    const center: Vec3 = [cx / m, cy / m, cz / m];
    let sum = 0,
      sumsq = 0;
    for (let i = 0; i < m; i++) {
      const r = dist(center, at(pts, i));
      sum += r;
      sumsq += r * r;
    }
    const mean = sum / m;
    const variance = Math.max(0, sumsq / m - mean * mean);
    const spread = mean > 1e-9 ? Math.sqrt(variance) / mean : Infinity;
    if (mean > 1e-6 && spread < 0.02) {
      // Plane normal: average the cross products of successive radius vectors,
      // which all point along the circle axis for a planar loop.
      let nx = 0,
        ny = 0,
        nz = 0;
      for (let i = 0; i < m; i++) {
        const r0 = sub(at(pts, i), center);
        const r1 = sub(at(pts, (i + 1) % m), center);
        const c = cross(r0, r1);
        nx += c[0];
        ny += c[1];
        nz += c[2];
      }
      circle = { center, radius: mean, normal: norm([nx, ny, nz]) };
    }
  }
  return { length, closed, circle };
}

function dot(a: Vec3, b: Vec3): number {
  return a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
}
function add(a: Vec3, b: Vec3, s: number): Vec3 {
  return [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s];
}
const clamp01 = (x: number) => (x < 0 ? 0 : x > 1 ? 1 : x);

/**
 * Closest points between two segments [p1,q1] and [p2,q2] (Ericson,
 * Real-Time Collision Detection). Handles degenerate (point) segments and
 * interior crossings exactly — vertex-to-segment probing alone misses the
 * latter on sparse polylines.
 */
function closestSegmentSegment(
  p1: Vec3,
  q1: Vec3,
  p2: Vec3,
  q2: Vec3,
): { d2: number; c1: Vec3; c2: Vec3 } {
  const d1 = sub(q1, p1);
  const d2 = sub(q2, p2);
  const r = sub(p1, p2);
  const a = dot(d1, d1);
  const e = dot(d2, d2);
  const f = dot(d2, r);
  let s: number;
  let t: number;
  if (a <= 1e-18 && e <= 1e-18) {
    s = 0;
    t = 0;
  } else if (a <= 1e-18) {
    s = 0;
    t = clamp01(f / e);
  } else {
    const c = dot(d1, r);
    if (e <= 1e-18) {
      t = 0;
      s = clamp01(-c / a);
    } else {
      const b = dot(d1, d2);
      const denom = a * e - b * b;
      s = denom > 1e-18 ? clamp01((b * f - c * e) / denom) : 0;
      t = (b * s + f) / e;
      if (t < 0) {
        t = 0;
        s = clamp01(-c / a);
      } else if (t > 1) {
        t = 1;
        s = clamp01((b - c) / a);
      }
    }
  }
  const c1 = add(p1, d1, s);
  const c2 = add(p2, d2, t);
  const dd = sub(c1, c2);
  return { d2: dot(dd, dd), c1, c2 };
}

/**
 * Minimum distance between two sampled polylines, computed over every
 * segment-pair (exact for crossings; O(n·m) stays cheap for two edges on the
 * kernel's dense sampling). A single-point "polyline" is treated as a
 * degenerate segment.
 */
export function minDistanceBetweenPolylines(
  aPts: number[],
  bPts: number[],
): DistanceMeasure | null {
  const na = Math.floor(aPts.length / 3);
  const nb = Math.floor(bPts.length / 3);
  if (na < 1 || nb < 1) return null;

  let best = Infinity;
  let bestA: Vec3 = at(aPts, 0);
  let bestB: Vec3 = at(bPts, 0);

  const segsA = Math.max(1, na - 1);
  const segsB = Math.max(1, nb - 1);
  for (let i = 0; i < segsA; i++) {
    const a0 = at(aPts, i);
    const a1 = at(aPts, na === 1 ? 0 : i + 1);
    for (let j = 0; j < segsB; j++) {
      const b0 = at(bPts, j);
      const b1 = at(bPts, nb === 1 ? 0 : j + 1);
      const r = closestSegmentSegment(a0, a1, b0, b1);
      if (r.d2 < best) {
        best = r.d2;
        bestA = r.c1;
        bestB = r.c2;
      }
    }
  }

  return {
    min: Math.sqrt(best),
    a: bestA,
    b: bestB,
    delta: sub(bestB, bestA),
  };
}
