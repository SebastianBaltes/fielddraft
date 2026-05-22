/**
 * Deferred Raymarching SDF renderer -- 3-pass dual-mesh pipeline.
 *
 * Pipeline per frame:
 *   1. OPAQUE RASTER:      Non-inflated hull-meshes -> opaqueRT (worldPos+tag)
 *   2. TRANSPARENT COLLECT: Inflated/volume hull-meshes -> transparentRT
 *   3. UNIFIED SHADE:      Fullscreen quad reads both RTs; sphere-traces
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
import {
  BODY_COLORS,
  createPrimitiveGeometry,
  applyLeafTransform,
} from "./sdfHybridRenderer";
import type { HybridInfo, HybridLeafInfo } from "./kernel";

// ── Constants ───────────────────────────────────────────────────────

const DEFERRED_MAX_OPCODES = 4096;
const MAX_BODIES = 32;
const DEFERRED_MARCH_STEPS = 128;
const SURFACE_EPS = 0.001;
const MAX_MARCH_DIST = 3000;
const HULL_INFLATE = 1.005;

// ── Types ───────────────────────────────────────────────────────────

export interface DeferredPipelineState {
  opaqueRT: THREE.WebGLRenderTarget;
  transparentRT: THREE.WebGLRenderTarget;
  opaqueScene: THREE.Scene;
  transparentScene: THREE.Scene;
  shadeScene: THREE.Scene;
  shadeCamera: THREE.OrthographicCamera;
  shadeMaterial: MeshBasicNodeMaterial;
  opcodeBuf: ReturnType<typeof uniformArray>;
  opcodeData: Float32Array;
  offsetBuf: ReturnType<typeof uniformArray>;
  offsetData: Float32Array;
  colorBuf: ReturnType<typeof uniformArray>;
  colorData: Float32Array;
  camPosUniform: ReturnType<typeof uniform>;
  bodyOpaqueMeshes: Map<number, THREE.Mesh[]>;
  bodyTransparentMeshes: Map<number, THREE.Mesh[]>;
  bodyOpcodeRanges: Map<number, { start: number; length: number }>;
  bitmaskAttr: any;
  bitmaskBuf: any;
  clearNode: any;
  width: number;
  height: number;
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
  width: number,
  height: number,
): MeshBasicNodeMaterial {
  const material = new MeshBasicNodeMaterial();
  material.outputNode = Fn(() => {
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    const px = int(screenCoordinate.x);
    const py = int(screenCoordinate.y);
    const pixelIdx = py.mul(int(width)).add(px);
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

// ── Shade material (reads both G-buffers, sphere-traces, lights) ────

function createShadeMaterial(
  opaqueGbufTex: THREE.Texture,
  transGbufTex: THREE.Texture,
  bitmaskBuf: any,
  opcodeBuf: ReturnType<typeof uniformArray>,
  offsetBuf: ReturnType<typeof uniformArray>,
  colorBuf: ReturnType<typeof uniformArray>,
  camPosU: ReturnType<typeof uniform>,
  width: number,
  height: number,
): MeshBasicNodeMaterial {
  const sdfEval = buildVmSdfEvalFn(opcodeBuf, { withBaseOffset: true });

  const material = new MeshBasicNodeMaterial();
  material.outputNode = Fn(() => {
    const texUv = vec2(uv().x, float(1).sub(uv().y));
    const opaqueGbuf = texture(opaqueGbufTex, texUv);
    const transGbuf = texture(transGbufTex, texUv);

    const opaqueTag = opaqueGbuf.w;
    const hasOpaque = opaqueTag.greaterThan(float(0.5));

    const px = int(screenCoordinate.x);
    const py = int(screenCoordinate.y);
    const pixelIdx = py.mul(int(width)).add(px);
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

      const tMax = float(MAX_MARCH_DIST).toVar();
      If(hasOpaque, () => {
        const opaqueDist = opaqueGbuf.xyz.sub(transWorldPos).dot(rd);
        If(opaqueDist.greaterThan(float(0)), () => { tMax.assign(opaqueDist); });
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
            const d = sdfEval({ p, baseOffset: baseOff });
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

      If(hit.greaterThan(float(0.5)), () => {
        shadePos.assign(hitPos);
        shadeBodyIdx.assign(hitBody);
      }).Else(() => {
        If(hasOpaque, () => {
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
    const p = shadePos;
    const eps = float(NORMAL_EPS);
    const ex = vec3(eps, 0, 0);
    const ey = vec3(0, eps, 0);
    const ez = vec3(0, 0, eps);
    const n = normalize(vec3(
      sdfEval({ p: p.add(ex), baseOffset: baseOff })
        .sub(sdfEval({ p: p.sub(ex), baseOffset: baseOff })),
      sdfEval({ p: p.add(ey), baseOffset: baseOff })
        .sub(sdfEval({ p: p.sub(ey), baseOffset: baseOff })),
      sdfEval({ p: p.add(ez), baseOffset: baseOff })
        .sub(sdfEval({ p: p.sub(ez), baseOffset: baseOff })),
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
  const makeRT = () => new THREE.WebGLRenderTarget(width, height, {
    type: THREE.FloatType,
    format: THREE.RGBAFormat,
    minFilter: THREE.NearestFilter,
    magFilter: THREE.NearestFilter,
    depthBuffer: true,
    depthTexture: new THREE.DepthTexture(width, height, THREE.FloatType),
  });

  const opaqueRT = makeRT();
  const transparentRT = makeRT();

  const opaqueScene = new THREE.Scene();
  const transparentScene = new THREE.Scene();

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

  const camPosUniform = uniform(new THREE.Vector3(0, 0, 100));

  const bitmaskAttr = new StorageBufferAttribute(width * height, 1, Uint32Array);
  const bitmaskBuf = storage(bitmaskAttr, 'uint', width * height).toAtomic();
  const clearNode = Fn(() => {
    atomicStore(bitmaskBuf.element(instanceIndex), uint(0));
  })().compute(width * height);

  const shadeMaterial = createShadeMaterial(
    opaqueRT.texture,
    transparentRT.texture,
    bitmaskBuf,
    opcodeBuf,
    offsetBuf,
    colorBuf,
    camPosUniform,
    width,
    height,
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
    opaqueScene,
    transparentScene,
    shadeScene,
    shadeCamera,
    shadeMaterial,
    opcodeBuf,
    opcodeData,
    offsetBuf,
    offsetData,
    colorBuf,
    colorData,
    camPosUniform,
    bodyOpaqueMeshes: new Map(),
    bodyTransparentMeshes: new Map(),
    bodyOpcodeRanges: new Map(),
    bitmaskAttr,
    bitmaskBuf,
    clearNode,
    width,
    height,
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
  state.bodyOpaqueMeshes.clear();
  state.bodyTransparentMeshes.clear();
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
      bodyIndex, state.bitmaskBuf, state.width, state.height,
    );
    const opaqueMeshes: THREE.Mesh[] = [];
    const transMeshes: THREE.Mesh[] = [];

    if (hybridInfo.needs_full_sdf) {
      // Non-meshable (gyroid, domain warp) -> transparent AABB only
      const fb = aabb
        ? [aabb[0], aabb[1], aabb[2], aabb[3], aabb[4], aabb[5]] as const
        : computeLeafAABB(hybridInfo.leaves);
      if (fb) {
        addAabbBox(fb, transMat, state.transparentScene, transMeshes);
      }
    } else {
      for (const leaf of hybridInfo.leaves) {
        if (leaf.role === "skip") continue;
        const geo = createPrimitiveGeometry(leaf);
        if (!geo) continue;

        if (leaf.kind === "box") {
          // Box: opaque only (exact convex shape, no inflation needed)
          const mesh = new THREE.Mesh(geo, gbufMaterial);
          applyLeafTransform(mesh, leaf);
          mesh.frustumCulled = false;
          state.opaqueScene.add(mesh);
          opaqueMeshes.push(mesh);
        } else {
          // All other primitives: opaque (exact) + transparent (inflated)
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
        }
      }

      // Fallback: AABB box as transparent if no meshes created
      if (opaqueMeshes.length === 0 && transMeshes.length === 0 && opcodes.length > 0) {
        const fb = aabb
          ? [aabb[0], aabb[1], aabb[2], aabb[3], aabb[4], aabb[5]] as const
          : computeLeafAABB(hybridInfo.leaves);
        if (fb) {
          addAabbBox(fb, transMat, state.transparentScene, transMeshes);
        }
      }
    }

    state.bodyOpaqueMeshes.set(bodyIndex, opaqueMeshes);
    state.bodyTransparentMeshes.set(bodyIndex, transMeshes);
  }
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

// ── Render frame ────────────────────────────────────────────────────

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

  // Pass 2: transparent G-buffer
  renderer.setRenderTarget(state.transparentRT);
  renderer.setClearColor(new THREE.Color(0, 0, 0), 0);
  renderer.clear(true, true, false);
  renderer.render(state.transparentScene, camera);

  // Pass 3: shade (reads both RTs, composites to screen)
  renderer.setRenderTarget(null);
  renderer.render(state.shadeScene, state.shadeCamera);
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
  state.bodyOpaqueMeshes.clear();
  state.bodyTransparentMeshes.clear();
  state.opaqueRT.dispose();
  state.transparentRT.dispose();
  state.shadeMaterial.dispose();
}
