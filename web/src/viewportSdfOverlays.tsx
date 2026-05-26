import { useEffect, useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
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

export function SdfVmOverlay() {
  const { store } = useKernelStore();
  const scene = useThree((s) => s.scene);
  const invalidate = useThree((s) => s.invalidate);

  const pipelineRef = useRef<VmPipelineState | null>(null);
  const structFpRef = useRef("");
  const dataFpRef = useRef("");
  const compilingRef = useRef(false);
  const readyRef = useRef(false);

  useEffect(() => {
    pipelineRef.current = initVmPipeline();
    structFpRef.current = "";
    dataFpRef.current = "";
    compilingRef.current = false;
    readyRef.current = false;

    const overlay = document.getElementById("fd-shader-compiling");
    const label = document.getElementById("fd-shader-compiling-label");
    if (overlay) overlay.style.display = "flex";
    if (label) label.textContent = "Initializing…";

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

    const structParts: (string | number)[] = [count];
    const dataParts: number[] = [];
    for (let i = 0; i < count; i++) {
      structParts.push(k.bodyId(i), k.bodySdfOpcodesView(i).length, k.bodyVolumeCount(i));
      dataParts.push(k.bodyVersion(i));
    }
    const structFp = structParts.join(",");
    const dataFp = dataParts.join(",");
    const needsCompile = structFp !== structFpRef.current;
    const needsDataUpdate = !needsCompile && dataFp !== dataFpRef.current;

    if (needsCompile && !compilingRef.current) {
      const overlay = document.getElementById("fd-shader-compiling");
      const label = document.getElementById("fd-shader-compiling-label");
      if (overlay) overlay.style.display = "flex";
      if (label) label.textContent = "Building SDF…";

      const bodies = [];
      for (let i = 0; i < count; i++) {
        const opcodesView = k.bodySdfOpcodesView(i);
        const opcodes = opcodesView.length > 0 ? opcodesView.slice() : new Float32Array(0);
        const volCount = k.bodyVolumeCount(i);
        let volumes: { data: Float32Array; dims: [number, number, number] }[] | undefined;
        if (volCount > 0) {
          volumes = [];
          for (let v = 0; v < volCount; v++) {
            volumes.push({ data: k.bodyVolumeDataView(i, v), dims: k.bodyVolumeDims(i, v) });
          }
        }
        bodies.push({ bodyIndex: i, opcodes, volumes });
      }
      rebuildVmBodies(pipeline, bodies);
      structFpRef.current = structFp;
      dataFpRef.current = dataFp;

      if (label) label.textContent = "Compiling shader…";
      compilingRef.current = true;
      const wgpu = renderer as unknown as THREE.WebGPURenderer;
      compileVmPipeline(pipeline, wgpu, camera)
        .then(() => {
          compilingRef.current = false;
          readyRef.current = true;
          if (overlay) overlay.style.display = "none";
          invalidate();
        })
        .catch((err) => {
          compilingRef.current = false;
          const spinnerEl = overlay?.querySelector(".spinner") as HTMLElement | null;
          if (spinnerEl) spinnerEl.style.display = "none";
          if (label) label.textContent = `GPU error: ${err.message}`;
          if (overlay) {
            overlay.style.opacity = "1";
            overlay.style.color = "var(--color-error, #c00)";
          }
        });
    }

    if (needsDataUpdate && readyRef.current && !compilingRef.current) {
      const bodies = [];
      for (let i = 0; i < count; i++) {
        const opcodesView = k.bodySdfOpcodesView(i);
        const opcodes = opcodesView.length > 0 ? opcodesView.slice() : new Float32Array(0);
        const volCount = k.bodyVolumeCount(i);
        let volumes: { data: Float32Array; dims: [number, number, number] }[] | undefined;
        if (volCount > 0) {
          volumes = [];
          for (let v = 0; v < volCount; v++) {
            volumes.push({ data: k.bodyVolumeDataView(i, v), dims: k.bodyVolumeDims(i, v) });
          }
        }
        bodies.push({ bodyIndex: i, opcodes, volumes });
      }
      updateVmData(pipeline, bodies);
      dataFpRef.current = dataFp;
    }

    const vmMaterials = resolveMaterials(k, store.getState().features);
    applyMaterials(pipeline, vmMaterials, count);

    if (!readyRef.current || compilingRef.current) return;

    const wgpuRenderer = renderer as unknown as THREE.WebGPURenderer;
    try {
      wgpuRenderer.render(scene, camera);
      const prevAutoClear = (wgpuRenderer as any).autoClear;
      (wgpuRenderer as any).autoClear = false;
      renderVmFrame(pipeline, wgpuRenderer, camera);

      const savedBg = scene.background;
      scene.background = null;
      const savedMask = camera.layers.mask;
      camera.layers.set(OVERLAY_LAYER);
      wgpuRenderer.clearDepth();
      wgpuRenderer.render(scene, camera);
      camera.layers.mask = savedMask;
      scene.background = savedBg;

      (wgpuRenderer as any).autoClear = prevAutoClear;
    } catch (err) {
      readyRef.current = false;
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

    invalidate();
  }, 1);

  return null;
}

export function SdfDeferredOverlay() {
  const { store } = useKernelStore();
  const scene = useThree((s) => s.scene);
  const size = useThree((s) => s.size);
  const invalidate = useThree((s) => s.invalidate);

  const pipelineRef = useRef<DeferredPipelineState | null>(null);
  const structFpRef = useRef("");
  const dataFpRef = useRef("");
  const compilingRef = useRef(false);
  const readyRef = useRef(false);

  useEffect(() => {
    pipelineRef.current = initDeferredPipeline(1, 1);
    structFpRef.current = "";
    dataFpRef.current = "";
    compilingRef.current = false;
    readyRef.current = false;

    const overlay = document.getElementById("fd-shader-compiling");
    const label = document.getElementById("fd-shader-compiling-label");
    if (overlay) overlay.style.display = "flex";
    if (label) label.textContent = "Initializing…";

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

    const structParts: (string | number)[] = [count];
    const dataParts: number[] = [];
    for (let i = 0; i < count; i++) {
      structParts.push(k.bodyId(i), k.bodySdfOpcodesView(i).length, k.bodyVolumeCount(i));
      dataParts.push(k.bodyVersion(i));
    }
    const structFp = structParts.join(",");
    const dataFp = dataParts.join(",");
    const needsCompile = structFp !== structFpRef.current;
    const needsDataUpdate = !needsCompile && dataFp !== dataFpRef.current;

    if (needsCompile && !compilingRef.current) {
      const overlay = document.getElementById("fd-shader-compiling");
      const label = document.getElementById("fd-shader-compiling-label");

      if (overlay) overlay.style.display = "flex";
      if (label) label.textContent = "Building SDF…";

      const bodies = [];
      for (let i = 0; i < count; i++) {
        const hybridInfo = k.bodyHybridInfo(i);
        const opcodesView = k.bodySdfOpcodesView(i);
        const opcodes = opcodesView.length > 0 ? opcodesView.slice() : new Float32Array(0);
        const aabb = k.bodySdfAabb(i);

        const volCount = k.bodyVolumeCount(i);
        let volumes: { data: Float32Array; dims: [number, number, number] }[] | undefined;
        if (volCount > 0) {
          volumes = [];
          for (let v = 0; v < volCount; v++) {
            volumes.push({ data: k.bodyVolumeDataView(i, v), dims: k.bodyVolumeDims(i, v) });
          }
        }

        bodies.push({ bodyIndex: i, hybridInfo, opcodes, aabb, volumes });
      }
      rebuildDeferredBodies(pipeline, bodies);
      rebuildShadeMaterialIfNeeded(pipeline);
      structFpRef.current = structFp;
      dataFpRef.current = dataFp;

      if (label) label.textContent = "Compiling shader…";
      compilingRef.current = true;
      const wgpu = renderer as unknown as THREE.WebGPURenderer;
      compileDeferredPipeline(pipeline, wgpu, camera)
        .then(() => {
          compilingRef.current = false;
          readyRef.current = true;
          if (overlay) overlay.style.display = "none";
          invalidate();
        })
        .catch((err) => {
          compilingRef.current = false;
          const spinnerEl = overlay?.querySelector(".spinner") as HTMLElement | null;
          if (spinnerEl) spinnerEl.style.display = "none";
          if (label) label.textContent = `GPU error: ${err.message}`;
          if (overlay) {
            overlay.style.opacity = "1";
            overlay.style.color = "var(--color-error, #c00)";
          }
        });
    }

    if (needsDataUpdate && readyRef.current && !compilingRef.current) {
      const bodies = [];
      for (let i = 0; i < count; i++) {
        const hybridInfo = k.bodyHybridInfo(i);
        const opcodesView = k.bodySdfOpcodesView(i);
        const opcodes = opcodesView.length > 0 ? opcodesView.slice() : new Float32Array(0);
        const aabb = k.bodySdfAabb(i);

        const volCount = k.bodyVolumeCount(i);
        let volumes: { data: Float32Array; dims: [number, number, number] }[] | undefined;
        if (volCount > 0) {
          volumes = [];
          for (let v = 0; v < volCount; v++) {
            volumes.push({ data: k.bodyVolumeDataView(i, v), dims: k.bodyVolumeDims(i, v) });
          }
        }

        bodies.push({ bodyIndex: i, hybridInfo, opcodes, aabb, volumes });
      }
      updateDeferredData(pipeline, bodies);
      dataFpRef.current = dataFp;
    }

    const storeState = store.getState();
    const materials = resolveMaterials(k, storeState.features);
    applyMaterials(pipeline, materials, count);

    const selection = storeState.selection;
    const selectedBodies = new Set<number>();
    for (let i = 0; i < count; i++) {
      if (selection.has(k.bodySeedFeatureId(i))) selectedBodies.add(i);
    }
    updateDeferredSelection(pipeline, selectedBodies);

    if (!readyRef.current || compilingRef.current) {
        return;
    }
    const wgpuRenderer = renderer as unknown as THREE.WebGPURenderer;

    try {
      wgpuRenderer.render(scene, camera);

      const prevAutoClear = (wgpuRenderer as any).autoClear;
      (wgpuRenderer as any).autoClear = false;
      renderDeferredFrame(pipeline, wgpuRenderer, camera);

      const savedBg = scene.background;
      scene.background = null;
      const savedMask = camera.layers.mask;
      camera.layers.set(OVERLAY_LAYER);
      wgpuRenderer.clearDepth();
      wgpuRenderer.render(scene, camera);
      camera.layers.mask = savedMask;
      scene.background = savedBg;

      (wgpuRenderer as any).autoClear = prevAutoClear;
    } catch (err) {
      console.error("SDF render error:", err);
      readyRef.current = false;
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

    invalidate();
  }, 1);

  return null;
}
