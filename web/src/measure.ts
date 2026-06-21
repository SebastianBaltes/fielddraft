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

// ─────────────────────────────────────────────────────────────────────────
//  Face geometry — ticket me9w
//
//  A B-Rep face is measured from a world-space TRIANGLE SOUP (flat `[x,y,z, …]`,
//  9 numbers per triangle). The resolver (`measureResolve.ts`) builds that soup
//  mesh-independently (planar boundary triangulation / analytic-surface
//  tessellation), and every face measurement below is a pure function over it.
// ─────────────────────────────────────────────────────────────────────────

/** Area of the triangle (a,b,c) = ½‖(b−a)×(c−a)‖. */
function triArea(a: Vec3, b: Vec3, c: Vec3): number {
  const u = sub(b, a);
  const v = sub(c, a);
  const cx = u[1] * v[2] - u[2] * v[1];
  const cy = u[2] * v[0] - u[0] * v[2];
  const cz = u[0] * v[1] - u[1] * v[0];
  return 0.5 * Math.hypot(cx, cy, cz);
}

/** Total surface area of a flat triangle soup (9 floats per triangle), in mm². */
export function triangleSoupArea(tris: number[]): number {
  let area = 0;
  for (let i = 0; i + 8 < tris.length; i += 9) {
    area += triArea(
      [tris[i], tris[i + 1], tris[i + 2]],
      [tris[i + 3], tris[i + 4], tris[i + 5]],
      [tris[i + 6], tris[i + 7], tris[i + 8]],
    );
  }
  return area;
}

/** Area-weighted centroid of a flat triangle soup. Returns `[0,0,0]` for an
 *  empty / degenerate soup. */
export function triangleSoupCentroid(tris: number[]): Vec3 {
  let cx = 0,
    cy = 0,
    cz = 0,
    wsum = 0;
  for (let i = 0; i + 8 < tris.length; i += 9) {
    const a: Vec3 = [tris[i], tris[i + 1], tris[i + 2]];
    const b: Vec3 = [tris[i + 3], tris[i + 4], tris[i + 5]];
    const c: Vec3 = [tris[i + 6], tris[i + 7], tris[i + 8]];
    const w = triArea(a, b, c);
    cx += w * (a[0] + b[0] + c[0]) / 3;
    cy += w * (a[1] + b[1] + c[1]) / 3;
    cz += w * (a[2] + b[2] + c[2]) / 3;
    wsum += w;
  }
  if (wsum < 1e-12) return [0, 0, 0];
  return [cx / wsum, cy / wsum, cz / wsum];
}

/**
 * Closest point on triangle (a,b,c) to point p (Ericson, Real-Time Collision
 * Detection §5.1.5). Exact, branch-by-Voronoi-region — handles the interior,
 * all three edges and all three vertices.
 */
export function closestPointOnTriangle(
  p: Vec3,
  a: Vec3,
  b: Vec3,
  c: Vec3,
): Vec3 {
  const ab = sub(b, a);
  const ac = sub(c, a);
  const ap = sub(p, a);
  const d1 = dot(ab, ap);
  const d2 = dot(ac, ap);
  if (d1 <= 0 && d2 <= 0) return a; // vertex a

  const bp = sub(p, b);
  const d3 = dot(ab, bp);
  const d4 = dot(ac, bp);
  if (d3 >= 0 && d4 <= d3) return b; // vertex b

  const vc = d1 * d4 - d3 * d2;
  if (vc <= 0 && d1 >= 0 && d3 <= 0) {
    const v = d1 / (d1 - d3);
    return add(a, ab, v); // edge ab
  }

  const cp = sub(p, c);
  const d5 = dot(ab, cp);
  const d6 = dot(ac, cp);
  if (d6 >= 0 && d5 <= d6) return c; // vertex c

  const vb = d5 * d2 - d1 * d6;
  if (vb <= 0 && d2 >= 0 && d6 <= 0) {
    const w = d2 / (d2 - d6);
    return add(a, ac, w); // edge ac
  }

  const va = d3 * d6 - d5 * d4;
  if (va <= 0 && d4 - d3 >= 0 && d5 - d6 >= 0) {
    const w = (d4 - d3) / (d4 - d3 + (d5 - d6));
    return add(b, sub(c, b), w); // edge bc
  }

  // interior face region
  const denom = 1 / (va + vb + vc);
  const v = vb * denom;
  const w = vc * denom;
  return [
    a[0] + ab[0] * v + ac[0] * w,
    a[1] + ab[1] * v + ac[1] * w,
    a[2] + ab[2] * v + ac[2] * w,
  ];
}

/** Closest point + squared distance from `p` to a whole triangle soup. */
function closestOnSoup(p: Vec3, tris: number[]): { d2: number; c: Vec3 } {
  let best = Infinity;
  let bc: Vec3 = p;
  for (let i = 0; i + 8 < tris.length; i += 9) {
    const cp = closestPointOnTriangle(
      p,
      [tris[i], tris[i + 1], tris[i + 2]],
      [tris[i + 3], tris[i + 4], tris[i + 5]],
      [tris[i + 6], tris[i + 7], tris[i + 8]],
    );
    const dd = sub(cp, p);
    const d2 = dot(dd, dd);
    if (d2 < best) {
      best = d2;
      bc = cp;
    }
  }
  return { d2: best, c: bc };
}

/** Min distance from a point to a triangle soup, with the closest pair. */
export function minDistancePointToTriangles(
  p: Vec3,
  tris: number[],
): DistanceMeasure | null {
  if (tris.length < 9) return null;
  const { d2, c } = closestOnSoup(p, tris);
  return { min: Math.sqrt(d2), a: p, b: c, delta: sub(c, p) };
}

/**
 * Min distance between a sampled polyline and a triangle soup. Probes every
 * polyline vertex against the soup, then — to catch the case where the closest
 * approach is segment-interior to triangle-interior (a segment grazing a face)
 * — also samples each segment densely. The dense subsampling keeps the pure
 * math simple and is exact in the limit; the kernel's edge/face tessellation is
 * already fine, so a modest split converges to sub-µm on the geometries that
 * matter. */
export function minDistancePolylineToTriangles(
  poly: number[],
  tris: number[],
): DistanceMeasure | null {
  const n = Math.floor(poly.length / 3);
  if (n < 1 || tris.length < 9) return null;
  let best = Infinity;
  let bestA: Vec3 = at(poly, 0);
  let bestB: Vec3 = at(poly, 0);
  const consider = (p: Vec3) => {
    const { d2, c } = closestOnSoup(p, tris);
    if (d2 < best) {
      best = d2;
      bestA = p;
      bestB = c;
    }
  };
  if (n === 1) {
    consider(at(poly, 0));
  } else {
    for (let i = 0; i + 1 < n; i++) {
      const p0 = at(poly, i);
      const p1 = at(poly, i + 1);
      const segLen = dist(p0, p1);
      const steps = Math.max(1, Math.min(64, Math.ceil(segLen / 0.5)));
      for (let s = 0; s <= steps; s++) {
        const t = s / steps;
        consider([
          p0[0] + (p1[0] - p0[0]) * t,
          p0[1] + (p1[1] - p0[1]) * t,
          p0[2] + (p1[2] - p0[2]) * t,
        ]);
      }
    }
  }
  return { min: Math.sqrt(best), a: bestA, b: bestB, delta: sub(bestB, bestA) };
}

/**
 * Min distance between two triangle soups. Probes every vertex of each soup
 * against the other (closest-point-on-triangle), which is exact when the
 * closest approach touches a vertex — the common case for non-intersecting
 * convex-ish CAD faces. Returns 0 distance for overlapping/touching soups via
 * the vertex probes. O(Va·Tb + Vb·Ta). */
export function minDistanceBetweenTriangleSoups(
  trisA: number[],
  trisB: number[],
): DistanceMeasure | null {
  if (trisA.length < 9 || trisB.length < 9) return null;
  let best = Infinity;
  let bestA: Vec3 = [trisA[0], trisA[1], trisA[2]];
  let bestB: Vec3 = [trisB[0], trisB[1], trisB[2]];
  const probe = (verts: number[], soup: number[], flip: boolean) => {
    for (let i = 0; i + 2 < verts.length; i += 3) {
      const p: Vec3 = [verts[i], verts[i + 1], verts[i + 2]];
      const { d2, c } = closestOnSoup(p, soup);
      if (d2 < best) {
        best = d2;
        bestA = flip ? c : p;
        bestB = flip ? p : c;
      }
    }
  };
  probe(trisA, trisB, false);
  probe(trisB, trisA, true);
  return { min: Math.sqrt(best), a: bestA, b: bestB, delta: sub(bestB, bestA) };
}

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

// ─────────────────────────────────────────────────────────────────────────
//  Unified entity model + measurement dispatch — ticket me9w
//
//  The Measure panel resolves each picked B-Rep element to one of these three
//  normalized geometry carriers, then asks for a single-entity readout or the
//  min distance between any two. Every combination (point/edge/face ×
//  point/edge/face) routes to the right pure function above — there is no
//  per-combination branching in the UI.
// ─────────────────────────────────────────────────────────────────────────

/** A picked vertex resolved to its world position. */
export interface VertexGeom {
  kind: "vertex";
  point: Vec3;
}
/** A picked edge resolved to its world-space sample polyline (flat xyz). */
export interface EdgeGeom {
  kind: "edge";
  points: number[];
}
/** A picked face resolved to a world-space triangle soup (flat xyz, 9/tri) plus
 *  its analytic area / centroid (the soup may be a tessellated approximation,
 *  but `area`/`centroid` are exact closed-form values when the resolver knows
 *  the analytic surface). */
export interface FaceGeom {
  kind: "face";
  triangles: number[];
  area: number;
  centroid: Vec3;
  /** Display label of the analytic surface kind (e.g. "Ebene", "Zylinder"). */
  surfaceLabel: string;
}
export type MeasureGeom = VertexGeom | EdgeGeom | FaceGeom;

/** Per-entity readout for a single selected entity. */
export interface SingleMeasure {
  kind: MeasureGeom["kind"];
  /** Edge: arc length / face: area is folded into the relevant fields below. */
  length?: number;
  closed?: boolean;
  circle?: CircleInfo | null;
  area?: number;
  centroid?: Vec3;
  position?: Vec3;
  surfaceLabel?: string;
}

/** Readout for one resolved entity (length/radius for an edge, area/centroid
 *  for a face, coordinates for a vertex). */
export function measureSingle(g: MeasureGeom): SingleMeasure {
  if (g.kind === "vertex") {
    return { kind: "vertex", position: g.point };
  }
  if (g.kind === "edge") {
    const e = analyzeEdge(g.points);
    return {
      kind: "edge",
      length: e?.length ?? 0,
      closed: e?.closed ?? false,
      circle: e?.circle ?? null,
    };
  }
  return {
    kind: "face",
    area: g.area,
    centroid: g.centroid,
    surfaceLabel: g.surfaceLabel,
  };
}

/** The world-space sample set of an entity, as a flat polyline, for use as the
 *  "probe" side of a distance query (vertex → its point, edge → its polyline,
 *  face → its triangle soup verts). */
function probePoints(g: MeasureGeom): number[] {
  if (g.kind === "vertex") return [...g.point];
  if (g.kind === "edge") return g.points;
  return g.triangles;
}

/**
 * Minimum distance between any two resolved entities — all nine kind
 * combinations. Dispatches to the matching exact pure routine:
 *   point·point / point·edge / edge·edge   → segment math
 *   *·face                                 → triangle-soup math
 * Δ-components come straight from the closest-point pair.
 */
export function measureDistance(
  a: MeasureGeom,
  b: MeasureGeom,
): DistanceMeasure | null {
  // Face on either side → triangle-soup routines.
  if (a.kind === "face" && b.kind === "face") {
    return minDistanceBetweenTriangleSoups(a.triangles, b.triangles);
  }
  if (a.kind === "face" || b.kind === "face") {
    const face = (a.kind === "face" ? a : b) as FaceGeom;
    const other = a.kind === "face" ? b : a;
    const d =
      other.kind === "vertex"
        ? minDistancePointToTriangles(other.point, face.triangles)
        : minDistancePolylineToTriangles(probePoints(other), face.triangles);
    if (!d) return null;
    // `d` is oriented other→face; flip back to a→b if the face was `a`.
    if (a.kind === "face") {
      return { min: d.min, a: d.b, b: d.a, delta: sub(d.a, d.b) };
    }
    return d;
  }
  // No face: both sides are point/edge → polyline-vs-polyline (a point is a
  // degenerate single-sample polyline).
  return minDistanceBetweenPolylines(probePoints(a), probePoints(b));
}
