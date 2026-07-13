/**
 * Deferred Raymarching SDF renderer -- 3-pass dual-mesh pipeline.
 *
 * Pipeline per frame:
 *   1. OPAQUE RASTER:      Exact hull-meshes -> opaqueRT (worldPos+tag)
 *   2. TRANSPARENT COLLECT: Same exact hull-meshes -> transparentRT
 *  2b. BACK-FACE COLLECT:  Same hulls, BackSide -> transparentBackRT (exit worldPos)
 *   3. UNIFIED SHADE:      Fullscreen quad reads all RTs; sphere-traces
 *                           transparent volumes; shades via VM SDF normals.
 *
 * Dual-mesh: each body provides opaque AND/OR transparent hull-meshes.
 * Box -> opaque only.  Sphere/Cylinder/Torus etc. -> opaque + transparent.
 * Gyroid/DomainWarp -> transparent only (AABB).
 *
 * Alle drei Pässe (opaque, transparent, back) nutzen dasselbe EXAKTE
 * parametrische Mesh (createPrimitiveGeometry) -- keine Inflation. Der
 * March-Start ist stattdessen um MARCH_START_BACKOFF in Kamera-Richtung
 * zurückversetzt, damit der Trace garantiert im Freiraum beginnt (Ticket vlhp).
 * TODO: konservative Coverage (Silhouetten-Aufweitung im Screenspace) ist ein
 * noch offener, späterer Schritt.
 *
 * Die TSL-Shader-Builder leben in sdfDeferredRenderer.shaders.ts
 * (@ts-nocheck); diese Datei ist voll typisiert.
 */

import * as THREE from "three";
import { uniform, uniformArray } from "three/tsl";
import {
  StorageBufferAttribute,
  type ComputeNode,
  type MeshBasicNodeMaterial,
  type UniformArrayNode,
  type UniformNode,
  type WebGPURenderer,
} from "three/webgpu";

import {
  leafGeoKey,
  createPrimitiveGeometry,
  applyLeafTransform,
} from "./leafMeshFactory";
import type { HybridInfo, HybridLeafInfo } from "./kernel";
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
  SDF_MAX_OPCODES,
  AABB_HULL_PAD,
  type CameraUniformSet,
  type MaterialBufferSet,
  type OpcodeBufferSet,
} from "./sdfPipelineCommon";
import {
  BODY_ID_WORDS_PER_PIXEL,
  createBodyIdBuffer,
  createGBufferMaterial,
  createTransparentGBufferMaterial,
  createBackFaceMaterial,
  createShadeMaterial,
  createBlitMaterial,
} from "./sdfDeferredRenderer.shaders";

// ── Constants ───────────────────────────────────────────────────────

/** Every body gets its own raster proxy meshes, so the cap is generous —
 *  unlike the VM renderer's 8-body fullscreen union march. */
const DEFERRED_MAX_BODIES = 256;
const MAX_PIXELS = 4096 * 2160;

// ── Types ───────────────────────────────────────────────────────────

export interface DeferredPipelineState
  extends CameraUniformSet,
    OpcodeBufferSet,
    MaterialBufferSet {
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
  /** ONE opaque + ONE transparent G-buffer material for every body — the
   *  body id travels on the geometry (`bodyId` attribute), not in the shader
   *  source, so the whole app compiles a single G-buffer program instead of
   *  one per body (ticket 853b80c5). */
  gbufferMaterial: MeshBasicNodeMaterial;
  transparentGbufferMaterial: MeshBasicNodeMaterial;
  selectionBuf: UniformArrayNode<"float">;
  selectionData: Float32Array;
  widthU: UniformNode<"float", number>;
  bgColorU: UniformNode<"color", THREE.Color>;
  bodyOpaqueMeshes: Map<number, THREE.Mesh[]>;
  bodyTransparentMeshes: Map<number, THREE.Mesh[]>;
  bodyBackMeshes: Map<number, THREE.Mesh[]>;
  bodyOpcodeRanges: Map<number, { start: number; length: number }>;
  bodyIdAttr: StorageBufferAttribute;
  /** Atomic uint storage node (built in the shaders module); only threaded
   *  through to the shader builders, never dereferenced here. */
  bodyIdBuf: unknown;
  clearNode: ComputeNode;
  /** Off-screen target the shade pass renders into; its colour + depth are
   *  blitted to the screen each frame. Lets an overlay-only invalidate
   *  (sketch hover/drag, ticket 40001a3a) replay the cached shade result
   *  instead of re-running the whole SDF pipeline. */
  shadeRT: THREE.WebGLRenderTarget;
  blitScene: THREE.Scene;
  blitMaterial: MeshBasicNodeMaterial;
  /** Camera/background fingerprint of the frame cached in `shadeRT`;
   *  `null` = nothing cached (initial, after resize). */
  lastFrameKey: string | null;
  width: number;
  height: number;
  needsShadeRebuild: boolean;
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
      opts.depthTexture = new THREE.DepthTexture(
        width,
        height,
        THREE.FloatType,
      );
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

  const opBufs = createOpcodeBuffers(SDF_MAX_OPCODES, DEFERRED_MAX_BODIES);
  const matBufs = createMaterialBuffers(DEFERRED_MAX_BODIES);

  const selectionData = new Float32Array(DEFERRED_MAX_BODIES);
  // uniformArray's typing wants unknown[]; it accepts typed arrays at runtime.
  const selectionBuf = uniformArray<"float">(
    selectionData as unknown as unknown[],
    "float",
  );

  // Shared camera uniform set; refreshed per frame in renderDeferredFrame.
  const camU = createCameraUniforms();
  const widthU = uniform(width);
  // Viewport background colour for SDF miss pixels (ticket a2ps); set each frame
  // from the renderer's clear colour (ClearColorSync). White by default.
  const bgColorU = uniform(new THREE.Color(1, 1, 1));
  const backFaceMaterial = createBackFaceMaterial(
    camU.camPosU,
    camU.camFwdU,
    camU.isOrthoU,
  );
  const gbufferMaterial = createGBufferMaterial();

  const { bodyIdAttr, bodyIdBuf, clearNode } = createBodyIdBuffer(
    MAX_PIXELS,
    width,
    height,
  );

  const transparentGbufferMaterial = createTransparentGBufferMaterial(
    bodyIdBuf,
    widthU,
  );

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
    opBufs.marchRelaxBuf,
    camU.camPosU,
    camU.camFwdU,
    camU.viewMatU,
    widthU,
    camU.nearU,
    camU.farU,
    bgColorU,
    camU.isOrthoU,
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

  // Shade-result cache: the shade pass renders here (colour + the per-pixel
  // depth it reconstructs), and a trivial blit replays it to the screen.
  const shadeRT = makeRT();
  const blitMaterial = createBlitMaterial(
    shadeRT.texture,
    shadeRT.depthTexture!,
  );
  const blitMesh = new THREE.Mesh(new THREE.PlaneGeometry(2, 2), blitMaterial);
  blitMesh.frustumCulled = false;
  const blitScene = new THREE.Scene();
  blitScene.add(blitMesh);

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
    gbufferMaterial,
    transparentGbufferMaterial,
    ...opBufs,
    ...matBufs,
    ...camU,
    selectionBuf,
    selectionData,
    widthU,
    bgColorU,
    bodyOpaqueMeshes: new Map(),
    bodyTransparentMeshes: new Map(),
    bodyBackMeshes: new Map(),
    bodyOpcodeRanges: new Map(),
    bodyIdAttr,
    bodyIdBuf,
    clearNode,
    shadeRT,
    blitScene,
    blitMaterial,
    lastFrameKey: null,
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
    marchRelax?: number;
  }[],
  renderer?: WebGPURenderer,
): void {
  const ranges = packOpcodes(
    state.opcodeData,
    state.offsetData,
    bodies,
    SDF_MAX_OPCODES,
    DEFERRED_MAX_BODIES,
  );
  state.opcodeAttr.needsUpdate = true;
  state.offsetAttr.needsUpdate = true;
  packMarchRelax(state.marchRelaxData, bodies, DEFERRED_MAX_BODIES);
  state.marchRelaxAttr.needsUpdate = true;
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
            ? ([aabb[0], aabb[1], aabb[2], aabb[3], aabb[4], aabb[5]] as const)
            : computeLeafAABB(hybridInfo.leaves);
          if (bodyAABB) {
            const pad = AABB_HULL_PAD;
            // Ticket sgzw: defer the OLD geometry's GPU disposal until the
            // device is idle. Disposing it synchronously here races the
            // in-flight frame's submit → "Buffer used in submit while
            // destroyed" → corrupted/smeared render that persists. Same race
            // the rebuild/teardown paths already guard (fg9o/323i).
            const retiredGeo = m.geometry;
            m.geometry = tagBodyId(
              new THREE.BoxGeometry(
                bodyAABB[3] - bodyAABB[0] + pad * 2,
                bodyAABB[4] - bodyAABB[1] + pad * 2,
                bodyAABB[5] - bodyAABB[2] + pad * 2,
              ),
              bodyIndex,
            );
            disposeAfterGpuIdle(renderer, () => retiredGeo.dispose());
            m.position.set(
              (bodyAABB[0] + bodyAABB[3]) * 0.5,
              (bodyAABB[1] + bodyAABB[4]) * 0.5,
              (bodyAABB[2] + bodyAABB[5]) * 0.5,
            );
          }
        } else if (m.userData.leafIdx !== undefined) {
          const leaf = hybridInfo.leaves[m.userData.leafIdx];
          // An extrude/revolve/sweep/box bakes its SHAPE (depth, profile, size)
          // into the mesh geometry — NOT the transform — so a depth/size edit
          // needs a fresh proxy mesh, else the SDF march stays clamped to the
          // stale hull and the body renders at its OLD depth (ticket c9ic). Only
          // rebuild when the geometry key actually changed (a pure translate
          // keeps it, so it stays cheap). Defer the old geometry's GPU disposal
          // (the sgzw "Buffer used in submit while destroyed" race).
          const key = leafGeoKey(leaf);
          if (m.userData.geoKey !== key) {
            const fresh = createPrimitiveGeometry(leaf);
            if (fresh) {
              tagBodyId(fresh, bodyIndex);
              const retiredGeo = m.geometry;
              m.geometry = fresh;
              m.userData.geoKey = key;
              disposeAfterGpuIdle(renderer, () => retiredGeo.dispose());
            }
          }
          applyLeafTransform(m, leaf);
        }
      }
    };
    updateMeshList(opaqueMeshes);
    updateMeshList(transMeshes);
    updateMeshList(backMeshes);
  }

  updateVolumeData(
    state.volumeTextures,
    state.volumeBaseData,
    bodies,
    DEFERRED_MAX_BODIES,
  );
  state.volumeBaseAttr.needsUpdate = true;
}

export function updateDeferredSelection(
  state: DeferredPipelineState,
  selectedBodies: Set<number>,
): void {
  state.selectionData.fill(0);
  for (const idx of selectedBodies) {
    if (idx < DEFERRED_MAX_BODIES) state.selectionData[idx] = 1;
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
  renderer?: WebGPURenderer,
): void {
  // Ticket fg9o: detach the retired meshes from their scenes NOW (they stop
  // rendering immediately) but defer the geometry/material/texture GPU disposal
  // until the device confirms it's done with every in-flight submit. Destroying
  // them synchronously raced the previous frame's submit → "Buffer used in
  // submit while destroyed" → frozen device.
  // Only GEOMETRIES retire — the G-buffer / back-face materials are shared
  // across every body and live on the pipeline state (ticket 853b80c5); they
  // are disposed once, in disposeDeferredPipeline.
  const retired: THREE.BufferGeometry[] = [];
  for (const meshes of state.bodyOpaqueMeshes.values()) {
    for (const m of meshes) {
      state.opaqueScene.remove(m);
      retired.push(m.geometry);
    }
  }
  for (const meshes of state.bodyTransparentMeshes.values()) {
    for (const m of meshes) {
      state.transparentScene.remove(m);
      retired.push(m.geometry);
    }
  }
  for (const meshes of state.bodyBackMeshes.values()) {
    for (const m of meshes) {
      state.transparentBackScene.remove(m);
      retired.push(m.geometry);
    }
  }
  const retiredTextures = state.volumeTextures;
  disposeAfterGpuIdle(renderer, () => {
    for (const g of retired) g.dispose();
    for (const tex of retiredTextures) tex.dispose();
  });
  state.bodyOpaqueMeshes.clear();
  state.bodyTransparentMeshes.clear();
  state.bodyBackMeshes.clear();
  state.bodyOpcodeRanges.clear();

  const ranges = packOpcodes(
    state.opcodeData,
    state.offsetData,
    bodies,
    SDF_MAX_OPCODES,
    DEFERRED_MAX_BODIES,
  );
  state.opcodeAttr.needsUpdate = true;
  state.offsetAttr.needsUpdate = true;
  for (const [bi, range] of ranges) state.bodyOpcodeRanges.set(bi, range);

  for (const { bodyIndex, hybridInfo, opcodes, aabb } of bodies) {
    if (bodyIndex >= DEFERRED_MAX_BODIES) continue;
    if (!ranges.has(bodyIndex)) continue;

    const gbufMaterial = state.gbufferMaterial;
    const transMat = state.transparentGbufferMaterial;
    const opaqueMeshes: THREE.Mesh[] = [];
    const transMeshes: THREE.Mesh[] = [];
    const backMeshes: THREE.Mesh[] = [];

    if (hybridInfo.needs_full_sdf) {
      const bodyAABB = aabb
        ? ([aabb[0], aabb[1], aabb[2], aabb[3], aabb[4], aabb[5]] as const)
        : computeLeafAABB(hybridInfo.leaves);
      if (bodyAABB) {
        addAabbBox(
          bodyAABB,
          transMat,
          state.transparentScene,
          transMeshes,
          bodyIndex,
        );
        transMeshes[transMeshes.length - 1].userData.isAabb = true;
        addAabbBox(
          bodyAABB,
          state.backFaceMaterial,
          state.transparentBackScene,
          backMeshes,
          bodyIndex,
        );
        backMeshes[backMeshes.length - 1].userData.isAabb = true;
      }
    } else {
      for (let leafIdx = 0; leafIdx < hybridInfo.leaves.length; leafIdx++) {
        const leaf = hybridInfo.leaves[leafIdx];
        if (leaf.role === "skip") continue;
        const geo = createPrimitiveGeometry(leaf);
        if (!geo) continue;
        tagBodyId(geo, bodyIndex);
        // Record the geometry key so a later data-only update (ticket c9ic)
        // can tell a shape edit (depth/profile) from a pure transform.
        const gKey = leafGeoKey(leaf);

        // A convex box hull's front face IS its surface, so for a PURE-UNION
        // body we can rasterize the box as opaque only and skip the sphere-trace
        // (the shade pass's opaque-only branch shades that surface directly).
        // BUT when the body is NOT union-only (it carries a Difference/
        // Intersect — `is_union_only=false`), the opcode SDF carves the box: its
        // raw hull is NOT the final surface. Such a box leaf MUST get a
        // transparent + back-face march proxy like every other leaf, so the
        // shade pass sphere-traces the boolean SDF and the hole gets carved.
        // Without this, box-minus-cylinder rendered as a SOLID box — the
        // subtracted bore was invisible in the deferred SDF (ticket 87bf;
        // regression surfaced once orthographic became the default camera in
        // 0cdb6c1, but the opaque-box-only shortcut had been unsound for
        // non-union bodies since the renderer's inception).
        const boxOpaqueOnly =
          leaf.kind === "box" && hybridInfo.is_union_only;

        if (boxOpaqueOnly) {
          const mesh = new THREE.Mesh(geo, gbufMaterial);
          applyLeafTransform(mesh, leaf);
          mesh.frustumCulled = false;
          mesh.userData.leafIdx = leafIdx;
          mesh.userData.geoKey = gKey;
          state.opaqueScene.add(mesh);
          opaqueMeshes.push(mesh);
        } else {
          const mesh = new THREE.Mesh(geo, gbufMaterial);
          applyLeafTransform(mesh, leaf);
          mesh.frustumCulled = false;
          mesh.userData.leafIdx = leafIdx;
          mesh.userData.geoKey = gKey;
          state.opaqueScene.add(mesh);
          opaqueMeshes.push(mesh);

          // Transparent- und Back-Pass nutzen dasselbe EXAKTE parametrische
          // Mesh wie der Opaque-Pass (createPrimitiveGeometry(leaf)) -- keine
          // Inflation. Drei separate Instanzen, damit die Disposal-Logik unten
          // jede Mesh-Geometrie einzeln freigeben kann. Konservative
          // Coverage-Aufweitung (Silhouetten-Dilatation) folgt in späterem Schritt.
          const tGeo = createPrimitiveGeometry(leaf);
          if (tGeo) {
            tagBodyId(tGeo, bodyIndex);
            const tMesh = new THREE.Mesh(tGeo, transMat);
            applyLeafTransform(tMesh, leaf);
            tMesh.frustumCulled = false;
            tMesh.userData.leafIdx = leafIdx;
            tMesh.userData.geoKey = gKey;
            state.transparentScene.add(tMesh);
            transMeshes.push(tMesh);
          }

          const bGeo = createPrimitiveGeometry(leaf);
          if (bGeo) {
            tagBodyId(bGeo, bodyIndex);
            const bMesh = new THREE.Mesh(bGeo, state.backFaceMaterial);
            applyLeafTransform(bMesh, leaf);
            bMesh.frustumCulled = false;
            bMesh.userData.leafIdx = leafIdx;
            bMesh.userData.geoKey = gKey;
            state.transparentBackScene.add(bMesh);
            backMeshes.push(bMesh);
          }
        }
      }

      if (
        opaqueMeshes.length === 0 &&
        transMeshes.length === 0 &&
        opcodes.length > 0
      ) {
        const bodyAABB = aabb
          ? ([aabb[0], aabb[1], aabb[2], aabb[3], aabb[4], aabb[5]] as const)
          : computeLeafAABB(hybridInfo.leaves);
        if (bodyAABB) {
          addAabbBox(
            bodyAABB,
            transMat,
            state.transparentScene,
            transMeshes,
            bodyIndex,
          );
          transMeshes[transMeshes.length - 1].userData.isAabb = true;
          addAabbBox(
            bodyAABB,
            state.backFaceMaterial,
            state.transparentBackScene,
            backMeshes,
            bodyIndex,
          );
          backMeshes[backMeshes.length - 1].userData.isAabb = true;
        }
      }
    }

    state.bodyOpaqueMeshes.set(bodyIndex, opaqueMeshes);
    state.bodyTransparentMeshes.set(bodyIndex, transMeshes);
    state.bodyBackMeshes.set(bodyIndex, backMeshes);
  }

  const { textures, countChanged } = rebuildVolumeTextures(
    state.volumeTextures,
    state.volumeBaseData,
    bodies,
    DEFERRED_MAX_BODIES,
    false, // old textures disposed via disposeAfterGpuIdle above (ticket fg9o)
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
  bodyIndex: number,
): void {
  const pad = AABB_HULL_PAD;
  const sx = aabb[3] - aabb[0] + pad * 2;
  const sy = aabb[4] - aabb[1] + pad * 2;
  const sz = aabb[5] - aabb[2] + pad * 2;
  const geo = tagBodyId(new THREE.BoxGeometry(sx, sy, sz), bodyIndex);
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
  let minX = Infinity,
    minY = Infinity,
    minZ = Infinity;
  let maxX = -Infinity,
    maxY = -Infinity,
    maxZ = -Infinity;
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

export function rebuildShadeMaterialIfNeeded(
  state: DeferredPipelineState,
  renderer?: WebGPURenderer,
): void {
  if (!state.needsShadeRebuild) return;
  state.needsShadeRebuild = false;

  // Ticket fg9o: the shade material runs in the per-frame shade pass — detach
  // it now, dispose only once the GPU is done with the frames that used it.
  const oldShadeMaterial = state.shadeMaterial;
  const oldMesh = state.shadeScene.children[0] as THREE.Mesh | undefined;
  if (oldMesh) state.shadeScene.remove(oldMesh);
  disposeAfterGpuIdle(renderer, () => {
    oldShadeMaterial.dispose();
    oldMesh?.geometry.dispose();
  });

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
    state.marchRelaxBuf,
    state.camPosU,
    state.camFwdU,
    state.viewMatU,
    state.widthU,
    state.nearU,
    state.farU,
    state.bgColorU,
    state.isOrthoU,
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
  for (const rt of [state.opaqueRT, state.transparentRT, state.shadeRT]) {
    if (rt.depthTexture) {
      rt.depthTexture.dispose();
      rt.depthTexture = new THREE.DepthTexture(width, height, THREE.FloatType);
    }
    rt.setSize(width, height);
  }
  state.transparentBackRT.setSize(width, height);
  state.lastFrameKey = null;
  state.widthU.value = width;
  state.clearNode.count = width * height * BODY_ID_WORDS_PER_PIXEL;
  state.width = width;
  state.height = height;
}

/** Stamp the body id (bodyIndex + 1) onto every vertex of a proxy geometry.
 *  The G-buffer shaders read it as an attribute instead of a baked constant,
 *  so ONE program serves every body (ticket 853b80c5). */
function tagBodyId(geo: THREE.BufferGeometry, bodyIndex: number): THREE.BufferGeometry {
  const count = geo.getAttribute("position")?.count ?? 0;
  const ids = new Float32Array(count).fill(bodyIndex + 1);
  geo.setAttribute("bodyId", new THREE.BufferAttribute(ids, 1));
  return geo;
}

const _savedClearColor = new THREE.Color();

/** Everything OUTSIDE the kernel fingerprints that changes the shade
 *  result: camera pose + projection, canvas size, background colour.
 *  When this key AND the scene are unchanged, the cached shadeRT frame
 *  is byte-identical and the whole SDF pipeline can be skipped. */
function deferredFrameKey(
  state: DeferredPipelineState,
  camera: THREE.Camera,
  clearColor: THREE.Color,
  clearAlpha: number,
): string {
  return (
    camera.matrixWorld.elements.join(",") +
    "|" +
    camera.projectionMatrix.elements.join(",") +
    "|" +
    state.width +
    "x" +
    state.height +
    "|" +
    clearColor.getHexString() +
    clearAlpha
  );
}

export function renderDeferredFrame(
  state: DeferredPipelineState,
  renderer: WebGPURenderer,
  camera: THREE.Camera,
  /** Did the SDF scene content change since the last frame (rebuild /
   *  data update / material / selection upload)? `true` forces a full
   *  re-render; `false` allows replaying the cached shade result when
   *  the camera is also unchanged (ticket 40001a3a: overlay-only
   *  invalidates — sketch hover/drag — must not re-march the SDF). */
  sceneDirty = true,
): void {
  updateCameraUniforms(state, camera);

  // getClearColor's typing wants the renderer-internal Color4; a plain Color
  // receives the rgb components just fine.
  renderer.getClearColor(
    _savedClearColor as Parameters<WebGPURenderer["getClearColor"]>[0],
  );
  const savedClearAlpha = renderer.getClearAlpha();
  // Feed the viewport background (ClearColorSync) into the shade shader so SDF
  // miss pixels paint it opaquely (ticket a2ps).
  state.bgColorU.value.copy(_savedClearColor);

  const frameKey = deferredFrameKey(
    state,
    camera,
    _savedClearColor,
    savedClearAlpha,
  );
  if (sceneDirty || frameKey !== state.lastFrameKey) {
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

    // Pass 3: shade (reads all RTs, sphere-traces) into the cache target.
    renderer.setRenderTarget(state.shadeRT);
    renderer.clear(true, true, false);
    renderer.render(state.shadeScene, state.shadeCamera);

    state.lastFrameKey = frameKey;
  }

  // Blit the (fresh or cached) shade result — colour + per-pixel depth —
  // to the screen. The depth replay keeps the shared z-buffer contract
  // with the datum/overlay passes (ticket 1m8g).
  renderer.setRenderTarget(null);
  renderer.setClearColor(_savedClearColor, savedClearAlpha);
  renderer.render(state.blitScene, state.shadeCamera);
}

// ── Async pipeline compilation ─────────────────────────────────────

export async function compileDeferredPipeline(
  state: DeferredPipelineState,
  renderer: WebGPURenderer,
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

  renderer.setRenderTarget(state.shadeRT);
  await renderer.compileAsync(state.shadeScene, state.shadeCamera);

  renderer.setRenderTarget(null);
  await renderer.compileAsync(state.blitScene, state.shadeCamera);

  renderer.setRenderTarget(prevTarget);

  if (device) {
    const err = await device.popErrorScope();
    if (err) throw new Error(err.message);
  }
}

// ── Dispose ─────────────────────────────────────────────────────────

export function disposeDeferredPipeline(
  state: DeferredPipelineState,
  renderer?: WebGPURenderer,
): void {
  // Ticket 323i: free the GPU resources only AFTER the device confirms it's
  // done with every in-flight submit. Tearing the pipeline down synchronously
  // (render-mode switch / StrictMode remount / unmount) raced the previous
  // frame's submit → "Buffer used in submit while destroyed" → frozen device —
  // the same race the rebuild path already guards (fg9o), just on teardown.
  // Detach the JS bookkeeping now; the actual `.dispose()` waits for GPU idle.
  const geometries: THREE.BufferGeometry[] = [];
  const materials: THREE.Material[] = [];
  for (const meshes of state.bodyOpaqueMeshes.values()) {
    for (const m of meshes) geometries.push(m.geometry);
  }
  for (const meshes of state.bodyTransparentMeshes.values()) {
    for (const m of meshes) geometries.push(m.geometry);
  }
  for (const meshes of state.bodyBackMeshes.values()) {
    for (const m of meshes) geometries.push(m.geometry);
  }
  state.bodyOpaqueMeshes.clear();
  state.bodyTransparentMeshes.clear();
  state.bodyBackMeshes.clear();
  const textures = state.volumeTextures;
  state.volumeTextures = [];
  const renderTargets = [
    state.opaqueRT,
    state.transparentRT,
    state.transparentBackRT,
    state.shadeRT,
  ];
  const pipelineMaterials = [
    state.backFaceMaterial,
    state.shadeMaterial,
    state.blitMaterial,
    state.gbufferMaterial,
    state.transparentGbufferMaterial,
  ];
  // disposeAfterGpuIdle falls back to synchronous disposal when no renderer /
  // queue is available, so older callers keep working unchanged.
  disposeAfterGpuIdle(renderer, () => {
    for (const g of geometries) g.dispose();
    for (const m of materials) m.dispose();
    for (const t of textures) t.dispose();
    for (const rt of renderTargets) rt.dispose();
    for (const m of pipelineMaterials) m.dispose();
  });
}
