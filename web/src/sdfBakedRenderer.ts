/**
 * Baked 3D Distance Field SDF renderer.
 *
 * Pre-computes the SDF on a 3D grid via compute shader, stores it in a
 * Storage3DTexture (R32Float), then raymarches with O(1) trilinear
 * texture lookups per step — independent of CSG complexity.
 *
 * Progressive refinement: 64³ → 128³ → 256³ as the scene settles.
 *
 * Trade-off vs. transpiled/VM modes: loses sharp edges at low res,
 * uses more GPU memory (8–64 MB/body), but per-step cost is constant.
 */

// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import {
  Break,
  Discard,
  Fn,
  If,
  Loop,
  cameraPosition,
  cameraProjectionMatrix,
  cameraViewMatrix,
  clamp,
  float,
  int,
  instanceIndex,
  max,
  modelWorldMatrix,
  normalize,
  positionGeometry,
  property,
  texture3D,
  textureStore,
  uint,
  uniformArray,
  uvec3,
  vec3,
  vec4,
} from "three/tsl";
import { MeshBasicNodeMaterial, Storage3DTexture } from "three/webgpu";

import { MAX_STEPS, SURFACE_EPS, NORMAL_EPS, MAX_DIST } from "./sdfRenderer";
import { buildVmSdfEvalFn, VM_MAX_OPCODES } from "./sdfVmInterpreter";

// ── Constants ───────────────────────────────────────────────────────

export type BakeResolution = 64 | 128 | 256;

// ── Compute bake kernel ─────────────────────────────────────────────

interface BakeKernel {
  computeNode: any;
  opcodeBuf: ReturnType<typeof uniformArray>;
  opcodeData: Float32Array;
  aabbBuf: ReturnType<typeof uniformArray>;
  aabbData: Float32Array;
  volumeTex: Storage3DTexture;
  res: BakeResolution;
}

function createBakeKernel(res: BakeResolution): BakeKernel {
  const opcodeData = new Float32Array(VM_MAX_OPCODES);
  const opcodeBuf = uniformArray(opcodeData, "float");

  const aabbData = new Float32Array(6);
  const aabbBuf = uniformArray(aabbData, "float");

  const volumeTex = new Storage3DTexture(res, res, res);
  volumeTex.type = THREE.FloatType;
  volumeTex.format = THREE.RedFormat;
  volumeTex.internalFormat = "r32float";

  const sdfEval = buildVmSdfEvalFn(opcodeBuf);

  const bakeKernelFn = Fn(({ volumeTex, aabbBuf }: any) => {
    const resU = uint(res);
    const ix = instanceIndex.mod(resU);
    const iy = instanceIndex.div(resU).mod(resU);
    const iz = instanceIndex.div(resU.mul(resU));
    const coord = uvec3(ix, iy, iz);

    const aabbMinX = aabbBuf.element(0);
    const aabbMinY = aabbBuf.element(1);
    const aabbMinZ = aabbBuf.element(2);
    const aabbMaxX = aabbBuf.element(3);
    const aabbMaxY = aabbBuf.element(4);
    const aabbMaxZ = aabbBuf.element(5);

    const cellSize = vec3(
      aabbMaxX.sub(aabbMinX).div(float(res)),
      aabbMaxY.sub(aabbMinY).div(float(res)),
      aabbMaxZ.sub(aabbMinZ).div(float(res)),
    );

    const worldPos = vec3(
      aabbMinX.add(ix.toFloat().add(0.5).mul(cellSize.x)),
      aabbMinY.add(iy.toFloat().add(0.5).mul(cellSize.y)),
      aabbMinZ.add(iz.toFloat().add(0.5).mul(cellSize.z)),
    );

    const dist = sdfEval({ p: worldPos });

    textureStore(volumeTex, coord, vec4(dist, 0, 0, 0)).toWriteOnly();
  });

  const computeNode = bakeKernelFn({ volumeTex, aabbBuf })
    .compute(res * res * res, [64]);

  return { computeNode, opcodeBuf, opcodeData, aabbBuf, aabbData, volumeTex, res };
}

// ── Volume raymarching material ─────────────────────────────────────

function createVolumeMaterial(
  volumeTex: Storage3DTexture,
  aabbBuf: ReturnType<typeof uniformArray>,
): MeshBasicNodeMaterial {
  const hitDepthProp = property('float', 'hitDepth');

  const colorNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    const ro = cameraPosition;
    const rd = normalize(worldPos.sub(ro));

    const aabbMinX = aabbBuf.element(0);
    const aabbMinY = aabbBuf.element(1);
    const aabbMinZ = aabbBuf.element(2);
    const aabbMaxX = aabbBuf.element(3);
    const aabbMaxY = aabbBuf.element(4);
    const aabbMaxZ = aabbBuf.element(5);
    const aabbMin = vec3(aabbMinX, aabbMinY, aabbMinZ);
    const aabbMax = vec3(aabbMaxX, aabbMaxY, aabbMaxZ);
    const aabbSize = aabbMax.sub(aabbMin);

    const t = float(0).toVar();
    const hit = float(0).toVar();
    const hitPos = vec3(0).toVar();

    Loop(int(MAX_STEPS), () => {
      const p = ro.add(rd.mul(t));
      const uvw = p.sub(aabbMin).div(aabbSize);
      const clamped = clamp(uvw, 0, 1);
      const inside = clamped.sub(uvw).length().lessThan(0.001);

      const d = float(0).toVar();
      If(inside, () => {
        d.assign(texture3D(volumeTex, uvw).x);
      }).Else(() => {
        d.assign(float(1.0));
      });

      If(d.lessThan(SURFACE_EPS), () => {
        hit.assign(1);
        hitPos.assign(p);
        Break();
      });
      t.addAssign(max(d.mul(0.8), float(SURFACE_EPS)));
      If(t.greaterThan(MAX_DIST), () => {
        Break();
      });
    });

    const finalColor = vec3(0).toVar();
    If(hit.greaterThan(0.5), () => {
      const eps = NORMAL_EPS;
      const ex = vec3(eps, 0, 0);
      const ey = vec3(0, eps, 0);
      const ez = vec3(0, 0, eps);

      const sampleAt = Fn(({ p }: { p: any }) => {
        const uv = p.sub(aabbMin).div(aabbSize);
        return texture3D(volumeTex, clamp(uv, 0, 1)).x;
      });

      const n = normalize(vec3(
        sampleAt({ p: hitPos.add(ex) }).sub(sampleAt({ p: hitPos.sub(ex) })),
        sampleAt({ p: hitPos.add(ey) }).sub(sampleAt({ p: hitPos.sub(ey) })),
        sampleAt({ p: hitPos.add(ez) }).sub(sampleAt({ p: hitPos.sub(ez) })),
      ));

      const key = normalize(vec3(0.4, 1, 0.5));
      const fill = normalize(vec3(-0.6, 0.2, 0.8));
      const rim = normalize(vec3(0.0, -0.3, -1.0));
      const dKey = max(n.dot(key), float(0));
      const dFill = max(n.dot(fill), float(0));
      const dRim = max(n.dot(rim), float(0));
      const ambient = float(0.3);
      const light = dKey.mul(0.45).add(dFill.mul(0.2)).add(dRim.mul(0.1)).add(ambient);
      const baseColor = vec3(0.55, 0.65, 0.85);
      finalColor.assign(baseColor.mul(light));
      const clipPos = cameraProjectionMatrix.mul(cameraViewMatrix).mul(vec4(hitPos, 1));
      hitDepthProp.assign(clipPos.z.div(clipPos.w));
    }).Else(() => {
      Discard();
    });

    return vec4(finalColor, 1);
  });

  const material = new MeshBasicNodeMaterial();
  material.outputNode = colorNode();
  material.depthNode = hitDepthProp;
  material.side = THREE.BackSide;
  material.depthWrite = true;
  material.depthTest = true;
  material.toneMapped = false;

  return material;
}

// ── Public API ──────────────────────────────────────────────────────

export interface SdfBakedBodyHandle {
  mesh: THREE.Mesh;
  material: MeshBasicNodeMaterial;
  bakeKernels: Map<BakeResolution, BakeKernel>;
  currentRes: BakeResolution;
  pendingRefine: ReturnType<typeof setTimeout> | null;
  opcodes: Float32Array;
  aabb: Float32Array;
}

export function createSdfBakedBody(
  opcodes: Float32Array,
  aabb: Float32Array,
): SdfBakedBodyHandle {
  console.time("sdf-baked:create");

  const kernel64 = createBakeKernel(64);
  kernel64.opcodeData.set(opcodes.subarray(0, Math.min(opcodes.length, VM_MAX_OPCODES)));

  const margin = 2.0;
  const expandedAabb = new Float32Array(6);
  expandedAabb[0] = aabb[0] - margin;
  expandedAabb[1] = aabb[1] - margin;
  expandedAabb[2] = aabb[2] - margin;
  expandedAabb[3] = aabb[3] + margin;
  expandedAabb[4] = aabb[4] + margin;
  expandedAabb[5] = aabb[5] + margin;
  kernel64.aabbData.set(expandedAabb);

  const material = createVolumeMaterial(kernel64.volumeTex, kernel64.aabbBuf);

  const minX = expandedAabb[0], minY = expandedAabb[1], minZ = expandedAabb[2];
  const maxX = expandedAabb[3], maxY = expandedAabb[4], maxZ = expandedAabb[5];
  const sx = maxX - minX;
  const sy = maxY - minY;
  const sz = maxZ - minZ;
  const cx = (minX + maxX) * 0.5;
  const cy = (minY + maxY) * 0.5;
  const cz = (minZ + maxZ) * 0.5;

  const geometry = new THREE.BoxGeometry(sx, sy, sz);
  const mesh = new THREE.Mesh(geometry, material);
  mesh.position.set(cx, cy, cz);
  mesh.frustumCulled = false;

  const bakeKernels = new Map<BakeResolution, BakeKernel>();
  bakeKernels.set(64, kernel64);

  console.timeEnd("sdf-baked:create");

  return {
    mesh,
    material,
    bakeKernels,
    currentRes: 64,
    pendingRefine: null,
    opcodes: opcodes.slice(),
    aabb: expandedAabb,
  };
}

export function bakeVolume(
  handle: SdfBakedBodyHandle,
  renderer: THREE.WebGPURenderer,
  res: BakeResolution = 64,
): void {
  let kernel = handle.bakeKernels.get(res);
  if (!kernel) {
    kernel = createBakeKernel(res);
    handle.bakeKernels.set(res, kernel);
  }

  kernel.opcodeData.fill(0);
  kernel.opcodeData.set(
    handle.opcodes.subarray(0, Math.min(handle.opcodes.length, VM_MAX_OPCODES)),
  );
  kernel.aabbData.set(handle.aabb);

  console.time(`sdf-baked:bake@${res}`);
  renderer.compute(kernel.computeNode);
  console.timeEnd(`sdf-baked:bake@${res}`);

  handle.currentRes = res;
}

export function scheduleRefinement(
  handle: SdfBakedBodyHandle,
  renderer: THREE.WebGPURenderer,
  invalidate: () => void,
): void {
  if (handle.pendingRefine !== null) {
    clearTimeout(handle.pendingRefine);
  }

  if (handle.currentRes < 128) {
    handle.pendingRefine = setTimeout(() => {
      bakeVolume(handle, renderer, 128);
      invalidate();

      handle.pendingRefine = setTimeout(() => {
        bakeVolume(handle, renderer, 256);
        invalidate();
        handle.pendingRefine = null;
      }, 300);
    }, 100);
  } else if (handle.currentRes < 256) {
    handle.pendingRefine = setTimeout(() => {
      bakeVolume(handle, renderer, 256);
      invalidate();
      handle.pendingRefine = null;
    }, 300);
  }
}

export function updateSdfBakedBody(
  handle: SdfBakedBodyHandle,
  opcodes: Float32Array,
  aabb: Float32Array,
  renderer: THREE.WebGPURenderer,
  invalidate: () => void,
): SdfBakedBodyHandle {
  handle.opcodes = opcodes.slice();

  const margin = 2.0;
  handle.aabb[0] = aabb[0] - margin;
  handle.aabb[1] = aabb[1] - margin;
  handle.aabb[2] = aabb[2] - margin;
  handle.aabb[3] = aabb[3] + margin;
  handle.aabb[4] = aabb[4] + margin;
  handle.aabb[5] = aabb[5] + margin;

  const minX = handle.aabb[0], minY = handle.aabb[1], minZ = handle.aabb[2];
  const maxX = handle.aabb[3], maxY = handle.aabb[4], maxZ = handle.aabb[5];
  handle.mesh.position.set(
    (minX + maxX) * 0.5,
    (minY + maxY) * 0.5,
    (minZ + maxZ) * 0.5,
  );
  const sx = maxX - minX;
  const sy = maxY - minY;
  const sz = maxZ - minZ;
  const geom = handle.mesh.geometry as THREE.BoxGeometry;
  const gp = geom.parameters;
  if (
    Math.abs(gp.width - sx) > 0.01 ||
    Math.abs(gp.height - sy) > 0.01 ||
    Math.abs(gp.depth - sz) > 0.01
  ) {
    geom.dispose();
    handle.mesh.geometry = new THREE.BoxGeometry(sx, sy, sz);
  }

  bakeVolume(handle, renderer, 64);
  scheduleRefinement(handle, renderer, invalidate);

  return handle;
}

export function disposeSdfBakedBody(handle: SdfBakedBodyHandle): void {
  if (handle.pendingRefine !== null) {
    clearTimeout(handle.pendingRefine);
    handle.pendingRefine = null;
  }
  handle.mesh.geometry.dispose();
  handle.material.dispose();
  for (const kernel of handle.bakeKernels.values()) {
    kernel.volumeTex.dispose();
  }
  handle.bakeKernels.clear();
}
