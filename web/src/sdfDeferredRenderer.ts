/**
 * Deferred Raymarching SDF renderer.
 *
 * Pipeline per frame:
 *   1. G-BUFFER: Inflated hull-meshes → float RT (worldPos + bodyIndex)
 *   2. SHADE:    Fullscreen quad reads G-Buffer, sphere-traces from hull
 *                surface into the volume to find the actual SDF surface,
 *                evaluates normals via VM interpreter, applies lighting.
 *                Pixels where no SDF surface is found are discarded
 *                (transparency for gyroid holes, thread gaps, etc.).
 *
 * Hull meshes are scaled slightly larger than the mathematical SDF surface
 * (HULL_INFLATE) so that tessellated polygon edges never clip the true
 * silhouette.  The sphere-tracer converges from the inflated position to
 * the actual surface in 1–3 steps for simple primitives.
 */

// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import * as THREE from "three";
import {
  Break,
  Discard,
  Fn,
  If,
  Loop,
  float,
  int,
  max,
  normalize,
  texture,
  uniform,
  uv,
  vec2,
  vec3,
  vec4,
  modelWorldMatrix,
  positionGeometry,
  uniformArray,
} from "three/tsl";
import { MeshBasicNodeMaterial } from "three/webgpu";

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
const DEFERRED_MARCH_STEPS = 64;
const SURFACE_EPS = 0.001;
const MAX_MARCH_DIST = 3000;
const HULL_INFLATE = 1.02;

// ── Types ───────────────────────────────────────────────────────────

export interface DeferredPipelineState {
  gBufferRT: THREE.WebGLRenderTarget;
  opaqueScene: THREE.Scene;
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
  bodyMeshes: Map<number, THREE.Mesh[]>;
  bodyOpcodeRanges: Map<number, { start: number; length: number }>;
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

// ── Shade material (sphere-traces from hull surface to SDF surface) ─

function createShadeMaterial(
  gBufferTex: THREE.Texture,
  opcodeBuf: ReturnType<typeof uniformArray>,
  offsetBuf: ReturnType<typeof uniformArray>,
  colorBuf: ReturnType<typeof uniformArray>,
  camPosU: ReturnType<typeof uniform>,
): MeshBasicNodeMaterial {
  const sdfEval = buildVmSdfEvalFn(opcodeBuf, { withBaseOffset: true });

  const material = new MeshBasicNodeMaterial();
  material.outputNode = Fn(() => {
    const texUv = vec2(uv().x, float(1).sub(uv().y));
    const gbuf = texture(gBufferTex, texUv);
    const meshWorldPos = gbuf.xyz;
    const bodyTag = gbuf.w;

    If(bodyTag.lessThan(float(0.5)), () => {
      Discard();
    });

    const bodyIdx = int(bodyTag.sub(0.5));
    const baseOff = int(offsetBuf.element(bodyIdx.mul(2)));

    // View ray from camera through this pixel
    const rd = normalize(meshWorldPos.sub(camPosU));

    // Sphere-trace from hull mesh surface into the volume
    const t = float(0).toVar();
    const hitPos = vec3(0, 0, 0).toVar();
    const hit = float(0).toVar();

    Loop(int(DEFERRED_MARCH_STEPS), () => {
      const p = meshWorldPos.add(rd.mul(t));
      const d = sdfEval({ p, baseOffset: baseOff });
      If(d.lessThan(float(SURFACE_EPS)), () => {
        hit.assign(1);
        hitPos.assign(p);
        Break();
      });
      t.addAssign(max(d, float(SURFACE_EPS)));
      If(t.greaterThan(float(MAX_MARCH_DIST)), () => { Break(); });
    });

    // No surface found → transparent (gyroid holes, thread gaps, etc.)
    If(hit.lessThan(float(0.5)), () => { Discard(); });

    // Normal via central differences at the actual hit point
    const p = hitPos;
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

    const cr = colorBuf.element(bodyIdx.mul(3));
    const cg = colorBuf.element(bodyIdx.mul(3).add(1));
    const cb = colorBuf.element(bodyIdx.mul(3).add(2));
    const baseColor = vec3(cr, cg, cb);

    return vec4(baseColor.mul(light), 1);
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
  const gBufferRT = new THREE.WebGLRenderTarget(width, height, {
    type: THREE.FloatType,
    format: THREE.RGBAFormat,
    depthBuffer: true,
    depthTexture: new THREE.DepthTexture(width, height, THREE.FloatType),
  });

  const opaqueScene = new THREE.Scene();

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

  const shadeMaterial = createShadeMaterial(
    gBufferRT.texture,
    opcodeBuf,
    offsetBuf,
    colorBuf,
    camPosUniform,
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
    gBufferRT,
    opaqueScene,
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
    bodyMeshes: new Map(),
    bodyOpcodeRanges: new Map(),
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
  for (const meshes of state.bodyMeshes.values()) {
    for (const m of meshes) {
      state.opaqueScene.remove(m);
      m.geometry.dispose();
      (m.material as THREE.Material).dispose();
    }
  }
  state.bodyMeshes.clear();
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

    const meshes: THREE.Mesh[] = [];
    const material = createGBufferMaterial(bodyIndex);

    for (const leaf of hybridInfo.leaves) {
      if (leaf.role === "skip") continue;
      const geo = createPrimitiveGeometry(leaf);
      if (!geo) continue;
      geo.scale(HULL_INFLATE, HULL_INFLATE, HULL_INFLATE);
      const mesh = new THREE.Mesh(geo, material);
      applyLeafTransform(mesh, leaf);
      mesh.frustumCulled = false;
      state.opaqueScene.add(mesh);
      meshes.push(mesh);
    }

    // Fallback: use body AABB (from kernel) or leaf AABB union
    if (meshes.length === 0 && opcodes.length > 0) {
      const fallbackAabb = aabb
        ? [aabb[0], aabb[1], aabb[2], aabb[3], aabb[4], aabb[5]] as const
        : computeLeafAABB(hybridInfo.leaves);
      if (fallbackAabb) {
        const pad = 0.5;
        const sx = (fallbackAabb[3] - fallbackAabb[0]) + pad * 2;
        const sy = (fallbackAabb[4] - fallbackAabb[1]) + pad * 2;
        const sz = (fallbackAabb[5] - fallbackAabb[2]) + pad * 2;
        const geo = new THREE.BoxGeometry(sx, sy, sz);
        const mesh = new THREE.Mesh(geo, material);
        mesh.position.set(
          (fallbackAabb[0] + fallbackAabb[3]) * 0.5,
          (fallbackAabb[1] + fallbackAabb[4]) * 0.5,
          (fallbackAabb[2] + fallbackAabb[5]) * 0.5,
        );
        mesh.frustumCulled = false;
        state.opaqueScene.add(mesh);
        meshes.push(mesh);
      }
    }

    state.bodyMeshes.set(bodyIndex, meshes);
  }
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
  // Update camera position uniform for the shade pass view-ray computation
  camera.getWorldPosition(_camWorldPos);
  state.camPosUniform.value.copy(_camWorldPos);

  renderer.setRenderTarget(state.gBufferRT);
  renderer.setClearColor(new THREE.Color(0, 0, 0), 0);
  renderer.clear(true, true, false);
  renderer.render(state.opaqueScene, camera);
  renderer.setRenderTarget(null);

  renderer.render(state.shadeScene, state.shadeCamera);
}

// ── Dispose ─────────────────────────────────────────────────────────

export function disposeDeferredPipeline(state: DeferredPipelineState): void {
  for (const meshes of state.bodyMeshes.values()) {
    for (const m of meshes) {
      m.geometry.dispose();
      (m.material as THREE.Material).dispose();
    }
  }
  state.bodyMeshes.clear();
  state.gBufferRT.dispose();
  state.shadeMaterial.dispose();
}
