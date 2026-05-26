// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import { uniformArray } from "three/tsl";
import { BODY_COLORS } from "./sdfDeferredRenderer";
import { DEFAULT_ROUGHNESS } from "./pbrShading";

// ── Shared material buffer types ───────────────────────────────────

export interface MaterialBufferSet {
  colorData: Float32Array;
  colorBuf: ReturnType<typeof uniformArray>;
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
  const colorData = new Float32Array(maxBodies * 3);
  for (let i = 0; i < maxBodies; i++) {
    const [r, g, b] = BODY_COLORS[i % BODY_COLORS.length];
    colorData[i * 3] = r;
    colorData[i * 3 + 1] = g;
    colorData[i * 3 + 2] = b;
  }
  const colorBuf = uniformArray(colorData, "float");

  const roughnessData = new Float32Array(maxBodies);
  roughnessData.fill(DEFAULT_ROUGHNESS);
  const roughnessBuf = uniformArray(roughnessData, "float");

  const metalnessData = new Float32Array(maxBodies);
  const metalnessBuf = uniformArray(metalnessData, "float");

  const emissiveData = new Float32Array(maxBodies * 3);
  const emissiveBuf = uniformArray(emissiveData, "float");

  const opacityData = new Float32Array(maxBodies);
  opacityData.fill(1);
  const opacityBuf = uniformArray(opacityData, "float");

  const wireframeData = new Float32Array(maxBodies);
  const wireframeBuf = uniformArray(wireframeData, "float");

  const displaceDepthData = new Float32Array(maxBodies);
  const displaceDepthBuf = uniformArray(displaceDepthData, "float");

  const displaceFreqData = new Float32Array(maxBodies);
  const displaceFreqBuf = uniformArray(displaceFreqData, "float");

  return {
    colorData, colorBuf,
    roughnessData, roughnessBuf,
    metalnessData, metalnessBuf,
    emissiveData, emissiveBuf,
    opacityData, opacityBuf,
    wireframeData, wireframeBuf,
    displaceDepthData, displaceDepthBuf,
    displaceFreqData, displaceFreqBuf,
  };
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
