/**
 * Fullscreen single-pass SDF renderer using the GPU bytecode VM.
 *
 * Renders ALL bodies in one fullscreen quad with true multi-body
 * raymarching.  Serves as a simple baseline to compare against the
 * deferred pipeline.
 */

// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import {
  Break,
  Discard,
  Fn,
  If,
  Loop,
  cameraProjectionMatrix,
  float,
  int,
  max,
  min,
  normalize,
  positionGeometry,
  uniform,
  vec3,
  vec4,
  uniformArray,
} from "three/tsl";
import { MeshBasicNodeMaterial } from "three/webgpu";

import { NORMAL_EPS } from "./sdfRenderer";
import { buildVmSdfEvalFn, VM_MAX_OPCODES } from "./sdfVmInterpreter";
import { BODY_COLORS } from "./sdfDeferredRenderer";

const MAX_BODIES = 8;
const GLOBAL_MAX_OPCODES = 4096;
const MARCH_STEPS = 512;
const SURFACE_EPS = 0.002;
const MAX_MARCH_DIST = 3000;

// ── Types ───────────────────────────────────────────────────────────

export interface VmPipelineState {
  scene: THREE.Scene;
  camera: THREE.OrthographicCamera;
  material: MeshBasicNodeMaterial | null;
  mesh: THREE.Mesh | null;
  opcodeBuf: ReturnType<typeof uniformArray>;
  opcodeData: Float32Array;
  offsetBuf: ReturnType<typeof uniformArray>;
  offsetData: Float32Array;
  colorBuf: ReturnType<typeof uniformArray>;
  colorData: Float32Array;
  bodyCountU: ReturnType<typeof uniform>;
  camPosU: ReturnType<typeof uniform>;
  invProjU: ReturnType<typeof uniform>;
  invViewU: ReturnType<typeof uniform>;
  volumeTextures: THREE.Data3DTexture[];
  volumeBaseData: Float32Array;
  volumeBaseBuf: ReturnType<typeof uniformArray>;
  viewMatU: ReturnType<typeof uniform>;
}

// ── Init ────────────────────────────────────────────────────────────

export function initVmPipeline(): VmPipelineState {
  const opcodeData = new Float32Array(GLOBAL_MAX_OPCODES);
  const opcodeBuf = uniformArray(opcodeData, "float");

  const offsetData = new Float32Array(MAX_BODIES * 2);
  const offsetBuf = uniformArray(offsetData, "float");

  const colorData = new Float32Array(MAX_BODIES * 3);
  for (let i = 0; i < MAX_BODIES; i++) {
    const [r, g, b] = BODY_COLORS[i % BODY_COLORS.length];
    colorData[i * 3] = r;
    colorData[i * 3 + 1] = g;
    colorData[i * 3 + 2] = b;
  }
  const colorBuf = uniformArray(colorData, "float");

  const volumeBaseData = new Float32Array(MAX_BODIES);
  const volumeBaseBuf = uniformArray(volumeBaseData, "float");

  const scene = new THREE.Scene();
  const camera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 2);
  camera.position.set(0, 0, 1);
  camera.updateProjectionMatrix();

  return {
    scene,
    camera,
    material: null,
    mesh: null,
    opcodeBuf,
    opcodeData,
    offsetBuf,
    offsetData,
    colorBuf,
    colorData,
    bodyCountU: uniform(0),
    camPosU: uniform(new THREE.Vector3()),
    invProjU: uniform(new THREE.Matrix4()),
    invViewU: uniform(new THREE.Matrix4()),
    volumeTextures: [],
    volumeBaseData,
    volumeBaseBuf,
    viewMatU: uniform(new THREE.Matrix4()),
  };
}

// ── Update opcode data only (no shader recompile) ──────────────────

export function updateVmData(
  state: VmPipelineState,
  bodies: {
    bodyIndex: number;
    opcodes: Float32Array;
    volumes?: { data: Float32Array; dims: [number, number, number] }[];
  }[],
): void {
  state.opcodeData.fill(0);
  state.offsetData.fill(0);

  let cursor = 0;
  for (const { bodyIndex, opcodes } of bodies) {
    if (bodyIndex >= MAX_BODIES) continue;
    const len = Math.min(opcodes.length, GLOBAL_MAX_OPCODES - cursor);
    if (len <= 0) continue;
    state.opcodeData.set(opcodes.subarray(0, len), cursor);
    state.offsetData[bodyIndex * 2] = cursor;
    state.offsetData[bodyIndex * 2 + 1] = len;
    cursor += len;
  }

  state.volumeBaseData.fill(0);
  let volIdx = 0;
  for (const { bodyIndex, volumes } of bodies) {
    if (bodyIndex >= MAX_BODIES || !volumes || volumes.length === 0) continue;
    state.volumeBaseData[bodyIndex] = volIdx;
    for (let v = 0; v < volumes.length; v++) {
      const tex = state.volumeTextures[volIdx];
      if (tex) {
        (tex.image as any).data.set(volumes[v].data);
        tex.needsUpdate = true;
      }
      volIdx++;
    }
  }

  state.bodyCountU.value = bodies.length;
}

// ── Rebuild bodies (full — new material + shader compile needed) ────

export function rebuildVmBodies(
  state: VmPipelineState,
  bodies: {
    bodyIndex: number;
    opcodes: Float32Array;
    volumes?: { data: Float32Array; dims: [number, number, number] }[];
  }[],
): void {
  state.opcodeData.fill(0);
  state.offsetData.fill(0);

  let cursor = 0;
  for (const { bodyIndex, opcodes } of bodies) {
    if (bodyIndex >= MAX_BODIES) continue;
    const len = Math.min(opcodes.length, GLOBAL_MAX_OPCODES - cursor);
    if (len <= 0) continue;
    state.opcodeData.set(opcodes.subarray(0, len), cursor);
    state.offsetData[bodyIndex * 2] = cursor;
    state.offsetData[bodyIndex * 2 + 1] = len;
    cursor += len;
  }

  // Volume textures
  for (const tex of state.volumeTextures) tex.dispose();
  state.volumeTextures = [];
  state.volumeBaseData.fill(0);
  let volIdx = 0;
  for (const { bodyIndex, volumes } of bodies) {
    if (bodyIndex >= MAX_BODIES || !volumes || volumes.length === 0) continue;
    state.volumeBaseData[bodyIndex] = volIdx;
    for (const vol of volumes) {
      const tex = new THREE.Data3DTexture(
        new Float32Array(vol.data),
        vol.dims[0], vol.dims[1], vol.dims[2],
      );
      tex.format = THREE.RedFormat;
      tex.type = THREE.FloatType;
      tex.minFilter = THREE.LinearFilter;
      tex.magFilter = THREE.LinearFilter;
      tex.wrapS = THREE.ClampToEdgeWrapping;
      tex.wrapT = THREE.ClampToEdgeWrapping;
      tex.wrapR = THREE.ClampToEdgeWrapping;
      tex.needsUpdate = true;
      state.volumeTextures.push(tex);
      volIdx++;
    }
  }

  state.bodyCountU.value = bodies.length;

  if (state.material) state.material.dispose();
  if (state.mesh) state.scene.remove(state.mesh);

  state.material = createVmMaterial(state);
  state.mesh = new THREE.Mesh(new THREE.PlaneGeometry(2, 2), state.material);
  state.mesh.frustumCulled = false;
  state.scene.add(state.mesh);
}

// ── Material ────────────────────────────────────────────────────────

function createVmMaterial(state: VmPipelineState): MeshBasicNodeMaterial {
  const sdfEval = buildVmSdfEvalFn(state.opcodeBuf, {
    withBaseOffset: true,
    volumeTextures: state.volumeTextures.length > 0 ? state.volumeTextures : undefined,
    withVolumeBaseIdx: state.volumeTextures.length > 0,
  });

  const material = new MeshBasicNodeMaterial();
  material.outputNode = Fn(() => {
    // Reconstruct world-space ray from fullscreen UV
    const ndc = positionGeometry.xy;
    const clipPos = vec4(ndc, float(-1), float(1));
    const eyeDir4 = state.invProjU.mul(clipPos);
    const eyeDir = eyeDir4.xyz.div(eyeDir4.w);
    const rd = normalize(state.invViewU.mul(vec4(eyeDir, 0)).xyz);
    const ro = state.camPosU.toVar();

    const bodyCount = int(state.bodyCountU);
    const t = float(0).toVar();
    const hit = float(0).toVar();
    const hitPos = vec3(0).toVar();
    const hitBody = int(0).toVar();

    Loop(int(MARCH_STEPS), () => {
      const p = ro.add(rd.mul(t));
      const minD = float(1e30).toVar();
      const closestBody = int(0).toVar();

      Loop(int(MAX_BODIES), ({ i: bodyIdx }) => {
        If(bodyIdx.lessThan(bodyCount), () => {
          const offLen = state.offsetBuf.element(bodyIdx.mul(2).add(1));
          If(int(offLen).greaterThan(int(0)), () => {
            const baseOff = int(state.offsetBuf.element(bodyIdx.mul(2)));
            const volBase = int(state.volumeBaseBuf.element(bodyIdx));
            const d = sdfEval({ p, baseOffset: baseOff, volumeBaseIdx: volBase });
            If(d.lessThan(minD), () => {
              minD.assign(d);
              closestBody.assign(bodyIdx);
            });
          });
        });
      });

      If(minD.lessThan(float(SURFACE_EPS)), () => {
        hit.assign(1);
        hitPos.assign(p);
        hitBody.assign(closestBody);
        Break();
      });
      t.addAssign(max(minD, float(SURFACE_EPS)));
      If(t.greaterThan(float(MAX_MARCH_DIST)), () => { Break(); });
    });

    If(hit.lessThan(float(0.5)), () => { Discard(); });

    // Normal via central differences
    const eps = float(NORMAL_EPS);
    const ex = vec3(eps, 0, 0);
    const ey = vec3(0, eps, 0);
    const ez = vec3(0, 0, eps);
    const baseOff = int(state.offsetBuf.element(hitBody.mul(2)));
    const volBase = int(state.volumeBaseBuf.element(hitBody));
    const n = normalize(vec3(
      sdfEval({ p: hitPos.add(ex), baseOffset: baseOff, volumeBaseIdx: volBase })
        .sub(sdfEval({ p: hitPos.sub(ex), baseOffset: baseOff, volumeBaseIdx: volBase })),
      sdfEval({ p: hitPos.add(ey), baseOffset: baseOff, volumeBaseIdx: volBase })
        .sub(sdfEval({ p: hitPos.sub(ey), baseOffset: baseOff, volumeBaseIdx: volBase })),
      sdfEval({ p: hitPos.add(ez), baseOffset: baseOff, volumeBaseIdx: volBase })
        .sub(sdfEval({ p: hitPos.sub(ez), baseOffset: baseOff, volumeBaseIdx: volBase })),
    ));

    // 3-light model (view-space — camera-fixed)
    const nView = normalize(state.viewMatU.mul(vec4(n, 0.0)).xyz);
    const keyDir = normalize(vec3(0.2, 0.3, 1.0));
    const fillDir = normalize(vec3(-0.5, 0.1, 0.6));
    const rimDir = normalize(vec3(0.0, -0.2, -1.0));
    const dKey = max(nView.dot(keyDir), float(0));
    const dFill = max(nView.dot(fillDir), float(0));
    const dRim = max(nView.dot(rimDir), float(0));
    const light = dKey.mul(0.45).add(dFill.mul(0.2)).add(dRim.mul(0.1)).add(0.3);

    const cr = state.colorBuf.element(hitBody.mul(3));
    const cg = state.colorBuf.element(hitBody.mul(3).add(1));
    const cb = state.colorBuf.element(hitBody.mul(3).add(2));
    const bodyColor = vec3(cr, cg, cb);

    return vec4(bodyColor.mul(light), 1);
  })();

  material.side = THREE.DoubleSide;
  material.depthWrite = false;
  material.depthTest = false;
  material.toneMapped = false;

  return material;
}

// ── Compile ─────────────────────────────────────────────────────────

export async function compileVmPipeline(
  state: VmPipelineState,
  renderer: THREE.WebGPURenderer,
  camera: THREE.Camera,
): Promise<void> {
  if (!state.material) return;
  updateCameraUniforms(state, camera);
  await renderer.compileAsync(state.scene, state.camera);
}

// ── Render ──────────────────────────────────────────────────────────

export function renderVmFrame(
  state: VmPipelineState,
  renderer: THREE.WebGPURenderer,
  camera: THREE.Camera,
): void {
  if (!state.material) return;
  updateCameraUniforms(state, camera);
  renderer.render(state.scene, state.camera);
}

function updateCameraUniforms(state: VmPipelineState, camera: THREE.Camera): void {
  const cam = camera as THREE.PerspectiveCamera;
  state.camPosU.value.copy(cam.position);
  if (cam.parent) cam.updateMatrixWorld();
  state.camPosU.value.setFromMatrixPosition(cam.matrixWorld);
  state.invProjU.value.copy(cam.projectionMatrixInverse);
  state.invViewU.value.copy(cam.matrixWorldInverse).invert();
  state.viewMatU.value.copy(cam.matrixWorldInverse);
}

// ── Dispose ─────────────────────────────────────────────────────────

export function disposeVmPipeline(state: VmPipelineState): void {
  if (state.material) state.material.dispose();
  if (state.mesh) state.mesh.geometry.dispose();
  for (const tex of state.volumeTextures) tex.dispose();
}
