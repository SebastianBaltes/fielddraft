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
  depth,
  float,
  instanceIndex,
  int,
  max,
  normalize,
  positionGeometry,
  storage,
  uniform,
  vec3,
  vec4,
  viewZToPerspectiveDepth,
} from "three/tsl";
import { MeshBasicNodeMaterial, StorageBufferAttribute } from "three/webgpu";
import { buildVmSdfEvalFn } from "./sdfVmInterpreter";
import {
  createMaterialBuffers,
  createOpcodeBuffers,
  disposeAfterGpuIdle,
  packOpcodes,
  packMarchRelax,
  updateVolumeData,
  rebuildVolumeTextures,
  MAT_STRIDE,
  MAT_DISPLACE_DEPTH,
  MAT_DISPLACE_FREQ,
  DISPLACE_FREQ_Y,
  DISPLACE_FREQ_Z,
  shadeSdfHit,
} from "./sdfPipelineCommon";

const MAX_BODIES = 8;
const GLOBAL_MAX_OPCODES = 16384;
const MARCH_STEPS = 512;
const SURFACE_EPS = 0.002;

// ── Types ───────────────────────────────────────────────────────────

export interface VmPipelineState {
  scene: THREE.Scene;
  camera: THREE.OrthographicCamera;
  material: MeshBasicNodeMaterial | null;
  mesh: THREE.Mesh | null;
  opcodeBuf: ReturnType<typeof storage>;
  opcodeData: Float32Array;
  opcodeAttr: StorageBufferAttribute;
  offsetBuf: ReturnType<typeof storage>;
  offsetData: Float32Array;
  offsetAttr: StorageBufferAttribute;
  materialData: Float32Array;
  materialAttr: any;
  materialBuf: any;
  bodyCountU: ReturnType<typeof uniform>;
  camPosU: ReturnType<typeof uniform>;
  invProjU: ReturnType<typeof uniform>;
  invViewU: ReturnType<typeof uniform>;
  volumeTextures: THREE.Data3DTexture[];
  volumeBaseData: Float32Array;
  volumeBaseAttr: StorageBufferAttribute;
  volumeBaseBuf: ReturnType<typeof storage>;
  // Per-body conservative raymarch step factor (≤1), 1.0 = full step. The
  // march does `t += max(minD · marchRelax[closestBody], ε)` so a non-Lipschitz
  // SDF (revolve, gyroid) isn't overshot — matches the deferred path.
  marchRelaxData: Float32Array;
  marchRelaxAttr: StorageBufferAttribute;
  marchRelaxBuf: ReturnType<typeof storage>;
  viewMatU: ReturnType<typeof uniform>;
  nearU: ReturnType<typeof uniform>;
  farU: ReturnType<typeof uniform>;
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
    nearU: uniform(1),
    farU: uniform(4000),
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
    GLOBAL_MAX_OPCODES,
    MAX_BODIES,
  );
  state.opcodeAttr.needsUpdate = true;
  state.offsetAttr.needsUpdate = true;
  packMarchRelax(state.marchRelaxData, bodies, MAX_BODIES);
  state.marchRelaxAttr.needsUpdate = true;
  updateVolumeData(
    state.volumeTextures,
    state.volumeBaseData,
    bodies,
    MAX_BODIES,
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
  renderer?: THREE.WebGPURenderer,
): void {
  packOpcodes(
    state.opcodeData,
    state.offsetData,
    bodies,
    GLOBAL_MAX_OPCODES,
    MAX_BODIES,
  );
  state.opcodeAttr.needsUpdate = true;
  state.offsetAttr.needsUpdate = true;
  packMarchRelax(state.marchRelaxData, bodies, MAX_BODIES);
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
    MAX_BODIES,
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

// ── Material ────────────────────────────────────────────────────────

function createVmMaterial(state: VmPipelineState): MeshBasicNodeMaterial {
  const sdfEvalRaw = buildVmSdfEvalFn(state.opcodeBuf, {
    withBaseOffset: true,
    volumeTextures:
      state.volumeTextures.length > 0 ? state.volumeTextures : undefined,
    withVolumeBaseIdx: state.volumeTextures.length > 0,
  });

  const sdfEval = (args: {
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
    const depth = state.materialBuf.element(matBase.add(MAT_DISPLACE_DEPTH));
    const freq = state.materialBuf.element(matBase.add(MAT_DISPLACE_FREQ));
    const disp = args.p.x
      .mul(freq)
      .sin()
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
    const hit = float(0).toVar();
    const hitPos = vec3(0).toVar();
    const hitBody = int(0).toVar();
    // Distance along the ray to the nearest body surface found so far. The
    // winner per pixel is the body whose own surface is hit FIRST — i.e. the
    // body that occludes via the hardware depth buffer, exactly as two
    // separate opaque solids should.
    const bestT = float(1e30).toVar();

    // Each body is sphere-traced INDEPENDENTLY against its own SDF; we keep the
    // nearest hit across bodies.  Deliberately NOT a `min()` over all bodies'
    // SDFs in a single march — that is a boolean UNION and would fuse two
    // separate bodies that merely overlap in space into one surface (tickets
    // deeg/9ljc). Separate bodies must occlude, never merge; only an EXPLICIT
    // boolean (encoded inside a single body's opcode stream) fuses geometry.
    Loop(int(MAX_BODIES), ({ i: bodyIdx }) => {
      If(bodyIdx.lessThan(bodyCount), () => {
        const offLen = state.offsetBuf.element(bodyIdx.mul(2).add(1));
        If(int(offLen).greaterThan(int(0)), () => {
          const baseOff = int(state.offsetBuf.element(bodyIdx.mul(2)));
          const volBase = int(state.volumeBaseBuf.element(bodyIdx));
          const relax = state.marchRelaxBuf.element(bodyIdx);
          const t = float(0).toVar();

          Loop(int(MARCH_STEPS), () => {
            const p = ro.add(rd.mul(t));
            const d = sdfEval({
              p,
              baseOffset: baseOff,
              volumeBaseIdx: volBase,
              bodyIdx,
            });
            If(d.lessThan(float(SURFACE_EPS)), () => {
              // This body's surface is here — claim the pixel only if it is
              // nearer than any body already hit (depth-buffer semantics).
              If(t.lessThan(bestT), () => {
                bestT.assign(t);
                hit.assign(1);
                hitPos.assign(p);
                hitBody.assign(bodyIdx);
              });
              Break();
            });
            // Per-body conservative step (Ticket 1wnm): a non-Lipschitz SDF
            // (revolve march_relax 0.7, gyroid, …) overestimates distance and a
            // full step would tunnel through thin walls; simple primitives keep
            // 1.0. Same factor the deferred march applies.
            t.addAssign(max(d.mul(relax), float(SURFACE_EPS)));
            // No point marching past a closer body's surface, nor past the
            // camera far plane (adapted per frame, ticket lbzq).
            If(t.greaterThan(bestT).or(t.greaterThan(state.farU)), () => {
              Break();
            });
          });
        });
      });
    });

    If(hit.lessThan(float(0.5)), () => {
      Discard();
    });

    // Write per-pixel canvas depth so the B-Rep edge overlay can occlude in
    // "visible" mode. `hitPos` is the true raymarched surface point;
    // reconstruct its perspective depth via the scene camera's view matrix +
    // near/far (the fullscreen quad is drawn with an ortho camera, so the
    // rasterizer's own depth is meaningless). Misses already discarded above,
    // so they never touch depth. Harmless in X-Ray/Hidden modes — there
    // renderOverlayLayer clears depth before drawing the edges.
    // 0.1% depth bias behind the true surface so on-surface B-Rep edges
    // win the depth test instead of z-fighting the face (ticket b9xz).
    const viewZHit = state.viewMatU.mul(vec4(hitPos, 1.0)).z.mul(1.001);
    depth
      .assign(viewZToPerspectiveDepth(viewZHit, state.nearU, state.farU))
      .toStack();

    // Shade the hit (normal → PBR → wireframe) via the shared SDF pipeline.
    // The view direction is the negated ray direction (a unit vector
    // already). This single-pass renderer has no per-body selection overlay.
    return shadeSdfHit({
      p: hitPos,
      bi: hitBody,
      offsetBuf: state.offsetBuf,
      volumeBaseBuf: state.volumeBaseBuf,
      materialBuf: state.materialBuf,
      viewMatU: state.viewMatU,
      viewDirWorld: rd.negate(),
      sdfEval,
    });
  })();

  material.side = THREE.DoubleSide;
  material.depthWrite = true;
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

function updateCameraUniforms(
  state: VmPipelineState,
  camera: THREE.Camera,
): void {
  const cam = camera as THREE.PerspectiveCamera;
  if (cam.parent) cam.updateMatrixWorld();
  state.camPosU.value.setFromMatrixPosition(cam.matrixWorld);
  state.invProjU.value.copy(cam.projectionMatrixInverse);
  // invView is the camera's world matrix (the inverse of the view matrix).
  state.invViewU.value.copy(cam.matrixWorld);
  state.viewMatU.value.copy(cam.matrixWorldInverse);
  state.nearU.value = cam.near;
  state.farU.value = cam.far;
}

// ── GPU probe (dev / parity testing) ────────────────────────────────
//
// Evaluates the SAME TSL interpreter the raymarcher uses, in a compute
// pass, at arbitrary world points. This is the bridge for GPU↔CPU parity
// checks (exposed as `window.__fdSdfVmProbe`): the CPU mirror
// `eval_opcodes` is the reference, so any disagreement here is a GPU-side
// interpreter bug — the 32-iteration loop cap was exactly this class and
// only visible as a wrongly-rendered body before this hook existed.

const PROBE_CAP = 256;

interface VmProbe {
  ptsData: Float32Array;
  ptsAttr: StorageBufferAttribute;
  outAttr: StorageBufferAttribute;
  kernel: any;
}

function getProbe(state: VmPipelineState): VmProbe {
  const cached = (state as any)._probe as VmProbe | undefined;
  if (cached) return cached;
  const ptsData = new Float32Array(PROBE_CAP * 4);
  const ptsAttr = new StorageBufferAttribute(ptsData, 4);
  const outAttr = new StorageBufferAttribute(new Float32Array(PROBE_CAP), 1);
  const ptsBuf = storage(ptsAttr, "vec4", PROBE_CAP);
  const outBuf = storage(outAttr, "float", PROBE_CAP);
  const evalFn = buildVmSdfEvalFn(state.opcodeBuf, {
    withBaseOffset: true,
    volumeTextures:
      state.volumeTextures.length > 0 ? state.volumeTextures : undefined,
    withVolumeBaseIdx: state.volumeTextures.length > 0,
  });
  const kernel = Fn(() => {
    // Each point carries its body's opcode base offset in .w.
    const pt = ptsBuf.element(instanceIndex);
    const d = evalFn({
      p: pt.xyz,
      baseOffset: int(pt.w),
      volumeBaseIdx: int(0),
    });
    outBuf.element(instanceIndex).assign(d);
  })().compute(PROBE_CAP);
  const probe: VmProbe = { ptsData, ptsAttr, outAttr, kernel };
  (state as any)._probe = probe;
  return probe;
}

export async function probeVmSdf(
  state: VmPipelineState,
  renderer: THREE.WebGPURenderer,
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
  renderer?: THREE.WebGPURenderer,
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
