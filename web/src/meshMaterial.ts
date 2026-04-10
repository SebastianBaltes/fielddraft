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

export function createKernelMeshMaterial(): MeshBasicNodeMaterial {
  const material = new MeshBasicNodeMaterial();
  material.side = THREE.DoubleSide;
  material.toneMapped = false;

  const colorNode = Fn(() => {
    const n = normalize(normalWorld);
    const lightDir = normalize(vec3(0.4, 1, 0.5));
    const diffuse = max(n.dot(lightDir), float(0));
    const ambient = float(0.35);
    const base = vec3(0.55, 0.65, 0.85);
    return vec4(base.mul(diffuse.add(ambient)), 1);
  });

  material.colorNode = colorNode();
  return material;
}
