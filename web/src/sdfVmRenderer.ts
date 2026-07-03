/**
 * Fullscreen single-pass SDF renderer using the GPU bytecode VM.
 *
 * Renders ALL bodies in one fullscreen quad with true multi-body
 * raymarching.  Serves as a simple baseline to compare against the
 * deferred pipeline. The TSL shader builders live in
 * sdfVmRenderer.shaders.ts (@ts-nocheck); this file is fully typed.
 */

import * as THREE from "three";
import { uniform } from "three/tsl";
import {
  StorageBufferAttribute,
  type MeshBasicNodeMaterial,
  type StorageBufferNode,
  type UniformNode,
  type WebGPURenderer,
} from "three/webgpu";
import {
  createMaterialBuffers,
  createOpcodeBuffers,
  createCameraUniforms,
  updateCameraUniforms,
  disposeAfterGpuIdle,
  packOpcodes,
  packMarchRelax,
  updateVolumeData,
  rebuildVolumeTextures,
  SDF_MAX_OPCODES,
  type CameraUniformSet,
} from "./sdfPipelineCommon";
import {
  VM_MAX_BODIES,
  createVmMaterial,
  getProbe,
  PROBE_CAP,
} from "./sdfVmRenderer.shaders";

// ── Types ───────────────────────────────────────────────────────────

export interface VmPipelineState extends CameraUniformSet {
  scene: THREE.Scene;
  camera: THREE.OrthographicCamera;
  material: MeshBasicNodeMaterial | null;
  mesh: THREE.Mesh | null;
  opcodeBuf: StorageBufferNode<"float">;
  opcodeData: Float32Array;
  opcodeAttr: StorageBufferAttribute;
  offsetBuf: StorageBufferNode<"float">;
  offsetData: Float32Array;
  offsetAttr: StorageBufferAttribute;
  materialData: Float32Array;
  materialAttr: StorageBufferAttribute;
  materialBuf: StorageBufferNode<"float">;
  bodyCountU: UniformNode<"float", number>;
  volumeTextures: THREE.Data3DTexture[];
  volumeBaseData: Float32Array;
  volumeBaseAttr: StorageBufferAttribute;
  volumeBaseBuf: StorageBufferNode<"float">;
  // Per-body conservative raymarch step factor (≤1), 1.0 = full step. The
  // march does `t += max(minD · marchRelax[closestBody], ε)` so a non-Lipschitz
  // SDF (revolve, gyroid) isn't overshot — matches the deferred path.
  marchRelaxData: Float32Array;
  marchRelaxAttr: StorageBufferAttribute;
  marchRelaxBuf: StorageBufferNode<"float">;
}

// ── Init ────────────────────────────────────────────────────────────

export function initVmPipeline(): VmPipelineState {
  const matBufs = createMaterialBuffers(VM_MAX_BODIES);
  const opBufs = createOpcodeBuffers(SDF_MAX_OPCODES, VM_MAX_BODIES);

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
    ...createCameraUniforms(),
    bodyCountU: uniform(0),
  };
}

// ── Update opcode data only (no shader recompile) ──────────────────

export function updateVmData(
  state: VmPipelineState,
  bodies: {
    bodyIndex: number;
    opcodes: Float32Array;
    marchRelax?: number;
    volumes?: { data: Float32Array; dims: [number, number, number] }[];
  }[],
): void {
  packOpcodes(
    state.opcodeData,
    state.offsetData,
    bodies,
    SDF_MAX_OPCODES,
    VM_MAX_BODIES,
  );
  state.opcodeAttr.needsUpdate = true;
  state.offsetAttr.needsUpdate = true;
  packMarchRelax(state.marchRelaxData, bodies, VM_MAX_BODIES);
  state.marchRelaxAttr.needsUpdate = true;
  updateVolumeData(
    state.volumeTextures,
    state.volumeBaseData,
    bodies,
    VM_MAX_BODIES,
  );
  state.volumeBaseAttr.needsUpdate = true;
  state.bodyCountU.value = bodies.length;
}

// ── Rebuild bodies (full — new material + shader compile needed) ────

export function rebuildVmBodies(
  state: VmPipelineState,
  bodies: {
    bodyIndex: number;
    opcodes: Float32Array;
    marchRelax?: number;
    volumes?: { data: Float32Array; dims: [number, number, number] }[];
  }[],
  renderer?: WebGPURenderer,
): void {
  packOpcodes(
    state.opcodeData,
    state.offsetData,
    bodies,
    SDF_MAX_OPCODES,
    VM_MAX_BODIES,
  );
  state.opcodeAttr.needsUpdate = true;
  state.offsetAttr.needsUpdate = true;
  packMarchRelax(state.marchRelaxData, bodies, VM_MAX_BODIES);
  state.marchRelaxAttr.needsUpdate = true;

  // Ticket fg9o: capture the resources this rebuild retires and dispose them
  // only once the GPU confirms it's done with every in-flight submit that may
  // still reference them (disposeAfterGpuIdle). Destroying them synchronously
  // raced the previous frame's submit → "Buffer used in submit while destroyed"
  // → frozen device.
  const oldTextures = state.volumeTextures;
  const oldMaterial = state.material;
  const oldMesh = state.mesh;

  const { textures } = rebuildVolumeTextures(
    state.volumeTextures,
    state.volumeBaseData,
    bodies,
    VM_MAX_BODIES,
    false, // defer texture disposal until the GPU is idle (see below)
  );
  state.volumeTextures = textures;
  state.volumeBaseAttr.needsUpdate = true;
  state.bodyCountU.value = bodies.length;

  if (oldMesh) state.scene.remove(oldMesh);
  disposeAfterGpuIdle(renderer, () => {
    oldMaterial?.dispose();
    oldMesh?.geometry.dispose();
    for (const tex of oldTextures) tex.dispose();
  });

  state.material = createVmMaterial(state);
  state.mesh = new THREE.Mesh(new THREE.PlaneGeometry(2, 2), state.material);
  state.mesh.frustumCulled = false;
  state.scene.add(state.mesh);
}

// ── Compile ─────────────────────────────────────────────────────────

export async function compileVmPipeline(
  state: VmPipelineState,
  renderer: WebGPURenderer,
  camera: THREE.Camera,
): Promise<void> {
  if (!state.material) return;
  updateCameraUniforms(state, camera);
  await renderer.compileAsync(state.scene, state.camera);
}

// ── Render ──────────────────────────────────────────────────────────

export function renderVmFrame(
  state: VmPipelineState,
  renderer: WebGPURenderer,
  camera: THREE.Camera,
): void {
  if (!state.material) return;
  updateCameraUniforms(state, camera);
  renderer.render(state.scene, state.camera);
}

// ── GPU probe (dev / parity testing) ────────────────────────────────
//
// Evaluates the SAME TSL interpreter the raymarcher uses, in a compute
// pass, at arbitrary world points. This is the bridge for GPU↔CPU parity
// checks (exposed as `window.__fdSdfVmProbe`): the CPU mirror
// `eval_opcodes` is the reference, so any disagreement here is a GPU-side
// interpreter bug — the 32-iteration loop cap was exactly this class and
// only visible as a wrongly-rendered body before this hook existed.

export async function probeVmSdf(
  state: VmPipelineState,
  renderer: WebGPURenderer,
  points: [number, number, number][],
  bodyIndex = 0,
): Promise<number[]> {
  const probe = getProbe(state);
  const base = state.offsetData[bodyIndex * 2];
  const out: number[] = [];
  for (let off = 0; off < points.length; off += PROBE_CAP) {
    const chunk = points.slice(off, off + PROBE_CAP);
    probe.ptsData.fill(0);
    chunk.forEach((p, i) => probe.ptsData.set([p[0], p[1], p[2], base], i * 4));
    probe.ptsAttr.needsUpdate = true;
    await renderer.computeAsync(probe.kernel);
    const buf = await renderer.getArrayBufferAsync(probe.outAttr);
    out.push(...Array.from(new Float32Array(buf)).slice(0, chunk.length));
  }
  return out;
}

// ── Dispose ─────────────────────────────────────────────────────────

export function disposeVmPipeline(
  state: VmPipelineState,
  renderer?: WebGPURenderer,
): void {
  // Ticket 323i (mirrors the deferred teardown): defer the GPU `.dispose()`
  // until the device is done with in-flight submits so tearing the pipeline
  // down (render-mode switch / remount) can't race the last frame's submit.
  const material = state.material;
  const geometry = state.mesh?.geometry;
  const textures = state.volumeTextures;
  disposeAfterGpuIdle(renderer, () => {
    material?.dispose();
    geometry?.dispose();
    for (const tex of textures) tex.dispose();
  });
}
