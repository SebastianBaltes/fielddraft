/**
 * Map ONE Onshape sketch region onto the FieldDraft imprint faces it covers
 * (#440).
 *
 * Onshape hands us a region as its BOUNDARY: a ring of imprint edges, each a
 * named sketch curve trimmed between two imprint vertices (`onshapeRegion.ts`).
 * FieldDraft arranges the same curves into its own atomic faces — and the two
 * arrangements do NOT have to cut in the same places. Where ours cuts more
 * finely, ONE Onshape region is SEVERAL of our faces, so the mapping is 1:n and
 * not, as #412 built it, 1:1.
 *
 * What that cost, measured on the #419 document: Onshape's `Revolve 1` selects
 * the rectangle x ∈ [9.25; 16.99] × y ∈ [−60.93; −46.68]; our arrangement splits
 * it at y = −52.83 (a neighbouring rectangle's top edge runs through). #412
 * scored both halves, kept the better one and dropped the other WITHOUT a word —
 * the shortfall warning only counted queries against faces, and here one query
 * did produce one face. The revolve came out with a 6.15 mm gap in its profile,
 * and the cylinder wall that Onshape's `Thicken 6` cuts into never existed.
 *
 * Two readings of the same probe decide the mapping, and they are independent:
 *
 *  - RANKING picks the anchor. Walking a trimmed boundary edge and probing both
 *    sides says which faces it separates; the face whose own perimeter those
 *    edges account for the most, weighted by how much of the boundary it
 *    accounts for in turn, is the region's seed. (Both fractions matter: the
 *    ambient face around everything borders every named curve too, but they are
 *    a rounding error of ITS perimeter.)
 *
 *  - MERGING grows the seed. A single Onshape imprint edge has exactly ONE face
 *    on each side. So when the face on one side CHANGES as we walk along it, the
 *    two faces we saw are both inside that one Onshape face — our arrangement
 *    simply cut where Onshape's did not. That is positive evidence, not the
 *    absence of a divider, which is why it cannot leak into the whole sketch.
 *
 * A named edge that separates two faces is proof they are NOT the same region.
 * If the merge evidence ever contradicts that, the resolution keeps the seed
 * alone and says so — guessing wide is how a profile silently swallows its
 * neighbour.
 */

import type { RegionBoundaryEdge } from "./onshapeRegion";

/** A sketch curve flattened to 2D sketch-space millimetres. */
export type Polyline = [number, number][];

/** What `listSketchRegions` reports about one atomic imprint face. */
export interface FaceMetrics {
  /** Total boundary length (outer loop + hole loops) in mm. */
  perimeter: number;
  /** Face area in mm². */
  area: number;
}

/** How one Onshape region landed on our arrangement. */
export interface RegionResolution {
  /** The best-ranked face — the region's anchor, or null when nothing ranked. */
  seed: number | null;
  /** Every face the region covers, seed included. Sorted, deduplicated. */
  faces: number[];
  /** The boundary named a sketch curve this sketch does not carry. */
  unknownCurve: boolean;
  /** Merge evidence and separation evidence disagreed — `faces` is the seed
   *  alone, deliberately narrow. */
  conflict: boolean;
}

/** How many stations to probe along one boundary edge. The stations are spread
 *  by ARC LENGTH, so a long edge is sampled as thoroughly as a short one and
 *  the measured coverage is a length, not a vertex count. */
const EDGE_STATIONS = 32;
/** Absolute probe-offset clamp (mm) — a long edge still probes close enough to
 *  stay inside the thin plate it bounds (#412 plates are 22 mm thick). */
const EDGE_PROBE_MAX_MM = 4;
/** How close two points must lie to be the same imprint vertex (mm). Sketch
 *  coordinates arrive as metres ×1000, so this is well above float noise and
 *  far below any real feature. */
const VERTEX_TOL_MM = 1e-3;

const EMPTY: RegionResolution = { seed: null, faces: [], unknownCurve: false, conflict: false };

function segmentLengths(pts: Polyline): { lens: number[]; total: number } {
  const lens: number[] = [];
  let total = 0;
  for (let i = 0; i + 1 < pts.length; i++) {
    const l = Math.hypot(pts[i + 1][0] - pts[i][0], pts[i + 1][1] - pts[i][1]);
    lens.push(l);
    total += l;
  }
  return { lens, total };
}

/** Every point where two polylines meet — TOUCHES INCLUDED. An imprint vertex
 *  is usually a T-junction (one curve ending on another), so an intersection
 *  test that only accepts proper crossings finds nothing at all. */
function intersectPolylines(a: Polyline, b: Polyline): [number, number][] {
  const hits: [number, number][] = [];
  for (let i = 0; i + 1 < a.length; i++) {
    const px = a[i][0];
    const py = a[i][1];
    const rx = a[i + 1][0] - px;
    const ry = a[i + 1][1] - py;
    const rl = Math.hypot(rx, ry);
    if (rl < 1e-12) continue;
    for (let j = 0; j + 1 < b.length; j++) {
      const qx = b[j][0];
      const qy = b[j][1];
      const sx = b[j + 1][0] - qx;
      const sy = b[j + 1][1] - qy;
      const sl = Math.hypot(sx, sy);
      if (sl < 1e-12) continue;
      const denom = rx * sy - ry * sx;
      // Parallel (or collinear): no ISOLATED meeting point to call a vertex.
      if (Math.abs(denom) < 1e-9 * rl * sl) continue;
      const t = ((qx - px) * sy - (qy - py) * sx) / denom;
      const u = ((qx - px) * ry - (qy - py) * rx) / denom;
      const et = VERTEX_TOL_MM / rl;
      const eu = VERTEX_TOL_MM / sl;
      if (t < -et || t > 1 + et || u < -eu || u > 1 + eu) continue;
      hits.push([px + rx * t, py + ry * t]);
    }
  }
  return hits;
}

/** Arc length along `pts` of the point closest to `p`. */
function arcLengthAt(pts: Polyline, p: [number, number]): number {
  const { lens } = segmentLengths(pts);
  let best = 0;
  let bestD = Infinity;
  let travelled = 0;
  for (let i = 0; i < lens.length; i++) {
    const dx = pts[i + 1][0] - pts[i][0];
    const dy = pts[i + 1][1] - pts[i][1];
    const t = Math.max(0, Math.min(1, ((p[0] - pts[i][0]) * dx + (p[1] - pts[i][1]) * dy) / (lens[i] * lens[i])));
    const d = Math.hypot(pts[i][0] + dx * t - p[0], pts[i][1] + dy * t - p[1]);
    if (d < bestD) {
      bestD = d;
      best = travelled + lens[i] * t;
    }
    travelled += lens[i];
  }
  return best;
}

/** The piece of `pts` between two arc lengths. */
function slicePolyline(pts: Polyline, from: number, to: number): Polyline {
  const { lens } = segmentLengths(pts);
  const at = (s: number): [number, number] => {
    let rest = s;
    for (let i = 0; i < lens.length; i++) {
      if (rest <= lens[i] || i === lens.length - 1) {
        const t = lens[i] > 0 ? Math.max(0, Math.min(1, rest / lens[i])) : 0;
        return [
          pts[i][0] + (pts[i + 1][0] - pts[i][0]) * t,
          pts[i][1] + (pts[i + 1][1] - pts[i][1]) * t,
        ];
      }
      rest -= lens[i];
    }
    return pts[pts.length - 1];
  };
  const out: Polyline = [at(from)];
  let travelled = 0;
  for (let i = 0; i < lens.length; i++) {
    travelled += lens[i];
    if (travelled > from + VERTEX_TOL_MM && travelled < to - VERTEX_TOL_MM) out.push(pts[i + 1]);
  }
  out.push(at(to));
  return out;
}

/** Where one end of an imprint edge sits: the point at which the named curves
 *  cross `base`. Several curves may be named — they all pass through the one
 *  vertex, so the point they AGREE on is the answer. */
function endPoint(
  base: Polyline,
  baseId: string,
  curveIds: string[],
  geom: Map<string, Polyline>,
): [number, number] | null {
  const hits: [number, number][] = [];
  for (const id of curveIds) {
    if (id === baseId) continue;
    const other = geom.get(id);
    if (!other || other.length < 2) continue;
    hits.push(...intersectPolylines(base, other));
  }
  if (hits.length === 0) return null;
  let best: [number, number] | null = null;
  let bestVotes = 0;
  for (const h of hits) {
    const votes = hits.filter((o) => Math.hypot(o[0] - h[0], o[1] - h[1]) < VERTEX_TOL_MM).length;
    if (votes > bestVotes) {
      bestVotes = votes;
      best = h;
    }
  }
  return best;
}

/** One boundary edge, ready to walk. */
interface WalkableEdge {
  poly: Polyline;
  /** Is `poly` exactly the imprint edge? Only an exact edge may merge faces:
   *  the merge argument is "ONE Onshape face lies along THIS edge", and an
   *  untrimmed stand-in runs past the region into the next one. */
  exact: boolean;
}

function walkableEdge(
  edge: RegionBoundaryEdge,
  geom: Map<string, Polyline>,
): WalkableEdge | null {
  const base = geom.get(edge.entityId);
  if (!base || base.length < 2) return null;
  // No trim named: the whole curve IS the imprint edge, exactly.
  if (edge.ends.length !== 2) return { poly: base, exact: edge.ends.length === 0 };
  // A closed curve trimmed at two vertices leaves TWO arcs and nothing says
  // which one — walk the whole rim and refuse to merge on it.
  const closed = Math.hypot(base[0][0] - base[base.length - 1][0], base[0][1] - base[base.length - 1][1]) < VERTEX_TOL_MM;
  if (closed) return { poly: base, exact: false };
  const a = endPoint(base, edge.entityId, edge.ends[0], geom);
  const b = endPoint(base, edge.entityId, edge.ends[1], geom);
  if (!a || !b) return { poly: base, exact: false };
  const sa = arcLengthAt(base, a);
  const sb = arcLengthAt(base, b);
  const from = Math.min(sa, sb);
  const to = Math.max(sa, sb);
  if (to - from < VERTEX_TOL_MM) return { poly: base, exact: false };
  return { poly: slicePolyline(base, from, to), exact: true };
}

const pairKey = (a: number, b: number) => (a < b ? `${a}|${b}` : `${b}|${a}`);

/**
 * Resolve ONE decoded region query to the atomic faces it covers.
 *
 * Strictness the #412 ticket paid for and #440 keeps: a boundary edge naming a
 * sketch curve this document does NOT carry makes the whole query unresolvable.
 * Substituting a same-prefix sibling (`R.left` for a missing `R.bottom`) is a
 * guess, and in the #412 document it guessed a 604×1500 block Onshape never
 * extruded — silent wrong geometry. An honest empty result lets the caller warn
 * and drop the region instead.
 */
export function resolveRegionFaces(
  edges: RegionBoundaryEdge[],
  geom: Map<string, Polyline>,
  faceAt: (u: number, v: number) => number | null,
  metrics: (index: number) => FaceMetrics | null,
): RegionResolution {
  if (edges.length === 0) return EMPTY;

  const walkable: WalkableEdge[] = [];
  for (const edge of edges) {
    if (!geom.has(edge.entityId)) return { ...EMPTY, unknownCurve: true };
    const w = walkableEdge(edge, geom);
    if (w) walkable.push(w);
  }
  if (walkable.length === 0) return EMPTY;

  /** face index → length of boundary edge running along its own boundary. */
  const covered = new Map<number, number>();
  /** face index → which boundary edges touch it. */
  const touched = new Map<number, Set<number>>();
  /** "these two faces lie on the same side of one imprint edge". */
  const mergeHints: [number, number][] = [];
  /** "an imprint edge runs BETWEEN these two faces" — they are not one region. */
  const separated = new Set<string>();

  walkable.forEach((edge, index) => {
    const { lens, total } = segmentLengths(edge.poly);
    if (total < 1e-6) return;
    const weight = total / EDGE_STATIONS;
    // Probe half a station spacing off the curve: far enough to be unambiguously
    // on one side, close enough not to jump over a thin neighbouring face.
    const off = Math.min(weight / 2, EDGE_PROBE_MAX_MM);
    /** The face seen on the left / right of the previous station, or null at a
     *  break — merging needs the two faces to be DIRECTLY consecutive. */
    let prevLeft: number | null = null;
    let prevRight: number | null = null;
    for (let k = 0; k < EDGE_STATIONS; k++) {
      // Station at arc length s, offset by half a spacing so neither endpoint
      // (where the neighbouring faces change) is probed.
      let s = (total * (k + 0.5)) / EDGE_STATIONS;
      let i = 0;
      while (i < lens.length - 1 && s > lens[i]) {
        s -= lens[i];
        i += 1;
      }
      if (lens[i] < 1e-9) continue;
      const t = s / lens[i];
      const dx = edge.poly[i + 1][0] - edge.poly[i][0];
      const dy = edge.poly[i + 1][1] - edge.poly[i][1];
      const nx = -dy / lens[i];
      const ny = dx / lens[i];
      const px = edge.poly[i][0] + dx * t;
      const py = edge.poly[i][1] + dy * t;
      const a = faceAt(px + off * nx, py + off * ny);
      const b = faceAt(px - off * nx, py - off * ny);
      // Same face on both sides: here the curve runs THROUGH that face rather
      // than bounding it (a dangling trim line) — it says nothing about it.
      if (a === b) {
        prevLeft = null;
        prevRight = null;
        continue;
      }
      for (const f of [a, b]) {
        if (f === null) continue;
        covered.set(f, (covered.get(f) ?? 0) + weight);
        let who = touched.get(f);
        if (!who) touched.set(f, (who = new Set()));
        who.add(index);
      }
      if (a !== null && b !== null) separated.add(pairKey(a, b));
      if (edge.exact) {
        if (prevLeft !== null && a !== null && prevLeft !== a) mergeHints.push([prevLeft, a]);
        if (prevRight !== null && b !== null && prevRight !== b) mergeHints.push([prevRight, b]);
      }
      prevLeft = a;
      prevRight = b;
    }
  });

  let seed: number | null = null;
  let bestScore = 0;
  let bestArea = Infinity;
  for (const [face, len] of covered) {
    const m = metrics(face);
    if (!m || !(m.perimeter > 0)) continue;
    const share = (touched.get(face)?.size ?? 0) / walkable.length;
    const score = (len / m.perimeter) * share;
    if (score > bestScore + 1e-9 || (score > bestScore - 1e-9 && m.area < bestArea)) {
      seed = face;
      bestScore = score;
      bestArea = m.area;
    }
  }
  if (seed === null) return EMPTY;

  // Grow the seed along the merge evidence.
  const group = new Set<number>([seed]);
  for (let grew = true; grew; ) {
    grew = false;
    for (const [x, y] of mergeHints) {
      if (group.has(x) === group.has(y)) continue;
      group.add(x);
      group.add(y);
      grew = true;
    }
  }
  // …and check it against the evidence that divides: an imprint edge lying
  // between two members means they were never one region.
  for (const x of group) {
    for (const y of group) {
      if (x < y && separated.has(pairKey(x, y))) {
        return { seed, faces: [seed], unknownCurve: false, conflict: true };
      }
    }
  }
  return {
    seed,
    faces: [...group].sort((p, q) => p - q),
    unknownCurve: false,
    conflict: false,
  };
}
