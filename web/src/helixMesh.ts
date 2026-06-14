/**
 * Parametric helix sweep mesh.
 *
 * A helix leaf sweeps a closed 2D sketch profile — in the `(r, w)` frame
 * (x = radial distance from the axis, y = axial offset) — helically around
 * the local Y axis. This is the analytic counterpart to the kernel's
 * `helix_profile_point` SDF warp: instead of marching the field we build the
 * swept boundary directly, so the WebGL path (no raymarcher) and the
 * SDF-deferred hull get a tight solid mesh instead of an AABB box.
 *
 * Forward map (helix-LOCAL), the algebraic inverse of `helix_profile_point`,
 * for a profile point `(rp, w)` at sweep parameter `t ∈ [0, 2π·turns]`:
 *
 *     Y = w + pitch · t / 2π
 *     r = rp + taper · (Y − yAnchor)
 *     θ = hand · (t + startAngle)        hand = leftHanded ? −1 : +1
 *     X = r · cos θ ,  Z = r · sin θ
 *
 * The result is in helix-local space; `applyLeafTransform` then maps it to
 * world via the leaf's `world_transform` (same convention as extrude/revolve).
 */

import * as THREE from "three";
import type { ContourData } from "./kernel";
import { assembleShell, contourLoopRanges } from "./sweptShell";

/** Sweep parameters as emitted by the kernel helix leaf (`params`). */
export interface HelixParams {
  pitch: number;
  turns: number;
  leftHanded: boolean;
  taper: number;
  startAngle: number;
  yAnchor: number;
}

const TAU = Math.PI * 2;
const SEGMENTS_PER_TURN = 48;

/** Decode a helix leaf's flat `params` array into named fields. */
export function decodeHelixParams(params: number[]): HelixParams {
  return {
    pitch: params[0],
    turns: params[1],
    leftHanded: params[2] !== 0,
    taper: params[3],
    startAngle: params[4],
    yAnchor: params[5],
  };
}

/**
 * Build a solid swept-profile mesh for a helix leaf. Returns `null` if the
 * profile is degenerate (fewer than 3 outer points) or the sweep is empty.
 */
export function buildHelixGeometry(
  contour: ContourData,
  params: HelixParams,
): THREE.BufferGeometry | null {
  if (!contour || contour.outer.length < 3) return null;
  const { pitch, turns, leftHanded, taper, startAngle, yAnchor } = params;
  if (!(turns > 0) || !(pitch > 0)) return null;

  const hand = leftHanded ? -1 : 1;
  const tEnd = TAU * turns;
  const segs = Math.max(1, Math.ceil(turns * SEGMENTS_PER_TURN));
  const tStep = tEnd / segs;
  const nRings = segs + 1;

  // One full cross-section per ring, placed via the forward helix map; the
  // shared shell assembler stitches the walls and caps.
  const flat2d = [contour.outer, ...contour.holes].flat();
  const { pointsPerRing } = contourLoopRanges(contour);
  const positions = new Float32Array(nRings * pointsPerRing * 3);
  for (let k = 0; k < nRings; k++) {
    const t = Math.min(k * tStep, tEnd);
    const yAxial = pitch * t * (1 / TAU);
    const theta = hand * (t + startAngle);
    const cos = Math.cos(theta);
    const sin = Math.sin(theta);
    const base = k * pointsPerRing;
    for (let j = 0; j < pointsPerRing; j++) {
      const rp = flat2d[j][0];
      const w = flat2d[j][1];
      const yy = w + yAxial;
      const r = rp + taper * (yy - yAnchor);
      const o = (base + j) * 3;
      positions[o] = r * cos;
      positions[o + 1] = yy;
      positions[o + 2] = r * sin;
    }
  }

  return assembleShell(positions, nRings, contour);
}
