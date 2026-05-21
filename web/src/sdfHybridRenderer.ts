// @ts-nocheck — TSL loose typing, same convention as sdfRenderer.ts

import * as THREE from "three";
import {
  Fn,
  float,
  max,
  normalize,
  normalWorld,
  vec3,
  vec4,
} from "three/tsl";
import { MeshBasicNodeMaterial } from "three/webgpu";
import type { HybridInfo, HybridLeafInfo } from "./kernel";
import {
  createSdfBody,
  updateSdfBody,
  disposeSdfBody,
  type SdfBodyHandle,
} from "./sdfRenderer";

const BODY_COLORS: [number, number, number][] = [
  [0.55, 0.65, 0.85],
  [0.65, 0.85, 0.55],
  [0.85, 0.55, 0.65],
  [0.85, 0.75, 0.50],
  [0.55, 0.80, 0.80],
  [0.75, 0.55, 0.85],
  [0.85, 0.85, 0.55],
  [0.55, 0.70, 0.70],
];

function createLambertMaterial(bodyIndex: number): MeshBasicNodeMaterial {
  const material = new MeshBasicNodeMaterial();
  material.side = THREE.DoubleSide;
  material.toneMapped = false;
  const [r, g, b] = BODY_COLORS[bodyIndex % BODY_COLORS.length];

  const colorNode = Fn(() => {
    const n = normalize(normalWorld);
    const key = normalize(vec3(0.4, 1, 0.5));
    const fill = normalize(vec3(-0.6, 0.2, 0.8));
    const rim = normalize(vec3(0.0, -0.3, -1.0));
    const dKey = max(n.dot(key), float(0));
    const dFill = max(n.dot(fill), float(0));
    const dRim = max(n.dot(rim), float(0));
    const ambient = float(0.3);
    const light = dKey.mul(0.45).add(dFill.mul(0.2)).add(dRim.mul(0.1)).add(ambient);
    const base = vec3(r, g, b);
    return vec4(base.mul(light), 1);
  });

  material.colorNode = colorNode();
  return material;
}

function createPrimitiveGeometry(leaf: HybridLeafInfo): THREE.BufferGeometry | null {
  const p = leaf.params;
  switch (leaf.kind) {
    case "box":
      return new THREE.BoxGeometry(p[0] * 2, p[1] * 2, p[2] * 2);
    case "sphere":
      return new THREE.SphereGeometry(p[0], 32, 24);
    case "cylinder":
      return new THREE.CylinderGeometry(p[0], p[0], p[1] * 2, 32);
    case "torus":
      return new THREE.TorusGeometry(p[0], p[1], 24, 48);
    case "cone":
      return new THREE.CylinderGeometry(p[1], p[0], p[2] * 2, 32);
    case "capsule":
      return new THREE.CapsuleGeometry(p[0], p[1] * 2, 16, 24);
    default:
      return null;
  }
}

function applyLeafTransform(mesh: THREE.Mesh, leaf: HybridLeafInfo): void {
  const t = leaf.transform;
  const translate = new THREE.Vector3(t[0], t[1], t[2]);
  const quat = new THREE.Quaternion(t[3], t[4], t[5], t[6]);
  const scale = t[7];

  // Compose primitive-local center offset into world position
  const centerOffset = getCenterOffset(leaf);
  if (centerOffset) {
    const localOffset = new THREE.Vector3(
      centerOffset[0] * scale,
      centerOffset[1] * scale,
      centerOffset[2] * scale,
    );
    localOffset.applyQuaternion(quat);
    translate.add(localOffset);
  }

  mesh.matrixAutoUpdate = false;
  mesh.matrix.compose(
    translate,
    quat,
    new THREE.Vector3(scale, scale, scale),
  );
}

function getCenterOffset(leaf: HybridLeafInfo): [number, number, number] | null {
  const p = leaf.params;
  switch (leaf.kind) {
    case "box":
      return [p[3], p[4], p[5]];
    case "sphere":
      return [p[1], p[2], p[3]];
    case "cylinder":
      return [p[2], p[3], p[4]];
    case "torus":
      return [p[2], p[3], p[4]];
    default:
      return null;
  }
}

export interface SdfHybridBodyHandle {
  meshes: THREE.Mesh[];
  sdfFallback: (SdfBodyHandle & { version: number }) | null;
}

export function createHybridBody(
  hybridInfo: HybridInfo,
  bodyIndex: number,
  opcodes: Float32Array,
  aabb: Float32Array,
): SdfHybridBodyHandle {
  if (hybridInfo.needs_full_sdf || !hybridInfo.is_union_only) {
    const sdf = createSdfBody(opcodes, aabb);
    return { meshes: [], sdfFallback: { ...sdf, version: 0 } };
  }

  const material = createLambertMaterial(bodyIndex);
  const meshes: THREE.Mesh[] = [];
  for (const leaf of hybridInfo.leaves) {
    const geo = createPrimitiveGeometry(leaf);
    if (!geo) {
      const sdf = createSdfBody(opcodes, aabb);
      for (const m of meshes) {
        m.geometry.dispose();
      }
      return { meshes: [], sdfFallback: { ...sdf, version: 0 } };
    }
    const mesh = new THREE.Mesh(geo, material);
    applyLeafTransform(mesh, leaf);
    mesh.frustumCulled = false;
    meshes.push(mesh);
  }

  return { meshes, sdfFallback: null };
}

export function disposeHybridBody(handle: SdfHybridBodyHandle): void {
  for (const mesh of handle.meshes) {
    mesh.geometry.dispose();
    (mesh.material as THREE.Material).dispose();
  }
  if (handle.sdfFallback) {
    disposeSdfBody(handle.sdfFallback);
  }
}

export function updateHybridBody(
  handle: SdfHybridBodyHandle,
  hybridInfo: HybridInfo,
  bodyIndex: number,
  opcodes: Float32Array,
  aabb: Float32Array,
): SdfHybridBodyHandle {
  disposeHybridBody(handle);
  return createHybridBody(hybridInfo, bodyIndex, opcodes, aabb);
}
