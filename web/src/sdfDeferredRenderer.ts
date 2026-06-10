/**
 * Deferred Raymarching SDF renderer -- 3-pass dual-mesh pipeline.
 *
 * Pipeline per frame:
 *   1. OPAQUE RASTER:      Non-inflated hull-meshes -> opaqueRT (worldPos+tag)
 *   2. TRANSPARENT COLLECT: Inflated/volume hull-meshes -> transparentRT
 *  2b. BACK-FACE COLLECT:  Same hulls, BackSide -> transparentBackRT (exit worldPos)
 *   3. UNIFIED SHADE:      Fullscreen quad reads all RTs; sphere-traces
 *                           transparent volumes; shades via VM SDF normals.
 *
 * Dual-mesh: each body provides opaque AND/OR transparent hull-meshes.
 * Box -> opaque only.  Sphere/Cylinder/Torus etc. -> opaque + transparent
 * (inflated HULL_INFLATE).  Gyroid/DomainWarp -> transparent only (AABB).
 */

// @ts-nocheck -- TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import {
  Break,
  Fn,
  If,
  Loop,
  atomicAdd,
  atomicLoad,
  atomicOr,
  atomicStore,
  bitAnd,
  depth,
  float,
  instanceIndex,
  int,
  max,
  min,
  normalize,
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
  viewZToPerspectiveDepth,
  modelWorldMatrix,
  positionGeometry,
} from "three/tsl";
import { MeshBasicNodeMaterial, StorageBufferAttribute } from "three/webgpu";

import { buildVmSdfEvalFn } from "./sdfVmInterpreter";
import type { HybridInfo, HybridLeafInfo } from "./kernel";
import {
  createMaterialBuffers,
  createOpcodeBuffers,
  packOpcodes,
  updateVolumeData,
  rebuildVolumeTextures,
  MAT_STRIDE,
  MAT_DISPLACE_DEPTH, MAT_DISPLACE_FREQ,
  DISPLACE_FREQ_Y,
  DISPLACE_FREQ_Z,
  shadeSdfHit,
} from "./sdfPipelineCommon";

// ── Constants ───────────────────────────────────────────────────────

const DEFERRED_MAX_OPCODES = 16384;
const MAX_BODIES = 256;
const DEFERRED_MARCH_STEPS = 512;
const SURFACE_EPS = 0.002;
const HULL_INFLATE = 1.01;
const MAX_PIXELS = 4096 * 2160;
const BODY_ID_WORDS_PER_PIXEL = 2;

// ── Body colors ────────────────────────────────────────────────────

export const BODY_COLORS: [number, number, number][] = [
  [0.55, 0.65, 0.85],
  [0.65, 0.85, 0.55],
  [0.85, 0.55, 0.65],
  [0.85, 0.75, 0.50],
  [0.55, 0.80, 0.80],
  [0.75, 0.55, 0.85],
  [0.85, 0.85, 0.55],
  [0.55, 0.70, 0.70],
];

// ── Primitive mesh factory ─────────────────────────────────────────

export function createPrimitiveGeometry(leaf: HybridLeafInfo): THREE.BufferGeometry | null {
  const p = leaf.params;
  switch (leaf.kind) {
    case "box":
      return new THREE.BoxGeometry(p[0] * 2, p[1] * 2, p[2] * 2);
    case "sphere":
      return new THREE.SphereGeometry(p[0], 32, 24);
    case "cylinder":
      return new THREE.CylinderGeometry(p[0], p[0], p[1] * 2, 32);
    case "torus": {
      const tgeo = new THREE.TorusGeometry(p[0], p[1], 24, 48);
      tgeo.rotateX(Math.PI / 2);
      return tgeo;
    }
    case "cone":
      return new THREE.CylinderGeometry(p[1], p[0], p[2] * 2, 32);
    case "capsule":
      return new THREE.CapsuleGeometry(p[0], p[1] * 2, 16, 24);
    case "extrude":
      return createExtrudeGeometry(leaf);
    case "revolve":
      return createRevolveGeometry(leaf);
    default:
      return null;
  }
}

function createExtrudeGeometry(leaf: HybridLeafInfo): THREE.BufferGeometry | null {
  const contour = leaf.contour;
  if (!contour || contour.outer.length < 3) return null;
  const halfHeight = leaf.params[0];
  const shape = new THREE.Shape(
    contour.outer.map(([x, y]) => new THREE.Vector2(x, y)),
  );
  for (const hole of contour.holes) {
    shape.holes.push(
      new THREE.Path(hole.map(([x, y]) => new THREE.Vector2(x, y))),
    );
  }
  const geo = new THREE.ExtrudeGeometry(shape, {
    depth: 2 * halfHeight,
    bevelEnabled: false,
  });
  geo.translate(0, 0, -halfHeight);
  return geo;
}

function createRevolveGeometry(leaf: HybridLeafInfo): THREE.BufferGeometry | null {
  const contour = leaf.contour;
  if (!contour || contour.outer.length < 3) return null;
  const points = contour.outer.map(([x, y]) => new THREE.Vector2(x, y));
  return new THREE.LatheGeometry(points, 48);
}

export function applyLeafTransform(mesh: THREE.Mesh, leaf: HybridLeafInfo): void {
  const t = leaf.transform;
  const translate = new THREE.Vector3(t[0], t[1], t[2]);
  const quat = new THREE.Quaternion(t[3], t[4], t[5], t[6]);
  const scaleX = t[7], scaleY = t[8], scaleZ = t[9];
  const centerOffset = getCenterOffset(leaf);
  if (centerOffset) {
    const localOffset = new THREE.Vector3(
      centerOffset[0] * scaleX,
      centerOffset[1] * scaleY,
      centerOffset[2] * scaleZ,
    );
    localOffset.applyQuaternion(quat);
    translate.add(localOffset);
  }
  mesh.matrixAutoUpdate = false;
  mesh.matrix.compose(translate, quat, new THREE.Vector3(scaleX, scaleY, scaleZ));
  mesh.matrixWorldNeedsUpdate = true;
}

function getCenterOffset(leaf: HybridLeafInfo): [number, number, number] | null {
  const p = leaf.params;
  switch (leaf.kind) {
    case "box": return [p[3], p[4], p[5]];
    case "sphere": return [p[1], p[2], p[3]];
    case "cylinder": return [p[2], p[3], p[4]];
    case "torus": return [p[2], p[3], p[4]];
    default: return null;
  }
}

// ── Types ───────────────────────────────────────────────────────────

export interface DeferredPipelineState {
  opaqueRT: THREE.WebGLRenderTarget;
  transparentRT: THREE.WebGLRenderTarget;
  transparentBackRT: THREE.WebGLRenderTarget;
  opaqueScene: THREE.Scene;
  transparentScene: THREE.Scene;
  transparentBackScene: THREE.Scene;
  shadeScene: THREE.Scene;
  shadeCamera: THREE.OrthographicCamera;
  shadeMaterial: MeshBasicNodeMaterial;
  backFaceMaterial: MeshBasicNodeMaterial;
  opcodeBuf: ReturnType<typeof storage>;
  opcodeData: Float32Array;
  opcodeAttr: StorageBufferAttribute;
  offsetBuf: ReturnType<typeof storage>;
  offsetData: Float32Array;
  offsetAttr: StorageBufferAttribute;
  materialData: Float32Array;
  materialAttr: any;
  materialBuf: any;
  selectionBuf: ReturnType<typeof uniformArray>;
  selectionData: Float32Array;
  volumeBaseBuf: ReturnType<typeof storage>;
  volumeBaseData: Float32Array;
  volumeBaseAttr: StorageBufferAttribute;
  camPosUniform: ReturnType<typeof uniform>;
  viewMatU: ReturnType<typeof uniform>;
  widthU: ReturnType<typeof uniform>;
  nearU: ReturnType<typeof uniform>;
  farU: ReturnType<typeof uniform>;
  bodyOpaqueMeshes: Map<number, THREE.Mesh[]>;
  bodyTransparentMeshes: Map<number, THREE.Mesh[]>;
  bodyBackMeshes: Map<number, THREE.Mesh[]>;
  bodyOpcodeRanges: Map<number, { start: number; length: number }>;
  bodyIdAttr: any;
  bodyIdBuf: any;
  clearNode: any;
  width: number;
  height: number;
  volumeTextures: THREE.Data3DTexture[];
  showEdgesU: ReturnType<typeof uniform>;
  needsShadeRebuild: boolean;
}

// ── G-Buffer material ───────────────────────────────────────────────

function createGBufferMaterial(bodyIndex: number): MeshBasicNodeMaterial {
  const material = new MeshBasicNodeMaterial();
  material.outputNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    return vec4(worldPos, float(bodyIndex + 1));
  })();
  material.depthWrite = true;
  material.depthTest = true;
  material.side = THREE.FrontSide;
  material.toneMapped = false;
  return material;
}

// ── Transparent G-Buffer material (+ packed body-ID insertion) ─────

function createTransparentGBufferMaterial(
  bodyIndex: number,
  bodyIdBuf: any,
  widthU: ReturnType<typeof uniform>,
): MeshBasicNodeMaterial {
  const material = new MeshBasicNodeMaterial();
  material.outputNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    const px = int(screenCoordinate.x);
    const py = int(screenCoordinate.y);
    const pixelIdx = py.mul(int(widthU)).add(px);
    const word0Idx = pixelIdx.mul(BODY_ID_WORDS_PER_PIXEL);
    const word1Idx = word0Idx.add(1);
    const encodedId = uint(bodyIndex + 1);
    const oldWord0 = atomicAdd(bodyIdBuf.element(word0Idx), uint(0x10000));
    const oldCount = bitAnd(shiftRight(oldWord0, uint(16)), uint(0xFFFF));
    If(oldCount.equal(uint(0)), () => {
      atomicOr(bodyIdBuf.element(word0Idx), encodedId);
    });
    If(oldCount.equal(uint(1)), () => {
      atomicOr(bodyIdBuf.element(word1Idx), encodedId);
    });
    return vec4(worldPos, float(bodyIndex + 1));
  })();
  material.depthWrite = true;
  material.depthTest = true;
  material.side = THREE.FrontSide;
  material.toneMapped = false;
  return material;
}

// ── Back-face material (BackSide → worldPos for exit depth) ────────

function createBackFaceMaterial(
  camPosU: ReturnType<typeof uniform>,
): MeshBasicNodeMaterial {
  const material = new MeshBasicNodeMaterial();
  material.outputNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    const dist = worldPos.sub(camPosU).length();
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

function createShadeMaterial(
  opaqueGbufTex: THREE.Texture,
  transGbufTex: THREE.Texture,
  transBackTex: THREE.Texture,
  bodyIdBuf: any,
  opcodeBuf: ReturnType<typeof storage>,
  offsetBuf: ReturnType<typeof storage>,
  materialBuf: any,
  selectionBuf: ReturnType<typeof uniformArray>,
  volumeBaseBuf: ReturnType<typeof storage>,
  camPosU: ReturnType<typeof uniform>,
  viewMatU: ReturnType<typeof uniform>,
  widthU: ReturnType<typeof uniform>,
  showEdgesU: ReturnType<typeof uniform>,
  nearU: ReturnType<typeof uniform>,
  farU: ReturnType<typeof uniform>,
  volumeTextures: THREE.Data3DTexture[] = [],
): MeshBasicNodeMaterial {
  const sdfEvalRaw = buildVmSdfEvalFn(opcodeBuf, {
    withBaseOffset: true,
    volumeTextures,
    withVolumeBaseIdx: volumeTextures.length > 0,
  });

  const sdfEval = (args: { p: any; baseOffset: any; volumeBaseIdx: any; bodyIdx?: any }) => {
    const d = sdfEvalRaw({ p: args.p, baseOffset: args.baseOffset, volumeBaseIdx: args.volumeBaseIdx });
    if (!args.bodyIdx) return d;
    const matBase = args.bodyIdx.mul(MAT_STRIDE);
    const depth = materialBuf.element(matBase.add(MAT_DISPLACE_DEPTH));
    const freq = materialBuf.element(matBase.add(MAT_DISPLACE_FREQ));
    const disp = args.p.x.mul(freq).sin()
      .mul(args.p.y.mul(freq).mul(DISPLACE_FREQ_Y).sin())
      .mul(args.p.z.mul(freq).mul(DISPLACE_FREQ_Z).sin());
    return d.sub(depth.mul(disp));
  };

  const evalBodySdf = (p: any, bodyIdx: any, minD: any, closestBody: any) => {
    const baseOff = int(offsetBuf.element(bodyIdx.mul(2)));
    const volBase = int(volumeBaseBuf.element(bodyIdx));
    const d = sdfEval({ p, baseOffset: baseOff, volumeBaseIdx: volBase, bodyIdx });
    If(d.lessThan(minD), () => {
      minD.assign(d);
      closestBody.assign(bodyIdx);
    });
  };

  const material = new MeshBasicNodeMaterial();
  material.outputNode = Fn(() => {
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
    const transCount = min(bitAnd(shiftRight(word0, uint(16)), uint(0xFFFF)), uint(2)).toVar();
    const transId0 = bitAnd(word0, uint(0xFFFF)).toVar();
    const transId1 = bitAnd(word1, uint(0xFFFF)).toVar();
    const hasTrans = transCount.greaterThan(uint(0));

    const shadePos = vec3(0, 0, 0).toVar();
    const shadeBodyIdx = int(0).toVar();
    const hasHit = int(0).toVar();

    If(hasOpaque.or(hasTrans), () => {
      If(hasTrans, () => {
        const transWorldPos = transGbuf.xyz;
        const rd = normalize(transWorldPos.sub(camPosU));

        const backDist = texture(transBackTex, texUv).x;
        const frontDist = transWorldPos.sub(camPosU).length();
        const tMax = backDist.sub(frontDist).toVar();

        If(hasOpaque, () => {
          const opaqueDist = opaqueGbuf.xyz.sub(transWorldPos).dot(rd);
          const opaqueBodyEncoded = uint(int(opaqueTag.sub(0.5)).add(1));
          const opaqueIsAlsoTrans = transId0.equal(opaqueBodyEncoded).or(
            transCount.greaterThanEqual(uint(2)).and(transId1.equal(opaqueBodyEncoded)),
          );
          If(opaqueDist.greaterThan(float(0)).and(opaqueIsAlsoTrans.not()), () => {
            tMax.assign(min(tMax, opaqueDist));
          });
        });

        const t = float(0).toVar();
        const hitPos = vec3(0, 0, 0).toVar();
        const hit = float(0).toVar();
        const hitBody = int(0).toVar();

        Loop(int(DEFERRED_MARCH_STEPS), () => {
          const p = transWorldPos.add(rd.mul(t));
          const minD = float(1e30).toVar();
          const closestBody = int(0).toVar();

          If(transCount.greaterThanEqual(uint(1)), () => {
            evalBodySdf(p, int(transId0.sub(uint(1))), minD, closestBody);
          });
          If(transCount.greaterThanEqual(uint(2)), () => {
            evalBodySdf(p, int(transId1.sub(uint(1))), minD, closestBody);
          });

          If(minD.lessThan(float(SURFACE_EPS)), () => {
            hit.assign(1);
            hitPos.assign(p);
            hitBody.assign(closestBody);
            Break();
          });
          t.addAssign(max(minD, float(SURFACE_EPS)));
          If(t.greaterThan(tMax), () => { Break(); });
        });

        const opaqueBodyEncoded2 = uint(int(opaqueTag.sub(0.5)).add(1));
        const opaqueIsAlsoTrans = transId0.equal(opaqueBodyEncoded2).or(
          transCount.greaterThanEqual(uint(2)).and(transId1.equal(opaqueBodyEncoded2)),
        );
        const opaqueDist2 = opaqueGbuf.xyz.sub(transWorldPos).dot(rd);

        If(hit.greaterThan(float(0.5)), () => {
          If(hasOpaque.and(opaqueIsAlsoTrans.not()).and(opaqueDist2.lessThan(t)), () => {
            shadePos.assign(opaqueGbuf.xyz);
            shadeBodyIdx.assign(int(opaqueTag.sub(0.5)));
          }).Else(() => {
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

    const result = vec4(0, 0, 0, 0).toVar();

    If(hasHit.greaterThan(int(0)), () => {
      // Shade the hit (normal → PBR → edges → selection tint → wireframe) via
      // the shared SDF pipeline. The view direction points from the shaded
      // surface point back to the camera.
      const viewDirWorld = normalize(camPosU.sub(shadePos));
      result.assign(shadeSdfHit({
        p: shadePos,
        bi: shadeBodyIdx,
        offsetBuf,
        volumeBaseBuf,
        materialBuf,
        viewMatU,
        viewDirWorld,
        showEdgesU,
        sdfEval,
        selectionBuf,
      }));
    });

    // Write per-pixel canvas depth so the B-Rep edge overlay can occlude in
    // "visible" mode. `shadePos` is the true raymarched (or opaque) surface
    // point; reconstruct its perspective depth via the scene camera's view
    // matrix + near/far (the shade quad itself is drawn with an ortho camera,
    // so the rasterizer's own depth is meaningless here). Misses write the far
    // plane so empty pixels never occlude. Harmless in X-Ray/Hidden modes —
    // there renderOverlayLayer clears depth before drawing the edges.
    If(hasHit.greaterThan(int(0)), () => {
      // Push the written depth a hair (0.1% of viewZ) behind the true
      // surface: the B-Rep edges lie EXACTLY on the surface, so without the
      // bias they z-fight the face and render ragged/washed-out in
      // "visible" mode (ticket b9xz).
      const viewZ = viewMatU.mul(vec4(shadePos, 1.0)).z.mul(1.001);
      depth.assign(viewZToPerspectiveDepth(viewZ, nearU, farU)).toStack();
    }).Else(() => {
      depth.assign(float(1.0)).toStack();
    });

    return result;
  })();

  material.depthWrite = true;
  material.depthTest = false;
  material.toneMapped = false;
  material.transparent = true;
  return material;
}

// ── Pipeline init ───────────────────────────────────────────────────

export function initDeferredPipeline(
  width: number,
  height: number,
): DeferredPipelineState {
  const makeRT = (withDepthTex = true) => {
    const opts: any = {
      type: THREE.FloatType,
      format: THREE.RGBAFormat,
      minFilter: THREE.NearestFilter,
      magFilter: THREE.NearestFilter,
      depthBuffer: true,
    };
    if (withDepthTex) {
      opts.depthTexture = new THREE.DepthTexture(width, height, THREE.FloatType);
    }
    return new THREE.WebGLRenderTarget(width, height, opts);
  };

  const opaqueRT = makeRT();
  const transparentRT = makeRT();
  const transparentBackRT = new THREE.WebGLRenderTarget(width, height, {
    type: THREE.HalfFloatType,
    format: THREE.RGBAFormat,
    minFilter: THREE.NearestFilter,
    magFilter: THREE.NearestFilter,
    depthBuffer: false,
  });

  const opaqueScene = new THREE.Scene();
  const transparentScene = new THREE.Scene();
  const transparentBackScene = new THREE.Scene();

  const opBufs = createOpcodeBuffers(DEFERRED_MAX_OPCODES, MAX_BODIES);
  const matBufs = createMaterialBuffers(MAX_BODIES);

  const selectionData = new Float32Array(MAX_BODIES);
  const selectionBuf = uniformArray(selectionData, "float");

  const camPosUniform = uniform(new THREE.Vector3(0, 0, 100));
  const viewMatU = uniform(new THREE.Matrix4());
  const widthU = uniform(width);
  const showEdgesU = uniform(1);
  const nearU = uniform(1);
  const farU = uniform(4000);
  const backFaceMaterial = createBackFaceMaterial(camPosUniform);

  const bodyIdCount = MAX_PIXELS * BODY_ID_WORDS_PER_PIXEL;
  const bodyIdAttr = new StorageBufferAttribute(bodyIdCount, 1, Uint32Array);
  const bodyIdBuf = storage(bodyIdAttr, 'uint', bodyIdCount).toAtomic();
  const clearNode = Fn(() => {
    atomicStore(bodyIdBuf.element(instanceIndex), uint(0));
  })().compute(width * height * BODY_ID_WORDS_PER_PIXEL);

  const shadeMaterial = createShadeMaterial(
    opaqueRT.texture,
    transparentRT.texture,
    transparentBackRT.texture,
    bodyIdBuf,
    opBufs.opcodeBuf,
    opBufs.offsetBuf,
    matBufs.materialBuf,
    selectionBuf,
    opBufs.volumeBaseBuf,
    camPosUniform,
    viewMatU,
    widthU,
    showEdgesU,
    nearU,
    farU,
  );

  const shadeMesh = new THREE.Mesh(
    new THREE.PlaneGeometry(2, 2),
    shadeMaterial,
  );
  shadeMesh.frustumCulled = false;

  const shadeScene = new THREE.Scene();
  shadeScene.add(shadeMesh);

  const shadeCamera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 2);
  shadeCamera.position.set(0, 0, 1);
  shadeCamera.updateProjectionMatrix();

  return {
    opaqueRT,
    transparentRT,
    transparentBackRT,
    opaqueScene,
    transparentScene,
    transparentBackScene,
    shadeScene,
    shadeCamera,
    shadeMaterial,
    backFaceMaterial,
    ...opBufs,
    ...matBufs,
    selectionBuf,
    selectionData,
    camPosUniform,
    viewMatU,
    widthU,
    showEdgesU,
    nearU,
    farU,
    bodyOpaqueMeshes: new Map(),
    bodyTransparentMeshes: new Map(),
    bodyBackMeshes: new Map(),
    bodyOpcodeRanges: new Map(),
    bodyIdAttr,
    bodyIdBuf,
    clearNode,
    width,
    height,
    needsShadeRebuild: false,
  };
}

// ── Update data only (no shader recompile) ─────────────────────────

export function updateDeferredData(
  state: DeferredPipelineState,
  bodies: {
    bodyIndex: number;
    hybridInfo: HybridInfo;
    opcodes: Float32Array;
    aabb?: Float32Array;
    volumes?: { data: Float32Array; dims: [number, number, number] }[];
  }[],
): void {
  const ranges = packOpcodes(state.opcodeData, state.offsetData, bodies, DEFERRED_MAX_OPCODES, MAX_BODIES);
  state.opcodeAttr.needsUpdate = true;
  state.offsetAttr.needsUpdate = true;
  for (const [bi, range] of ranges) state.bodyOpcodeRanges.set(bi, range);

  for (const { bodyIndex, hybridInfo, aabb } of bodies) {
    const opaqueMeshes = state.bodyOpaqueMeshes.get(bodyIndex);
    const transMeshes = state.bodyTransparentMeshes.get(bodyIndex);
    const backMeshes = state.bodyBackMeshes.get(bodyIndex);
    if (!opaqueMeshes && !transMeshes && !backMeshes) continue;

    const updateMeshList = (meshes: THREE.Mesh[] | undefined) => {
      if (!meshes) return;
      for (const m of meshes) {
        if (m.userData.isAabb) {
          const bodyAABB = aabb
            ? [aabb[0], aabb[1], aabb[2], aabb[3], aabb[4], aabb[5]] as const
            : computeLeafAABB(hybridInfo.leaves);
          if (bodyAABB) {
            const pad = 0.5;
            m.geometry.dispose();
            m.geometry = new THREE.BoxGeometry(
              (bodyAABB[3] - bodyAABB[0]) + pad * 2,
              (bodyAABB[4] - bodyAABB[1]) + pad * 2,
              (bodyAABB[5] - bodyAABB[2]) + pad * 2,
            );
            m.position.set(
              (bodyAABB[0] + bodyAABB[3]) * 0.5,
              (bodyAABB[1] + bodyAABB[4]) * 0.5,
              (bodyAABB[2] + bodyAABB[5]) * 0.5,
            );
          }
        } else if (m.userData.leafIdx !== undefined) {
          applyLeafTransform(m, hybridInfo.leaves[m.userData.leafIdx]);
        }
      }
    };
    updateMeshList(opaqueMeshes);
    updateMeshList(transMeshes);
    updateMeshList(backMeshes);
  }

  updateVolumeData(state.volumeTextures, state.volumeBaseData, bodies, MAX_BODIES);
  state.volumeBaseAttr.needsUpdate = true;
}

export function updateDeferredSelection(
  state: DeferredPipelineState,
  selectedBodies: Set<number>,
): void {
  state.selectionData.fill(0);
  for (const idx of selectedBodies) {
    if (idx < MAX_BODIES) state.selectionData[idx] = 1;
  }
}

// ── Body management (full rebuild — new meshes + shader compile) ────

export function rebuildDeferredBodies(
  state: DeferredPipelineState,
  bodies: {
    bodyIndex: number;
    hybridInfo: HybridInfo;
    opcodes: Float32Array;
    aabb?: Float32Array;
    volumes?: { data: Float32Array; dims: [number, number, number] }[];
  }[],
): void {
  for (const meshes of state.bodyOpaqueMeshes.values()) {
    for (const m of meshes) {
      state.opaqueScene.remove(m);
      m.geometry.dispose();
      (m.material as THREE.Material).dispose();
    }
  }
  for (const meshes of state.bodyTransparentMeshes.values()) {
    for (const m of meshes) {
      state.transparentScene.remove(m);
      m.geometry.dispose();
      (m.material as THREE.Material).dispose();
    }
  }
  for (const meshes of state.bodyBackMeshes.values()) {
    for (const m of meshes) {
      state.transparentBackScene.remove(m);
      m.geometry.dispose();
    }
  }
  state.bodyOpaqueMeshes.clear();
  state.bodyTransparentMeshes.clear();
  state.bodyBackMeshes.clear();
  state.bodyOpcodeRanges.clear();

  const ranges = packOpcodes(state.opcodeData, state.offsetData, bodies, DEFERRED_MAX_OPCODES, MAX_BODIES);
  state.opcodeAttr.needsUpdate = true;
  state.offsetAttr.needsUpdate = true;
  for (const [bi, range] of ranges) state.bodyOpcodeRanges.set(bi, range);

  for (const { bodyIndex, hybridInfo, opcodes, aabb } of bodies) {
    if (bodyIndex >= MAX_BODIES) continue;
    if (!ranges.has(bodyIndex)) continue;

    const gbufMaterial = createGBufferMaterial(bodyIndex);
    const transMat = createTransparentGBufferMaterial(
      bodyIndex, state.bodyIdBuf, state.widthU,
    );
    const opaqueMeshes: THREE.Mesh[] = [];
    const transMeshes: THREE.Mesh[] = [];
    const backMeshes: THREE.Mesh[] = [];

    if (hybridInfo.needs_full_sdf) {
      const bodyAABB = aabb
        ? [aabb[0], aabb[1], aabb[2], aabb[3], aabb[4], aabb[5]] as const
        : computeLeafAABB(hybridInfo.leaves);
      if (bodyAABB) {
        addAabbBox(bodyAABB, transMat, state.transparentScene, transMeshes);
        transMeshes[transMeshes.length - 1].userData.isAabb = true;
        addAabbBox(bodyAABB, state.backFaceMaterial, state.transparentBackScene, backMeshes);
        backMeshes[backMeshes.length - 1].userData.isAabb = true;
      }
    } else {
      for (let leafIdx = 0; leafIdx < hybridInfo.leaves.length; leafIdx++) {
        const leaf = hybridInfo.leaves[leafIdx];
        if (leaf.role === "skip") continue;
        const geo = createPrimitiveGeometry(leaf);
        if (!geo) continue;

        if (leaf.kind === "box") {
          const mesh = new THREE.Mesh(geo, gbufMaterial);
          applyLeafTransform(mesh, leaf);
          mesh.frustumCulled = false;
          mesh.userData.leafIdx = leafIdx;
          state.opaqueScene.add(mesh);
          opaqueMeshes.push(mesh);
        } else {
          const mesh = new THREE.Mesh(geo, gbufMaterial);
          applyLeafTransform(mesh, leaf);
          mesh.frustumCulled = false;
          mesh.userData.leafIdx = leafIdx;
          state.opaqueScene.add(mesh);
          opaqueMeshes.push(mesh);

          const tGeo = createPrimitiveGeometry(leaf);
          if (tGeo) {
            tGeo.scale(HULL_INFLATE, HULL_INFLATE, HULL_INFLATE);
            const tMesh = new THREE.Mesh(tGeo, transMat);
            applyLeafTransform(tMesh, leaf);
            tMesh.frustumCulled = false;
            tMesh.userData.leafIdx = leafIdx;
            state.transparentScene.add(tMesh);
            transMeshes.push(tMesh);
          }

          const bGeo = createPrimitiveGeometry(leaf);
          if (bGeo) {
            bGeo.scale(HULL_INFLATE, HULL_INFLATE, HULL_INFLATE);
            const bMesh = new THREE.Mesh(bGeo, state.backFaceMaterial);
            applyLeafTransform(bMesh, leaf);
            bMesh.frustumCulled = false;
            bMesh.userData.leafIdx = leafIdx;
            state.transparentBackScene.add(bMesh);
            backMeshes.push(bMesh);
          }
        }
      }

      if (opaqueMeshes.length === 0 && transMeshes.length === 0 && opcodes.length > 0) {
        const bodyAABB = aabb
          ? [aabb[0], aabb[1], aabb[2], aabb[3], aabb[4], aabb[5]] as const
          : computeLeafAABB(hybridInfo.leaves);
        if (bodyAABB) {
          addAabbBox(bodyAABB, transMat, state.transparentScene, transMeshes);
          transMeshes[transMeshes.length - 1].userData.isAabb = true;
          addAabbBox(bodyAABB, state.backFaceMaterial, state.transparentBackScene, backMeshes);
          backMeshes[backMeshes.length - 1].userData.isAabb = true;
        }
      }
    }

    state.bodyOpaqueMeshes.set(bodyIndex, opaqueMeshes);
    state.bodyTransparentMeshes.set(bodyIndex, transMeshes);
    state.bodyBackMeshes.set(bodyIndex, backMeshes);
  }

  const { textures, countChanged } = rebuildVolumeTextures(
    state.volumeTextures, state.volumeBaseData, bodies, MAX_BODIES,
  );
  state.volumeTextures = textures;
  state.volumeBaseAttr.needsUpdate = true;
  state.needsShadeRebuild = countChanged;
}

function addAabbBox(
  aabb: readonly [number, number, number, number, number, number],
  material: MeshBasicNodeMaterial,
  scene: THREE.Scene,
  meshes: THREE.Mesh[],
): void {
  const pad = 0.5;
  const sx = (aabb[3] - aabb[0]) + pad * 2;
  const sy = (aabb[4] - aabb[1]) + pad * 2;
  const sz = (aabb[5] - aabb[2]) + pad * 2;
  const geo = new THREE.BoxGeometry(sx, sy, sz);
  const mesh = new THREE.Mesh(geo, material);
  mesh.position.set(
    (aabb[0] + aabb[3]) * 0.5,
    (aabb[1] + aabb[4]) * 0.5,
    (aabb[2] + aabb[5]) * 0.5,
  );
  mesh.frustumCulled = false;
  scene.add(mesh);
  meshes.push(mesh);
}

function computeLeafAABB(
  leaves: HybridLeafInfo[],
): [number, number, number, number, number, number] | null {
  if (leaves.length === 0) return null;
  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
  for (const leaf of leaves) {
    const a = leaf.aabb;
    if (a[0] < minX) minX = a[0];
    if (a[1] < minY) minY = a[1];
    if (a[2] < minZ) minZ = a[2];
    if (a[3] > maxX) maxX = a[3];
    if (a[4] > maxY) maxY = a[4];
    if (a[5] > maxZ) maxZ = a[5];
  }
  return [minX, minY, minZ, maxX, maxY, maxZ];
}

// ── Shade material rebuild (when volume texture count changes) ──────

export function rebuildShadeMaterialIfNeeded(state: DeferredPipelineState): void {
  if (!state.needsShadeRebuild) return;
  state.needsShadeRebuild = false;

  state.shadeMaterial.dispose();
  const oldMesh = state.shadeScene.children[0];
  if (oldMesh) state.shadeScene.remove(oldMesh);

  state.shadeMaterial = createShadeMaterial(
    state.opaqueRT.texture,
    state.transparentRT.texture,
    state.transparentBackRT.texture,
    state.bodyIdBuf,
    state.opcodeBuf,
    state.offsetBuf,
    state.materialBuf,
    state.selectionBuf,
    state.volumeBaseBuf,
    state.camPosUniform,
    state.viewMatU,
    state.widthU,
    state.showEdgesU,
    state.nearU,
    state.farU,
    state.volumeTextures,
  );

  const shadeMesh = new THREE.Mesh(
    new THREE.PlaneGeometry(2, 2),
    state.shadeMaterial,
  );
  shadeMesh.frustumCulled = false;
  state.shadeScene.add(shadeMesh);
}

// ── Render frame ────────────────────────────────────────────────────

export function resizeDeferredPipeline(
  state: DeferredPipelineState,
  width: number,
  height: number,
): void {
  if (state.width === width && state.height === height) return;
  for (const rt of [state.opaqueRT, state.transparentRT]) {
    if (rt.depthTexture) {
      rt.depthTexture.dispose();
      rt.depthTexture = new THREE.DepthTexture(width, height, THREE.FloatType);
    }
    rt.setSize(width, height);
  }
  state.transparentBackRT.setSize(width, height);
  state.widthU.value = width;
  state.clearNode.count = width * height * BODY_ID_WORDS_PER_PIXEL;
  state.width = width;
  state.height = height;
}

const _camWorldPos = new THREE.Vector3();
const _savedClearColor = new THREE.Color();

export function renderDeferredFrame(
  state: DeferredPipelineState,
  renderer: THREE.WebGPURenderer,
  camera: THREE.Camera,
): void {
  camera.getWorldPosition(_camWorldPos);
  state.camPosUniform.value.copy(_camWorldPos);
  state.viewMatU.value.copy(camera.matrixWorldInverse);
  const persp = camera as THREE.PerspectiveCamera;
  state.nearU.value = persp.near;
  state.farU.value = persp.far;

  renderer.getClearColor(_savedClearColor);
  const savedClearAlpha = renderer.getClearAlpha();

  // Pass 0: clear body-ID buffer
  renderer.compute(state.clearNode);

  // Pass 1: opaque G-buffer
  renderer.setRenderTarget(state.opaqueRT);
  renderer.setClearColor(new THREE.Color(0, 0, 0), 0);
  renderer.clear(true, true, false);
  renderer.render(state.opaqueScene, camera);

  // Pass 2: transparent G-buffer (front faces)
  renderer.setRenderTarget(state.transparentRT);
  renderer.clear(true, true, false);
  renderer.render(state.transparentScene, camera);

  // Pass 2b: back-face distances (MAX-blended scalar, no depth buffer)
  renderer.setRenderTarget(state.transparentBackRT);
  renderer.clear(true, false, false);
  renderer.render(state.transparentBackScene, camera);

  // Pass 3: shade (reads all RTs, composites to screen)
  renderer.setRenderTarget(null);
  renderer.render(state.shadeScene, state.shadeCamera);

  renderer.setClearColor(_savedClearColor, savedClearAlpha);
}

// ── Async pipeline compilation ─────────────────────────────────────

export async function compileDeferredPipeline(
  state: DeferredPipelineState,
  renderer: THREE.WebGPURenderer,
  camera: THREE.Camera,
): Promise<void> {
  const device = (renderer as any).backend?.device as GPUDevice | undefined;

  if (device) device.pushErrorScope("validation");

  // `compileAsync` builds each pipeline keyed to the render target that is
  // *currently set* on the renderer (its attachment formats are part of the
  // pipeline cache key). `renderDeferredFrame` draws each scene into its own
  // MRT render target, so we must precompile with the matching target bound —
  // otherwise the cache key differs and the first real frame falls back to the
  // SYNCHRONOUS `createRenderPipeline` path, compiling the heavy SDF shaders on
  // the main thread / GPU process and freezing the whole browser. Bind the
  // same target each pass uses, compile, then restore. Sequential (not
  // `Promise.all`) because `setRenderTarget` is stateful on the renderer.
  const prevTarget = renderer.getRenderTarget();

  await renderer.computeAsync(state.clearNode);

  renderer.setRenderTarget(state.opaqueRT);
  await renderer.compileAsync(state.opaqueScene, camera);

  renderer.setRenderTarget(state.transparentRT);
  await renderer.compileAsync(state.transparentScene, camera);

  renderer.setRenderTarget(state.transparentBackRT);
  await renderer.compileAsync(state.transparentBackScene, camera);

  renderer.setRenderTarget(null);
  await renderer.compileAsync(state.shadeScene, state.shadeCamera);

  renderer.setRenderTarget(prevTarget);

  if (device) {
    const err = await device.popErrorScope();
    if (err) throw new Error(err.message);
  }
}

// ── Dispose ─────────────────────────────────────────────────────────

export function disposeDeferredPipeline(state: DeferredPipelineState): void {
  for (const meshes of state.bodyOpaqueMeshes.values()) {
    for (const m of meshes) {
      m.geometry.dispose();
      (m.material as THREE.Material).dispose();
    }
  }
  for (const meshes of state.bodyTransparentMeshes.values()) {
    for (const m of meshes) {
      m.geometry.dispose();
      (m.material as THREE.Material).dispose();
    }
  }
  for (const meshes of state.bodyBackMeshes.values()) {
    for (const m of meshes) {
      m.geometry.dispose();
    }
  }
  state.bodyOpaqueMeshes.clear();
  state.bodyTransparentMeshes.clear();
  state.bodyBackMeshes.clear();
  for (const tex of state.volumeTextures) tex.dispose();
  state.volumeTextures = [];
  state.opaqueRT.dispose();
  state.transparentRT.dispose();
  state.transparentBackRT.dispose();
  state.backFaceMaterial.dispose();
  state.shadeMaterial.dispose();
}
