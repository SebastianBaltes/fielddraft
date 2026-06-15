// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import {
  If,
  Loop,
  abs,
  clamp,
  cos,
  cross,
  float,
  fract,
  int,
  max,
  min,
  mix,
  normalize,
  select,
  sin,
  smoothstep,
  storage,
  vec3,
  vec4,
} from "three/tsl";
import { StorageBufferAttribute } from "three/webgpu";
import { BODY_COLORS } from "./sdfDeferredRenderer";
import { DEFAULT_ROUGHNESS, pbrShade } from "./pbrShading";
import { NORMAL_EPS } from "./sdfRenderer";

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
  opcodeAttr: StorageBufferAttribute;
  opcodeBuf: ReturnType<typeof storage>;
  offsetData: Float32Array;
  offsetAttr: StorageBufferAttribute;
  offsetBuf: ReturnType<typeof storage>;
  volumeBaseData: Float32Array;
  volumeBaseAttr: StorageBufferAttribute;
  volumeBaseBuf: ReturnType<typeof storage>;
  // Per-body conservative raymarch step factor (≤1), 1.0 = full step. The
  // renderer does `t += max(minD · marchRelax[closestBody], ε)` so a thin-walled
  // revolve/shell isn't overshot at grazing angles (Ticket 1wnm).
  marchRelaxData: Float32Array;
  marchRelaxAttr: StorageBufferAttribute;
  marchRelaxBuf: ReturnType<typeof storage>;
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
  const opcodeAttr = new StorageBufferAttribute(maxOpcodes, 1);
  const opcodeData = opcodeAttr.array as Float32Array;
  const opcodeBuf = storage(opcodeAttr, "float", maxOpcodes).toReadOnly();

  const offsetCount = maxBodies * 2;
  const offsetAttr = new StorageBufferAttribute(offsetCount, 1);
  const offsetData = offsetAttr.array as Float32Array;
  const offsetBuf = storage(offsetAttr, "float", offsetCount).toReadOnly();

  const volumeBaseAttr = new StorageBufferAttribute(maxBodies, 1);
  const volumeBaseData = volumeBaseAttr.array as Float32Array;
  const volumeBaseBuf = storage(volumeBaseAttr, "float", maxBodies).toReadOnly();

  const marchRelaxAttr = new StorageBufferAttribute(maxBodies, 1);
  const marchRelaxData = marchRelaxAttr.array as Float32Array;
  marchRelaxData.fill(1); // full step until a body declares a conservative factor
  const marchRelaxBuf = storage(marchRelaxAttr, "float", maxBodies).toReadOnly();

  return {
    opcodeData, opcodeAttr, opcodeBuf,
    offsetData, offsetAttr, offsetBuf,
    volumeBaseData, volumeBaseAttr, volumeBaseBuf,
    marchRelaxData, marchRelaxAttr, marchRelaxBuf,
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

// ── Ambient occlusion ──────────────────────────────────────────────
//
// Hemisphere AO over the SDF. A single ray along the normal cannot
// darken WIDE cavities (straight out of a vase cavity is free space —
// the occlusion comes from the walls all around), so we sample four
// directions: the normal plus three rays tilted ~53° towards it,
// spread 120° apart in azimuth. Tap distances are in model units (mm)
// and reach feature scale so concave interiors read as interiors —
// without AO a shell cavity is indistinguishable from a flat lid.
// Per-tap weights sum to 1 PER RAY, and ray contributions add up
// without renormalizing: a fully open hemisphere scores 0, while a
// single fully-blocked ray already darkens by SDF_AO_STRENGTH·1.
// Dividing by the ray count instead would need every ray of the
// hemisphere blocked for full effect — real cavities (vase floor:
// occ ≈ 0.4) would never get past a ~10% dim.
export const SDF_AO_TAPS: [number, number][] = [
  [4, 0.3],
  [12, 0.35],
  [36, 0.35],
];
export const SDF_AO_STRENGTH = 0.85;
export const SDF_AO_FLOOR = 0.2;

// ── Shared hit shading ─────────────────────────────────────────────

/**
 * Shade a raymarched SDF surface hit: central-difference normal → PBR lighting
 * → selection tint and wireframe-grid alpha. Emits its nodes into the calling
 * `Fn()` body (it uses `If`), and returns the final `vec4(color, alpha)`.
 * Shared by the single-pass VM renderer and the deferred renderer so the
 * lighting/material/wireframe logic lives in one place.
 *
 * @param p              — world-space hit position (vec3 node)
 * @param bi             — body index (int node)
 * @param offsetBuf      — per-body opcode (offset,length) storage buffer
 * @param volumeBaseBuf  — per-body volume-texture base-index storage buffer
 * @param materialBuf    — per-body material storage buffer (MAT_* layout)
 * @param viewMatU       — view-matrix uniform (world → view)
 * @param viewDirWorld   — world-space direction toward the camera (vec3 node)
 * @param sdfEval        — compiled SDF eval node-fn
 * @param selectionBuf   — optional per-body selection-flag buffer; when given,
 *                         selected bodies (>0.5) get a blue highlight tint
 */
export function shadeSdfHit({
  p,
  bi,
  offsetBuf,
  volumeBaseBuf,
  materialBuf,
  viewMatU,
  viewDirWorld,
  sdfEval,
  selectionBuf,
}: any) {
  const baseOff = int(offsetBuf.element(bi.mul(2)));
  const volBase = int(volumeBaseBuf.element(bi));

  // Normal via central differences.
  const eps = float(NORMAL_EPS);
  const ex = vec3(eps, 0, 0);
  const ey = vec3(0, eps, 0);
  const ez = vec3(0, 0, eps);
  const n = normalize(vec3(
    sdfEval({ p: p.add(ex), baseOffset: baseOff, volumeBaseIdx: volBase, bodyIdx: bi })
      .sub(sdfEval({ p: p.sub(ex), baseOffset: baseOff, volumeBaseIdx: volBase, bodyIdx: bi })),
    sdfEval({ p: p.add(ey), baseOffset: baseOff, volumeBaseIdx: volBase, bodyIdx: bi })
      .sub(sdfEval({ p: p.sub(ey), baseOffset: baseOff, volumeBaseIdx: volBase, bodyIdx: bi })),
    sdfEval({ p: p.add(ez), baseOffset: baseOff, volumeBaseIdx: volBase, bodyIdx: bi })
      .sub(sdfEval({ p: p.sub(ez), baseOffset: baseOff, volumeBaseIdx: volBase, bodyIdx: bi })),
  ));

  const nView = normalize(viewMatU.mul(vec4(n, 0.0)).xyz);
  const viewDir = normalize(viewMatU.mul(vec4(viewDirWorld, 0.0)).xyz);

  const matBase = bi.mul(MAT_STRIDE);
  const cr = materialBuf.element(matBase.add(MAT_COLOR_R));
  const cg = materialBuf.element(matBase.add(MAT_COLOR_G));
  const cb = materialBuf.element(matBase.add(MAT_COLOR_B));
  const baseColor = vec3(cr, cg, cb);
  const roughness = materialBuf.element(matBase.add(MAT_ROUGHNESS));
  const metalness = materialBuf.element(matBase.add(MAT_METALNESS));
  const er = materialBuf.element(matBase.add(MAT_EMISSIVE_R));
  const eg = materialBuf.element(matBase.add(MAT_EMISSIVE_G));
  const eb = materialBuf.element(matBase.add(MAT_EMISSIVE_B));
  const emissive = vec3(er, eg, eb);
  const color = pbrShade({ nView, viewDir, baseColor, roughness, metalness, emissive }).toVar();

  // Hemisphere AO (see SDF_AO_TAPS): accumulate, per ray and tap, how
  // far the SDF falls short of the UNOCCLUDED expectation. Over an
  // infinite flat surface a ray tilted by θ from the normal sees
  // sdf = dist·cos(θ), not dist — so that (not dist itself) is the
  // zero-occlusion baseline; comparing against dist would darken
  // every flat surface by a constant ~40% and wash out the cavity
  // contrast this pass exists for.
  //
  // Both loops MUST be runtime TSL Loops so there is exactly ONE
  // sdfEval call site here: every call site inlines the entire VM
  // interpreter into the shader, and unrolling 4×3 copies blows the
  // WGSL module up far enough to hang the shader compiler (observed
  // as "WebGPU Device Lost" mid-compile on the vase).
  const helper = select(abs(n.y).greaterThan(float(0.9)), vec3(1, 0, 0), vec3(0, 1, 0));
  const t1 = normalize(cross(n, helper));
  const t2 = cross(n, t1);
  const occ = float(0).toVar();
  const rayCount = 4;
  Loop(int(rayCount), ({ i: ray }) => {
    const az = float(ray.sub(int(1))).mul((2 * Math.PI) / 3);
    const tilted = normalize(
      n.mul(0.6).add(t1.mul(cos(az).mul(0.8))).add(t2.mul(sin(az).mul(0.8))),
    );
    const dir = select(ray.equal(int(0)), n, tilted).toVar();
    Loop(int(SDF_AO_TAPS.length), ({ i: tap }) => {
      const [dist, weight] = SDF_AO_TAPS.reduceRight(
        ([accDist, accWeight], [tDist, tWeight], idx) => [
          select(tap.equal(int(idx)), float(tDist), accDist),
          select(tap.equal(int(idx)), float(tWeight), accWeight),
        ],
        [float(0), float(0)],
      );
      const d = sdfEval({
        p: p.add(dir.mul(dist)),
        baseOffset: baseOff,
        volumeBaseIdx: volBase,
        bodyIdx: bi,
      });
      const expected = dist.mul(dir.dot(n));
      occ.addAssign(clamp(expected.sub(d).div(expected), float(0), float(1)).mul(weight));
    });
  });
  const ao = clamp(
    float(1).sub(occ.mul(SDF_AO_STRENGTH)),
    float(SDF_AO_FLOOR),
    float(1),
  );
  color.assign(color.mul(ao));

  if (selectionBuf) {
    const sel = selectionBuf.element(bi);
    If(sel.greaterThan(float(0.5)), () => {
      color.assign(mix(color, vec3(0.541, 0.706, 0.973), float(0.35)));
    });
  }

  const bodyOpacity = materialBuf.element(matBase.add(MAT_OPACITY));
  const wf = materialBuf.element(matBase.add(MAT_WIREFRAME));
  const alpha = bodyOpacity.toVar();
  If(wf.greaterThan(float(0.5)), () => {
    const fresnel = float(1).sub(abs(n.dot(viewDirWorld)));
    const edgeFactor = smoothstep(float(FRESNEL_EDGE_MIN), float(FRESNEL_EDGE_MAX), fresnel);
    const gridSize = float(WIREFRAME_GRID_SIZE);
    const gx = min(fract(p.x.div(gridSize)), float(1).sub(fract(p.x.div(gridSize))));
    const gy = min(fract(p.y.div(gridSize)), float(1).sub(fract(p.y.div(gridSize))));
    const gz = min(fract(p.z.div(gridSize)), float(1).sub(fract(p.z.div(gridSize))));
    const minGrid = min(gx, min(gy, gz));
    const gridAlpha = float(1).sub(smoothstep(float(0), float(WIREFRAME_GRID_THRESHOLD), minGrid));
    alpha.assign(max(edgeFactor, gridAlpha).mul(bodyOpacity));
  });

  return vec4(color, alpha);
}
