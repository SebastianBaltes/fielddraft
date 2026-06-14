/**
 * Parametric sweep mesh — a 2D profile swept along an arbitrary 3D spine.
 *
 * The cross-section is carried along the spine by a ROTATION-MINIMIZING
 * frame (RMF), computed with the double-reflection method (Wang, Jüttler,
 * Zheng & Liu, *ACM TOG* 2008). RMF avoids the spurious profile twist a
 * Frenet frame produces at inflection points (where its normal flips).
 *
 * This mesher assumes the spine already PASSED the sweep Gatekeeper
 * (`crates/kernel/sweep_gate.rs`): no hard kinks, curvature radius above the
 * profile radius, no global self-intersection. Outside that envelope the
 * mesh would self-overlap and the caller must fall back to the SDF mesher.
 *
 * A profile point `(x, y)` is placed at spine sample `i` as
 * `P[i] + x·N[i] + y·B[i]`, where `{T, N, B}` is the RMF at that sample.
 */

import * as THREE from "three";
import type { ContourData } from "./kernel";
import { assembleShell, contourLoopRanges } from "./sweptShell";

type V3 = [number, number, number];

const sub = (a: V3, b: V3): V3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const dot = (a: V3, b: V3): number => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a: V3, b: V3): V3 => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
];
const norm = (a: V3): number => Math.sqrt(dot(a, a));
function normalize(a: V3): V3 {
  const l = norm(a);
  return l > 1e-12 ? [a[0] / l, a[1] / l, a[2] / l] : [0, 0, 0];
}

/** Per-sample unit tangents (central differences; wraps when closed). */
function computeTangents(path: V3[], closed: boolean): V3[] {
  const n = path.length;
  const T: V3[] = new Array(n);
  for (let i = 0; i < n; i++) {
    let t: V3;
    if (closed) {
      t = sub(path[(i + 1) % n], path[(i - 1 + n) % n]);
    } else if (i === 0) {
      t = sub(path[1], path[0]);
    } else if (i === n - 1) {
      t = sub(path[n - 1], path[n - 2]);
    } else {
      t = sub(path[i + 1], path[i - 1]);
    }
    T[i] = normalize(t);
  }
  return T;
}

/** A normal seed orthogonal to `t` (pick the world axis least aligned). */
function seedNormal(t: V3): V3 {
  const ref: V3 = Math.abs(t[1]) < 0.9 ? [0, 1, 0] : [1, 0, 0];
  const d = dot(ref, t);
  return normalize([ref[0] - d * t[0], ref[1] - d * t[1], ref[2] - d * t[2]]);
}

/**
 * Rotation-minimizing frames along the spine via double reflection.
 * Returns parallel `N` and `B` arrays (the tangents are the input `T`).
 */
function rmFrames(path: V3[], T: V3[]): { N: V3[]; B: V3[] } {
  const n = path.length;
  const N: V3[] = new Array(n);
  const B: V3[] = new Array(n);
  N[0] = seedNormal(T[0]);
  B[0] = cross(T[0], N[0]);

  for (let i = 0; i < n - 1; i++) {
    const v1 = sub(path[i + 1], path[i]);
    const c1 = dot(v1, v1);
    if (c1 < 1e-12) {
      // Duplicate point — carry the frame unchanged.
      N[i + 1] = N[i];
      B[i + 1] = B[i];
      continue;
    }
    // Reflect the previous normal and tangent across the plane ⊥ v1.
    const f1n = (2 / c1) * dot(v1, N[i]);
    const rLn: V3 = [N[i][0] - f1n * v1[0], N[i][1] - f1n * v1[1], N[i][2] - f1n * v1[2]];
    const f1t = (2 / c1) * dot(v1, T[i]);
    const tL: V3 = [T[i][0] - f1t * v1[0], T[i][1] - f1t * v1[1], T[i][2] - f1t * v1[2]];
    // Second reflection aligns the reflected tangent with the next tangent.
    const v2 = sub(T[i + 1], tL);
    const c2 = dot(v2, v2);
    let nNext: V3;
    if (c2 < 1e-12) {
      nNext = rLn; // tangents already equal — no second reflection
    } else {
      const f2 = (2 / c2) * dot(v2, rLn);
      nNext = [rLn[0] - f2 * v2[0], rLn[1] - f2 * v2[1], rLn[2] - f2 * v2[2]];
    }
    N[i + 1] = normalize(nNext);
    B[i + 1] = cross(T[i + 1], N[i + 1]);
  }
  return { N, B };
}

/**
 * Sweep `contour` along the world spine `path`. Returns null for a
 * degenerate profile or a spine shorter than two points. `closed` wraps the
 * last ring to the first and omits end caps (the RMF may leave a small seam
 * twist on closed loops — acceptable for a render hull).
 */
export function buildSweepGeometry(
  path: V3[],
  contour: ContourData,
  opts: { closed?: boolean } = {},
): THREE.BufferGeometry | null {
  if (!contour || contour.outer.length < 3) return null;
  const n = path.length;
  if (n < 2) return null;

  const closed = opts.closed ?? false;
  const T = computeTangents(path, closed);
  const { N, B } = rmFrames(path, T);

  const flat2d = [contour.outer, ...contour.holes].flat();
  const { pointsPerRing } = contourLoopRanges(contour);
  const positions = new Float32Array(n * pointsPerRing * 3);
  for (let i = 0; i < n; i++) {
    const p = path[i];
    const ni = N[i];
    const bi = B[i];
    const base = i * pointsPerRing;
    for (let j = 0; j < pointsPerRing; j++) {
      const u = flat2d[j][0];
      const v = flat2d[j][1];
      const o = (base + j) * 3;
      positions[o] = p[0] + u * ni[0] + v * bi[0];
      positions[o + 1] = p[1] + u * ni[1] + v * bi[1];
      positions[o + 2] = p[2] + u * ni[2] + v * bi[2];
    }
  }

  return assembleShell(positions, n, contour, { closed });
}

// Exposed for unit tests of the framing alone.
export const __test = { computeTangents, rmFrames, seedNormal };
