import { useEffect, useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import type { WebGPURenderer } from "three/webgpu";
import { useKernelStore } from "./store";
import {
  initVmPipeline,
  rebuildVmBodies,
  updateVmData,
  compileVmPipeline,
  renderVmFrame,
  disposeVmPipeline,
  type VmPipelineState,
} from "./sdfVmRenderer";
import {
  initDeferredPipeline,
  resizeDeferredPipeline,
  rebuildDeferredBodies,
  updateDeferredData,
  updateDeferredSelection,
  rebuildShadeMaterialIfNeeded,
  renderDeferredFrame,
  compileDeferredPipeline,
  disposeDeferredPipeline,
  type DeferredPipelineState,
} from "./sdfDeferredRenderer";
import { resolveMaterials, applyMaterials } from "./materialBridge";

export const OVERLAY_LAYER = 1;

type BodyVolumes = { data: Float32Array; dims: [number, number, number] }[];

interface KernelLike {
  bodyCount(): number;
  bodyId(i: number): number;
  bodyVersion(i: number): number;
  bodySdfOpcodesView(i: number): Float32Array;
  bodyVolumeCount(i: number): number;
  bodyVolumeDataView(i: number, v: number): Float32Array;
  bodyVolumeDims(i: number, v: number): [number, number, number];
}

function collectBodyVolumes(k: KernelLike, bodyIndex: number): BodyVolumes | undefined {
  const volCount = k.bodyVolumeCount(bodyIndex);
  if (volCount === 0) return undefined;
  const volumes: BodyVolumes = [];
  for (let v = 0; v < volCount; v++) {
    volumes.push({ data: k.bodyVolumeDataView(bodyIndex, v), dims: k.bodyVolumeDims(bodyIndex, v) });
  }
  return volumes;
}

function computeFingerprints(k: KernelLike, count: number): { volumeFp: string; structFp: string; dataFp: string } {
  let totalVolumes = 0;
  const structParts: (string | number)[] = [count];
  const dataParts: number[] = [];
  for (let i = 0; i < count; i++) {
    const volCount = k.bodyVolumeCount(i);
    totalVolumes += volCount;
    structParts.push(k.bodyId(i), k.bodySdfOpcodesView(i).length, volCount);
    dataParts.push(k.bodyVersion(i));
  }
  return { volumeFp: String(totalVolumes), structFp: structParts.join(","), dataFp: dataParts.join(",") };
}

function showCompilationStatus(text: string): void {
  const overlay = document.getElementById("fd-shader-compiling");
  const label = document.getElementById("fd-shader-compiling-label");
  if (overlay) overlay.style.display = "flex";
  if (label) label.textContent = text;
}

function hideCompilationOverlay(): void {
  const overlay = document.getElementById("fd-shader-compiling");
  if (overlay) overlay.style.display = "none";
}

function showGpuError(err: unknown): void {
  const overlay = document.getElementById("fd-shader-compiling");
  const label = document.getElementById("fd-shader-compiling-label");
  const spinnerEl = overlay?.querySelector(".spinner") as HTMLElement | null;
  if (spinnerEl) spinnerEl.style.display = "none";
  if (label) label.textContent = `GPU error: ${err instanceof Error ? err.message : String(err)}`;
  if (overlay) {
    overlay.style.display = "flex";
    overlay.style.opacity = "1";
    overlay.style.color = "var(--color-error, #c00)";
  }
}

function renderOverlayLayer(
  wgpuRenderer: WebGPURenderer,
  scene: THREE.Scene,
  camera: THREE.Camera,
  renderSdf: () => void,
): void {
  wgpuRenderer.render(scene, camera);
  const prevAutoClear = (wgpuRenderer as any).autoClear;
  (wgpuRenderer as any).autoClear = false;
  renderSdf();

  const savedBg = scene.background;
  scene.background = null;
  const savedMask = camera.layers.mask;
  camera.layers.set(OVERLAY_LAYER);
  wgpuRenderer.clearDepth();
  wgpuRenderer.render(scene, camera);
  camera.layers.mask = savedMask;
  scene.background = savedBg;

  (wgpuRenderer as any).autoClear = prevAutoClear;
}

function copyOpcodes(k: KernelLike, bodyIndex: number): Float32Array {
  const view = k.bodySdfOpcodesView(bodyIndex);
  return view.length > 0 ? view.slice() : new Float32Array(0);
}

export function SdfVmOverlay({ showEdges }: { showEdges: boolean }) {
  const { store } = useKernelStore();
  const scene = useThree((s) => s.scene);
  const invalidate = useThree((s) => s.invalidate);

  const pipelineRef = useRef<VmPipelineState | null>(null);
  const volumeFpRef = useRef("");
  const dataFpRef = useRef("");
  const compilingRef = useRef(false);
  const readyRef = useRef(false);
  const matFeaturesRef = useRef<unknown>(null);

  useEffect(() => {
    pipelineRef.current = initVmPipeline();
    volumeFpRef.current = "";
    dataFpRef.current = "";
    matFeaturesRef.current = null;
    compilingRef.current = false;
    readyRef.current = false;
    showCompilationStatus("Initializing…");

    return () => {
      if (pipelineRef.current) {
        disposeVmPipeline(pipelineRef.current);
        pipelineRef.current = null;
      }
    };
  }, []);

  useFrame(({ gl: renderer, camera }) => {
    if (!store || !pipelineRef.current) return;
    const pipeline = pipelineRef.current;
    const k = store.kernel;
    const count = k.bodyCount();

    const { volumeFp, structFp, dataFp } = computeFingerprints(k, count);
    const needsCompile = volumeFp !== volumeFpRef.current;
    const needsDataUpdate = !needsCompile && (structFp + dataFp) !== dataFpRef.current;

    if (needsCompile && !compilingRef.current) {
      showCompilationStatus("Building SDF…");

      const bodies = [];
      for (let i = 0; i < count; i++) {
        bodies.push({ bodyIndex: i, opcodes: copyOpcodes(k, i), volumes: collectBodyVolumes(k, i) });
      }
      rebuildVmBodies(pipeline, bodies);
      volumeFpRef.current = volumeFp;
      dataFpRef.current = structFp + dataFp;

      showCompilationStatus("Compiling shader…");
      compilingRef.current = true;
      const wgpu = renderer as unknown as WebGPURenderer;
      compileVmPipeline(pipeline, wgpu, camera)
        .then(() => {
          // Ignore a compile that finished for a pipeline we've since
          // replaced (StrictMode remounts dispose the old pipeline while
          // its async compile is still in flight). Acting on a stale
          // result would flip `readyRef` true for a disposed pipeline and
          // wedge the loop on a blank frame.
          if (pipeline !== pipelineRef.current) return;
          compilingRef.current = false;
          readyRef.current = true;
          hideCompilationOverlay();
          invalidate();
        })
        .catch((err) => {
          if (pipeline !== pipelineRef.current) return;
          compilingRef.current = false;
          showGpuError(err);
        });
    }

    if (needsDataUpdate && readyRef.current && !compilingRef.current) {
      const bodies = [];
      for (let i = 0; i < count; i++) {
        bodies.push({ bodyIndex: i, opcodes: copyOpcodes(k, i), volumes: collectBodyVolumes(k, i) });
      }
      updateVmData(pipeline, bodies);
      dataFpRef.current = structFp + dataFp;
    }

    // Materials only change when the feature list changes (any edit commits a
    // fresh array) or when geometry rebuilt/updated this frame. Gate the
    // resolve+GPU upload behind that so a static scene doesn't re-upload the
    // material buffer every frame.
    const featuresArr = store.getState().features;
    if (featuresArr !== matFeaturesRef.current || needsCompile || needsDataUpdate) {
      const vmMaterials = resolveMaterials(k, featuresArr);
      applyMaterials(pipeline, vmMaterials, count);
      matFeaturesRef.current = featuresArr;
    }
    pipeline.showEdgesU.value = showEdges ? 1 : 0;

    if (!readyRef.current || compilingRef.current) return;

    const wgpuRenderer = renderer as unknown as WebGPURenderer;
    try {
      renderOverlayLayer(wgpuRenderer, scene, camera, () =>
        renderVmFrame(pipeline, wgpuRenderer, camera),
      );
    } catch (err) {
      readyRef.current = false;
      showGpuError(err);
    }

    invalidate();
  }, 1);

  return null;
}

export function SdfDeferredOverlay({ showEdges }: { showEdges: boolean }) {
  const { store } = useKernelStore();
  const scene = useThree((s) => s.scene);
  const size = useThree((s) => s.size);
  const invalidate = useThree((s) => s.invalidate);

  const pipelineRef = useRef<DeferredPipelineState | null>(null);
  const structFpRef = useRef("");
  const dataFpRef = useRef("");
  const compilingRef = useRef(false);
  const readyRef = useRef(false);
  const matFeaturesRef = useRef<unknown>(null);
  const selectionRef = useRef<unknown>(null);

  useEffect(() => {
    pipelineRef.current = initDeferredPipeline(1, 1);
    structFpRef.current = "";
    dataFpRef.current = "";
    matFeaturesRef.current = null;
    selectionRef.current = null;
    compilingRef.current = false;
    readyRef.current = false;
    showCompilationStatus("Initializing…");

    return () => {
      if (pipelineRef.current) {
        disposeDeferredPipeline(pipelineRef.current);
        pipelineRef.current = null;
      }
    };
  }, []);

  useEffect(() => {
    if (!pipelineRef.current) return;
    const w = Math.max(size.width, 1);
    const h = Math.max(size.height, 1);
    resizeDeferredPipeline(pipelineRef.current, w, h);
    invalidate();
  }, [size.width, size.height]);

  useFrame(({ gl: renderer, camera }) => {
    if (!store || !pipelineRef.current) return;
    const pipeline = pipelineRef.current;
    const k = store.kernel;
    const count = k.bodyCount();

    const { structFp, dataFp } = computeFingerprints(k, count);
    const needsRebuild = structFp !== structFpRef.current;
    const needsDataUpdate = !needsRebuild && dataFp !== dataFpRef.current;

    const collectDeferredBodies = () => {
      const bodies = [];
      for (let i = 0; i < count; i++) {
        bodies.push({
          bodyIndex: i,
          hybridInfo: k.bodyHybridInfo(i),
          opcodes: copyOpcodes(k, i),
          aabb: k.bodySdfAabb(i),
          volumes: collectBodyVolumes(k, i),
        });
      }
      return bodies;
    };

    if (needsRebuild && !compilingRef.current) {
      showCompilationStatus("Building SDF…");

      rebuildDeferredBodies(pipeline, collectDeferredBodies());
      rebuildShadeMaterialIfNeeded(pipeline);
      structFpRef.current = structFp;
      dataFpRef.current = dataFp;

      showCompilationStatus("Compiling shader…");
      compilingRef.current = true;
      const wgpu = renderer as unknown as WebGPURenderer;
      compileDeferredPipeline(pipeline, wgpu, camera)
        .then(() => {
          // Ignore a compile that finished for a pipeline we've since
          // replaced (StrictMode remounts dispose the old pipeline while
          // its async compile is still in flight). Acting on a stale
          // result would flip `readyRef` true for a disposed pipeline and
          // wedge the loop on a blank frame.
          if (pipeline !== pipelineRef.current) return;
          compilingRef.current = false;
          readyRef.current = true;
          hideCompilationOverlay();
          invalidate();
        })
        .catch((err) => {
          if (pipeline !== pipelineRef.current) return;
          compilingRef.current = false;
          showGpuError(err);
        });
    }

    if (needsDataUpdate && readyRef.current && !compilingRef.current) {
      updateDeferredData(pipeline, collectDeferredBodies());
      dataFpRef.current = dataFp;
    }


    const storeState = store.getState();

    // Re-resolve + re-upload materials only when the feature list changed or
    // geometry rebuilt/updated this frame — not on every static frame.
    if (storeState.features !== matFeaturesRef.current || needsRebuild || needsDataUpdate) {
      const materials = resolveMaterials(k, storeState.features);
      applyMaterials(pipeline, materials, count);
      matFeaturesRef.current = storeState.features;
    }

    // Selection likewise only changes via a fresh selection Set (or when the
    // body set rebuilt).
    const selection = storeState.selection;
    if (selection !== selectionRef.current || needsRebuild) {
      const selectedBodies = new Set<number>();
      for (let i = 0; i < count; i++) {
        if (selection.has(k.bodySeedFeatureId(i))) selectedBodies.add(i);
      }
      updateDeferredSelection(pipeline, selectedBodies);
      selectionRef.current = selection;
    }
    pipeline.showEdgesU.value = showEdges ? 1 : 0;

    if (!readyRef.current || compilingRef.current) return;

    const wgpuRenderer = renderer as unknown as WebGPURenderer;
    try {
      renderOverlayLayer(wgpuRenderer, scene, camera, () =>
        renderDeferredFrame(pipeline, wgpuRenderer, camera),
      );
    } catch (err) {
      console.error("SDF render error:", err);
      readyRef.current = false;
      showGpuError(err);
    }

    invalidate();
  }, 1);

  return null;
}
