/**
 * Main 3D viewport.
 *
 * Render modes: `sdf-vm` (SdfVmOverlay) and `sdf-deferred`
 * (SdfDeferredOverlay).
 */

import {
  Component,
  type ErrorInfo,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  useEffect,
  useRef,
  useState,
} from "react";
import { Canvas, useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import { useKernelStore } from "./store";
import type { PickHit, PickResult } from "./kernel";
import type { BRepEdgeMode } from "./SettingsMenu";
import { SdfVmOverlay, SdfDeferredOverlay } from "./viewportSdfOverlays";
import {
  CameraBridgePublisher,
  CameraRig,
  ViewCommandConsumer,
  CameraQuaternionBridge,
  ViewCube,
  ClearColorSync,
} from "./viewportCamera";
import {
  SketchOverlay3D,
  SelectionTransformGizmo,
  Invalidator,
  BRepEdgeOverlay,
  BRepSelectionOverlay,
  MeshPreview,
  AssemblyParts,
  MateConnectorOverlay,
  DatumOverlay,
  WorldGrid,
  ReferenceMesh,
  PickHud,
  PerfProbe,
} from "./viewportOverlays";
import { PerfHud } from "./perfMonitor";
import { SketchCanvas2d } from "./SketchCanvas2d";
import { invalidateCanvas } from "./viewCommands";
import { getActive as getActiveQueryPicker } from "./queryPickerBridge";
import { getPublishedPickCamera } from "./pickCameraBridge";
import { getHoveredElement } from "./brepPickBridge";
import { mapPickToQuery, filterAllows } from "./elementQuery";

const BUILD_VOLUME_SIZE = 600;

interface PickProbeProps {
  ndc: { x: number; y: number; active: boolean };
  onPick: (result: PickResult | null) => void;
}

function PickProbe({ ndc, onPick }: PickProbeProps) {
  const { store } = useKernelStore();
  const { camera } = useThree();
  const raycaster = useRef(new THREE.Raycaster());

  useFrame(() => {
    if (!store || !ndc.active) {
      onPick(null);
      return;
    }
    raycaster.current.setFromCamera(
      new THREE.Vector2(ndc.x, ndc.y),
      camera,
    );
    const o = raycaster.current.ray.origin;
    const d = raycaster.current.ray.direction;
    const result = store.kernel.pick(
      [o.x, o.y, o.z],
      [d.x, d.y, d.z],
    );
    onPick(result);
  });

  return null;
}

interface SceneProps {
  ndc: { x: number; y: number; active: boolean };
  onPick: (result: PickResult | null) => void;
  renderMode: string;
  showEdges: boolean;
  brepEdgeMode: BRepEdgeMode;
}
function Scene({ ndc, onPick, renderMode, showEdges, brepEdgeMode }: SceneProps) {
  const { store } = useKernelStore();
  if (!store) return null;
  return (
    <>
      {renderMode === "sdf-vm" && <SdfVmOverlay showEdges={showEdges} brepEdgeMode={brepEdgeMode} />}
      {renderMode === "sdf-deferred" && <SdfDeferredOverlay showEdges={showEdges} brepEdgeMode={brepEdgeMode} />}
      {renderMode === "mesh" && <MeshPreview />}
      <CameraBridgePublisher />
      <PickProbe ndc={ndc} onPick={onPick} />
      <SelectionTransformGizmo />
      <SketchOverlay3D />
      <DatumOverlay />
      <Invalidator ndc={ndc} />
      <PerfProbe renderMode={renderMode} />
      <gridHelper args={[BUILD_VOLUME_SIZE, 30, "#b8b8ae", "#d6d6ce"]} />
    </>
  );
}

interface CanvasBoundaryProps {
  children: ReactNode;
}
interface CanvasBoundaryState {
  error: Error | null;
}
class CanvasErrorBoundary extends Component<CanvasBoundaryProps, CanvasBoundaryState> {
  state: CanvasBoundaryState = { error: null };
  static getDerivedStateFromError(error: Error): CanvasBoundaryState {
    return { error };
  }
  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("fielddraft: viewport crashed", error, info);
  }
  render() {
    if (this.state.error) {
      return (
        <div className="error">
          viewport error: {this.state.error.message}
        </div>
      );
    }
    return this.props.children;
  }
}

function pickAtMouse(
  event: React.MouseEvent<HTMLDivElement>,
  host: HTMLDivElement | null,
  store: { kernel: { pick(origin: number[], dir: number[]): PickResult } },
): PickResult | null {
  const camera = getPublishedPickCamera();
  if (!host || !camera) return null;
  const rect = host.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return null;
  const cx = ((event.clientX - rect.left) / rect.width) * 2 - 1;
  const cy = -(((event.clientY - rect.top) / rect.height) * 2 - 1);
  const raycaster = new THREE.Raycaster();
  raycaster.setFromCamera(new THREE.Vector2(cx, cy), camera);
  const o = raycaster.ray.origin;
  const d = raycaster.ray.direction;
  return store.kernel.pick([o.x, o.y, o.z], [d.x, d.y, d.z]);
}

/**
 * Convert a viewport click into the query string the active picker wants.
 *
 * Element-level first: if a B-Rep face/edge/vertex is currently hovered
 * (published to `brepPickBridge` by the overlays) and the picker's
 * `entityFilter` allows that kind, emit the element query (`qFace`/`qEdge`/
 * `qVertex` — built in one place by `mapPickToQuery`). Otherwise fall back
 * to feature-level `qCreatedBy` (extrude/boolean and other body/feature
 * pickers), but only when the filter doesn't restrict to element kinds the
 * click didn't hit.
 */
function tryQueryPick(click: PickResult | null): boolean {
  const picker = getActiveQueryPicker();
  if (!picker) return false;

  const hovered = getHoveredElement();
  if (hovered) {
    const query = mapPickToQuery(hovered, picker.entityFilter);
    if (query) return picker.onPick(query);
  }

  // Feature-level fallback. Skip it when the picker restricts to element
  // kinds (face/edge/vertex) — a body-level qCreatedBy would violate the
  // filter — unless no filter is set (undefined → anything goes).
  const filter = picker.entityFilter;
  const elementOnly =
    filter !== undefined &&
    (filterAllows("face", filter) ||
      filterAllows("edge", filter) ||
      filterAllows("vertex", filter));
  if (elementOnly) return false;

  if (!click?.hit || click.contributors.length === 0) return false;
  const contributor = click.contributors[0];
  if (contributor <= 0) return false;
  return picker.onPick(`qCreatedBy(${contributor})`);
}

function handleFacePickClick(
  store: { enterSketchFromFacePick(n: [number, number, number]): void; cancelFacePickMode(): void; enterSketch(q: string): void },
  click: PickResult | null,
): void {
  if (click?.hit && click.contributors.length > 0) {
    store.enterSketchFromFacePick(click.normal as [number, number, number]);
  } else {
    store.cancelFacePickMode();
    store.enterSketch("qCreatedBy(origin_plane_xy)");
  }
}

function handleIkClick(
  store: { runIK(opts: { position: [number, number, number]; endEffectorPartId: string; endEffectorConnectorId: number }): void },
  state: { assembly: { document: { mates: { type: string }[]; parts: { id: string; mateConnectors: { featureId: number }[] }[] } } },
  click: PickHit,
): void {
  const revMates = state.assembly.document.mates.filter((m) => m.type === "revolute");
  if (revMates.length === 0) return;
  const lastPart = state.assembly.document.parts[state.assembly.document.parts.length - 1];
  if (!lastPart || lastPart.mateConnectors.length === 0) return;
  store.runIK({
    position: click.point,
    endEffectorPartId: lastPart.id,
    endEffectorConnectorId: lastPart.mateConnectors[0].featureId,
  });
}

function handleMateClick(
  store: { addMateConnectorAtPick(p: [number, number, number], n: [number, number, number]): void },
  click: PickHit,
): void {
  store.addMateConnectorAtPick(click.point, click.normal);
}

function handleFilletClick(
  store: { addFilletAtEdge(a: number, b: number, min: number[], max: number[], r: number): void; showTransientError(msg: string): void },
  click: PickResult | null,
): void {
  if (!click?.hit) return;
  if (click.contributors.length < 2) {
    store.showTransientError("Fillet requires an edge between two features — click where two bodies meet.");
    return;
  }
  const a = click.contributors[0];
  const b = click.contributors[1];
  if (a > 0 && b > 0) {
    const pt = click.point;
    store.addFilletAtEdge(
      a, b,
      [pt[0] - 20, pt[1] - 20, pt[2] - 20],
      [pt[0] + 20, pt[1] + 20, pt[2] + 20],
      3,
    );
  }
}

type MeasurePoints = { a: [number, number, number]; b?: [number, number, number] } | null;

function handleMeasureClick(
  click: PickHit,
  measurePoints: MeasurePoints,
  setMeasurePoints: (pts: MeasurePoints) => void,
): void {
  const pt: [number, number, number] = [click.point[0], click.point[1], click.point[2]];
  if (!measurePoints || measurePoints.b) {
    setMeasurePoints({ a: pt });
  } else {
    setMeasurePoints({ ...measurePoints, b: pt });
  }
}

function handleSelectionClick(
  store: { clearSelection(): void; selectRangeTo(id: number): void; toggleSelect(id: number): void; selectOnly(id: number): void },
  click: PickResult | null,
  event: React.MouseEvent,
  setMeasurePoints: (pts: MeasurePoints) => void,
): void {
  if (!click || !click.hit) {
    if (!event.shiftKey && !event.ctrlKey && !event.metaKey) {
      store.clearSelection();
    }
    setMeasurePoints(null);
    invalidateCanvas();
    return;
  }
  const featureId = click.contributors[0];
  if (featureId !== undefined && featureId > 0) {
    if (event.shiftKey) {
      store.selectRangeTo(featureId);
    } else if (event.ctrlKey || event.metaKey) {
      store.toggleSelect(featureId);
    } else {
      store.selectOnly(featureId);
    }
  }
  invalidateCanvas();
}

export function Viewport({ renderMode, showEdges, brepEdgeMode }: { renderMode: string; showEdges: boolean; brepEdgeMode: BRepEdgeMode }) {
  const { store, state } = useKernelStore();
  const hostRef = useRef<HTMLDivElement | null>(null);
  const rendererRef = useRef<{ dispose(): void } | null>(null);
  const [webgpuError, setWebgpuError] = useState(false);
  const [canvasReady, setCanvasReady] = useState(false);

  // Free the WebGPU renderer (device, swapchain, all pipelines) when the page
  // goes away. R3F unmount cleanup does NOT run on reload/navigation, so
  // without this the GPU keeps the orphaned device + compiled pipelines alive.
  // They accumulate across reloads and push shader compile from ~3 s to 30 s+.
  // `pagehide` fires on both reload and bfcache navigation, unlike
  // `beforeunload`. Disposing the renderer tears down its backend device,
  // which releases everything hanging off it.
  useEffect(() => {
    const release = () => {
      rendererRef.current?.dispose();
      rendererRef.current = null;
    };
    window.addEventListener("pagehide", release);
    return () => window.removeEventListener("pagehide", release);
  }, []);
  const [ndc, setNdc] = useState<{ x: number; y: number; active: boolean }>({
    x: 0,
    y: 0,
    active: false,
  });
  const [pickResult, setPickResult] = useState<PickResult | null>(null);
  const [measurePoints, setMeasurePoints] = useState<MeasurePoints>(null);

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>): void => {
    const host = hostRef.current;
    if (!host) return;
    const rect = host.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;
    const x = ((event.clientX - rect.left) / rect.width) * 2 - 1;
    const y = -(((event.clientY - rect.top) / rect.height) * 2 - 1);
    setNdc({ x, y, active: true });
  };

  const handlePointerLeave = (): void => {
    setNdc((prev) => ({ ...prev, active: false }));
    setPickResult(null);
  };

  const handleClick = (event: React.MouseEvent<HTMLDivElement>): void => {
    if (!store) return;

    const click = pickAtMouse(event, hostRef.current, store);
    setPickResult(click);

    if (tryQueryPick(click)) return;
    if (state?.facePickMode) { handleFacePickClick(store, click); return; }
    if (state?.ikMode && state?.assembly.active && click?.hit) { handleIkClick(store, state, click); return; }
    if (state?.mateMode && click?.hit) { handleMateClick(store, click); return; }
    if (state?.filletMode) { handleFilletClick(store, click); return; }
    if (state?.measureMode && click?.hit) {
      handleMeasureClick(click, measurePoints, setMeasurePoints);
      return;
    }

    handleSelectionClick(store, click, event, setMeasurePoints);
  };

  const handleDragOver = (e: React.DragEvent<HTMLDivElement>): void => {
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
  };

  const handleDrop = (e: React.DragEvent<HTMLDivElement>): void => {
    e.preventDefault();
    if (!store) return;
    const kind = e.dataTransfer.getData("fielddraft/primitive");
    if (kind) {
      type PrimName = "box" | "sphere" | "cylinder" | "torus" | "cone" | "capsule" | "hex_prism" | "ellipsoid";
      const valid: PrimName[] = ["box", "sphere", "cylinder", "torus", "cone", "capsule", "hex_prism", "ellipsoid"];
      const translate: [number, number, number] | undefined = pickResult?.hit
        ? [pickResult.point[0], pickResult.point[1], pickResult.point[2]]
        : undefined;
      if (valid.includes(kind as PrimName)) {
        store.addPrimitive(kind as PrimName, undefined, translate);
      } else if (kind === "hole") {
        store.addHole();
      } else if (kind === "gyroid") {
        store.addGyroid();
      }
      return;
    }
    const sketchDrop = e.dataTransfer.getData("fielddraft/sketch");
    if (sketchDrop && pickResult?.hit) {
      const n = pickResult.normal;
      const ax = Math.abs(n[0]);
      const ay = Math.abs(n[1]);
      const az = Math.abs(n[2]);
      let plane = "qCreatedBy(origin_plane_xy)";
      if (ax > ay && ax > az) {
        plane = "qCreatedBy(origin_plane_yz)";
      } else if (az > ay) {
        plane = "qCreatedBy(origin_plane_xz)";
      }
      store.enterSketch(plane);
    }
  };

  return (
    <div
      ref={hostRef}
      className="fd-viewport"
      data-testid="fielddraft-canvas"
      onPointerMove={webgpuError ? undefined : handlePointerMove}
      onPointerLeave={webgpuError ? undefined : handlePointerLeave}
      onClick={webgpuError ? undefined : handleClick}
      onDragOver={webgpuError ? undefined : handleDragOver}
      onDrop={webgpuError ? undefined : handleDrop}
    >
      {webgpuError && <div className="error">WebGPU is not available. A secure context (HTTPS or localhost) is required.</div>}
      <CanvasErrorBoundary>
        <Canvas
          shadows={false}
          frameloop="always"
          camera={{ position: [350, 280, 350], fov: 45, near: 1, far: 4000 }}
          gl={async (props) => {
            const { WebGPURenderer } = await import("three/webgpu");

            if (!navigator.gpu) {
              // Surface the error UI and bail before constructing the renderer —
              // WebGPURenderer.init() would otherwise reject and the rejection
              // would escape the async factory unhandled.
              setWebgpuError(true);
              throw new Error("WebGPU unavailable: navigator.gpu is undefined");
            }

            const renderer = new WebGPURenderer({
              ...(props as object),
              antialias: true,
            });
            await renderer.init();
            (window as any).__threeRenderer = renderer;
            rendererRef.current = renderer;

            const canvas = (props as any).canvas as HTMLCanvasElement;
            const parent = canvas.parentElement;
            if (parent) {
              const { width, height } = parent.getBoundingClientRect();
              if (width > 0 && height > 0) {
                renderer.setSize(width, height, false);
              }
            }
            return renderer;
          }}
          onCreated={(rootState) => {
            // Gate the scene graph until the renderer is fully created.
            // With an async `gl` factory, children that commit before the
            // renderer/scene are ready never attach to the live scene and
            // stay invisible (planes, grid and mesh all missing on boot).
            // Mounting them only after `onCreated` guarantees they land in
            // the ready scene.
            setCanvasReady(true);
            // One-shot present nudge. On first boot the WebGPU swapchain
            // can come up mis-configured: the scene renders but never
            // reaches the screen until something resizes the canvas — the
            // "blank until I resize the window" symptom. Re-apply the
            // canvas size (reconfiguring the swapchain) and bounce a
            // resize so R3F and the SDF overlays re-sync to the real
            // viewport. Two animation frames in, after layout settled.
            const nudge = () => {
              const gl = rootState.gl;
              const parent = gl.domElement.parentElement;
              if (parent) {
                const { width, height } = parent.getBoundingClientRect();
                if (width > 0 && height > 0) gl.setSize(width, height, false);
              }
              window.dispatchEvent(new Event("resize"));
            };
            requestAnimationFrame(() => requestAnimationFrame(nudge));
          }}
        >
          {canvasReady && (
            <>
              <ClearColorSync />
              <Scene ndc={ndc} onPick={setPickResult} renderMode={renderMode} showEdges={showEdges} brepEdgeMode={brepEdgeMode} />
              <ReferenceMesh />
              <BRepEdgeOverlay pick={pickResult} mode={brepEdgeMode} />
              <BRepSelectionOverlay ndc={ndc} />
              <MateConnectorOverlay />
              <AssemblyParts />
              <WorldGrid />
              <CameraRig />
              <ViewCommandConsumer />
              <CameraQuaternionBridge />
            </>
          )}
        </Canvas>
      </CanvasErrorBoundary>
      {!state?.sketchSession.active && <ViewCube />}
      {state?.sketchSession.active ? (
        <SketchCanvas2d store={store} session={state.sketchSession} />
      ) : null}
      <div
        id="fd-shader-compiling"
        className="shader-compiling"
        style={{ display: "none" }}
      >
        <div className="spinner" />
        <span id="fd-shader-compiling-label">Building SDF…</span>
      </div>
      <PerfHud renderMode={renderMode} />
      <PickHud result={pickResult} />
      {measurePoints ? (
        <div className="measure-hud" data-testid="measure-hud">
          {measurePoints.b ? (
            <>
              {Math.hypot(
                measurePoints.b[0] - measurePoints.a[0],
                measurePoints.b[1] - measurePoints.a[1],
                measurePoints.b[2] - measurePoints.a[2],
              ).toFixed(2)}{" "}
              mm
            </>
          ) : (
            "Click second point…"
          )}
        </div>
      ) : null}
      {state?.facePickMode && (
        <div className="face-pick-hud" data-testid="face-pick-hud">
          Click a face to set sketch plane — or press Esc for XY
        </div>
      )}
      {state?.mateMode && (
        <div className="face-pick-hud" data-testid="mate-mode-hud">
          Click a surface to place a mate connector
        </div>
      )}
      {state?.ikMode && (
        <div className="face-pick-hud" data-testid="ik-mode-hud">
          Click to set IK target position
        </div>
      )}
    </div>
  );
}
