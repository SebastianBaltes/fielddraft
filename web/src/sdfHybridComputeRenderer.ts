/**
 * Compute-shader hybrid SDF renderer (Phase 5).
 *
 * Replaces fragment-shader SDF overlays with compute dispatches.
 * Key benefit: opcodes are buffer data, NOT baked into shader code,
 * so adding/removing CSG features never triggers shader recompilation.
 *
 * Pipeline per frame:
 *   1. CLEAR dispatch: zero the SDF color StorageTexture
 *   2. MARCH+SHADE dispatch per body: VM-interpreted SDF raymarching
 *   3. Main scene render (meshes, grid, helpers)
 *   4. BLIT render: fullscreen quad composites SDF over scene
 *
 * Sub-Phase A: full-body compute SDF for all bodies (no mesh split yet).
 * Sub-Phase B will add mesh+clip for union parts, compute only for overlaps.
 */

// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import {
  Break,
  Discard,
  Fn,
  If,
  Loop,
  float,
  int,
  instanceIndex,
  max,
  normalize,
  texture,
  textureStore,
  uint,
  uniformArray,
  uv,
  uvec2,
  vec2,
  vec3,
  vec4,
} from "three/tsl";
import { MeshBasicNodeMaterial, StorageTexture } from "three/webgpu";

import { SURFACE_EPS, MAX_DIST } from "./sdfRenderer";
import { buildVmSdfEvalFn, VM_MAX_OPCODES } from "./sdfVmInterpreter";

const MAX_STEPS = 64;
const NORMAL_EPS = 0.01;
const COMPUTE_SCALE = 0.5;
export const MAX_COMPUTE_BODIES = 4;

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

// ── Types ──────────────────────────────────────────────────────────

export interface ComputePipelineState {
  sdfColorTex: StorageTexture;
  clearNode: any;
  blitMesh: THREE.Mesh;
  blitMaterial: MeshBasicNodeMaterial;
  blitScene: THREE.Scene;
  blitCamera: THREE.OrthographicCamera;
  cameraBuf: ReturnType<typeof uniformArray>;
  cameraData: Float32Array;
  width: number;
  height: number;
}

export interface ComputeBodyHandle {
  opcodeBuf: ReturnType<typeof uniformArray>;
  opcodeData: Float32Array;
  bodyColorBuf: ReturnType<typeof uniformArray>;
  marchNode: any;
  bodyIndex: number;
}

// ── Pipeline init ──────────────────────────────────────────────────

export function initComputePipeline(
  viewportWidth: number,
  viewportHeight: number,
): ComputePipelineState {
  const width = Math.max(1, Math.round(viewportWidth * COMPUTE_SCALE));
  const height = Math.max(1, Math.round(viewportHeight * COMPUTE_SCALE));
  const sdfColorTex = new StorageTexture(width, height);
  sdfColorTex.type = THREE.UnsignedByteType;
  sdfColorTex.format = THREE.RGBAFormat;
  sdfColorTex.internalFormat = "rgba8unorm";
  sdfColorTex.minFilter = THREE.LinearFilter;
  sdfColorTex.magFilter = THREE.LinearFilter;

  const cameraData = new Float32Array(16);
  const cameraBuf = uniformArray(cameraData, "float");

  // ── Clear kernel ────────────────────────────────────────────────

  const clearKernel = Fn(({ ct }: any) => {
    const w = uint(width);
    const px = instanceIndex.mod(w);
    const py = instanceIndex.div(w);
    textureStore(ct, uvec2(px, py), vec4(0, 0, 0, 0)).toWriteOnly();
  });
  const clearNode = clearKernel({ ct: sdfColorTex })
    .compute(width * height, [64]);

  // ── Blit material + scene ───────────────────────────────────────

  const blitMaterial = new MeshBasicNodeMaterial();
  blitMaterial.outputNode = Fn(() => {
    const flippedUv = vec2(uv().x, float(1).sub(uv().y));
    const c = texture(sdfColorTex, flippedUv);
    If(c.w.lessThan(0.5), () => { Discard(); });
    return c;
  })();
  blitMaterial.depthWrite = false;
  blitMaterial.depthTest = false;
  blitMaterial.toneMapped = false;

  const blitMesh = new THREE.Mesh(
    new THREE.PlaneGeometry(2, 2),
    blitMaterial,
  );
  blitMesh.frustumCulled = false;

  const blitScene = new THREE.Scene();
  blitScene.add(blitMesh);

  const blitCamera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 2);
  blitCamera.position.set(0, 0, 1);
  blitCamera.updateProjectionMatrix();

  return {
    sdfColorTex,
    clearNode,
    blitMesh,
    blitMaterial,
    blitScene,
    blitCamera,
    cameraBuf,
    cameraData,
    width,
    height,
  };
}

// ── Per-body compute kernel ────────────────────────────────────────

export function createComputeBody(
  pipeline: ComputePipelineState,
  opcodes: Float32Array,
  bodyIndex: number,
): ComputeBodyHandle {
  const opcodeData = new Float32Array(VM_MAX_OPCODES);
  opcodeData.set(
    opcodes.subarray(0, Math.min(opcodes.length, VM_MAX_OPCODES)),
  );
  const opcodeBuf = uniformArray(opcodeData, "float");

  const [cr, cg, cb] = BODY_COLORS[bodyIndex % BODY_COLORS.length];
  const bodyColorData = new Float32Array([cr, cg, cb]);
  const bodyColorBuf = uniformArray(bodyColorData, "float");

  const sdfEval = buildVmSdfEvalFn(opcodeBuf);
  const { width, height, sdfColorTex, cameraBuf } = pipeline;

  const marchKernel = Fn(({ ct, camBuf, colBuf }: any) => {
    const w = uint(width);
    const px = instanceIndex.mod(w);
    const py = instanceIndex.div(w);

    // Ray construction (same encoding as sdfBatchmarch.ts)
    const ndcX = px.toFloat().add(0.5).div(float(width)).mul(2).sub(1);
    const ndcY = float(1).sub(
      py.toFloat().add(0.5).div(float(height)).mul(2),
    );

    const halfFov = camBuf.element(12).mul(0.5).toVar();
    const tanHalf = halfFov.sin().div(halfFov.cos());
    const sx = ndcX.mul(tanHalf).mul(camBuf.element(13));
    const sy = ndcY.mul(tanHalf);

    const fwd = vec3(
      camBuf.element(3), camBuf.element(4), camBuf.element(5),
    );
    const right = vec3(
      camBuf.element(6), camBuf.element(7), camBuf.element(8),
    );
    const up = vec3(
      camBuf.element(9), camBuf.element(10), camBuf.element(11),
    );
    const rd = normalize(fwd.add(right.mul(sx)).add(up.mul(sy)));
    const ro = vec3(
      camBuf.element(0), camBuf.element(1), camBuf.element(2),
    );

    // Sphere tracing
    const t = float(0).toVar();
    const hit = float(0).toVar();

    Loop(int(MAX_STEPS), () => {
      const p = ro.add(rd.mul(t));
      const d = sdfEval({ p });
      If(d.lessThan(SURFACE_EPS), () => {
        hit.assign(1);
        Break();
      });
      t.addAssign(max(d.mul(0.8), float(SURFACE_EPS)));
      If(t.greaterThan(MAX_DIST), () => { Break(); });
    });

    // Shade hit pixels
    If(hit.greaterThan(0.5), () => {
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

      const keyDir = normalize(vec3(0.4, 1, 0.5));
      const fillDir = normalize(vec3(-0.6, 0.2, 0.8));
      const rimDir = normalize(vec3(0.0, -0.3, -1.0));
      const dKey = max(n.dot(keyDir), float(0));
      const dFill = max(n.dot(fillDir), float(0));
      const dRim = max(n.dot(rimDir), float(0));
      const light = dKey.mul(0.45)
        .add(dFill.mul(0.2))
        .add(dRim.mul(0.1))
        .add(0.3);

      const baseColor = vec3(
        colBuf.element(0), colBuf.element(1), colBuf.element(2),
      );
      textureStore(
        ct,
        uvec2(px, py),
        vec4(baseColor.mul(light), 1),
      ).toWriteOnly();
    });
  });

  const marchNode = marchKernel({
    ct: sdfColorTex,
    camBuf: cameraBuf,
    colBuf: bodyColorBuf,
  }).compute(width * height, [64]);

  return { opcodeBuf, opcodeData, bodyColorBuf, marchNode, bodyIndex };
}

// ── Update opcodes (data-only, no shader recompile) ────────────────

export function updateComputeBodyOpcodes(
  handle: ComputeBodyHandle,
  opcodes: Float32Array,
): void {
  handle.opcodeData.fill(0);
  handle.opcodeData.set(
    opcodes.subarray(0, Math.min(opcodes.length, VM_MAX_OPCODES)),
  );
}

// ── Update camera uniforms ─────────────────────────────────────────

export function updateComputeCamera(
  state: ComputePipelineState,
  camera: THREE.Camera,
): void {
  const cam = camera as THREE.PerspectiveCamera;
  const pos = cam.position;
  const fwd = new THREE.Vector3(0, 0, -1).applyQuaternion(cam.quaternion);
  const right = new THREE.Vector3(1, 0, 0).applyQuaternion(cam.quaternion);
  const up = new THREE.Vector3(0, 1, 0).applyQuaternion(cam.quaternion);

  const d = state.cameraData;
  d[0] = pos.x;  d[1] = pos.y;  d[2] = pos.z;
  d[3] = fwd.x;  d[4] = fwd.y;  d[5] = fwd.z;
  d[6] = right.x; d[7] = right.y; d[8] = right.z;
  d[9] = up.x;   d[10] = up.y;  d[11] = up.z;
  d[12] = cam.fov !== undefined ? (cam.fov * Math.PI) / 180 : 1.0;
  d[13] = cam.aspect !== undefined ? cam.aspect : state.width / state.height;
  d[14] = cam.near !== undefined ? cam.near : 0.1;
  d[15] = cam.far !== undefined ? cam.far : 1000;
}

// ── Run compute frame ──────────────────────────────────────────────

export function runComputeFrame(
  state: ComputePipelineState,
  bodies: ComputeBodyHandle[],
  renderer: THREE.WebGPURenderer,
): void {
  renderer.compute(state.clearNode);
  for (const body of bodies) {
    renderer.compute(body.marchNode);
  }
}

// ── Dispose ────────────────────────────────────────────────────────

export function disposeComputeBody(_handle: ComputeBodyHandle): void {
  // uniformArray Float32Arrays are GC'd; compute nodes are managed internally.
}

export function disposeComputePipeline(state: ComputePipelineState): void {
  state.sdfColorTex.dispose();
  state.blitMesh.geometry.dispose();
  state.blitMaterial.dispose();
}
