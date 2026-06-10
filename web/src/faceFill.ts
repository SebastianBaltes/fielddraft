/**
 * Mesh-free planar face fill (ticket por8): triangulate a planar B-Rep face's
 * boundary so the selected face renders as a filled surface in the SDF render
 * modes (no main-thread triangle mesh), matching Onshape's full-face highlight.
 *
 * The boundary edges come from the weave and are NOT trusted as an ordered
 * loop — around coplanar seams the weave emits duplicate edge traces, corner
 * squiggles and unordered fallback loops (tickets oa9h/vyee). This module is a
 * TS port of the kernel's robust profile assembly
 * (`crates/kernel/src/brep/face_profile.rs`): coverage-deduplicate the
 * projected chains, assemble rings by nearest-endpoint proximity, drop rings
 * that cannot close, micro-rings and "holes" touching the outer boundary, then
 * triangulate outer − holes via `THREE.ShapeUtils.triangulateShape`.
 */

import * as THREE from "three";

export type Pt2 = [number, number];
type Chain = Pt2[];
/** One assembled ring: ordered, consistently-oriented chains. */
type Ring = Chain[];

const d2 = (a: Pt2, b: Pt2): number => {
  const dx = a[0] - b[0];
  const dy = a[1] - b[1];
  return dx * dx + dy * dy;
};

function chainLen(c: Chain): number {
  let len = 0;
  for (let i = 1; i < c.length; i++) len += Math.sqrt(d2(c[i - 1], c[i]));
  return len;
}

function pointSegDist2(p: Pt2, a: Pt2, b: Pt2): number {
  const abx = b[0] - a[0];
  const aby = b[1] - a[1];
  const len2 = abx * abx + aby * aby;
  const t = len2 > 0
    ? Math.min(1, Math.max(0, ((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / len2))
    : 0;
  return d2(p, [a[0] + t * abx, a[1] + t * aby]);
}

function pointNearPolyline(p: Pt2, chain: Chain, tol: number): boolean {
  const t2 = tol * tol;
  if (chain.length === 1) return d2(p, chain[0]) <= t2;
  for (let i = 1; i < chain.length; i++) {
    if (pointSegDist2(p, chain[i - 1], chain[i]) <= t2) return true;
  }
  return false;
}

function ringPoints(ring: Ring): Pt2[] {
  const pts: Pt2[] = [];
  for (const c of ring) pts.push(...c);
  return pts;
}

/** Shoelace area over a ring's concatenated points (signed). */
function ringArea(ring: Ring): number {
  const pts = ringPoints(ring);
  let area = 0;
  for (let i = 0; i < pts.length; i++) {
    const j = (i + 1) % pts.length;
    area += pts[i][0] * pts[j][1] - pts[j][0] * pts[i][1];
  }
  return area * 0.5;
}

function ringClosingGap(ring: Ring): number {
  const first = ring[0][0];
  const lastChain = ring[ring.length - 1];
  return Math.sqrt(d2(lastChain[lastChain.length - 1], first));
}

function ringPerimeter(ring: Ring): number {
  let len = 0;
  for (const c of ring) len += chainLen(c);
  return len;
}

function ringDiag(ring: Ring): number {
  let lo0 = Infinity, lo1 = Infinity, hi0 = -Infinity, hi1 = -Infinity;
  for (const c of ring) {
    for (const p of c) {
      lo0 = Math.min(lo0, p[0]);
      lo1 = Math.min(lo1, p[1]);
      hi0 = Math.max(hi0, p[0]);
      hi1 = Math.max(hi1, p[1]);
    }
  }
  return Math.hypot(hi0 - lo0, hi1 - lo1);
}

/**
 * Coverage-deduplicate chains (longest first): a chain whose vertices AND
 * segment midpoints all lie on the already-kept chains is a duplicate seam
 * trace (however it was split) and is dropped.
 */
function dedupChains(chains: Chain[]): Chain[] {
  let lo0 = Infinity, lo1 = Infinity, hi0 = -Infinity, hi1 = -Infinity;
  for (const c of chains) {
    for (const p of c) {
      lo0 = Math.min(lo0, p[0]);
      lo1 = Math.min(lo1, p[1]);
      hi0 = Math.max(hi0, p[0]);
      hi1 = Math.max(hi1, p[1]);
    }
  }
  const tol = Math.max(1e-3 * Math.hypot(hi0 - lo0, hi1 - lo1), 1e-5);
  const sorted = [...chains].sort((a, b) => chainLen(b) - chainLen(a));
  const unique: Chain[] = [];
  for (const c of sorted) {
    const near = (p: Pt2) => unique.some((u) => pointNearPolyline(p, u, tol));
    let covered = c.every(near);
    if (covered) {
      for (let i = 1; i < c.length && covered; i++) {
        covered = near([(c[i - 1][0] + c[i][0]) / 2, (c[i - 1][1] + c[i][1]) / 2]);
      }
    }
    if (!covered) unique.push(c);
  }
  return unique;
}

/** Greedy nearest-endpoint ring assembly (mirrors the kernel's). */
function assembleRingsFromChains(chains: Chain[]): Ring[] {
  const n = chains.length;
  const used = new Array<boolean>(n).fill(false);
  const rings: Ring[] = [];
  for (let seed = 0; seed < n; seed++) {
    if (used[seed]) continue;
    used[seed] = true;
    const ring: Ring = [chains[seed]];
    for (;;) {
      const lastChain = ring[ring.length - 1];
      const end = lastChain[lastChain.length - 1];
      const start = ring[0][0];
      const closeD = Math.sqrt(d2(end, start));
      let best: { i: number; rev: boolean; d: number } | null = null;
      for (let i = 0; i < n; i++) {
        if (used[i]) continue;
        const c = chains[i];
        const df = Math.sqrt(d2(end, c[0]));
        const dl = Math.sqrt(d2(end, c[c.length - 1]));
        const rev = dl < df;
        const dist = rev ? dl : df;
        if (!best || dist < best.d) best = { i, rev, d: dist };
      }
      if (best && best.d < closeD) {
        used[best.i] = true;
        const c = chains[best.i].slice();
        if (best.rev) c.reverse();
        ring.push(c);
      } else {
        break;
      }
    }
    rings.push(ring);
  }
  return rings;
}

/**
 * Assemble the projected boundary chains into clean rings: dedup → greedy
 * assembly → drop non-closing rings (orphaned duplicate leftovers), micro
 * rings and boundary-touching "holes" (corner squiggles). Returns the rings
 * as concatenated 2D point lists, largest |area| first (= the outer profile).
 */
export function assembleFaceRings(chains: Chain[]): Pt2[][] {
  const meaningful = chains.filter(
    (c) => c.length >= 2 && c.some((p) => d2(p, c[0]) > 1e-12),
  );
  if (meaningful.length === 0) return [];
  let rings = assembleRingsFromChains(dedupChains(meaningful));
  rings = rings.filter(
    (r) => ringClosingGap(r) <= Math.max(0.05 * ringPerimeter(r), 1e-6),
  );
  if (rings.length === 0) return [];
  rings.sort((a, b) => Math.abs(ringArea(b)) - Math.abs(ringArea(a)));
  const outerDiag = ringDiag(rings[0]);
  rings = rings.filter((r, i) => i === 0 || ringDiag(r) >= 1e-4 * outerDiag);
  const touchTol = Math.max(1e-3 * outerDiag, 1e-5);
  const outer = rings[0];
  rings = rings.filter(
    (r, i) =>
      i === 0 ||
      !ringPoints(r).some((p) => outer.some((c) => pointNearPolyline(p, c, touchTol))),
  );
  return rings.map(ringPoints);
}

/**
 * Triangulate a planar face from its boundary edge polylines (world space,
 * flat `[x,y,z,…]` as `BRepEdge.points`) and the face's world plane
 * (`origin` + unit `normal`). Returns a world-space triangle soup (flat xyz),
 * or `[]` when no clean profile can be assembled.
 */
export function triangulatePlanarFace(
  edgePolylines: number[][],
  origin: [number, number, number],
  normal: [number, number, number],
): number[] {
  // In-plane basis: world axis least aligned with the normal, crossed twice
  // (same seed rule as the kernel's `face_plane_frame`).
  const n = new THREE.Vector3(...normal).normalize();
  const ax = Math.abs(n.x), ay = Math.abs(n.y), az = Math.abs(n.z);
  const seed =
    ax <= ay && ax <= az
      ? new THREE.Vector3(1, 0, 0)
      : ay <= az
        ? new THREE.Vector3(0, 1, 0)
        : new THREE.Vector3(0, 0, 1);
  const u = new THREE.Vector3().crossVectors(seed, n).normalize();
  const v = new THREE.Vector3().crossVectors(n, u).normalize();
  const o = new THREE.Vector3(...origin);

  const chains: Chain[] = [];
  const w = new THREE.Vector3();
  for (const pl of edgePolylines) {
    const chain: Chain = [];
    for (let i = 0; i + 2 < pl.length; i += 3) {
      w.set(pl[i], pl[i + 1], pl[i + 2]).sub(o);
      const p: Pt2 = [w.dot(u), w.dot(v)];
      const last = chain[chain.length - 1];
      if (!last || d2(last, p) > 1e-12) chain.push(p);
    }
    if (chain.length >= 2) chains.push(chain);
  }

  const rings = assembleFaceRings(chains);
  if (rings.length === 0) return [];

  // triangulateShape wants the contour CCW and holes CW.
  const signedArea = (pts: Pt2[]) => {
    let a = 0;
    for (let i = 0; i < pts.length; i++) {
      const j = (i + 1) % pts.length;
      a += pts[i][0] * pts[j][1] - pts[j][0] * pts[i][1];
    }
    return a * 0.5;
  };
  const contourPts = signedArea(rings[0]) < 0 ? [...rings[0]].reverse() : rings[0];
  const holesPts = rings
    .slice(1)
    .map((r) => (signedArea(r) > 0 ? [...r].reverse() : r));

  const contour = contourPts.map(([x, y]) => new THREE.Vector2(x, y));
  const holes = holesPts.map((r) => r.map(([x, y]) => new THREE.Vector2(x, y)));
  let tris: number[][];
  try {
    tris = THREE.ShapeUtils.triangulateShape(contour, holes);
  } catch {
    return [];
  }

  const all = [...contour];
  for (const h of holes) all.push(...h);
  const out: number[] = [];
  const lift = (p: THREE.Vector2) => [
    o.x + p.x * u.x + p.y * v.x,
    o.y + p.x * u.y + p.y * v.y,
    o.z + p.x * u.z + p.y * v.z,
  ];
  for (const [a, b, c] of tris) {
    out.push(...lift(all[a]), ...lift(all[b]), ...lift(all[c]));
  }
  return out;
}
