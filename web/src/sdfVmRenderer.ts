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
  select,
  vec3,
  vec4,
  viewZToOrthographicDepth,
  viewZToPerspectiveDepth,
} from "three/tsl";
import { MeshBasicNodeMaterial, StorageBufferAttribute } from "three/webgpu";
import { buildVmSdfEvalFn } from "./sdfVmInterpreter";
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
  MAT_STRIDE,
  MAT_DISPLACE_DEPTH,
  MAT_DISPLACE_FREQ,
  DISPLACE_FREQ_Y,
  DISPLACE_FREQ_Z,
  SURFACE_EPS,
  SDF_MARCH_STEPS,
  SDF_MAX_OPCODES,
  shadeSdfHit,
  type CameraUniformSet,
} from "./sdfPipelineCommon";

/** This single fullscreen march unions ALL bodies in one shader — the body
 *  cap sizes the per-pixel inner loop, so it stays small. */
const VM_MAX_BODIES = 8;

// ── Types ───────────────────────────────────────────────────────────

export interface VmPipelineState extends CameraUniformSet {
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
  renderer?: THREE.WebGPURenderer,
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
    // Reconstruct the world-space primary ray from the fullscreen UV. The world
    // point on the near plane under this pixel is the same for both
    // projections: invView · (invProj · clip), after the perspective divide.
    // NDC z of the near plane is 0 — three.js builds the projection matrices in
    // the WebGPU clip-space convention (z' ∈ [0, 1]), NOT WebGL's [-1, 1].
    // With the GL-style z' = -1 the ORTHO inverse lands at z_eye = far - 2·near
    // — a point ~far BEHIND the camera — so the march burned its t budget in
    // dead space and broke on the far cap before reaching any body (ticket
    // yl0p: ortho rendered nothing). Perspective survived the same mistake
    // because there the point only feeds normalize(nearWorld - camPos), and the
    // persp inverse maps z' = -1 to a POSITIVE multiple along the pixel ray.
    const ndc = positionGeometry.xy;
    const clipPos = vec4(ndc, float(0), float(1));
    const eyePos4 = state.invProjU.mul(clipPos);
    const eyePos = eyePos4.xyz.div(eyePos4.w);
    const nearWorld = state.invViewU.mul(vec4(eyePos, 1)).xyz;
    // PERSPECTIVE: all rays diverge from the camera position, so the direction
    // is camera→near-point and the origin is the camera. ORTHOGRAPHIC: rays are
    // PARALLEL — one shared world direction (the camera's −Z), with a distinct
    // per-pixel origin sitting on the near plane.
    const camFwd = normalize(state.invViewU.mul(vec4(0, 0, -1, 0)).xyz);
    const perspDir = normalize(nearWorld.sub(state.camPosU));
    const isOrtho = state.isOrthoU.greaterThan(float(0.5));
    const rd = select(isOrtho, camFwd, perspDir).toVar();
    const ro = select(isOrtho, nearWorld, state.camPosU).toVar();

    const bodyCount = int(state.bodyCountU);
    const t = float(0).toVar();
    const hit = float(0).toVar();
    const hitPos = vec3(0).toVar();
    const hitBody = int(0).toVar();

    // SINGLE adaptive sphere-trace down the primary ray. Each step evaluates
    // every body's own SDF and keeps the SMALLEST distance (`minD`) together
    // with which body produced it (`closestBody`); the ray then steps by that
    // conservative distance. On a hit the pixel is shaded for `closestBody` —
    // i.e. the body whose own surface is nearest along the ray, exactly the
    // depth-buffer occlusion two separate opaque solids must have.
    //
    // NOTE on the "implicit boolean" concern (tickets deeg/9ljc): for OPAQUE
    // primary visibility this union march and a per-body nearest-hit march
    // produce the SAME image — both show the nearest surface and record
    // `closestBody`, so separate solids occlude, they do NOT fuse. The earlier
    // per-body NESTED march (Loop(MAX_BODIES){Loop(MARCH_STEPS)}, commit
    // e4e5727) gave no visual benefit but sliced bodies (each body got its own
    // capped step budget and a shared `bestT` early-out cut marches short) and
    // — in the deferred renderer — exploded the shader so it never finished
    // compiling. Reverted to this single union march (ticket 6yin).
    Loop(int(SDF_MARCH_STEPS), () => {
      const p = ro.add(rd.mul(t));
      const minD = float(1e30).toVar();
      const closestBody = int(0).toVar();

      Loop(int(VM_MAX_BODIES), ({ i: bodyIdx }) => {
        If(bodyIdx.lessThan(bodyCount), () => {
          const offLen = state.offsetBuf.element(bodyIdx.mul(2).add(1));
          If(int(offLen).greaterThan(int(0)), () => {
            const baseOff = int(state.offsetBuf.element(bodyIdx.mul(2)));
            const volBase = int(state.volumeBaseBuf.element(bodyIdx));
            const d = sdfEval({
              p,
              baseOffset: baseOff,
              volumeBaseIdx: volBase,
              bodyIdx,
            });
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
      // Per-body conservative step (Ticket 1wnm): a non-Lipschitz SDF
      // (revolve march_relax 0.7, gyroid, …) overestimates distance and a full
      // step would tunnel through thin walls; simple primitives keep 1.0. Same
      // factor the deferred march applies.
      t.addAssign(
        max(
          minD.mul(state.marchRelaxBuf.element(closestBody)),
          float(SURFACE_EPS),
        ),
      );
      // Stop at the camera far plane (now adapted to the scene each frame,
      // ticket lbzq) instead of a fixed 3000 mm cap that hid large/zoomed-out
      // scenes.
      If(t.greaterThan(state.farU), () => {
        Break();
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
    // Linear (ortho) vs reciprocal (perspective) depth encoding, matching the
    // active scene camera so the B-Rep edge overlay occludes correctly.
    depth
      .assign(
        select(
          state.isOrthoU.greaterThan(float(0.5)),
          viewZToOrthographicDepth(viewZHit, state.nearU, state.farU),
          viewZToPerspectiveDepth(viewZHit, state.nearU, state.farU),
        ),
      )
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
