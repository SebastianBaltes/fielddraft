// @ts-nocheck — TSL loose typing, same convention as sdfRenderer.ts

import * as THREE from "three";
import {
  Fn,
  If,
  Loop,
  Break,
  Discard,
  cameraPosition,
  float,
  int,
  max,
  modelWorldMatrix,
  normalize,
  normalWorld,
  positionGeometry,
  uniform,
  uniformArray,
  vec3,
  vec4,
} from "three/tsl";
import { MeshBasicNodeMaterial } from "three/webgpu";
import type { HybridInfo, HybridLeafInfo, SdfRegionInfo } from "./kernel";
import {
  createSdfBody,
  disposeSdfBody,
  type SdfBodyHandle,
} from "./sdfRenderer";
import { buildSdfEvalFn, extractTopology } from "./sdfRenderer";
import { buildVmSdfEvalFn, VM_MAX_OPCODES } from "./sdfVmInterpreter";

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

const MAX_STEPS = 96;
const SURFACE_EPS = 0.001;
const MAX_DIST = 1200;
const NORMAL_EPS = 0.01;

// ── Primitive mesh factory ────────────────────────────────────────

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
  mesh.matrix.compose(translate, quat, new THREE.Vector3(scale, scale, scale));
}

function getCenterOffset(leaf: HybridLeafInfo): [number, number, number] | null {
  const p = leaf.params;
  switch (leaf.kind) {
    case "box": return [p[3], p[4], p[5]];
    case "sphere": return [p[1], p[2], p[3]];
    case "cylinder": return [p[2], p[3], p[4]];
    case "torus": return [p[2], p[3], p[4]];
    default: return null;
  }
}

// ── Lambert material (plain, no clip) ─────────────────────────────

function createLambertMaterial(bodyIndex: number): MeshBasicNodeMaterial {
  const material = new MeshBasicNodeMaterial();
  material.side = THREE.DoubleSide;
  material.toneMapped = false;
  const [r, g, b] = BODY_COLORS[bodyIndex % BODY_COLORS.length];

  material.colorNode = Fn(() => {
    const n = normalize(normalWorld);
    const key = normalize(vec3(0.4, 1, 0.5));
    const fill = normalize(vec3(-0.6, 0.2, 0.8));
    const rim = normalize(vec3(0.0, -0.3, -1.0));
    const dKey = max(n.dot(key), float(0));
    const dFill = max(n.dot(fill), float(0));
    const dRim = max(n.dot(rim), float(0));
    const ambient = float(0.3);
    const light = dKey.mul(0.45).add(dFill.mul(0.2)).add(dRim.mul(0.1)).add(ambient);
    return vec4(vec3(r, g, b).mul(light), 1);
  })();

  return material;
}

// ── Lambert material with clip-AABB discard ───────────────────────

function createClipMaterial(
  bodyIndex: number,
  clipRegions: SdfRegionInfo[],
): MeshBasicNodeMaterial {
  if (clipRegions.length === 0) return createLambertMaterial(bodyIndex);

  const material = new MeshBasicNodeMaterial();
  material.side = THREE.DoubleSide;
  material.toneMapped = false;
  const [r, g, b] = BODY_COLORS[bodyIndex % BODY_COLORS.length];

  const clipData = new Float32Array(clipRegions.length * 6);
  for (let i = 0; i < clipRegions.length; i++) {
    const a = clipRegions[i].aabb;
    clipData.set(a, i * 6);
  }
  const clipBuf = uniformArray(clipData, "float");
  const clipCount = uniform(clipRegions.length, "int");

  material.colorNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;

    Loop(clipCount, ({ i }) => {
      const base = i.mul(6);
      const minX = clipBuf.element(base);
      const minY = clipBuf.element(base.add(1));
      const minZ = clipBuf.element(base.add(2));
      const maxX = clipBuf.element(base.add(3));
      const maxY = clipBuf.element(base.add(4));
      const maxZ = clipBuf.element(base.add(5));

      If(
        worldPos.x.greaterThanEqual(minX)
          .and(worldPos.x.lessThanEqual(maxX))
          .and(worldPos.y.greaterThanEqual(minY))
          .and(worldPos.y.lessThanEqual(maxY))
          .and(worldPos.z.greaterThanEqual(minZ))
          .and(worldPos.z.lessThanEqual(maxZ)),
        () => { Discard(); },
      );
    });

    const n = normalize(normalWorld);
    const key = normalize(vec3(0.4, 1, 0.5));
    const fill = normalize(vec3(-0.6, 0.2, 0.8));
    const rim = normalize(vec3(0.0, -0.3, -1.0));
    const dKey = max(n.dot(key), float(0));
    const dFill = max(n.dot(fill), float(0));
    const dRim = max(n.dot(rim), float(0));
    const ambient = float(0.3);
    const light = dKey.mul(0.45).add(dFill.mul(0.2)).add(dRim.mul(0.1)).add(ambient);
    return vec4(vec3(r, g, b).mul(light), 1);
  })();

  return material;
}

// ── SDF overlay box for overlap regions ───────────────────────────

function createSdfOverlay(
  opcodes: Float32Array,
  region: SdfRegionInfo,
  bodyIndex: number,
): THREE.Mesh {
  const { paramValues, paramCount } = extractTopology(opcodes);
  const paramData = new Float32Array(Math.max(paramCount, 1));
  paramData.set(paramValues);
  const paramBuf = uniformArray(paramData, "float");
  const sdfFn = buildSdfEvalFn(opcodes, paramBuf);
  const sceneSdf = Fn(({ p }: { p: any }) => sdfFn({ p }));

  const sceneNormal = Fn(({ p }: { p: any }) => {
    const ex = vec3(NORMAL_EPS, 0, 0);
    const ey = vec3(0, NORMAL_EPS, 0);
    const ez = vec3(0, 0, NORMAL_EPS);
    return normalize(vec3(
      sceneSdf({ p: p.add(ex) }).sub(sceneSdf({ p: p.sub(ex) })),
      sceneSdf({ p: p.add(ey) }).sub(sceneSdf({ p: p.sub(ey) })),
      sceneSdf({ p: p.add(ez) }).sub(sceneSdf({ p: p.sub(ez) })),
    ));
  });

  const [r, g, b] = BODY_COLORS[bodyIndex % BODY_COLORS.length];

  const colorNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    const ro = cameraPosition;
    const rd = normalize(worldPos.sub(ro));
    const t = float(0).toVar();
    const hit = float(0).toVar();
    const hitPos = vec3(0).toVar();

    Loop(int(MAX_STEPS), () => {
      const p = ro.add(rd.mul(t));
      const d = sceneSdf({ p });
      If(d.lessThan(SURFACE_EPS), () => {
        hit.assign(1);
        hitPos.assign(p);
        Break();
      });
      t.addAssign(d);
      If(t.greaterThan(MAX_DIST), () => { Break(); });
    });

    const finalColor = vec3(0).toVar();
    const finalAlpha = float(0).toVar();
    If(hit.greaterThan(0.5), () => {
      const n = sceneNormal({ p: hitPos });
      const key = normalize(vec3(0.4, 1, 0.5));
      const fill = normalize(vec3(-0.6, 0.2, 0.8));
      const rim = normalize(vec3(0.0, -0.3, -1.0));
      const dKey = max(n.dot(key), float(0));
      const dFill = max(n.dot(fill), float(0));
      const dRim = max(n.dot(rim), float(0));
      const ambient = float(0.3);
      const light = dKey.mul(0.45).add(dFill.mul(0.2)).add(dRim.mul(0.1)).add(ambient);
      finalColor.assign(vec3(r, g, b).mul(light));
      finalAlpha.assign(1);
    });

    return vec4(finalColor, finalAlpha);
  });

  const material = new MeshBasicNodeMaterial();
  material.outputNode = colorNode();
  material.side = THREE.BackSide;
  material.transparent = true;
  material.depthWrite = false;
  material.depthTest = true;
  material.toneMapped = false;

  const a = region.aabb;
  const margin = 2.0;
  const sx = a[3] - a[0] + margin * 2;
  const sy = a[4] - a[1] + margin * 2;
  const sz = a[5] - a[2] + margin * 2;
  const cx = (a[0] + a[3]) * 0.5;
  const cy = (a[1] + a[4]) * 0.5;
  const cz = (a[2] + a[5]) * 0.5;

  const geometry = new THREE.BoxGeometry(sx, sy, sz);
  const mesh = new THREE.Mesh(geometry, material);
  mesh.position.set(cx, cy, cz);
  mesh.frustumCulled = false;
  return mesh;
}

// ── SDF overlay box (VM interpreter, no shader recompilation) ────

function createSdfOverlayVm(
  opcodes: Float32Array,
  region: SdfRegionInfo,
  bodyIndex: number,
): THREE.Mesh {
  const opcodeData = new Float32Array(VM_MAX_OPCODES);
  opcodeData.set(opcodes.subarray(0, Math.min(opcodes.length, VM_MAX_OPCODES)));
  const opBuf = uniformArray(opcodeData, "float");
  const sceneSdf = buildVmSdfEvalFn(opBuf);

  const sceneNormal = Fn(({ p }: { p: any }) => {
    const ex = vec3(NORMAL_EPS, 0, 0);
    const ey = vec3(0, NORMAL_EPS, 0);
    const ez = vec3(0, 0, NORMAL_EPS);
    return normalize(vec3(
      sceneSdf({ p: p.add(ex) }).sub(sceneSdf({ p: p.sub(ex) })),
      sceneSdf({ p: p.add(ey) }).sub(sceneSdf({ p: p.sub(ey) })),
      sceneSdf({ p: p.add(ez) }).sub(sceneSdf({ p: p.sub(ez) })),
    ));
  });

  const [r, g, b] = BODY_COLORS[bodyIndex % BODY_COLORS.length];

  const colorNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    const ro = cameraPosition;
    const rd = normalize(worldPos.sub(ro));
    const t = float(0).toVar();
    const hit = float(0).toVar();
    const hitPos = vec3(0).toVar();

    Loop(int(MAX_STEPS), () => {
      const p = ro.add(rd.mul(t));
      const d = sceneSdf({ p });
      If(d.lessThan(SURFACE_EPS), () => {
        hit.assign(1);
        hitPos.assign(p);
        Break();
      });
      t.addAssign(d);
      If(t.greaterThan(MAX_DIST), () => { Break(); });
    });

    const finalColor = vec3(0).toVar();
    const finalAlpha = float(0).toVar();
    If(hit.greaterThan(0.5), () => {
      const n = sceneNormal({ p: hitPos });
      const key = normalize(vec3(0.4, 1, 0.5));
      const fill = normalize(vec3(-0.6, 0.2, 0.8));
      const rim = normalize(vec3(0.0, -0.3, -1.0));
      const dKey = max(n.dot(key), float(0));
      const dFill = max(n.dot(fill), float(0));
      const dRim = max(n.dot(rim), float(0));
      const ambient = float(0.3);
      const light = dKey.mul(0.45).add(dFill.mul(0.2)).add(dRim.mul(0.1)).add(ambient);
      finalColor.assign(vec3(r, g, b).mul(light));
      finalAlpha.assign(1);
    });

    return vec4(finalColor, finalAlpha);
  });

  const material = new MeshBasicNodeMaterial();
  material.outputNode = colorNode();
  material.side = THREE.BackSide;
  material.transparent = true;
  material.depthWrite = false;
  material.depthTest = true;
  material.toneMapped = false;

  const a = region.aabb;
  const margin = 2.0;
  const sx = a[3] - a[0] + margin * 2;
  const sy = a[4] - a[1] + margin * 2;
  const sz = a[5] - a[2] + margin * 2;
  const cx = (a[0] + a[3]) * 0.5;
  const cy = (a[1] + a[4]) * 0.5;
  const cz = (a[2] + a[5]) * 0.5;

  const geometry = new THREE.BoxGeometry(sx, sy, sz);
  const mesh = new THREE.Mesh(geometry, material);
  mesh.position.set(cx, cy, cz);
  mesh.frustumCulled = false;
  return mesh;
}

// ── Body handle ───────────────────────────────────────────────────

export interface SdfHybridBodyHandle {
  meshes: THREE.Mesh[];
  overlays: THREE.Mesh[];
  sdfFallback: (SdfBodyHandle & { version: number }) | null;
}

export function createHybridBody(
  hybridInfo: HybridInfo,
  bodyIndex: number,
  opcodes: Float32Array,
  aabb: Float32Array,
  useVm = false,
): SdfHybridBodyHandle {
  const empty: SdfHybridBodyHandle = { meshes: [], overlays: [], sdfFallback: null };

  // Full SDF fallback for domain warps etc.
  // Always use baked approach here — the VM shader is too heavy for
  // full-body raymarching and can crash the GPU on some hardware.
  if (hybridInfo.needs_full_sdf) {
    if (opcodes.length === 0) return empty;
    const sdf = createSdfBody(opcodes, aabb);
    return { ...empty, sdfFallback: { ...sdf, version: 0 } };
  }

  // Union-only: pure mesh, no SDF
  if (hybridInfo.is_union_only) {
    return createMeshOnlyBody(hybridInfo, bodyIndex);
  }

  // Phase 2: mesh + SDF overlay in overlap regions
  return createMeshPlusSdfBody(hybridInfo, bodyIndex, opcodes, aabb, useVm);
}

function createMeshOnlyBody(
  hybridInfo: HybridInfo,
  bodyIndex: number,
): SdfHybridBodyHandle {
  const material = createLambertMaterial(bodyIndex);
  const meshes: THREE.Mesh[] = [];
  for (const leaf of hybridInfo.leaves) {
    if (leaf.role === "skip") continue;
    const geo = createPrimitiveGeometry(leaf);
    if (!geo) {
      for (const m of meshes) m.geometry.dispose();
      return { meshes: [], overlays: [], sdfFallback: null };
    }
    const mesh = new THREE.Mesh(geo, material);
    applyLeafTransform(mesh, leaf);
    mesh.frustumCulled = false;
    meshes.push(mesh);
  }
  return { meshes, overlays: [], sdfFallback: null };
}

function createMeshPlusSdfBody(
  hybridInfo: HybridInfo,
  bodyIndex: number,
  opcodes: Float32Array,
  aabb: Float32Array,
  useVm = false,
): SdfHybridBodyHandle {
  const empty: SdfHybridBodyHandle = { meshes: [], overlays: [], sdfFallback: null };
  const meshLeaves = hybridInfo.leaves.filter(l => l.role === "mesh");
  const hasAllMeshable = meshLeaves.every(l => createPrimitiveGeometry(l) !== null);

  if (!hasAllMeshable || meshLeaves.length === 0 || opcodes.length === 0) {
    if (opcodes.length === 0) return empty;
    const sdf = createSdfBody(opcodes, aabb);
    return { ...empty, sdfFallback: { ...sdf, version: 0 } };
  }

  const material = createClipMaterial(bodyIndex, hybridInfo.sdf_regions);
  const meshes: THREE.Mesh[] = [];
  for (const leaf of meshLeaves) {
    const geo = createPrimitiveGeometry(leaf)!;
    const mesh = new THREE.Mesh(geo, material);
    applyLeafTransform(mesh, leaf);
    mesh.frustumCulled = false;
    meshes.push(mesh);
  }

  const overlays: THREE.Mesh[] = [];
  const overlayFactory = useVm ? createSdfOverlayVm : createSdfOverlay;
  for (const region of hybridInfo.sdf_regions) {
    overlays.push(overlayFactory(opcodes, region, bodyIndex));
  }

  return { meshes, overlays, sdfFallback: null };
}

export function disposeHybridBody(handle: SdfHybridBodyHandle): void {
  for (const mesh of handle.meshes) {
    mesh.geometry.dispose();
    (mesh.material as THREE.Material).dispose();
  }
  for (const overlay of handle.overlays) {
    overlay.geometry.dispose();
    (overlay.material as THREE.Material).dispose();
  }
  if (handle.sdfFallback) disposeSdfBody(handle.sdfFallback);
}

export function updateHybridBody(
  handle: SdfHybridBodyHandle,
  hybridInfo: HybridInfo,
  bodyIndex: number,
  opcodes: Float32Array,
  aabb: Float32Array,
  useVm = false,
): SdfHybridBodyHandle {
  disposeHybridBody(handle);
  return createHybridBody(hybridInfo, bodyIndex, opcodes, aabb, useVm);
}
