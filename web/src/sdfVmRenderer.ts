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
  abs,
  float,
  fract,
  int,
  max,
  min,
  normalize,
  positionGeometry,
  smoothstep,
  uniform,
  vec3,
  vec4,
  uniformArray,
} from "three/tsl";
import { MeshBasicNodeMaterial } from "three/webgpu";
import { pbrShade } from "./pbrShading";

import { NORMAL_EPS } from "./sdfRenderer";
import { buildVmSdfEvalFn, VM_MAX_OPCODES } from "./sdfVmInterpreter";
import {
  createMaterialBuffers,
  createOpcodeBuffers,
  packOpcodes,
  updateVolumeData,
  rebuildVolumeTextures,
  createVolumeTexture,
  WIREFRAME_GRID_SIZE,
  WIREFRAME_GRID_THRESHOLD,
  FRESNEL_EDGE_MIN,
  FRESNEL_EDGE_MAX,
  DISPLACE_FREQ_Y,
  DISPLACE_FREQ_Z,
} from "./sdfPipelineCommon";

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
  roughnessData: Float32Array;
  roughnessBuf: ReturnType<typeof uniformArray>;
  metalnessData: Float32Array;
  metalnessBuf: ReturnType<typeof uniformArray>;
  emissiveData: Float32Array;
  emissiveBuf: ReturnType<typeof uniformArray>;
  opacityData: Float32Array;
  opacityBuf: ReturnType<typeof uniformArray>;
  wireframeData: Float32Array;
  wireframeBuf: ReturnType<typeof uniformArray>;
  displaceDepthData: Float32Array;
  displaceDepthBuf: ReturnType<typeof uniformArray>;
  displaceFreqData: Float32Array;
  displaceFreqBuf: ReturnType<typeof uniformArray>;
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
  const matBufs = createMaterialBuffers(MAX_BODIES);
  const opBufs = createOpcodeBuffers(GLOBAL_MAX_OPCODES, MAX_BODIES);

  const scene = new THREE.Scene();
  const camera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 2);
  camera.position.set(0, 0, 1);
  camera.updateProjectionMatrix();

  return {
    scene,
    camera,
    material: null,
    mesh: null,
    ...opBufs,
    ...matBufs,
    bodyCountU: uniform(0),
    camPosU: uniform(new THREE.Vector3()),
    invProjU: uniform(new THREE.Matrix4()),
    invViewU: uniform(new THREE.Matrix4()),
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
  packOpcodes(state.opcodeData, state.offsetData, bodies, GLOBAL_MAX_OPCODES, MAX_BODIES);
  updateVolumeData(state.volumeTextures, state.volumeBaseData, bodies, MAX_BODIES);
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
  packOpcodes(state.opcodeData, state.offsetData, bodies, GLOBAL_MAX_OPCODES, MAX_BODIES);
  const { textures } = rebuildVolumeTextures(state.volumeTextures, state.volumeBaseData, bodies, MAX_BODIES);
  state.volumeTextures = textures;
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
  const sdfEvalRaw = buildVmSdfEvalFn(state.opcodeBuf, {
    withBaseOffset: true,
    volumeTextures: state.volumeTextures.length > 0 ? state.volumeTextures : undefined,
    withVolumeBaseIdx: state.volumeTextures.length > 0,
  });

  const sdfEval = (args: { p: any; baseOffset: any; volumeBaseIdx: any; bodyIdx?: any }) => {
    const d = sdfEvalRaw({ p: args.p, baseOffset: args.baseOffset, volumeBaseIdx: args.volumeBaseIdx });
    if (!args.bodyIdx) return d;
    const depth = state.displaceDepthBuf.element(args.bodyIdx);
    const freq = state.displaceFreqBuf.element(args.bodyIdx);
    const disp = args.p.x.mul(freq).sin()
      .mul(args.p.y.mul(freq).mul(DISPLACE_FREQ_Y).sin())
      .mul(args.p.z.mul(freq).mul(DISPLACE_FREQ_Z).sin());
    return d.sub(depth.mul(disp));
  };

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
            const d = sdfEval({ p, baseOffset: baseOff, volumeBaseIdx: volBase, bodyIdx });
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
    const bi = hitBody;
    const n = normalize(vec3(
      sdfEval({ p: hitPos.add(ex), baseOffset: baseOff, volumeBaseIdx: volBase, bodyIdx: bi })
        .sub(sdfEval({ p: hitPos.sub(ex), baseOffset: baseOff, volumeBaseIdx: volBase, bodyIdx: bi })),
      sdfEval({ p: hitPos.add(ey), baseOffset: baseOff, volumeBaseIdx: volBase, bodyIdx: bi })
        .sub(sdfEval({ p: hitPos.sub(ey), baseOffset: baseOff, volumeBaseIdx: volBase, bodyIdx: bi })),
      sdfEval({ p: hitPos.add(ez), baseOffset: baseOff, volumeBaseIdx: volBase, bodyIdx: bi })
        .sub(sdfEval({ p: hitPos.sub(ez), baseOffset: baseOff, volumeBaseIdx: volBase, bodyIdx: bi })),
    ));

    const nView = normalize(state.viewMatU.mul(vec4(n, 0.0)).xyz);
    const viewDir = normalize(state.viewMatU.mul(vec4(rd.negate(), 0.0)).xyz);

    const cr = state.colorBuf.element(hitBody.mul(3));
    const cg = state.colorBuf.element(hitBody.mul(3).add(1));
    const cb = state.colorBuf.element(hitBody.mul(3).add(2));
    const baseColor = vec3(cr, cg, cb);
    const roughness = state.roughnessBuf.element(hitBody);
    const metalness = state.metalnessBuf.element(hitBody);
    const er = state.emissiveBuf.element(hitBody.mul(3));
    const eg = state.emissiveBuf.element(hitBody.mul(3).add(1));
    const eb = state.emissiveBuf.element(hitBody.mul(3).add(2));
    const emissive = vec3(er, eg, eb);
    const color = pbrShade({ nView, viewDir, baseColor, roughness, metalness, emissive });

    const bodyOpacity = state.opacityBuf.element(hitBody);
    const wf = state.wireframeBuf.element(hitBody);
    const alpha = bodyOpacity.toVar();
    If(wf.greaterThan(float(0.5)), () => {
      const viewDirWorld = rd.negate();
      const fresnel = float(1).sub(abs(n.dot(viewDirWorld)));
      const edgeFactor = smoothstep(float(FRESNEL_EDGE_MIN), float(FRESNEL_EDGE_MAX), fresnel);
      const gridSize = float(WIREFRAME_GRID_SIZE);
      const gx = min(fract(hitPos.x.div(gridSize)), float(1).sub(fract(hitPos.x.div(gridSize))));
      const gy = min(fract(hitPos.y.div(gridSize)), float(1).sub(fract(hitPos.y.div(gridSize))));
      const gz = min(fract(hitPos.z.div(gridSize)), float(1).sub(fract(hitPos.z.div(gridSize))));
      const minGrid = min(gx, min(gy, gz));
      const gridAlpha = float(1).sub(smoothstep(float(0), float(WIREFRAME_GRID_THRESHOLD), minGrid));
      alpha.assign(max(edgeFactor, gridAlpha).mul(bodyOpacity));
    });

    return vec4(color, alpha);
  })();

  material.side = THREE.DoubleSide;
  material.depthWrite = false;
  material.depthTest = false;
  material.toneMapped = false;
  material.transparent = true;

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
