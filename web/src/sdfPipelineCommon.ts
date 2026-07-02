// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import {
  Break,
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
  depth,
  storage,
  uniform,
  vec3,
  vec4,
  viewZToOrthographicDepth,
  viewZToPerspectiveDepth,
} from "three/tsl";
import { StorageBufferAttribute } from "three/webgpu";
import { BODY_COLORS } from "./bodyColors";
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

// ── Shared shader constants ────────────────────────────────────────

export const WIREFRAME_GRID_SIZE = 10;
export const WIREFRAME_GRID_THRESHOLD = 0.03;
export const FRESNEL_EDGE_MIN = 0.65;
export const FRESNEL_EDGE_MAX = 0.85;
export const DISPLACE_FREQ_Y = 1.3;
export const DISPLACE_FREQ_Z = 0.7;

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
  camPosU: ReturnType<typeof uniform>;
  /** World-space camera forward (−Z) direction. In ORTHOGRAPHIC mode this is
   *  the SHARED parallel ray direction for every pixel; the along-ray distance
   *  of a world point is `dot(p − camPos, camFwd)`. Unused in perspective (rays
   *  diverge from camPos and the radial distance is the ray parameter). */
  camFwdU: ReturnType<typeof uniform>;
  invProjU: ReturnType<typeof uniform>;
  invViewU: ReturnType<typeof uniform>;
  viewMatU: ReturnType<typeof uniform>;
  nearU: ReturnType<typeof uniform>;
  farU: ReturnType<typeof uniform>;
  /** 1.0 when the scene camera is orthographic, 0.0 perspective — selects the
   *  parallel-vs-converging march ray AND the linear-vs-reciprocal depth
   *  formula when writing canvas depth. */
  isOrthoU: ReturnType<typeof uniform>;
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

// ── Shared TSL building blocks ─────────────────────────────────────

/**
 * Wrap a raw VM SDF eval with the per-body sine displacement from the
 * material buffer (MAT_DISPLACE_DEPTH/FREQ). Callers that pass no `bodyIdx`
 * get the raw distance — displacement is a per-body material property.
 * Identical wrapper for both pipelines (VM material + deferred shade pass).
 */
export function makeDisplacedSdfEval(sdfEvalRaw: any, materialBuf: any) {
  return (args: {
    p: any;
    baseOffset: any;
    volumeBaseIdx: any;
    bodyIdx?: any;
  }) => {
    const d = sdfEvalRaw({
      p: args.p,
      baseOffset: args.baseOffset,
      volumeBaseIdx: args.volumeBaseIdx,
    });
    if (!args.bodyIdx) return d;
    const matBase = args.bodyIdx.mul(MAT_STRIDE);
    const dispDepth = materialBuf.element(matBase.add(MAT_DISPLACE_DEPTH));
    const freq = materialBuf.element(matBase.add(MAT_DISPLACE_FREQ));
    const disp = args.p.x
      .mul(freq)
      .sin()
      .mul(args.p.y.mul(freq).mul(DISPLACE_FREQ_Y).sin())
      .mul(args.p.z.mul(freq).mul(DISPLACE_FREQ_Z).sin());
    return d.sub(dispDepth.mul(disp));
  };
}

/** Depth written for a raymarched hit sits 0.1% of viewZ BEHIND the true
 *  surface: the B-Rep edges lie EXACTLY on the surface, so without the bias
 *  they z-fight the face and render ragged/washed-out in "visible" edge mode
 *  (ticket b9xz). */
export const DEPTH_EDGE_BIAS = 1.001;

/**
 * Write per-pixel canvas depth for a raymarched world-space hit, so the
 * B-Rep edge overlay can occlude in "visible" mode. Both SDF pipelines draw
 * their final shading with an ortho fullscreen quad, so the rasterizer's own
 * depth is meaningless — the depth is reconstructed from the SCENE camera's
 * view matrix + near/far instead. Perspective depth is reciprocal in view-Z,
 * orthographic is linear; `isOrthoU` picks the matching encoder. Emits into
 * the calling `Fn()`/`If()` scope.
 */
export function emitSceneDepthWrite({
  hitPos,
  viewMatU,
  nearU,
  farU,
  isOrthoU,
}: any): void {
  const viewZ = viewMatU.mul(vec4(hitPos, 1.0)).z.mul(DEPTH_EDGE_BIAS);
  depth
    .assign(
      select(
        isOrthoU.greaterThan(float(0.5)),
        viewZToOrthographicDepth(viewZ, nearU, farU),
        viewZToPerspectiveDepth(viewZ, nearU, farU),
      ),
    )
    .toStack();
}

/**
 * Emit the SINGLE adaptive union sphere-trace both SDF pipelines march with:
 * each step lets `evalBodies` evaluate every candidate body's own SDF at the
 * current point and keep the SMALLEST distance (`minD`) plus which body
 * produced it (`closestBody`); the ray then steps by that conservative
 * distance, scaled by the per-body `marchRelax` factor (Ticket 1wnm: a
 * thin-walled revolve/shell declares < 1 so grazing rays don't overshoot its
 * wall; simple primitives keep 1.0). On a hit the pixel belongs to
 * `hitBody` — the body whose own surface is nearest along the ray, exactly
 * the depth-buffer occlusion two separate opaque solids must have.
 *
 * NOTE on the "implicit boolean" concern (tickets deeg/9ljc): for OPAQUE
 * primary visibility this union march and a per-body nearest-hit march
 * produce the SAME image — both show the nearest surface and record
 * `closestBody`, so separate solids occlude, they do NOT fuse. The earlier
 * per-body NESTED march (Loop(MAX_BODIES){Loop(MARCH_STEPS)}, commit
 * e4e5727) gave no visual benefit but sliced bodies (each body got its own
 * capped step budget and a shared `bestT` early-out cut marches short) and
 * — in the deferred renderer — exploded the shader so it never finished
 * compiling. Reverted to this single union march (ticket 6yin).
 *
 * @param ro          ray origin (vec3 node)
 * @param rd          ray direction (vec3 node)
 * @param tStart      initial ray parameter (number; deferred backs off 1 mm)
 * @param steps       max iterations (number, SDF_MARCH_STEPS)
 * @param marchRelaxBuf per-body conservative step factors (storage node)
 * @param tEnd        abort criterion: march stops once `t` exceeds this node
 *                    (VM: camera far plane; deferred: back-face exit depth)
 * @param evalBodies  `(p, minD, closestBody) => void` — emits the per-body
 *                    SDF evals of this pipeline and folds the running min
 * @returns `{ t, hit, hitPos, hitBody }` node vars for the caller's shading
 */
export function emitUnionMarch({
  ro,
  rd,
  tStart,
  steps,
  marchRelaxBuf,
  tEnd,
  evalBodies,
}: any): { t: any; hit: any; hitPos: any; hitBody: any } {
  const t = float(tStart).toVar();
  const hit = float(0).toVar();
  const hitPos = vec3(0, 0, 0).toVar();
  const hitBody = int(0).toVar();
  Loop(int(steps), () => {
    const p = ro.add(rd.mul(t));
    const minD = float(1e30).toVar();
    const closestBody = int(0).toVar();
    evalBodies(p, minD, closestBody);
    If(minD.lessThan(float(SURFACE_EPS)), () => {
      hit.assign(1);
      hitPos.assign(p);
      hitBody.assign(closestBody);
      Break();
    });
    t.addAssign(
      max(minD.mul(marchRelaxBuf.element(closestBody)), float(SURFACE_EPS)),
    );
    If(t.greaterThan(tEnd), () => {
      Break();
    });
  });
  return { t, hit, hitPos, hitBody };
}

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
// Finer, more closely-spaced tap distances than the original [4,12,36]: with
// only 3 taps the per-tap occlusion contribution switches on/off at discrete
// depths as a ray starts/stops hitting the opposite bore wall or the concave
// rim, quantising a smooth cylindrical bore into visible horizontal AO bands
// (ticket km43). More taps spread those transitions out into a smooth gradient.
// Cheap: the AO loop is a single runtime `Loop` (one sdfEval call site), so
// extra taps are just loop iterations, NOT extra inlined VM copies. Weights
// sum to 1 per ray.
export const SDF_AO_TAPS: [number, number][] = [
  [3, 0.12],
  [6, 0.16],
  [11, 0.18],
  [18, 0.18],
  [28, 0.18],
  [44, 0.18],
];
export const SDF_AO_STRENGTH = 0.85;
// Brighter cavity floor (was 0.2): the floor multiplies the ambient term too,
// so 0.2 drove deep bores to ~ambient·0.2 ≈ 5% → near black (ticket km43). 0.5
// keeps cavities readable while still reading as interiors.
export const SDF_AO_FLOOR = 0.5;

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
  const n = normalize(
    vec3(
      sdfEval({
        p: p.add(ex),
        baseOffset: baseOff,
        volumeBaseIdx: volBase,
        bodyIdx: bi,
      }).sub(
        sdfEval({
          p: p.sub(ex),
          baseOffset: baseOff,
          volumeBaseIdx: volBase,
          bodyIdx: bi,
        }),
      ),
      sdfEval({
        p: p.add(ey),
        baseOffset: baseOff,
        volumeBaseIdx: volBase,
        bodyIdx: bi,
      }).sub(
        sdfEval({
          p: p.sub(ey),
          baseOffset: baseOff,
          volumeBaseIdx: volBase,
          bodyIdx: bi,
        }),
      ),
      sdfEval({
        p: p.add(ez),
        baseOffset: baseOff,
        volumeBaseIdx: volBase,
        bodyIdx: bi,
      }).sub(
        sdfEval({
          p: p.sub(ez),
          baseOffset: baseOff,
          volumeBaseIdx: volBase,
          bodyIdx: bi,
        }),
      ),
    ),
  );

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
  const color = pbrShade({
    nView,
    viewDir,
    baseColor,
    roughness,
    metalness,
    emissive,
  }).toVar();

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
  const helper = select(
    abs(n.y).greaterThan(float(0.9)),
    vec3(1, 0, 0),
    vec3(0, 1, 0),
  );
  const t1 = normalize(cross(n, helper));
  const t2 = cross(n, t1);
  const occ = float(0).toVar();
  const rayCount = 4;
  Loop(int(rayCount), ({ i: ray }) => {
    const az = float(ray.sub(int(1))).mul((2 * Math.PI) / 3);
    const tilted = normalize(
      n
        .mul(0.6)
        .add(t1.mul(cos(az).mul(0.8)))
        .add(t2.mul(sin(az).mul(0.8))),
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
      occ.addAssign(
        clamp(expected.sub(d).div(expected), float(0), float(1)).mul(weight),
      );
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
    const edgeFactor = smoothstep(
      float(FRESNEL_EDGE_MIN),
      float(FRESNEL_EDGE_MAX),
      fresnel,
    );
    const gridSize = float(WIREFRAME_GRID_SIZE);
    const gx = min(
      fract(p.x.div(gridSize)),
      float(1).sub(fract(p.x.div(gridSize))),
    );
    const gy = min(
      fract(p.y.div(gridSize)),
      float(1).sub(fract(p.y.div(gridSize))),
    );
    const gz = min(
      fract(p.z.div(gridSize)),
      float(1).sub(fract(p.z.div(gridSize))),
    );
    const minGrid = min(gx, min(gy, gz));
    const gridAlpha = float(1).sub(
      smoothstep(float(0), float(WIREFRAME_GRID_THRESHOLD), minGrid),
    );
    alpha.assign(max(edgeFactor, gridAlpha).mul(bodyOpacity));
  });

  return vec4(color, alpha);
}
