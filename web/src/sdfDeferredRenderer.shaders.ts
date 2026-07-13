/**
 * TSL shader builders of the deferred SDF renderer: the three G-buffer
 * materials (opaque / transparent+body-ID / back-face exit depth), the
 * fullscreen shade material (union sphere-trace + shading) and the atomic
 * body-ID buffer with its clear kernel. Split out of sdfDeferredRenderer.ts
 * so @ts-nocheck stays confined to actual shader-emitting code.
 */

// @ts-nocheck -- TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import {
  Fn,
  If,
  atomicAdd,
  atomicLoad,
  attribute,
  atomicOr,
  atomicStore,
  bitAnd,
  depth,
  float,
  instanceIndex,
  int,
  min,
  mrt,
  normalize,
  property,
  screenCoordinate,
  shiftRight,
  storage,
  texture,
  uint,
  uniform,
  uniformArray,
  uv,
  vec2,
  vec3,
  vec4,
  select,
  modelWorldMatrix,
  positionGeometry,
} from "three/tsl";
import { MeshBasicNodeMaterial, StorageBufferAttribute } from "three/webgpu";
import { buildVmSdfEvalFn } from "./sdfVmInterpreter";
import { SDF_MARCH_STEPS } from "./sdfPipelineCommon";
import {
  makeDisplacedSdfEval,
  sceneDepthValue,
  emitUnionMarch,
  shadeSdfHit,
} from "./sdfPipelineCommon.shaders";

// mm -- Z-Offset ±1 mm in Kamera-Richtung: March-Start liegt 1 mm vor der
// exakten Front-Fläche (Richtung Kamera), tMax entsprechend +1 mm hinter der
// Rückwand. Damit startet der Trace garantiert im Freiraum (Ticket vlhp).
export const MARCH_START_BACKOFF = 1.0;
/** Two packed uint words per pixel: [count|id0] and [id1]. */
export const BODY_ID_WORDS_PER_PIXEL = 2;

// ── Atomic body-ID buffer + clear kernel ───────────────────────────

export function createBodyIdBuffer(maxPixels: number, width: number, height: number) {
  const bodyIdCount = maxPixels * BODY_ID_WORDS_PER_PIXEL;
  const bodyIdAttr = new StorageBufferAttribute(bodyIdCount, 1, Uint32Array);
  const bodyIdBuf = storage(bodyIdAttr, "uint", bodyIdCount).toAtomic();
  const clearNode = Fn(() => {
    atomicStore(bodyIdBuf.element(instanceIndex), uint(0));
  })().compute(width * height * BODY_ID_WORDS_PER_PIXEL);
  return { bodyIdAttr, bodyIdBuf, clearNode };
}

// ── G-Buffer material ───────────────────────────────────────────────

// The body id rides on the GEOMETRY as a per-vertex attribute (value =
// bodyIndex + 1, 0 means "empty"), NOT baked into the shader as a constant
// (ticket 853b80c5). Baking it produced one distinct WGSL program per body:
// every scene had to compile N G-buffer shaders, and no two documents shared
// a pipeline-cache entry. With the id as an attribute the source is invariant,
// so the whole app compiles ONE opaque + ONE transparent G-buffer program —
// and the browser's on-disk pipeline cache hits on the next load.
const BODY_ID_ATTR = "bodyId";

export function createGBufferMaterial(): MeshBasicNodeMaterial {
  const material = new MeshBasicNodeMaterial();
  material.outputNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    return vec4(worldPos, attribute(BODY_ID_ATTR, "float"));
  })();
  material.depthWrite = true;
  material.depthTest = true;
  material.side = THREE.FrontSide;
  material.toneMapped = false;
  return material;
}

// ── Transparent G-Buffer material (+ packed body-ID insertion) ─────

export function createTransparentGBufferMaterial(
  bodyIdBuf: any,
  widthU: any,
): MeshBasicNodeMaterial {
  const material = new MeshBasicNodeMaterial();
  material.outputNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    const px = int(screenCoordinate.x);
    const py = int(screenCoordinate.y);
    const pixelIdx = py.mul(int(widthU)).add(px);
    const word0Idx = pixelIdx.mul(BODY_ID_WORDS_PER_PIXEL);
    const word1Idx = word0Idx.add(1);
    const encodedId = uint(attribute(BODY_ID_ATTR, "float"));
    const oldWord0 = atomicAdd(bodyIdBuf.element(word0Idx), uint(0x10000));
    const oldCount = bitAnd(shiftRight(oldWord0, uint(16)), uint(0xffff));
    If(oldCount.equal(uint(0)), () => {
      atomicOr(bodyIdBuf.element(word0Idx), encodedId);
    });
    If(oldCount.equal(uint(1)), () => {
      atomicOr(bodyIdBuf.element(word1Idx), encodedId);
    });
    return vec4(worldPos, attribute(BODY_ID_ATTR, "float"));
  })();
  material.depthWrite = true;
  material.depthTest = true;
  material.side = THREE.FrontSide;
  material.toneMapped = false;
  return material;
}

// ── Cached-shade blit (ticket 40001a3a) ────────────────────────────
//
// Replays the LAST shade result (colour + per-pixel depth) from the
// shadeRT onto the screen. Used when neither the camera nor the scene
// changed since the previous frame — e.g. sketch hover/drag overlays
// invalidating at pointer-move rate — so the expensive sphere-trace
// shade pass is skipped entirely. Re-emitting the cached DEPTH keeps
// the shared z-buffer contract with the datum/overlay passes (1m8g).
//
// `depthTex` is the shade pass's second COLOUR attachment (its red channel
// carries the NDC depth), not a depth attachment — see `createShadeMaterial`.
export function createBlitMaterial(
  colorTex: THREE.Texture,
  depthTex: THREE.Texture,
): MeshBasicNodeMaterial {
  const material = new MeshBasicNodeMaterial();
  material.colorNode = Fn(() => {
    // Render-target textures sample v-flipped on the fullscreen quad —
    // same convention as the shade pass reading the G-buffer RTs.
    const texUv = vec2(uv().x, float(1).sub(uv().y));
    depth.assign(texture(depthTex, texUv).r).toStack();
    return texture(colorTex, texUv);
  })();
  // Mirror the shade material's raster state so the blended result over
  // the base-scene clear colour is byte-identical to a fresh shade pass.
  material.depthWrite = true;
  material.depthTest = false;
  material.toneMapped = false;
  material.transparent = true;
  return material;
}

// ── Back-face material (BackSide → worldPos for exit depth) ────────

export function createBackFaceMaterial(
  camPosU: any,
  camFwdU: any,
  isOrthoU: any,
): MeshBasicNodeMaterial {
  const material = new MeshBasicNodeMaterial();
  material.outputNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    // MAX-blended exit distance ALONG the primary ray, so the shade pass can
    // clamp the march at the body's far hull. PERSPECTIVE: rays diverge from
    // camPos, so the ray parameter is the radial distance |p − camPos|.
    // ORTHOGRAPHIC: rays are parallel along camFwd, so the ray parameter is the
    // projection dot(p − camPos, camFwd). Using the radial distance in ortho
    // (the old code) over-estimated the exit depth toward the silhouette and,
    // combined with a perspective march ray, drilled the parallel hole rays
    // into the bore wall → dark concentric rings (ticket g2ox).
    const rel = worldPos.sub(camPosU);
    const dist = select(
      isOrthoU.greaterThan(float(0.5)),
      rel.dot(camFwdU),
      rel.length(),
    );
    return vec4(dist, 0, 0, 1);
  })();
  material.side = THREE.BackSide;
  material.depthTest = false;
  material.depthWrite = false;
  material.blending = THREE.CustomBlending;
  material.blendEquation = THREE.MaxEquation;
  material.blendSrc = THREE.OneFactor;
  material.blendDst = THREE.OneFactor;
  material.toneMapped = false;
  return material;
}

// ── Shade material (reads all G-buffers, sphere-traces, lights) ────

export function createShadeMaterial(
  opaqueGbufTex: THREE.Texture,
  transGbufTex: THREE.Texture,
  transBackTex: THREE.Texture,
  bodyIdBuf: any,
  opcodeBuf: any,
  offsetBuf: any,
  materialBuf: any,
  selectionBuf: any,
  volumeBaseBuf: any,
  marchRelaxBuf: any,
  camPosU: any,
  camFwdU: any,
  viewMatU: any,
  widthU: any,
  nearU: any,
  farU: any,
  bgColorU: any,
  isOrthoU: any,
  volumeTextures: THREE.Data3DTexture[] = [],
): MeshBasicNodeMaterial {
  const sdfEvalRaw = buildVmSdfEvalFn(opcodeBuf, {
    withBaseOffset: true,
    volumeTextures,
    withVolumeBaseIdx: volumeTextures.length > 0,
  });

  const sdfEval = makeDisplacedSdfEval(sdfEvalRaw, materialBuf);

  // Evaluate one transparent body's SDF at `p` and keep the running minimum
  // across the (≤2) transparent bodies present at this pixel, recording which
  // body is currently closest. Only ever the bodies actually flagged at this
  // pixel are unioned, so this is NOT an implicit boolean of unrelated bodies
  // (tickets deeg/9ljc) — full march rationale at emitUnionMarch.
  const evalBodySdf = (p: any, bodyIdx: any, minD: any, closestBody: any) => {
    const baseOff = int(offsetBuf.element(bodyIdx.mul(2)));
    const volBase = int(volumeBaseBuf.element(bodyIdx));
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
  };

  const material = new MeshBasicNodeMaterial();
  // The reconstructed scene depth of this pixel — a shader-global the shade
  // flow writes and the MRT node below routes into the second colour
  // attachment (see the note at `material.mrtNode`).
  const sceneDepth = property("float", "fdSceneDepth");
  const shadeFn = Fn(() => {
    const texUv = vec2(uv().x, float(1).sub(uv().y));
    const opaqueGbuf = texture(opaqueGbufTex, texUv);
    const transGbuf = texture(transGbufTex, texUv);

    const opaqueTag = opaqueGbuf.w;
    const hasOpaque = opaqueTag.greaterThan(float(0.5));

    const px = int(screenCoordinate.x);
    const py = int(screenCoordinate.y);
    const pixelIdx = py.mul(int(widthU)).add(px);
    const word0Idx = pixelIdx.mul(BODY_ID_WORDS_PER_PIXEL);
    const word1Idx = word0Idx.add(1);
    const word0 = atomicLoad(bodyIdBuf.element(word0Idx)).toVar();
    const word1 = atomicLoad(bodyIdBuf.element(word1Idx)).toVar();
    const transCount = min(
      bitAnd(shiftRight(word0, uint(16)), uint(0xffff)),
      uint(2),
    ).toVar();
    const transId0 = bitAnd(word0, uint(0xffff)).toVar();
    const transId1 = bitAnd(word1, uint(0xffff)).toVar();
    const hasTrans = transCount.greaterThan(uint(0));

    const shadePos = vec3(0, 0, 0).toVar();
    const shadeBodyIdx = int(0).toVar();
    const hasHit = int(0).toVar();

    If(hasOpaque.or(hasTrans), () => {
      If(hasTrans, () => {
        const transWorldPos = transGbuf.xyz;
        // PERSPECTIVE: the primary ray diverges from the camera, so its
        // direction is camera→surface and the ray parameter is the radial
        // distance. ORTHOGRAPHIC: every ray is PARALLEL along camFwd, so the
        // shared direction is camFwd and the ray parameter is the projection
        // dot(p − camPos, camFwd). The old code always used the perspective
        // ray; in ortho top-views that tilted the per-pixel hole rays outward
        // so they drilled into the bore wall → dark concentric rings (ticket
        // g2ox). Matches the VM renderer's select(isOrtho, camFwd, perspDir).
        const isOrtho = isOrthoU.greaterThan(float(0.5));
        const rd = select(
          isOrtho,
          camFwdU,
          normalize(transWorldPos.sub(camPosU)),
        ).toVar();

        const backDist = texture(transBackTex, texUv).x;
        const relFront = transWorldPos.sub(camPosU);
        const frontDist = select(isOrtho, relFront.dot(camFwdU), relFront.length());
        const tMax = backDist.sub(frontDist).add(MARCH_START_BACKOFF).toVar();

        If(hasOpaque, () => {
          const opaqueDist = opaqueGbuf.xyz.sub(transWorldPos).dot(rd);
          const opaqueBodyEncoded = uint(int(opaqueTag.sub(0.5)).add(1));
          const opaqueIsAlsoTrans = transId0
            .equal(opaqueBodyEncoded)
            .or(
              transCount
                .greaterThanEqual(uint(2))
                .and(transId1.equal(opaqueBodyEncoded)),
            );
          If(
            opaqueDist.greaterThan(float(0)).and(opaqueIsAlsoTrans.not()),
            () => {
              tMax.assign(min(tMax, opaqueDist));
            },
          );
        });

        // Shared union sphere-trace through the (≤2) transparent bodies at
        // this pixel (see emitUnionMarch for the full march + deeg/9ljc/6yin
        // rationale). tStart: Z-Offset ±1 mm — March-Start 1 mm Richtung
        // Kamera zurückversetzt, damit der Trace garantiert im Freiraum
        // beginnt (Ticket vlhp). tEnd = back-face exit depth (tMax).
        const { t, hit, hitPos, hitBody } = emitUnionMarch({
          ro: transWorldPos,
          rd,
          tStart: -MARCH_START_BACKOFF,
          steps: SDF_MARCH_STEPS,
          marchRelaxBuf,
          tEnd: tMax,
          evalBodies: (p: any, minD: any, closestBody: any) => {
            If(transCount.greaterThanEqual(uint(1)), () => {
              evalBodySdf(p, int(transId0.sub(uint(1))), minD, closestBody);
            });
            If(transCount.greaterThanEqual(uint(2)), () => {
              evalBodySdf(p, int(transId1.sub(uint(1))), minD, closestBody);
            });
          },
        });

        const opaqueBodyEncoded2 = uint(int(opaqueTag.sub(0.5)).add(1));
        const opaqueIsAlsoTrans = transId0
          .equal(opaqueBodyEncoded2)
          .or(
            transCount
              .greaterThanEqual(uint(2))
              .and(transId1.equal(opaqueBodyEncoded2)),
          );
        const opaqueDist2 = opaqueGbuf.xyz.sub(transWorldPos).dot(rd);

        If(hit.greaterThan(float(0.5)), () => {
          If(
            hasOpaque.and(opaqueIsAlsoTrans.not()).and(opaqueDist2.lessThan(t)),
            () => {
              shadePos.assign(opaqueGbuf.xyz);
              shadeBodyIdx.assign(int(opaqueTag.sub(0.5)));
            },
          ).Else(() => {
            shadePos.assign(hitPos);
            shadeBodyIdx.assign(hitBody);
          });
          hasHit.assign(int(1));
        }).Else(() => {
          If(hasOpaque.and(opaqueIsAlsoTrans.not()), () => {
            shadePos.assign(opaqueGbuf.xyz);
            shadeBodyIdx.assign(int(opaqueTag.sub(0.5)));
            hasHit.assign(int(1));
          });
        });
      }).Else(() => {
        shadePos.assign(opaqueGbuf.xyz);
        shadeBodyIdx.assign(int(opaqueTag.sub(0.5)));
        hasHit.assign(int(1));
      });
    });

    // Ticket a2ps/qz52: miss pixels paint the viewport background OPAQUELY (was
    // vec4(0,0,0,0) transparent, which let the grey page body show through →
    // scene read grey). Done in-shader (not via a screen clear before the shade
    // pass — that broke SDF body rendering, ticket qz52). `bgColorU` tracks the
    // ClearColorSync viewport colour.
    const result = vec4(bgColorU, 1.0).toVar();

    If(hasHit.greaterThan(int(0)), () => {
      // Shade the hit (normal → PBR → selection tint → wireframe) via the
      // shared SDF pipeline. The view direction points from the shaded
      // surface point back to the camera.
      const viewDirWorld = normalize(camPosU.sub(shadePos));
      result.assign(
        shadeSdfHit({
          p: shadePos,
          bi: shadeBodyIdx,
          offsetBuf,
          volumeBaseBuf,
          materialBuf,
          viewMatU,
          viewDirWorld,
          sdfEval,
          selectionBuf,
        }),
      );
    });

    // Per-pixel scene depth, so the B-Rep edge overlay and the datum planes can
    // occlude against the body in "visible" mode. `shadePos` is the true
    // raymarched (or opaque) surface point; its depth is reconstructed via the
    // scene camera's view matrix + near/far (the shade quad itself is drawn
    // with an ortho camera, so the rasterizer's own depth is meaningless here).
    // Misses write the far plane so empty pixels never occlude.
    sceneDepth.assign(
      select(
        hasHit.greaterThan(int(0)),
        sceneDepthValue({ hitPos: shadePos, viewMatU, nearU, farU, isOrthoU }),
        float(1.0),
      ),
    );

    return result;
  });

  // MRT: colour → attachment 0, the scene depth → attachment 1 (a plain float
  // colour texture, NOT the target's depth attachment). `renderDeferredFrame`
  // blits both back to the screen, and the blit re-emits attachment 1 as the
  // canvas frag-depth — that is what re-establishes the shared z-buffer the
  // datum/overlay passes test against (ticket 1m8g).
  //
  // Why not simply write frag-depth into shadeRT's depth attachment and sample
  // that? Because reading a WebGPU depth attachment back through a texture
  // sample sees 0 everywhere in three r183 — verified in-browser against both
  // this target and the opaque G-buffer's depth (ticket #344): the blit then
  // stamped depth 0 (the near plane) over the whole canvas and every later pass
  // was depth-rejected — datum planes gone, edges gone in "visible" mode.
  // Float COLOUR attachments sample correctly (the G-buffer passes rely on it),
  // so the depth travels as colour.
  //
  // `shadeFn()` is called ONCE: its result feeds attachment 0, and the sceneDepth
  // property it assigns on the way feeds attachment 1. The march runs once.
  material.mrtNode = mrt({
    output: shadeFn(),
    depthOut: vec4(sceneDepth, 0.0, 0.0, 1.0),
  });
  material.depthWrite = false;
  material.depthTest = false;
  material.toneMapped = false;
  material.transparent = true;
  return material;
}

