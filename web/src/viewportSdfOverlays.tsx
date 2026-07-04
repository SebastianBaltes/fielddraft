import { useEffect, useRef, useSyncExternalStore, type RefObject } from "react";
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
  probeVmSdf,
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
import { disposeAfterGpuIdle } from "./sdfPipelineCommon";
import { OVERLAY_LAYER, DATUM_LAYER, PREVIEW_LAYER } from "./viewportLayers";
import {
  computeSdfFingerprints,
  type SdfFingerprintKernel,
} from "./bodyFingerprint";
import type { BRepEdgeMode } from "./settings";
import {
  getExportPreview,
  getExportPreviewOpacity,
  subscribeExportPreview,
} from "./exportPreviewStore";
import type { HybridInfo } from "./kernel";
import type { MaterialBufferTarget } from "./materialBridge";

type BodyVolumes = { data: Float32Array; dims: [number, number, number] }[];

interface KernelLike extends SdfFingerprintKernel {
  bodyVolumeDataView(i: number, v: number): Float32Array;
  bodyVolumeDims(i: number, v: number): [number, number, number];
  bodySdfMarchRelax(i: number): number;
  bodyHybridInfo(i: number): HybridInfo;
  bodySdfAabb(i: number): Float32Array;
  bodySeedFeatureId(i: number): number;
}

function collectBodyVolumes(
  k: KernelLike,
  bodyIndex: number,
): BodyVolumes | undefined {
  const volCount = k.bodyVolumeCount(bodyIndex);
  if (volCount === 0) return undefined;
  const volumes: BodyVolumes = [];
  for (let v = 0; v < volCount; v++) {
    volumes.push({
      data: k.bodyVolumeDataView(bodyIndex, v),
      dims: k.bodyVolumeDims(bodyIndex, v),
    });
  }
  return volumes;
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
  if (label)
    label.textContent = `GPU error: ${err instanceof Error ? err.message : String(err)}`;
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
  occlude: boolean,
): void {
  wgpuRenderer.render(scene, camera);
  const prevAutoClear = (wgpuRenderer as any).autoClear;
  (wgpuRenderer as any).autoClear = false;

  // Export preview crossfade: when a preview mesh is published, render it
  // on top of (or instead of) the SDF body. The mesh lives on
  // PREVIEW_LAYER so it draws AFTER the shade pass and blends over the
  // body by its opacity. Skip the SDF pass only when the mesh is fully
  // opaque (nothing would show through anyway).
  const previewActive = getExportPreview() !== null;
  const previewOpacity = getExportPreviewOpacity();
  if (!(previewActive && previewOpacity >= 0.99)) {
    renderSdf();
  }

  const savedBg = scene.background;
  scene.background = null;
  const savedMask = camera.layers.mask;

  if (previewActive) {
    // The mesh solid, blended over the body just shaded.
    camera.layers.set(PREVIEW_LAYER);
    wgpuRenderer.render(scene, camera);
  }

  // Datum planes/origin first, with the body depth the shade pass just wrote
  // STILL INTACT — so a solid in front occludes the datum behind it and both
  // live in one shared z-buffer (ticket 1m8g). The datum plane meshes
  // depth-test (depthWrite is off, they're translucent) against the bodies.
  camera.layers.set(DATUM_LAYER);
  wgpuRenderer.render(scene, camera);

  // Then the rest of the overlay (B-Rep edges, sketches, gizmos, selection).
  // X-Ray/Hidden: wipe depth so these draw on top of solids. "Visible only"
  // (occlude): keep the SDF depth so edges behind a face are depth-rejected.
  camera.layers.set(OVERLAY_LAYER);
  if (!occlude) wgpuRenderer.clearDepth();
  wgpuRenderer.render(scene, camera);

  camera.layers.mask = savedMask;
  scene.background = savedBg;

  (wgpuRenderer as any).autoClear = prevAutoClear;
}

function copyOpcodes(k: KernelLike, bodyIndex: number): Float32Array {
  const view = k.bodySdfOpcodesView(bodyIndex);
  return view.length > 0 ? view.slice() : new Float32Array(0);
}

// ── Shared pipeline lifecycle (hook + adapters) ─────────────────────

/** What a concrete SDF pipeline (VM / deferred) plugs into the shared
 *  lifecycle hook. `P` is the pipeline state, `B` its body payload. */
interface SdfPipelineAdapter<P extends MaterialBufferTarget, B> {
  init(): P;
  dispose(pipeline: P, renderer: WebGPURenderer | undefined): void;
  /** Read everything the pipeline needs per body from the kernel. */
  collectBodies(k: KernelLike, count: number): B[];
  /** Split the kernel fingerprints into the part that forces a full rebuild
   *  + shader recompile (`rebuildFp`) and the part a data re-upload covers
   *  (`dataFp`). VM: only a volume-texture-count change recompiles (texture
   *  bindings are baked into the pipeline) — opcode changes are data.
   *  Deferred: any structural change rebuilds (proxy meshes + buffer layout). */
  fingerprintPolicy(fps: {
    volumeFp: string;
    structFp: string;
    dataFp: string;
  }): { rebuildFp: string; dataFp: string };
  rebuild(pipeline: P, bodies: B[], renderer: WebGPURenderer): void;
  compile(
    pipeline: P,
    renderer: WebGPURenderer,
    camera: THREE.Camera,
  ): Promise<void>;
  updateData(pipeline: P, bodies: B[], renderer: WebGPURenderer): void;
  renderFrame(
    pipeline: P,
    renderer: WebGPURenderer,
    camera: THREE.Camera,
  ): void;
  /** Track the canvas size (deferred render targets); VM has no targets. */
  onResize?(pipeline: P, width: number, height: number): void;
  /** Upload the per-body selection flags (deferred highlight tint). */
  onSelection?(
    pipeline: P,
    k: KernelLike,
    count: number,
    selection: ReadonlySet<number>,
  ): void;
}

/**
 * The ONE lifecycle state machine both SDF overlays run: fingerprint-driven
 * rebuild/recompile vs. data re-upload, material + selection uploads, and
 * the composited per-frame render (base scene → SDF pass → datum → overlay).
 *
 * Baked in exactly once for both pipelines:
 *   - StrictMode stale guard: an async compile/rebuild that finishes for a
 *     pipeline we've since disposed (double-mount) is ignored — acting on it
 *     would flip `readyRef` for a dead pipeline and wedge the loop.
 *   - GPU-idle gate (ticket sgzw): the DESTRUCTIVE rebuild runs only after
 *     the device drained all in-flight submits. Rebuilding mid-submit lets
 *     three.js destroy an internal render-context buffer → "Buffer used in
 *     submit while destroyed" → persistent GPU corruption. The
 *     disposeAfterGpuIdle guard inside the pipelines covers OUR resources,
 *     the gate here covers three's own per-pass buffers too.
 *   - Error path: a failed compile or a throwing render pass parks the
 *     overlay (`readyRef=false`) and surfaces the GPU error banner.
 *
 * No trailing invalidate(): the overlays are demand-driven. Re-renders are
 * triggered by events — kernel mutations via <Invalidator>, camera moves via
 * the controls, resize via the effect, compile completion via .then().
 * Self-driving the loop would force a continuous 60 fps render of a static
 * scene.
 */
function useSdfPipelineOverlay<P extends MaterialBufferTarget, B>(
  adapter: SdfPipelineAdapter<P, B>,
  brepEdgeMode: BRepEdgeMode,
): {
  pipelineRef: RefObject<P | null>;
  rendererRef: RefObject<WebGPURenderer | null>;
} {
  const { store } = useKernelStore();
  const scene = useThree((s) => s.scene);
  const size = useThree((s) => s.size);
  const invalidate = useThree((s) => s.invalidate);

  // While the export dialog shows a solid preview, the SDF body pass is
  // suppressed (the preview mesh takes its place). Re-render on toggle —
  // the overlay is demand-driven, so a state change alone won't repaint.
  const previewActive = useSyncExternalStore(
    subscribeExportPreview,
    () => getExportPreview() !== null,
  );
  useEffect(() => {
    invalidate();
  }, [previewActive, invalidate]);

  const pipelineRef = useRef<P | null>(null);
  const rebuildFpRef = useRef("");
  const dataFpRef = useRef("");
  const compilingRef = useRef(false);
  const readyRef = useRef(false);
  const matFeaturesRef = useRef<unknown>(null);
  const selectionRef = useRef<unknown>(null);
  // Captured from useFrame so the unmount cleanup can defer the pipeline's GPU
  // disposal until the device is idle (ticket 323i).
  const rendererRef = useRef<WebGPURenderer | null>(null);

  useEffect(() => {
    pipelineRef.current = adapter.init();
    rebuildFpRef.current = "";
    dataFpRef.current = "";
    matFeaturesRef.current = null;
    selectionRef.current = null;
    compilingRef.current = false;
    readyRef.current = false;
    showCompilationStatus("Initializing…");

    return () => {
      if (pipelineRef.current) {
        adapter.dispose(pipelineRef.current, rendererRef.current ?? undefined);
        pipelineRef.current = null;
      }
    };
  }, [adapter]);

  useEffect(() => {
    if (!pipelineRef.current || !adapter.onResize) return;
    adapter.onResize(
      pipelineRef.current,
      Math.max(size.width, 1),
      Math.max(size.height, 1),
    );
    invalidate();
  }, [adapter, size.width, size.height, invalidate]);

  useFrame(({ gl: renderer, camera }) => {
    if (!store || !pipelineRef.current) return;
    rendererRef.current = renderer as unknown as WebGPURenderer;
    const wgpu = renderer as unknown as WebGPURenderer;
    const pipeline = pipelineRef.current;
    const k = store.kernel;
    const count = k.bodyCount();

    const { rebuildFp, dataFp } = adapter.fingerprintPolicy(
      computeSdfFingerprints(k, count),
    );
    const needsRebuild = rebuildFp !== rebuildFpRef.current;
    const needsDataUpdate = !needsRebuild && dataFp !== dataFpRef.current;

    if (needsRebuild && !compilingRef.current) {
      showCompilationStatus("Building SDF…");
      // Gate the render loop NOW (before any await) so no new frame submits
      // while the pipeline is torn down and rebuilt below.
      compilingRef.current = true;
      rebuildFpRef.current = rebuildFp;
      dataFpRef.current = dataFp;
      const bodies = adapter.collectBodies(k, count);

      const runRebuild = () => {
        // StrictMode stale guard (see hook doc).
        if (pipeline !== pipelineRef.current) return;
        adapter.rebuild(pipeline, bodies, wgpu);
        showCompilationStatus("Compiling shader…");
        adapter
          .compile(pipeline, wgpu, camera)
          .then(() => {
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
      };
      // GPU-idle gate (ticket sgzw, see hook doc): disposeAfterGpuIdle is
      // exactly "run this once the queue drained (or synchronously without a
      // WebGPU queue)".
      disposeAfterGpuIdle(wgpu, runRebuild);
    }

    if (needsDataUpdate && readyRef.current && !compilingRef.current) {
      adapter.updateData(pipeline, adapter.collectBodies(k, count), wgpu);
      dataFpRef.current = dataFp;
    }

    const storeState = store.getState();

    // Materials only change when the feature list changes (any edit commits a
    // fresh array) or when geometry rebuilt/updated this frame. Gate the
    // resolve+GPU upload behind that so a static scene doesn't re-upload the
    // material buffer every frame.
    if (
      storeState.features !== matFeaturesRef.current ||
      needsRebuild ||
      needsDataUpdate
    ) {
      applyMaterials(pipeline, resolveMaterials(k, storeState.features), count);
      matFeaturesRef.current = storeState.features;
    }

    // Selection likewise only changes via a fresh selection Set (or when the
    // body set rebuilt).
    if (adapter.onSelection) {
      const selection = storeState.selection;
      if (selection !== selectionRef.current || needsRebuild) {
        adapter.onSelection(pipeline, k, count, selection);
        selectionRef.current = selection;
      }
    }

    if (!readyRef.current || compilingRef.current) return;

    try {
      renderOverlayLayer(
        wgpu,
        scene,
        camera,
        () => adapter.renderFrame(pipeline, wgpu, camera),
        brepEdgeMode === "visible",
      );
    } catch (err) {
      console.error("SDF render error:", err);
      readyRef.current = false;
      showGpuError(err);
    }
  }, 1);

  return { pipelineRef, rendererRef };
}

// ── VM adapter + overlay ────────────────────────────────────────────

interface VmBody {
  bodyIndex: number;
  opcodes: Float32Array;
  marchRelax: number;
  volumes?: BodyVolumes;
}

const vmAdapter: SdfPipelineAdapter<VmPipelineState, VmBody> = {
  init: initVmPipeline,
  dispose: disposeVmPipeline,
  collectBodies: (k, count) => {
    const bodies: VmBody[] = [];
    for (let i = 0; i < count; i++) {
      bodies.push({
        bodyIndex: i,
        opcodes: copyOpcodes(k, i),
        marchRelax: k.bodySdfMarchRelax(i),
        volumes: collectBodyVolumes(k, i),
      });
    }
    return bodies;
  },
  // Only a volume-texture-count change bakes new bindings → recompile; any
  // opcode/structure change is a plain buffer re-upload for the VM.
  fingerprintPolicy: ({ volumeFp, structFp, dataFp }) => ({
    rebuildFp: volumeFp,
    dataFp: structFp + dataFp,
  }),
  rebuild: rebuildVmBodies,
  compile: compileVmPipeline,
  updateData: (pipeline, bodies) => updateVmData(pipeline, bodies),
  renderFrame: renderVmFrame,
};

export function SdfVmOverlay({ brepEdgeMode }: { brepEdgeMode: BRepEdgeMode }) {
  const { pipelineRef, rendererRef } = useSdfPipelineOverlay(
    vmAdapter,
    brepEdgeMode,
  );

  // Dev/E2E parity hook: evaluate the live GPU interpreter at world points
  // and compare against the CPU mirror (`eval_opcodes`). Installed ONCE per
  // mount (not per frame); resolves lazily against the current pipeline.
  useEffect(() => {
    const w = window as unknown as Record<string, unknown>;
    w.__fdSdfVmProbe = (points: [number, number, number][], bodyIndex = 0) => {
      const pipeline = pipelineRef.current;
      const renderer = rendererRef.current;
      if (!pipeline || !renderer) {
        return Promise.reject(new Error("VM pipeline not ready"));
      }
      return probeVmSdf(pipeline, renderer, points, bodyIndex);
    };
    return () => {
      delete w.__fdSdfVmProbe;
    };
  }, [pipelineRef, rendererRef]);

  return null;
}

// ── Deferred adapter + overlay ──────────────────────────────────────

interface DeferredBody {
  bodyIndex: number;
  hybridInfo: HybridInfo;
  opcodes: Float32Array;
  aabb: Float32Array;
  volumes?: BodyVolumes;
  marchRelax: number;
}

const deferredAdapter: SdfPipelineAdapter<DeferredPipelineState, DeferredBody> =
  {
    init: () => initDeferredPipeline(1, 1),
    dispose: disposeDeferredPipeline,
    collectBodies: (k, count) => {
      const bodies: DeferredBody[] = [];
      for (let i = 0; i < count; i++) {
        bodies.push({
          bodyIndex: i,
          hybridInfo: k.bodyHybridInfo(i),
          opcodes: copyOpcodes(k, i),
          aabb: k.bodySdfAabb(i),
          volumes: collectBodyVolumes(k, i),
          marchRelax: k.bodySdfMarchRelax(i),
        });
      }
      return bodies;
    },
    // Any structural change (opcode layout, hybrid leaf set) moves the proxy
    // meshes + packed buffers → full rebuild; version bumps are data updates.
    fingerprintPolicy: ({ structFp, dataFp }) => ({
      rebuildFp: structFp,
      dataFp,
    }),
    rebuild: (pipeline, bodies, renderer) => {
      rebuildDeferredBodies(pipeline, bodies, renderer);
      rebuildShadeMaterialIfNeeded(pipeline, renderer);
    },
    compile: compileDeferredPipeline,
    updateData: updateDeferredData,
    renderFrame: renderDeferredFrame,
    onResize: resizeDeferredPipeline,
    onSelection: (pipeline, k, count, selection) => {
      const selectedBodies = new Set<number>();
      for (let i = 0; i < count; i++) {
        if (selection.has(k.bodySeedFeatureId(i))) selectedBodies.add(i);
      }
      updateDeferredSelection(pipeline, selectedBodies);
    },
  };

export function SdfDeferredOverlay({
  brepEdgeMode,
}: {
  brepEdgeMode: BRepEdgeMode;
}) {
  useSdfPipelineOverlay(deferredAdapter, brepEdgeMode);
  return null;
}
