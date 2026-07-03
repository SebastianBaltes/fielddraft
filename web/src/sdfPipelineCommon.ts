/**
 * Shared SDF pipeline plumbing (fully typed): material/opcode buffer sets,
 * opcode packing, race-free GPU disposal, volume textures, march constants
 * and the camera uniform set. The TSL shader building blocks live in
 * sdfPipelineCommon.shaders.ts (@ts-nocheck).
 */

import * as THREE from "three";
import { storage, uniform } from "three/tsl";
import {
  StorageBufferAttribute,
  type StorageBufferNode,
  type UniformNode,
} from "three/webgpu";
import { BODY_COLORS } from "./bodyColors";
import { DEFAULT_ROUGHNESS } from "./pbrShading";

// ── Per-body material struct layout (single storage buffer) ────────

export const MAT_STRIDE = 12;
export const MAT_COLOR_R = 0;
export const MAT_COLOR_G = 1;
export const MAT_COLOR_B = 2;
export const MAT_ROUGHNESS = 3;
export const MAT_METALNESS = 4;
export const MAT_EMISSIVE_R = 5;
export const MAT_EMISSIVE_G = 6;
export const MAT_EMISSIVE_B = 7;
export const MAT_OPACITY = 8;
export const MAT_WIREFRAME = 9;
export const MAT_DISPLACE_DEPTH = 10;
export const MAT_DISPLACE_FREQ = 11;

// ── Shared material buffer types ───────────────────────────────────

export interface MaterialBufferSet {
  materialData: Float32Array;
  materialAttr: StorageBufferAttribute;
  materialBuf: StorageBufferNode<"float">;
}

export interface OpcodeBufferSet {
  opcodeData: Float32Array;
  opcodeAttr: StorageBufferAttribute;
  opcodeBuf: StorageBufferNode<"float">;
  offsetData: Float32Array;
  offsetAttr: StorageBufferAttribute;
  offsetBuf: StorageBufferNode<"float">;
  volumeBaseData: Float32Array;
  volumeBaseAttr: StorageBufferAttribute;
  volumeBaseBuf: StorageBufferNode<"float">;
  // Per-body conservative raymarch step factor (≤1), 1.0 = full step. The
  // renderer does `t += max(minD · marchRelax[closestBody], ε)` so a thin-walled
  // revolve/shell isn't overshot at grazing angles (Ticket 1wnm).
  marchRelaxData: Float32Array;
  marchRelaxAttr: StorageBufferAttribute;
  marchRelaxBuf: StorageBufferNode<"float">;
  volumeTextures: THREE.Data3DTexture[];
}

export function createMaterialBuffers(maxBodies: number): MaterialBufferSet {
  const count = maxBodies * MAT_STRIDE;
  const materialAttr = new StorageBufferAttribute(count, 1);
  const materialData = materialAttr.array as Float32Array;

  for (let i = 0; i < maxBodies; i++) {
    const base = i * MAT_STRIDE;
    const [r, g, b] = BODY_COLORS[i % BODY_COLORS.length];
    materialData[base + MAT_COLOR_R] = r;
    materialData[base + MAT_COLOR_G] = g;
    materialData[base + MAT_COLOR_B] = b;
    materialData[base + MAT_ROUGHNESS] = DEFAULT_ROUGHNESS;
    materialData[base + MAT_OPACITY] = 1;
  }

  const materialBuf = storage(materialAttr, "float", count).toReadOnly();
  return { materialData, materialAttr, materialBuf };
}

export function createOpcodeBuffers(
  maxOpcodes: number,
  maxBodies: number,
): OpcodeBufferSet {
  const opcodeAttr = new StorageBufferAttribute(maxOpcodes, 1);
  const opcodeData = opcodeAttr.array as Float32Array;
  const opcodeBuf = storage(opcodeAttr, "float", maxOpcodes).toReadOnly();

  const offsetCount = maxBodies * 2;
  const offsetAttr = new StorageBufferAttribute(offsetCount, 1);
  const offsetData = offsetAttr.array as Float32Array;
  const offsetBuf = storage(offsetAttr, "float", offsetCount).toReadOnly();

  const volumeBaseAttr = new StorageBufferAttribute(maxBodies, 1);
  const volumeBaseData = volumeBaseAttr.array as Float32Array;
  const volumeBaseBuf = storage(
    volumeBaseAttr,
    "float",
    maxBodies,
  ).toReadOnly();

  const marchRelaxAttr = new StorageBufferAttribute(maxBodies, 1);
  const marchRelaxData = marchRelaxAttr.array as Float32Array;
  marchRelaxData.fill(1); // full step until a body declares a conservative factor
  const marchRelaxBuf = storage(
    marchRelaxAttr,
    "float",
    maxBodies,
  ).toReadOnly();

  return {
    opcodeData,
    opcodeAttr,
    opcodeBuf,
    offsetData,
    offsetAttr,
    offsetBuf,
    volumeBaseData,
    volumeBaseAttr,
    volumeBaseBuf,
    marchRelaxData,
    marchRelaxAttr,
    marchRelaxBuf,
    volumeTextures: [],
  };
}

/** Write the per-body conservative raymarch step factors (default 1.0). */
export function packMarchRelax(
  marchRelaxData: Float32Array,
  bodies: { bodyIndex: number; marchRelax?: number }[],
  maxBodies: number,
): void {
  marchRelaxData.fill(1);
  for (const { bodyIndex, marchRelax } of bodies) {
    if (bodyIndex >= maxBodies) continue;
    marchRelaxData[bodyIndex] = marchRelax ?? 1;
  }
}

// ── Shared opcode packing ──────────────────────────────────────────

export function packOpcodes(
  opcodeData: Float32Array,
  offsetData: Float32Array,
  bodies: { bodyIndex: number; opcodes: Float32Array }[],
  maxOpcodes: number,
  maxBodies: number,
): Map<number, { start: number; length: number }> {
  opcodeData.fill(0);
  offsetData.fill(0);
  const ranges = new Map<number, { start: number; length: number }>();
  let cursor = 0;
  for (const { bodyIndex, opcodes } of bodies) {
    if (bodyIndex >= maxBodies) continue;
    const len = Math.min(opcodes.length, maxOpcodes - cursor);
    if (len <= 0) continue;
    opcodeData.set(opcodes.subarray(0, len), cursor);
    offsetData[bodyIndex * 2] = cursor;
    offsetData[bodyIndex * 2 + 1] = len;
    ranges.set(bodyIndex, { start: cursor, length: len });
    cursor += len;
  }
  return ranges;
}

// ── Race-free GPU resource disposal ────────────────────────────────

/**
 * Dispose GPU-backed three.js resources only AFTER the WebGPU device has
 * finished every submit queued up to *now*. Destroying a buffer/texture/material
 * that an in-flight submit still references trips WebGPU's
 * "[Buffer] used in submit while destroyed" validation error, which kills the
 * device and freezes the app (ticket fg9o).
 *
 * `queue.onSubmittedWorkDone()` resolves once all work submitted before this
 * call completes. Any work queued afterwards is built against the NEW resources,
 * so once the promise resolves the retired ones are provably unreferenced and
 * safe to destroy — regardless of how slow the raymarch shader is. This is
 * strictly correct where the earlier "defer disposal by one rebuild" heuristic
 * was not: under a heavy shader the GPU can still be mid-flight on the previous
 * frame when the next rebuild fires, so a fixed one-cycle delay still raced.
 *
 * Falls back to synchronous disposal if the device/queue isn't reachable (WebGL
 * backend, or teardown after the renderer is gone).
 */
export function disposeAfterGpuIdle(
  renderer: unknown,
  dispose: () => void,
): void {
  const queue = (renderer as any)?.backend?.device?.queue;
  if (queue && typeof queue.onSubmittedWorkDone === "function") {
    queue.onSubmittedWorkDone().then(dispose, dispose);
  } else {
    dispose();
  }
}

// ── Shared volume texture helpers ──────────────────────────────────

export function createVolumeTexture(
  data: Float32Array,
  dims: [number, number, number],
): THREE.Data3DTexture {
  const tex = new THREE.Data3DTexture(
    new Float32Array(data),
    dims[0],
    dims[1],
    dims[2],
  );
  tex.format = THREE.RedFormat;
  tex.type = THREE.FloatType;
  tex.minFilter = THREE.LinearFilter;
  tex.magFilter = THREE.LinearFilter;
  tex.wrapS = THREE.ClampToEdgeWrapping;
  tex.wrapT = THREE.ClampToEdgeWrapping;
  tex.wrapR = THREE.ClampToEdgeWrapping;
  tex.needsUpdate = true;
  return tex;
}

export interface VolumeBody {
  bodyIndex: number;
  volumes?: { data: Float32Array; dims: [number, number, number] }[];
}

export function updateVolumeData(
  volumeTextures: THREE.Data3DTexture[],
  volumeBaseData: Float32Array,
  bodies: VolumeBody[],
  maxBodies: number,
): void {
  volumeBaseData.fill(0);
  let volIdx = 0;
  for (const { bodyIndex, volumes } of bodies) {
    if (bodyIndex >= maxBodies || !volumes || volumes.length === 0) continue;
    volumeBaseData[bodyIndex] = volIdx;
    for (let v = 0; v < volumes.length; v++) {
      const tex = volumeTextures[volIdx];
      if (tex) {
        (tex.image as any).data.set(volumes[v].data);
        tex.needsUpdate = true;
      }
      volIdx++;
    }
  }
}

export function rebuildVolumeTextures(
  existingTextures: THREE.Data3DTexture[],
  volumeBaseData: Float32Array,
  bodies: VolumeBody[],
  maxBodies: number,
  // Ticket fg9o: callers that DEFER disposal (to avoid destroying a GPU
  // resource still referenced by an in-flight submit) pass false and dispose
  // the old textures themselves once the GPU is done with them.
  disposeExisting = true,
): { textures: THREE.Data3DTexture[]; countChanged: boolean } {
  const prevCount = existingTextures.length;
  if (disposeExisting) for (const tex of existingTextures) tex.dispose();

  const textures: THREE.Data3DTexture[] = [];
  volumeBaseData.fill(0);
  for (const { bodyIndex, volumes } of bodies) {
    if (bodyIndex >= maxBodies || !volumes || volumes.length === 0) continue;
    volumeBaseData[bodyIndex] = textures.length;
    for (const vol of volumes) {
      textures.push(createVolumeTexture(vol.data, vol.dims));
    }
  }
  return { textures, countChanged: textures.length !== prevCount };
}

// ── Shared march/pipeline constants ────────────────────────────────
//
// One set of march parameters for BOTH SDF pipelines (VM single-pass and
// deferred): identical hit epsilon, step budget and opcode budget. The
// per-pipeline body caps stay renderer-specific (VM_MAX_BODIES = 8,
// DEFERRED_MAX_BODIES = 256) because they size differently-shaped buffers.

/** Sphere-trace hit epsilon (mm) — a step below this distance is a surface hit. */
export const SURFACE_EPS = 0.002;
/** Max sphere-trace iterations per primary ray. */
export const SDF_MARCH_STEPS = 512;
/** Packed opcode buffer budget (floats) shared by all bodies of a pipeline. */
export const SDF_MAX_OPCODES = 16384;
/** Padding (mm) around an AABB fallback hull so the march starts outside. */
export const AABB_HULL_PAD = 0.5;

// ── Shared camera uniforms ─────────────────────────────────────────

/**
 * The camera uniforms both SDF pipelines feed their shaders. A SUPERSET by
 * design: the VM renderer additionally reads `invProjU`/`invViewU` (it
 * reconstructs the primary ray from the fullscreen UV), the deferred
 * renderer additionally reads `camFwdU` (parallel ortho march direction);
 * an unused uniform costs nothing until a shader references it.
 */
export interface CameraUniformSet {
  camPosU: UniformNode<"vec3", THREE.Vector3>;
  /** World-space camera forward (−Z) direction. In ORTHOGRAPHIC mode this is
   *  the SHARED parallel ray direction for every pixel; the along-ray distance
   *  of a world point is `dot(p − camPos, camFwd)`. Unused in perspective (rays
   *  diverge from camPos and the radial distance is the ray parameter). */
  camFwdU: UniformNode<"vec3", THREE.Vector3>;
  invProjU: UniformNode<"mat4", THREE.Matrix4>;
  invViewU: UniformNode<"mat4", THREE.Matrix4>;
  viewMatU: UniformNode<"mat4", THREE.Matrix4>;
  nearU: UniformNode<"float", number>;
  farU: UniformNode<"float", number>;
  /** 1.0 when the scene camera is orthographic, 0.0 perspective — selects the
   *  parallel-vs-converging march ray AND the linear-vs-reciprocal depth
   *  formula when writing canvas depth. */
  isOrthoU: UniformNode<"float", number>;
}

export function createCameraUniforms(): CameraUniformSet {
  return {
    camPosU: uniform(new THREE.Vector3(0, 0, 100)),
    camFwdU: uniform(new THREE.Vector3(0, 0, -1)),
    invProjU: uniform(new THREE.Matrix4()),
    invViewU: uniform(new THREE.Matrix4()),
    viewMatU: uniform(new THREE.Matrix4()),
    nearU: uniform(1),
    farU: uniform(4000),
    // 1.0 → orthographic scene camera (the default), 0.0 → perspective.
    isOrthoU: uniform(1),
  };
}

/** Refresh every camera uniform from the live scene camera (called before
 *  compile and per frame by both pipelines). */
export function updateCameraUniforms(
  u: CameraUniformSet,
  camera: THREE.Camera,
): void {
  const cam = camera as THREE.PerspectiveCamera;
  // getWorldPosition/getWorldDirection update the world-matrix chain (and,
  // being a Camera, matrixWorldInverse) before reading — robust against a
  // parented R3F camera.
  cam.getWorldPosition(u.camPosU.value);
  // World-space camera forward (−Z); the shared parallel march direction in
  // orthographic mode (ticket g2ox). getWorldDirection already normalises.
  cam.getWorldDirection(u.camFwdU.value);
  // projectionMatrixInverse is valid for both projections — it reconstructs the
  // near-plane world point either way; the shader picks parallel vs converging
  // rays from isOrthoU.
  u.invProjU.value.copy(cam.projectionMatrixInverse);
  // invView is the camera's world matrix (the inverse of the view matrix).
  u.invViewU.value.copy(cam.matrixWorld);
  u.viewMatU.value.copy(cam.matrixWorldInverse);
  // Both PerspectiveCamera and OrthographicCamera expose near/far; the depth
  // formula differs (reciprocal vs linear) and is selected by isOrthoU.
  u.nearU.value = cam.near;
  u.farU.value = cam.far;
  u.isOrthoU.value = (cam as unknown as THREE.OrthographicCamera)
    .isOrthographicCamera
    ? 1
    : 0;
}

