/**
 * TSL shader builders of the fullscreen VM SDF renderer: the raymarch
 * material (ray reconstruction -> union march -> depth write -> shading)
 * and the GPU parity-probe compute kernel. Split out of sdfVmRenderer.ts
 * so @ts-nocheck stays confined to actual shader-emitting code.
 */

// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import {
  Discard,
  Fn,
  If,
  Loop,
  float,
  instanceIndex,
  int,
  normalize,
  positionGeometry,
  storage,
  select,
  vec4,
} from "three/tsl";
import { MeshBasicNodeMaterial, StorageBufferAttribute } from "three/webgpu";
import { buildVmSdfEvalFn } from "./sdfVmInterpreter";
import { SDF_MARCH_STEPS } from "./sdfPipelineCommon";
import {
  makeDisplacedSdfEval,
  emitSceneDepthWrite,
  emitUnionMarch,
  shadeSdfHit,
} from "./sdfPipelineCommon.shaders";
import type { VmPipelineState } from "./sdfVmRenderer";

/** This single fullscreen march unions ALL bodies in one shader — the body
 *  cap sizes the per-pixel inner loop, so it stays small. */
export const VM_MAX_BODIES = 8;

// ── Material ────────────────────────────────────────────────────────

export function createVmMaterial(state: VmPipelineState): MeshBasicNodeMaterial {
  const sdfEvalRaw = buildVmSdfEvalFn(state.opcodeBuf, {
    withBaseOffset: true,
    volumeTextures:
      state.volumeTextures.length > 0 ? state.volumeTextures : undefined,
    withVolumeBaseIdx: state.volumeTextures.length > 0,
  });

  const sdfEval = makeDisplacedSdfEval(sdfEvalRaw, state.materialBuf);

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

    // Shared union sphere-trace (see emitUnionMarch for the full march +
    // deeg/9ljc/6yin rationale). tEnd = the camera far plane (adapted to the
    // scene each frame, ticket lbzq) instead of a fixed 3000 mm cap that hid
    // large/zoomed-out scenes.
    const { hit, hitPos, hitBody } = emitUnionMarch({
      ro,
      rd,
      tStart: 0,
      steps: SDF_MARCH_STEPS,
      marchRelaxBuf: state.marchRelaxBuf,
      tEnd: state.farU,
      evalBodies: (p: any, minD: any, closestBody: any) => {
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
      },
    });

    If(hit.lessThan(float(0.5)), () => {
      Discard();
    });

    // Canvas depth for the hit (emitSceneDepthWrite: shared reconstruction +
    // b9xz edge bias). Misses already discarded above, so they never touch
    // depth. Harmless in X-Ray/Hidden modes — there renderOverlayLayer clears
    // depth before drawing the edges.
    emitSceneDepthWrite({
      hitPos,
      viewMatU: state.viewMatU,
      nearU: state.nearU,
      farU: state.farU,
      isOrthoU: state.isOrthoU,
    });

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

// ── GPU parity-probe kernel ─────────────────────────────────────────

export const PROBE_CAP = 256;

export interface VmProbe {
  ptsData: Float32Array;
  ptsAttr: StorageBufferAttribute;
  outAttr: StorageBufferAttribute;
  kernel: any;
}

export function getProbe(state: VmPipelineState): VmProbe {
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

