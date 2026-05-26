// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import { storage, uniformArray } from "three/tsl";
import { StorageBufferAttribute } from "three/webgpu";
import { BODY_COLORS } from "./sdfDeferredRenderer";
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
  materialBuf: ReturnType<typeof storage>;
}

export interface OpcodeBufferSet {
  opcodeData: Float32Array;
  opcodeBuf: ReturnType<typeof uniformArray>;
  offsetData: Float32Array;
  offsetBuf: ReturnType<typeof uniformArray>;
  volumeBaseData: Float32Array;
  volumeBaseBuf: ReturnType<typeof uniformArray>;
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

export function createOpcodeBuffers(maxOpcodes: number, maxBodies: number): OpcodeBufferSet {
  const opcodeData = new Float32Array(maxOpcodes);
  const opcodeBuf = uniformArray(opcodeData, "float");
  const offsetData = new Float32Array(maxBodies * 2);
  const offsetBuf = uniformArray(offsetData, "float");
  const volumeBaseData = new Float32Array(maxBodies);
  const volumeBaseBuf = uniformArray(volumeBaseData, "float");
  return { opcodeData, opcodeBuf, offsetData, offsetBuf, volumeBaseData, volumeBaseBuf, volumeTextures: [] };
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

// ── Shared volume texture helpers ──────────────────────────────────

export function createVolumeTexture(
  data: Float32Array,
  dims: [number, number, number],
): THREE.Data3DTexture {
  const tex = new THREE.Data3DTexture(
    new Float32Array(data),
    dims[0], dims[1], dims[2],
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
): { textures: THREE.Data3DTexture[]; countChanged: boolean } {
  const prevCount = existingTextures.length;
  for (const tex of existingTextures) tex.dispose();

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

// ── Shared shader constants ────────────────────────────────────────

export const WIREFRAME_GRID_SIZE = 10;
export const WIREFRAME_GRID_THRESHOLD = 0.03;
export const FRESNEL_EDGE_MIN = 0.65;
export const FRESNEL_EDGE_MAX = 0.85;
export const DISPLACE_FREQ_Y = 1.3;
export const DISPLACE_FREQ_Z = 0.7;

export const EDGE_EPS = 0.5;
export const EDGE_THRESH_MIN = 0.1;
export const EDGE_THRESH_MAX = 0.5;
export const EDGE_COLOR = 0.12;
