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
import type { BRepEdgeMode, FrameLoopMode } from "./SettingsMenu";
import { SdfVmOverlay, SdfDeferredOverlay } from "./viewportSdfOverlays";
import {
  CameraBridgePublisher,
  CameraRig,
  ViewCommandConsumer,
  CameraQuaternionBridge,
  ViewCube,
  ClearColorSync,
} from "./viewportCamera";
import { ExtrudeDepthGizmo } from "./ExtrudeGizmo";
import {
  SketchOverlay3D,
  SelectionTransformGizmo,
  Invalidator,
  BRepEdgeOverlay,
  BRepEdgePointsOverlay,
  OctreeCellOverlay,
  BRepSelectionOverlay,
  BRepSelectedOverlay,
  BodyHighlightOverlay,
  MeshPreview,
  AssemblyParts,
  MateConnectorOverlay,
  DatumOverlay,
  pickDatumAtRay,
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
import { getHoveredElement, getHoveredElementOfKind } from "./brepPickBridge";
import { mapPickToQuery, filterAllows, elementQueryString, resolveElementClick } from "./elementQuery";
import { CONTROLS_PRESETS, isAdditiveClick, type ControlsPresetId } from "./controlsPreset";
import { bodiesInRect, elementsInRect, type NdcRect, type ProjectFn } from "./boxSelect";

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
  brepEdgeMode: BRepEdgeMode;
}
function Scene({ ndc, onPick, renderMode, brepEdgeMode }: SceneProps) {
  const { store } = useKernelStore();
  if (!store) return null;
  return (
    <>
      {renderMode === "sdf-vm" && <SdfVmOverlay brepEdgeMode={brepEdgeMode} />}
      {renderMode === "sdf-deferred" && <SdfDeferredOverlay brepEdgeMode={brepEdgeMode} />}
      {renderMode === "mesh" && <MeshPreview />}
      <CameraBridgePublisher />
      <PickProbe ndc={ndc} onPick={onPick} />
      <SelectionTransformGizmo />
      <ExtrudeDepthGizmo />
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

/** Body index owning the B-Rep face with this idHash, or `null`. Scans the
 *  kernel's bodies — cheap (face lists are tiny) and only runs on the
 *  escalation click (ticket evq2). */
function findBodyIndexForFace(
  kernel: { bodyCount(): number; bodyBrepFaces(i: number): { idHash: string }[] },
  idHash: string,
): number | null {
  const count = kernel.bodyCount();
  for (let i = 0; i < count; i++) {
    try {
      if (kernel.bodyBrepFaces(i).some((f) => f.idHash === idHash)) return i;
    } catch {
      continue;
    }
  }
  return null;
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
function tryQueryPick(
  click: PickResult | null,
  store: { kernel: { bodyCount(): number; bodyBrepFaces(i: number): { branchId: number; idHash: string }[] } } | null,
): boolean {
  const picker = getActiveQueryPicker();
  if (!picker) return false;

  const hovered = getHoveredElement();
  if (hovered) {
    const query = mapPickToQuery(hovered, picker.entityFilter);
    if (query) return picker.onPick(query);
  }

  // Click-time face resolution (tickets yx6a/bxoo): the hover overlay
  // publishes the hovered element from a useFrame that may not have run
  // between the pointer move and this click (demand frameloop, fast
  // clicks, automation). Resolve the face directly from the click's
  // analytic hit instead of dropping the pick.
  if (click?.hit && store && filterAllows("face", picker.entityFilter)) {
    try {
      const count = store.kernel.bodyCount();
      outer: for (let i = 0; i < count; i++) {
        for (const face of store.kernel.bodyBrepFaces(i)) {
          if (!click.branches.includes(face.branchId)) continue;
          if (picker.onPick(`qFace(${face.idHash})`)) return true;
          break outer;
        }
      }
    } catch {
      // fall through to the feature-level path below
    }
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
  store: {
    addMateConnectorAtFace(faceQuery: string): void;
    showTransientError(msg: string): void;
  },
  _click: PickHit,
): void {
  // C2: a mate connector anchors to a B-Rep face. Use the face the
  // overlays published to the pick bridge; emit `qFace(<idHash>)` for it.
  const hovered = getHoveredElement();
  const query = hovered ? mapPickToQuery(hovered, "FACE") : null;
  if (!query) {
    store.showTransientError("Mate connector needs a face — hover a B-Rep face and click.");
    return;
  }
  store.addMateConnectorAtFace(query);
}

function handleFilletClick(
  store: { addFilletAtEdge(edgeQuery: string, r?: number): void; showTransientError(msg: string): void },
  _click: PickResult | null,
): void {
  // DL-61: a fillet selects a B-Rep EDGE by query. Use the edge the
  // overlay published to the pick bridge and emit `qEdge(<idHash>)` for it
  // — the same element-query path the mate/sketch face clicks use.
  const edge = getHoveredElementOfKind("edge");
  const query = edge ? elementQueryString(edge) : null;
  if (!query) {
    store.showTransientError("Fillet needs an edge — hover a B-Rep edge and click.");
    return;
  }
  store.addFilletAtEdge(query);
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

export function Viewport({ renderMode, brepEdgeMode, frameLoop, showEdgePoints, showOctreeCells, controlsPreset }: { renderMode: string; brepEdgeMode: BRepEdgeMode; frameLoop: FrameLoopMode; showEdgePoints: boolean; showOctreeCells: boolean; controlsPreset: ControlsPresetId }) {
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

  // ── Box select (rectangle drag from empty space) ──────────────────
  // Armed on pointerdown over empty space in presets with `boxSelect`;
  // becomes a visible rectangle once the pointer travels a few px. The
  // mouseup that ends a drag also fires a click — `suppressClickRef`
  // swallows exactly that one so it can't clear the fresh selection.
  const boxDragRef = useRef<{ startX: number; startY: number } | null>(null);
  // The live rectangle lives in a REF (read synchronously by pointerup —
  // a fast drag finishes before React commits state) and is mirrored into
  // state purely to render the overlay.
  const boxRectRef = useRef<{ x0: number; y0: number; x1: number; y1: number } | null>(null);
  const [boxRect, setBoxRect] = useState<
    { x0: number; y0: number; x1: number; y1: number } | null
  >(null);
  const suppressClickRef = useRef(false);

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>): void => {
    const host = hostRef.current;
    if (!host) return;
    const rect = host.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;
    const x = ((event.clientX - rect.left) / rect.width) * 2 - 1;
    const y = -(((event.clientY - rect.top) / rect.height) * 2 - 1);
    setNdc({ x, y, active: true });

    const drag = boxDragRef.current;
    if (drag) {
      const px = event.clientX - rect.left;
      const py = event.clientY - rect.top;
      if (boxRectRef.current !== null || Math.hypot(px - drag.startX, py - drag.startY) > 4) {
        boxRectRef.current = { x0: drag.startX, y0: drag.startY, x1: px, y1: py };
        setBoxRect(boxRectRef.current);
      }
    }
  };

  const handlePointerLeave = (): void => {
    setNdc((prev) => ({ ...prev, active: false }));
    setPickResult(null);
  };

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>): void => {
    if (!store || event.button !== 0) return;
    const selection = CONTROLS_PRESETS[controlsPreset].selection;
    if (!selection.boxSelect) return;
    // Box select never starts inside another interaction mode…
    if (
      state?.sketchSession.active ||
      state?.measureMode ||
      state?.filletMode ||
      state?.mateMode ||
      state?.ikMode ||
      getActiveQueryPicker()
    )
      return;
    // …and only from EMPTY space: nothing hovered, nothing under the ray.
    if (getHoveredElement()) return;
    const click = pickAtMouse(event as unknown as React.MouseEvent<HTMLDivElement>, hostRef.current, store);
    if (click?.hit) return;
    const host = hostRef.current;
    if (!host) return;
    const rect = host.getBoundingClientRect();
    boxDragRef.current = {
      startX: event.clientX - rect.left,
      startY: event.clientY - rect.top,
    };
  };

  const handlePointerUp = (event: ReactPointerEvent<HTMLDivElement>): void => {
    const drag = boxDragRef.current;
    const boxRect = boxRectRef.current;
    boxDragRef.current = null;
    boxRectRef.current = null;
    if (!drag || boxRect === null) return;
    setBoxRect(null);
    suppressClickRef.current = true;
    if (!store) return;
    const host = hostRef.current;
    const camera = getPublishedPickCamera();
    if (!host || !camera) return;
    const rect = host.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;

    const selection = CONTROLS_PRESETS[controlsPreset].selection;
    // Industry idiom (Onshape "Selection", Fusion "Selection in Fusion"):
    // dragging left→right keeps only fully-enclosed entities (window),
    // right→left also catches crossing ones — where the preset's original
    // tool distinguishes the two.
    const mode =
      selection.boxSelect === "window-crossing" && boxRect.x1 < boxRect.x0
        ? "crossing"
        : "window";

    const toNdcX = (px: number) => (px / rect.width) * 2 - 1;
    const toNdcY = (py: number) => -((py / rect.height) * 2 - 1);
    const ndcRect: NdcRect = {
      minX: toNdcX(Math.min(boxRect.x0, boxRect.x1)),
      maxX: toNdcX(Math.max(boxRect.x0, boxRect.x1)),
      // The NDC y axis flips: the smaller pixel y is the LARGER ndc y.
      minY: toNdcY(Math.max(boxRect.y0, boxRect.y1)),
      maxY: toNdcY(Math.min(boxRect.y0, boxRect.y1)),
    };
    const v = new THREE.Vector3();
    const project: ProjectFn = (x, y, z) => {
      v.set(x, y, z).project(camera);
      if (v.z < -1 || v.z > 1) return null;
      return { x: v.x, y: v.y };
    };
    // Onshape's implicit multi-select accumulates box selections too; the
    // other presets replace unless their additive modifier is held.
    const additive = selection.implicitMultiSelect || isAdditiveClick(selection, event);
    if (selection.mode === "element") {
      store.applyBoxSelection(elementsInRect(store.kernel, project, ndcRect, mode), additive);
    } else {
      const ids = bodiesInRect(store.kernel, project, ndcRect, mode)
        .map((i) => store.kernel.bodySeedFeatureId(i))
        .filter((id) => id > 0);
      store.selectFeatureSet(ids, additive);
    }
    invalidateCanvas();
  };

  const handlePointerCancel = (): void => {
    boxDragRef.current = null;
    boxRectRef.current = null;
    setBoxRect(null);
  };

  const handleClick = (event: React.MouseEvent<HTMLDivElement>): void => {
    if (!store) return;
    // The mouseup that finished a box-select drag fires this click too —
    // swallow it so it can't immediately clear the fresh selection.
    if (suppressClickRef.current) {
      suppressClickRef.current = false;
      return;
    }

    const click = pickAtMouse(event, hostRef.current, store);
    setPickResult(click);

    if (tryQueryPick(click, store)) return;
    if (state?.ikMode && state?.assembly.active && click?.hit) { handleIkClick(store, state, click); return; }
    if (state?.mateMode && click?.hit) { handleMateClick(store, click); return; }
    if (state?.filletMode) { handleFilletClick(store, click); return; }
    if (state?.measureMode && click?.hit) {
      handleMeasureClick(click, measurePoints, setMeasurePoints);
      return;
    }

    // Tool-free B-Rep element selection. A plain click on a hovered
    // face/edge/vertex persistently selects it; an additive click (the
    // preset's multi-select modifier) toggles it in the multi-select set
    // (used to gather coplanar faces for one extrude); a second plain
    // click on the already-selected face escalates to the whole BODY
    // (ticket evq2) — highlighted via every B-Rep edge of the body plus
    // the Parts-list row. Object-mode presets (Three.js editor,
    // TinkerCAD; docs/UX-PRESETS.md) skip sub-elements entirely and fall
    // through to feature-level selection. Otherwise clear any element
    // selection and fall through to feature-level.
    const selection = CONTROLS_PRESETS[controlsPreset].selection;
    const hovered = selection.mode === "element" ? getHoveredElement() : null;
    if (hovered) {
      const additive = isAdditiveClick(selection, event);
      // Onshape implicit multi-select: every plain click toggles the element
      // in the multi-select set — accumulate without a modifier, click a
      // selected element again to deselect it.
      const action = selection.implicitMultiSelect
        ? "toggle"
        : resolveElementClick(
            hovered,
            state?.selectedElement ?? null,
            additive,
            selection.bodyEscalation,
          );
      if (action === "toggle") {
        store.toggleElement(hovered);
      } else if (action === "escalate-body") {
        const bodyIndex = findBodyIndexForFace(store.kernel, hovered.idHash);
        if (bodyIndex !== null) {
          store.clearElementSelection();
          store.highlightBody(bodyIndex);
        } else {
          store.selectElement(hovered);
        }
      } else {
        if (state?.highlightedBodyIndex != null) store.highlightBody(null);
        store.selectElement(hovered);
      }
      invalidateCanvas();
      return;
    }
    // Default geometry is selectable in element-selection presets (Onshape
    // UX, ticket r36f): a click that hits no B-Rep element but lands on a
    // visible datum plane (or the origin marker) selects that datum. Runs
    // AFTER element resolution so model geometry always wins over the
    // planes slicing through it.
    if (selection.mode === "element" && !click?.hit) {
      const host = hostRef.current;
      const camera = getPublishedPickCamera();
      if (host && camera) {
        const rect = host.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          const cx = ((event.clientX - rect.left) / rect.width) * 2 - 1;
          const cy = -(((event.clientY - rect.top) / rect.height) * 2 - 1);
          const raycaster = new THREE.Raycaster();
          raycaster.setFromCamera(new THREE.Vector2(cx, cy), camera);
          const datum = pickDatumAtRay(
            raycaster.ray.origin,
            raycaster.ray.direction,
            state?.hiddenDatums ?? new Set<string>(),
          );
          if (datum) {
            store.selectDatum(datum);
            invalidateCanvas();
            return;
          }
        }
      }
    }

    store.clearElementSelection();
    if (state?.highlightedBodyIndex != null) store.highlightBody(null);

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
      // Ticket yx6a: dropping the sketch tool on a body face anchors the
      // sketch to THAT face (qFace); only a miss falls back to the datum
      // plane closest to the hit normal.
      let plane: string | null = null;
      try {
        const count = store.kernel.bodyCount();
        outer: for (let i = 0; i < count; i++) {
          for (const f of store.kernel.bodyBrepFaces(i)) {
            if (!pickResult.branches.includes(f.branchId)) continue;
            plane = `qFace(${f.idHash})`;
            break outer;
          }
        }
      } catch {
        // fall back to a datum plane
      }
      if (!plane) {
        const n = pickResult.normal;
        const ax = Math.abs(n[0]);
        const ay = Math.abs(n[1]);
        const az = Math.abs(n[2]);
        plane = "qCreatedBy(origin_plane_xy)";
        if (ax > ay && ax > az) {
          plane = "qCreatedBy(origin_plane_yz)";
        } else if (az > ay) {
          plane = "qCreatedBy(origin_plane_xz)";
        }
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
      onPointerDown={webgpuError ? undefined : handlePointerDown}
      onPointerUp={webgpuError ? undefined : handlePointerUp}
      onPointerCancel={webgpuError ? undefined : handlePointerCancel}
      onClick={webgpuError ? undefined : handleClick}
      onDragOver={webgpuError ? undefined : handleDragOver}
      onDrop={webgpuError ? undefined : handleDrop}
    >
      {webgpuError && <div className="error">WebGPU is not available. A secure context (HTTPS or localhost) is required.</div>}
      {boxRect && (
        <div
          className={
            "select-rect" +
            (CONTROLS_PRESETS[controlsPreset].selection.boxSelect === "window-crossing" &&
            boxRect.x1 < boxRect.x0
              ? " is-crossing"
              : "")
          }
          style={{
            left: Math.min(boxRect.x0, boxRect.x1),
            top: Math.min(boxRect.y0, boxRect.y1),
            width: Math.abs(boxRect.x1 - boxRect.x0),
            height: Math.abs(boxRect.y1 - boxRect.y0),
          }}
        />
      )}
      <CanvasErrorBoundary>
        <Canvas
          shadows={false}
          frameloop={frameLoop}
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
          onCreated={() => {
            // Gate the scene graph until the renderer is fully created.
            // With an async `gl` factory, children that commit before the
            // renderer/scene are ready never attach to the live scene and
            // stay invisible (planes, grid and mesh all missing on boot).
            // Mounting them only after `onCreated` guarantees they land in
            // the ready scene.
            setCanvasReady(true);
          }}
        >
          {canvasReady && (
            <>
              <ClearColorSync />
              <Scene ndc={ndc} onPick={setPickResult} renderMode={renderMode} brepEdgeMode={brepEdgeMode} />
              <ReferenceMesh />
              <BRepEdgeOverlay ndc={ndc} mode={brepEdgeMode} />
              {showEdgePoints && <BRepEdgePointsOverlay />}
              {showOctreeCells && <OctreeCellOverlay />}
              <BRepSelectionOverlay ndc={ndc} />
              <BRepSelectedOverlay />
              <BodyHighlightOverlay />
              <MateConnectorOverlay />
              <AssemblyParts />
              <WorldGrid />
              <CameraRig preset={controlsPreset} />
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
