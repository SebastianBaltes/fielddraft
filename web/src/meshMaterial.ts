// @ts-nocheck — Three.js TSL in r183 is intentionally loose-typed; the
// project convention (see raymarch.ts) is to opt out of strict TS for
// any file that scripts TSL. Everything else in the codebase keeps
// strict checking.

/**
 * Kernel mesh material.
 *
 * Builds a `MeshBasicNodeMaterial` whose `colorNode` does a cheap
 * Lambert-ish shade using the mesh's own normals. The palette and
 * light direction mirror the (now retired) raymarcher look so the
 * switch from raymarching to Dual Contouring mesh rendering is
 * visually continuous for existing smoke tests.
 */

import * as THREE from "three";
import {
  Fn,
  float,
  max,
  normalWorld,
  normalize,
  vec3,
  vec4,
} from "three/tsl";
import { MeshBasicNodeMaterial } from "three/webgpu";

/** Per-body colour palette. The hue cycles so each body
 *  in a multi-body document is visually distinct. The first
 *  body gets the classic FieldDraft blue; subsequent bodies
 *  rotate through a set of muted CAD-style tones. */
const BODY_COLORS: [number, number, number][] = [
  [0.55, 0.65, 0.85], // blue (default)
  [0.65, 0.85, 0.55], // green
  [0.85, 0.55, 0.65], // rose
  [0.85, 0.75, 0.50], // amber
  [0.55, 0.80, 0.80], // teal
  [0.75, 0.55, 0.85], // violet
  [0.85, 0.85, 0.55], // lime
  [0.55, 0.70, 0.70], // slate
];

export function createKernelMeshMaterial(
  bodyIndex = 0,
): MeshBasicNodeMaterial {
  const material = new MeshBasicNodeMaterial();
  material.side = THREE.DoubleSide;
  material.toneMapped = false;

  const [r, g, b] = BODY_COLORS[bodyIndex % BODY_COLORS.length];

  const colorNode = Fn(() => {
    const n = normalize(normalWorld);
    const lightDir = normalize(vec3(0.4, 1, 0.5));
    const diffuse = max(n.dot(lightDir), float(0));
    const ambient = float(0.35);
    const base = vec3(r, g, b);
    return vec4(base.mul(diffuse.add(ambient)), 1);
  });

  material.colorNode = colorNode();
  return material;
}
