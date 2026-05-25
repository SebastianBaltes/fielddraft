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
  Discard,
  Fn,
  If,
  Loop,
  atomicLoad,
  atomicOr,
  atomicStore,
  bitAnd,
  float,
  instanceIndex,
  int,
  max,
  min,
  normalize,
  screenCoordinate,
  shiftLeft,
  storage,
  texture,
  uint,
  uniform,
  uniformArray,
  uv,
  vec2,
  vec3,
  vec4,
  modelWorldMatrix,
  positionGeometry,
} from "three/tsl";
import { MeshBasicNodeMaterial, StorageBufferAttribute } from "three/webgpu";

import { NORMAL_EPS } from "./sdfRenderer";
import { buildVmSdfEvalFn } from "./sdfVmInterpreter";
import type { HybridInfo, HybridLeafInfo } from "./kernel";

// ── Constants ───────────────────────────────────────────────────────

const DEFERRED_MAX_OPCODES = 4096;
const MAX_BODIES = 32;
const DEFERRED_MARCH_STEPS = 512;
const SURFACE_EPS = 0.002;
const MAX_MARCH_DIST = 3000;
const HULL_INFLATE = 1.01;
const MAX_BITMASK_PIXELS = 4096 * 2160;

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
  const scale = t[7];
  const centerOffset = getCenterOffset(leaf);
  if (centerOffset) {
    const localOffset = new THREE.Vector3(
      centerOffset[0] * scale,
      centerOffset[1] * scale,
      centerOffset[2] * scale,
    );
    localOffset.applyQuaternion(quat);
    translate.add(localOffset);
  }
  mesh.matrixAutoUpdate = false;
  mesh.matrix.compose(translate, quat, new THREE.Vector3(scale, scale, scale));
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
  opcodeBuf: ReturnType<typeof uniformArray>;
  opcodeData: Float32Array;
  offsetBuf: ReturnType<typeof uniformArray>;
  offsetData: Float32Array;
  colorBuf: ReturnType<typeof uniformArray>;
  colorData: Float32Array;
  volumeBaseBuf: ReturnType<typeof uniformArray>;
  volumeBaseData: Float32Array;
  camPosUniform: ReturnType<typeof uniform>;
  widthU: ReturnType<typeof uniform>;
  bodyOpaqueMeshes: Map<number, THREE.Mesh[]>;
  bodyTransparentMeshes: Map<number, THREE.Mesh[]>;
  bodyBackMeshes: Map<number, THREE.Mesh[]>;
  bodyOpcodeRanges: Map<number, { start: number; length: number }>;
  bitmaskAttr: any;
  bitmaskBuf: any;
  clearNode: any;
  width: number;
  height: number;
  volumeTextures: THREE.Data3DTexture[];
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

// ── Transparent G-Buffer material (+ bitmask atomicOr) ─────────────

function createTransparentGBufferMaterial(
  bodyIndex: number,
  bitmaskBuf: any,
  widthU: ReturnType<typeof uniform>,
): MeshBasicNodeMaterial {
  const material = new MeshBasicNodeMaterial();
  material.outputNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    const px = int(screenCoordinate.x);
    const py = int(screenCoordinate.y);
    const pixelIdx = py.mul(int(widthU)).add(px);
    const bit = shiftLeft(uint(1), uint(bodyIndex));
    atomicOr(bitmaskBuf.element(pixelIdx), bit);
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
  bitmaskBuf: any,
  opcodeBuf: ReturnType<typeof uniformArray>,
  offsetBuf: ReturnType<typeof uniformArray>,
  colorBuf: ReturnType<typeof uniformArray>,
  volumeBaseBuf: ReturnType<typeof uniformArray>,
  camPosU: ReturnType<typeof uniform>,
  widthU: ReturnType<typeof uniform>,
  volumeTextures: THREE.Data3DTexture[] = [],
): MeshBasicNodeMaterial {
  const sdfEval = buildVmSdfEvalFn(opcodeBuf, {
    withBaseOffset: true,
    volumeTextures,
    withVolumeBaseIdx: volumeTextures.length > 0,
  });

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
    const mask = atomicLoad(bitmaskBuf.element(pixelIdx)).toVar();
    const hasTrans = mask.greaterThan(uint(0));

    // Fall C: no geometry at this pixel
    If(hasOpaque.not().and(hasTrans.not()), () => { Discard(); });

    // Determine shade target (worldPos + bodyIdx) via Fall A/B logic
    const shadePos = vec3(0, 0, 0).toVar();
    const shadeBodyIdx = int(0).toVar();

    If(hasTrans, () => {
      // Fall B: transparent hull(s) -> multi-body sphere-trace
      const transWorldPos = transGbuf.xyz;
      const rd = normalize(transWorldPos.sub(camPosU));

      // tMax: farthest back-face distance (MAX-blended scalar from Pass 2b)
      const backDist = texture(transBackTex, texUv).x;
      const frontDist = transWorldPos.sub(camPosU).length();
      const tMax = backDist.sub(frontDist).toVar();

      // Opaque behind (different body) tightens the bound
      If(hasOpaque, () => {
        const opaqueDist = opaqueGbuf.xyz.sub(transWorldPos).dot(rd);
        const opaqueBodyBit = shiftLeft(uint(1), uint(int(opaqueTag.sub(0.5))));
        const opaqueIsAlsoTrans = bitAnd(mask, opaqueBodyBit).greaterThan(uint(0));
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

        Loop(int(MAX_BODIES), ({ i: bodyIdx }) => {
          const bit = shiftLeft(uint(1), uint(bodyIdx));
          If(bitAnd(mask, bit).greaterThan(uint(0)), () => {
            const baseOff = int(offsetBuf.element(bodyIdx.mul(2)));
            const volBase = int(volumeBaseBuf.element(bodyIdx));
            const d = sdfEval({ p, baseOffset: baseOff, volumeBaseIdx: volBase });
            If(d.lessThan(minD), () => {
              minD.assign(d);
              closestBody.assign(bodyIdx);
            });
          });
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

      const opaqueBodyBit2 = shiftLeft(uint(1), uint(int(opaqueTag.sub(0.5))));
      const opaqueIsAlsoTrans = bitAnd(mask, opaqueBodyBit2).greaterThan(uint(0));
      const opaqueDist2 = opaqueGbuf.xyz.sub(transWorldPos).dot(rd);

      If(hit.greaterThan(float(0.5)), () => {
        If(hasOpaque.and(opaqueIsAlsoTrans.not()).and(opaqueDist2.lessThan(t)), () => {
          shadePos.assign(opaqueGbuf.xyz);
          shadeBodyIdx.assign(int(opaqueTag.sub(0.5)));
        }).Else(() => {
          shadePos.assign(hitPos);
          shadeBodyIdx.assign(hitBody);
        });
      }).Else(() => {
        If(hasOpaque.and(opaqueIsAlsoTrans.not()), () => {
          shadePos.assign(opaqueGbuf.xyz);
          shadeBodyIdx.assign(int(opaqueTag.sub(0.5)));
        }).Else(() => {
          Discard();
        });
      });
    }).Else(() => {
      // Fall A: opaque only -- shade at mesh surface (no march needed)
      shadePos.assign(opaqueGbuf.xyz);
      shadeBodyIdx.assign(int(opaqueTag.sub(0.5)));
    });

    // Normal via central differences at shade point
    const baseOff = int(offsetBuf.element(shadeBodyIdx.mul(2)));
    const volBase = int(volumeBaseBuf.element(shadeBodyIdx));
    const p = shadePos;
    const eps = float(NORMAL_EPS);
    const ex = vec3(eps, 0, 0);
    const ey = vec3(0, eps, 0);
    const ez = vec3(0, 0, eps);
    const n = normalize(vec3(
      sdfEval({ p: p.add(ex), baseOffset: baseOff, volumeBaseIdx: volBase })
        .sub(sdfEval({ p: p.sub(ex), baseOffset: baseOff, volumeBaseIdx: volBase })),
      sdfEval({ p: p.add(ey), baseOffset: baseOff, volumeBaseIdx: volBase })
        .sub(sdfEval({ p: p.sub(ey), baseOffset: baseOff, volumeBaseIdx: volBase })),
      sdfEval({ p: p.add(ez), baseOffset: baseOff, volumeBaseIdx: volBase })
        .sub(sdfEval({ p: p.sub(ez), baseOffset: baseOff, volumeBaseIdx: volBase })),
    ));

    // Lighting
    const keyDir = normalize(vec3(0.4, 1, 0.5));
    const fillDir = normalize(vec3(-0.6, 0.2, 0.8));
    const rimDir = normalize(vec3(0.0, -0.3, -1.0));
    const dKey = max(n.dot(keyDir), float(0));
    const dFill = max(n.dot(fillDir), float(0));
    const dRim = max(n.dot(rimDir), float(0));
    const light = dKey.mul(0.45).add(dFill.mul(0.2)).add(dRim.mul(0.1)).add(0.3);

    const cr = colorBuf.element(shadeBodyIdx.mul(3));
    const cg = colorBuf.element(shadeBodyIdx.mul(3).add(1));
    const cb = colorBuf.element(shadeBodyIdx.mul(3).add(2));
    const color = vec3(cr, cg, cb).mul(light);

    return vec4(color, 1);
  })();

  material.depthWrite = false;
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

  const opcodeData = new Float32Array(DEFERRED_MAX_OPCODES);
  const opcodeBuf = uniformArray(opcodeData, "float");

  const offsetData = new Float32Array(MAX_BODIES * 2);
  const offsetBuf = uniformArray(offsetData, "float");

  const colorData = new Float32Array(MAX_BODIES * 3);
  for (let i = 0; i < MAX_BODIES; i++) {
    const [r, g, b] = BODY_COLORS[i % BODY_COLORS.length];
    colorData[i * 3] = r;
    colorData[i * 3 + 1] = g;
    colorData[i * 3 + 2] = b;
  }
  const colorBuf = uniformArray(colorData, "float");

  const volumeBaseData = new Float32Array(MAX_BODIES);
  const volumeBaseBuf = uniformArray(volumeBaseData, "float");

  const camPosUniform = uniform(new THREE.Vector3(0, 0, 100));
  const widthU = uniform(width);
  const backFaceMaterial = createBackFaceMaterial(camPosUniform);

  const bitmaskAttr = new StorageBufferAttribute(MAX_BITMASK_PIXELS, 1, Uint32Array);
  const bitmaskBuf = storage(bitmaskAttr, 'uint', MAX_BITMASK_PIXELS).toAtomic();
  const clearNode = Fn(() => {
    atomicStore(bitmaskBuf.element(instanceIndex), uint(0));
  })().compute(width * height);

  const shadeMaterial = createShadeMaterial(
    opaqueRT.texture,
    transparentRT.texture,
    transparentBackRT.texture,
    bitmaskBuf,
    opcodeBuf,
    offsetBuf,
    colorBuf,
    volumeBaseBuf,
    camPosUniform,
    widthU,
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
    opcodeBuf,
    opcodeData,
    offsetBuf,
    offsetData,
    colorBuf,
    colorData,
    volumeBaseBuf,
    volumeBaseData,
    camPosUniform,
    widthU,
    bodyOpaqueMeshes: new Map(),
    bodyTransparentMeshes: new Map(),
    bodyBackMeshes: new Map(),
    bodyOpcodeRanges: new Map(),
    bitmaskAttr,
    bitmaskBuf,
    clearNode,
    width,
    height,
    volumeTextures: [],
    needsShadeRebuild: false,
  };
}

// ── Body management ─────────────────────────────────────────────────

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

  state.opcodeData.fill(0);
  state.offsetData.fill(0);

  let cursor = 0;

  for (const { bodyIndex, hybridInfo, opcodes, aabb } of bodies) {
    if (bodyIndex >= MAX_BODIES) continue;

    const start = cursor;
    const len = Math.min(opcodes.length, DEFERRED_MAX_OPCODES - cursor);
    if (len <= 0) continue;

    state.opcodeData.set(opcodes.subarray(0, len), cursor);
    cursor += len;

    state.offsetData[bodyIndex * 2] = start;
    state.offsetData[bodyIndex * 2 + 1] = len;
    state.bodyOpcodeRanges.set(bodyIndex, { start, length: len });

    const gbufMaterial = createGBufferMaterial(bodyIndex);
    const transMat = createTransparentGBufferMaterial(
      bodyIndex, state.bitmaskBuf, state.widthU,
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
        addAabbBox(bodyAABB, state.backFaceMaterial, state.transparentBackScene, backMeshes);
      }
    } else {
      for (const leaf of hybridInfo.leaves) {
        if (leaf.role === "skip") continue;
        const geo = createPrimitiveGeometry(leaf);
        if (!geo) continue;

        if (leaf.kind === "box") {
          const mesh = new THREE.Mesh(geo, gbufMaterial);
          applyLeafTransform(mesh, leaf);
          mesh.frustumCulled = false;
          state.opaqueScene.add(mesh);
          opaqueMeshes.push(mesh);
        } else {
          const mesh = new THREE.Mesh(geo, gbufMaterial);
          applyLeafTransform(mesh, leaf);
          mesh.frustumCulled = false;
          state.opaqueScene.add(mesh);
          opaqueMeshes.push(mesh);

          const tGeo = createPrimitiveGeometry(leaf);
          if (tGeo) {
            tGeo.scale(HULL_INFLATE, HULL_INFLATE, HULL_INFLATE);
            const tMesh = new THREE.Mesh(tGeo, transMat);
            applyLeafTransform(tMesh, leaf);
            tMesh.frustumCulled = false;
            state.transparentScene.add(tMesh);
            transMeshes.push(tMesh);
          }

          // Back-face mesh (same inflated geometry, shared back-face material)
          const bGeo = createPrimitiveGeometry(leaf);
          if (bGeo) {
            bGeo.scale(HULL_INFLATE, HULL_INFLATE, HULL_INFLATE);
            const bMesh = new THREE.Mesh(bGeo, state.backFaceMaterial);
            applyLeafTransform(bMesh, leaf);
            bMesh.frustumCulled = false;
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
          addAabbBox(bodyAABB, state.backFaceMaterial, state.transparentBackScene, backMeshes);
        }
      }
    }

    state.bodyOpaqueMeshes.set(bodyIndex, opaqueMeshes);
    state.bodyTransparentMeshes.set(bodyIndex, transMeshes);
    state.bodyBackMeshes.set(bodyIndex, backMeshes);
  }

  // Volume textures: collect from all bodies, build global index
  const prevVolCount = state.volumeTextures.length;
  for (const tex of state.volumeTextures) tex.dispose();

  const newVolumes: THREE.Data3DTexture[] = [];
  state.volumeBaseData.fill(0);

  for (const { bodyIndex, volumes } of bodies) {
    if (bodyIndex >= MAX_BODIES || !volumes || volumes.length === 0) continue;
    state.volumeBaseData[bodyIndex] = newVolumes.length;
    for (const vol of volumes) {
      const tex = new THREE.Data3DTexture(
        new Float32Array(vol.data),
        vol.dims[0], vol.dims[1], vol.dims[2],
      );
      tex.format = THREE.RedFormat;
      tex.type = THREE.FloatType;
      tex.minFilter = THREE.LinearFilter;
      tex.magFilter = THREE.LinearFilter;
      tex.wrapS = THREE.ClampToEdgeWrapping;
      tex.wrapT = THREE.ClampToEdgeWrapping;
      tex.wrapR = THREE.ClampToEdgeWrapping;
      tex.needsUpdate = true;
      newVolumes.push(tex);
    }
  }

  state.volumeTextures = newVolumes;
  state.needsShadeRebuild = newVolumes.length !== prevVolCount;
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
    state.bitmaskBuf,
    state.opcodeBuf,
    state.offsetBuf,
    state.colorBuf,
    state.volumeBaseBuf,
    state.camPosUniform,
    state.widthU,
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
  state.clearNode.count = width * height;
  state.width = width;
  state.height = height;
}

const _camWorldPos = new THREE.Vector3();

export function renderDeferredFrame(
  state: DeferredPipelineState,
  renderer: THREE.WebGPURenderer,
  camera: THREE.Camera,
): void {
  camera.getWorldPosition(_camWorldPos);
  state.camPosUniform.value.copy(_camWorldPos);

  // Pass 0: clear bitmask buffer
  renderer.compute(state.clearNode);

  // Pass 1: opaque G-buffer
  renderer.setRenderTarget(state.opaqueRT);
  renderer.setClearColor(new THREE.Color(0, 0, 0), 0);
  renderer.clear(true, true, false);
  renderer.render(state.opaqueScene, camera);

  // Pass 2: transparent G-buffer (front faces)
  renderer.setRenderTarget(state.transparentRT);
  renderer.setClearColor(new THREE.Color(0, 0, 0), 0);
  renderer.clear(true, true, false);
  renderer.render(state.transparentScene, camera);

  // Pass 2b: back-face distances (MAX-blended scalar, no depth buffer)
  renderer.setRenderTarget(state.transparentBackRT);
  renderer.setClearColor(new THREE.Color(0, 0, 0), 0);
  renderer.clear(true, false, false);
  renderer.render(state.transparentBackScene, camera);

  // Pass 3: shade (reads all RTs, composites to screen)
  renderer.setRenderTarget(null);
  renderer.render(state.shadeScene, state.shadeCamera);
}

// ── Async pipeline compilation ─────────────────────────────────────

export async function compileDeferredPipeline(
  state: DeferredPipelineState,
  renderer: THREE.WebGPURenderer,
  camera: THREE.Camera,
): Promise<void> {
  const device = (renderer as any).backend?.device as GPUDevice | undefined;

  if (device) device.pushErrorScope("validation");

  await Promise.all([
    renderer.compileAsync(state.opaqueScene, camera),
    renderer.compileAsync(state.transparentScene, camera),
    renderer.compileAsync(state.transparentBackScene, camera),
    renderer.compileAsync(state.shadeScene, state.shadeCamera),
  ]);

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
