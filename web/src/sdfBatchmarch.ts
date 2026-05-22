/**
 * Batchmarching SDF renderer — compute-shader wavefront approach.
 *
 * Instead of per-pixel fragment-shader raymarching (where pixels in the
 * same wavefront diverge at different step counts), this renderer uses
 * compute dispatches that process ALL pixels for each batch of marching
 * steps.  Early-terminated pixels skip instantly, keeping warps coherent.
 *
 * Pipeline per frame:
 *   1. INIT dispatch: set ray_t = 0, hit = 0 for each pixel
 *   2. MARCH dispatches (6×, 16 steps each): advance rays, mark hits
 *   3. SHADE dispatch: compute normals + lighting for hit pixels
 *   4. BLIT: fullscreen quad reads output texture
 *
 * Trade-off: better warp coherence than fragment-based approaches, but
 * ~100 MB GPU memory for ray state + output at 4K, and complex
 * Three.js integration (compute-then-render sequencing).
 */

// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import {
  Break,
  Fn,
  If,
  Loop,
  float,
  int,
  instanceIndex,
  max,
  normalize,
  textureStore,
  uint,
  uniformArray,
  uvec2,
  vec3,
  vec4,
} from "three/tsl";
import { MeshBasicNodeMaterial, StorageTexture } from "three/webgpu";

import { SURFACE_EPS, MAX_DIST } from "./sdfRenderer";
import { buildVmSdfEvalFn, VM_MAX_OPCODES } from "./sdfVmInterpreter";

// ── Constants ───────────────────────────────────────────────────────

const STEPS_PER_BATCH = 16;
const NUM_BATCHES = 6;
const NORMAL_EPS = 0.01;

// ── Batchmarch pipeline ─────────────────────────────────────────────

export interface BatchmarchState {
  rayStateTex: StorageTexture;
  outputTex: StorageTexture;
  blitMesh: THREE.Mesh;
  blitMaterial: MeshBasicNodeMaterial;
  opcodeBuf: ReturnType<typeof uniformArray>;
  opcodeData: Float32Array;
  cameraBuf: ReturnType<typeof uniformArray>;
  cameraData: Float32Array;
  initNode: any;
  marchNodes: any[];
  shadeNode: any;
  width: number;
  height: number;
}

export function initBatchmarch(
  width: number,
  height: number,
): BatchmarchState {
  console.time("sdf-batch:init");

  const opcodeData = new Float32Array(VM_MAX_OPCODES);
  const opcodeBuf = uniformArray(opcodeData, "float");

  const cameraData = new Float32Array(16);
  const cameraBuf = uniformArray(cameraData, "float");

  const rayStateTex = new StorageTexture(width, height);
  rayStateTex.type = THREE.FloatType;
  rayStateTex.format = THREE.RGFormat;
  rayStateTex.internalFormat = "rg32float";

  const outputTex = new StorageTexture(width, height);
  outputTex.type = THREE.UnsignedByteType;
  outputTex.format = THREE.RGBAFormat;
  outputTex.internalFormat = "rgba8unorm";

  const sdfEval = buildVmSdfEvalFn(opcodeBuf);

  const buildRay = Fn(({ px, py, cameraBuf }: any) => {
    const camPosX = cameraBuf.element(0);
    const camPosY = cameraBuf.element(1);
    const camPosZ = cameraBuf.element(2);
    const rightX = cameraBuf.element(6);
    const rightY = cameraBuf.element(7);
    const rightZ = cameraBuf.element(8);
    const upX = cameraBuf.element(9);
    const upY = cameraBuf.element(10);
    const upZ = cameraBuf.element(11);
    const fovY = cameraBuf.element(12);
    const aspect = cameraBuf.element(13);

    const ndcX = px.toFloat().add(0.5).div(float(width)).mul(2).sub(1);
    const ndcY = float(1).sub(py.toFloat().add(0.5).div(float(height)).mul(2));

    const halfH = fovY.mul(0.5).toVar();
    const tanHalf = halfH.sin().div(halfH.cos());
    const sx = ndcX.mul(tanHalf).mul(aspect);
    const sy = ndcY.mul(tanHalf);

    const fwdX = cameraBuf.element(3);
    const fwdY = cameraBuf.element(4);
    const fwdZ = cameraBuf.element(5);

    const dirX = fwdX.add(rightX.mul(sx)).add(upX.mul(sy));
    const dirY = fwdY.add(rightY.mul(sx)).add(upY.mul(sy));
    const dirZ = fwdZ.add(rightZ.mul(sx)).add(upZ.mul(sy));
    const dir = normalize(vec3(dirX, dirY, dirZ));

    return { origin: vec3(camPosX, camPosY, camPosZ), dir };
  });

  // ── INIT dispatch ──────────────────────────────────────────────

  const initKernel = Fn(({ rayStateTex }: any) => {
    const w = uint(width);
    const px = instanceIndex.mod(w);
    const py = instanceIndex.div(w);
    textureStore(rayStateTex, uvec2(px, py), vec4(0, 0, 0, 0)).toWriteOnly();
  });

  const initNode = initKernel({ rayStateTex }).compute(width * height, [64]);

  // ── MARCH dispatches ───────────────────────────────────────────

  const marchKernel = Fn(({ rayStateTex, cameraBuf }: any) => {
    const w = uint(width);
    const px = instanceIndex.mod(w);
    const py = instanceIndex.div(w);

    const state = textureStore(rayStateTex, uvec2(px, py), null).toReadWrite();
    const t = state.x.toVar();
    const hitFlag = state.y.toVar();

    If(hitFlag.greaterThan(0.5), () => { return; });
    If(t.greaterThan(MAX_DIST), () => { return; });

    const { origin: ro, dir: rd } = buildRay({ px, py, cameraBuf });

    Loop(int(STEPS_PER_BATCH), () => {
      const p = ro.add(rd.mul(t));
      const d = sdfEval({ p });
      If(d.lessThan(SURFACE_EPS), () => {
        hitFlag.assign(1);
        Break();
      });
      t.addAssign(max(d, float(SURFACE_EPS)));
      If(t.greaterThan(MAX_DIST), () => {
        Break();
      });
    });

    textureStore(rayStateTex, uvec2(px, py), vec4(t, hitFlag, 0, 0)).toWriteOnly();
  });

  const marchNodes = [];
  for (let i = 0; i < NUM_BATCHES; i++) {
    marchNodes.push(
      marchKernel({ rayStateTex, cameraBuf }).compute(width * height, [64]),
    );
  }

  // ── SHADE dispatch ─────────────────────────────────────────────

  const shadeKernel = Fn(({ rayStateTex, outputTex, cameraBuf }: any) => {
    const w = uint(width);
    const px = instanceIndex.mod(w);
    const py = instanceIndex.div(w);

    const state = textureStore(rayStateTex, uvec2(px, py), null).toReadOnly();
    const t = state.x;
    const hitFlag = state.y;

    If(hitFlag.lessThan(0.5), () => {
      textureStore(outputTex, uvec2(px, py), vec4(0, 0, 0, 0)).toWriteOnly();
      return;
    });

    const { origin: ro, dir: rd } = buildRay({ px, py, cameraBuf });
    const hitPos = ro.add(rd.mul(t));

    const eps = float(NORMAL_EPS);
    const ex = vec3(eps, 0, 0);
    const ey = vec3(0, eps, 0);
    const ez = vec3(0, 0, eps);
    const n = normalize(vec3(
      sdfEval({ p: hitPos.add(ex) }).sub(sdfEval({ p: hitPos.sub(ex) })),
      sdfEval({ p: hitPos.add(ey) }).sub(sdfEval({ p: hitPos.sub(ey) })),
      sdfEval({ p: hitPos.add(ez) }).sub(sdfEval({ p: hitPos.sub(ez) })),
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
    const color = baseColor.mul(light);

    textureStore(outputTex, uvec2(px, py), vec4(color, 1)).toWriteOnly();
  });

  const shadeNode = shadeKernel({ rayStateTex, outputTex, cameraBuf })
    .compute(width * height, [64]);

  // ── Blit fullscreen quad ───────────────────────────────────────

  const blitMaterial = new MeshBasicNodeMaterial();
  blitMaterial.colorNode = vec4(1);
  blitMaterial.transparent = true;
  blitMaterial.depthWrite = false;
  blitMaterial.depthTest = false;
  blitMaterial.toneMapped = false;

  const blitGeometry = new THREE.PlaneGeometry(2, 2);
  const blitMesh = new THREE.Mesh(blitGeometry, blitMaterial);
  blitMesh.frustumCulled = false;

  console.timeEnd("sdf-batch:init");

  return {
    rayStateTex,
    outputTex,
    blitMesh,
    blitMaterial,
    opcodeBuf,
    opcodeData,
    cameraBuf,
    cameraData,
    initNode,
    marchNodes,
    shadeNode,
    width,
    height,
  };
}

export function updateBatchmarchOpcodes(
  state: BatchmarchState,
  opcodes: Float32Array,
): void {
  state.opcodeData.fill(0);
  state.opcodeData.set(
    opcodes.subarray(0, Math.min(opcodes.length, VM_MAX_OPCODES)),
  );
}

export function updateBatchmarchCamera(
  state: BatchmarchState,
  camera: THREE.Camera,
): void {
  const cam = camera as THREE.PerspectiveCamera;
  const pos = cam.position;
  const fwd = new THREE.Vector3(0, 0, -1).applyQuaternion(cam.quaternion);
  const right = new THREE.Vector3(1, 0, 0).applyQuaternion(cam.quaternion);
  const up = new THREE.Vector3(0, 1, 0).applyQuaternion(cam.quaternion);

  const d = state.cameraData;
  d[0] = pos.x; d[1] = pos.y; d[2] = pos.z;
  d[3] = fwd.x; d[4] = fwd.y; d[5] = fwd.z;
  d[6] = right.x; d[7] = right.y; d[8] = right.z;
  d[9] = up.x; d[10] = up.y; d[11] = up.z;
  d[12] = cam.fov !== undefined ? (cam.fov * Math.PI) / 180 : 1.0;
  d[13] = cam.aspect !== undefined ? cam.aspect : state.width / state.height;
  d[14] = cam.near !== undefined ? cam.near : 0.1;
  d[15] = cam.far !== undefined ? cam.far : 1000;
}

export function runBatchmarchFrame(
  state: BatchmarchState,
  renderer: THREE.WebGPURenderer,
): void {
  renderer.compute(state.initNode);
  for (const marchNode of state.marchNodes) {
    renderer.compute(marchNode);
  }
  renderer.compute(state.shadeNode);
}

export function disposeBatchmarch(state: BatchmarchState): void {
  state.rayStateTex.dispose();
  state.outputTex.dispose();
  state.blitMesh.geometry.dispose();
  state.blitMaterial.dispose();
}
