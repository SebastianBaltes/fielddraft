/**
 * Main 3D viewport.
 *
 * Render modes: `sdf-vm` (SdfVmOverlay) and `sdf-deferred`
 * (SdfDeferredOverlay).
 */

import {
  Component,
  type ErrorInfo,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { Canvas, useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import { ndcZVisible } from "./projectionSpace";
import { useKernelStore } from "./store";
import type { PickHit, PickResult } from "./kernel";
import type { BRepEdgeMode, FrameLoopMode, RenderMode } from "./settings";
import { SdfVmOverlay, SdfDeferredOverlay } from "./viewportSdfOverlays";
import {
  CameraBridgePublisher,
  CameraRig,
  CameraPoseSync,
  ViewCommandConsumer,
  CameraQuaternionBridge,
  ViewportCamera,
  publishCameraQuat,
  ViewCube,
  ClearColorSync,
  adaptClipPlanes,
} from "./viewportCamera";
import { ExtrudeDepthGizmo } from "./ExtrudeGizmo";
import { SweepPathGizmo } from "./SweepGizmo";
import { ToolHint } from "./ToolHint";
import { ContextMenu, type ContextMenuItem } from "./ContextMenu";
import {
  requestViewFit,
  requestViewNormal,
  requestViewNormalToFace,
} from "./viewCommands";
import { resolveFaceWorldGeometry } from "./faceGeometry";
import {
  SketchOverlay3D,
  SketchSession3D,
  SketchHud3D,
  SketchLabels3D,
} from "./sketchSceneOverlays";
import { SelectionTransformGizmo, Invalidator } from "./transformGizmo";
import {
  BRepEdgeOverlay,
  BRepEdgePointsOverlay,
  OctreeCellOverlay,
} from "./brepEdgeOverlays";
import {
  BRepSelectionOverlay,
  BRepSelectedOverlay,
  BodyHighlightOverlay,
} from "./brepSelectionOverlays";
import {
  BRepMeshPreview,
  WebGLBodies,
  WebGLSceneLights,
  MeshPreview,
} from "./webglBodies";
import {
  AssemblyParts,
  MateConnectorOverlay,
  MateInferenceOverlay,
} from "./mateOverlays";
import { DatumOverlay, pickDatumAtRay } from "./datumOverlay";
import { ReferenceMesh, PickHud } from "./viewportHud";
import { ExportPreviewMesh } from "./exportPreviewMesh";
import { SdfPlaneOverlay } from "./SdfPlaneOverlay";
import { MesherOverlay } from "./MesherOverlay";
import { PartAabbOverlay } from "./PartAabbOverlay";
import { PartAabbPanel } from "./PartAabbPanel";
import { PREVIEW_LAYER } from "./viewportLayers";
import { PerfHud, PerfProbe } from "./perfMonitor";
import { ScreenSpaceCsgPass } from "./screenSpaceCsgPass";
import { SketchCanvas2d } from "./SketchCanvas2d";
import { invalidateCanvas } from "./viewCommands";
import {
  getActive as getActiveQueryPicker,
  subscribe as subscribeQueryPicker,
} from "./queryPickerBridge";
import { DATUM_PLANE_QUERIES } from "./queryLabels";
import { getPublishedPickCamera, publishPickCamera } from "./pickCameraBridge";
import { publishSketchCamera } from "./sketchCameraBridge";
import { requestOverlayRedraw } from "./sketchOverlayBridge";
import { getHoveredElement, getHoveredElementOfKind } from "./brepPickBridge";
import { getMateInferenceSnap } from "./mateInferenceBridge";
import type { PickedElement } from "./elementQuery";
import { MeasurePanel } from "./MeasurePanel";
import { pickSketchAtRay } from "./sketchPickBridge";
import {
  pickSketchLineAtRay,
  setHoveredSketchLine,
} from "./sketchLinePickBridge";
import {
  mapPickToQuery,
  filterAllows,
  filterAllowsSketchLine,
  filterAllowsSketchRegion,
  elementQueryString,
  resolveElementClick,
} from "./elementQuery";
import {
  CONTROLS_PRESETS,
  isAdditiveClick,
  type ControlsPresetId,
} from "./controlsPreset";
import { routeSketchSceneClick } from "./sketchSceneClick";
import { setGpuInfo, type GpuInfo } from "./telemetry";
import {
  bodiesInRect,
  elementsInRect,
  type NdcRect,
  type ProjectFn,
} from "./boxSelect";

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
    raycaster.current.setFromCamera(new THREE.Vector2(ndc.x, ndc.y), camera);
    const o = raycaster.current.ray.origin;
    const d = raycaster.current.ray.direction;
    const result = store.kernel.pick([o.x, o.y, o.z], [d.x, d.y, d.z]);
    onPick(result);
  });

  return null;
}

interface SceneProps {
  ndc: { x: number; y: number; active: boolean };
  onPick: (result: PickResult | null) => void;
  renderMode: RenderMode;
  brepEdgeMode: BRepEdgeMode;
}
/** Reactively tracks whether a query-by-click picker is armed. While a picker
 *  is active the scene transform gizmos are hidden so they can't occlude the
 *  pickable geometry under them (ticket pqoe). */
function useQueryPickerActive(): boolean {
  return useSyncExternalStore(
    subscribeQueryPicker,
    () => getActiveQueryPicker() !== null,
  );
}

function Scene({ ndc, onPick, renderMode, brepEdgeMode }: SceneProps) {
  const { store } = useKernelStore();
  const queryPickerActive = useQueryPickerActive();
  if (!store) return null;
  return (
    <>
      {renderMode === "sdf-vm" && <SdfVmOverlay brepEdgeMode={brepEdgeMode} />}
      {renderMode === "sdf-deferred" && (
        <SdfDeferredOverlay brepEdgeMode={brepEdgeMode} />
      )}
      {renderMode === "mesh" && <MeshPreview />}
      {renderMode === "brep" && <BRepMeshPreview />}
      <CameraBridgePublisher />
      <PickProbe ndc={ndc} onPick={onPick} />
      {!queryPickerActive && (
        <>
          <SelectionTransformGizmo />
          <ExtrudeDepthGizmo />
          <SweepPathGizmo />
        </>
      )}
      <SketchOverlay3D />
      <SketchSession3D />
      <SketchHud3D />
      <SketchLabels3D />
      <DatumOverlay />
      <Invalidator ndc={ndc} />
      <PerfProbe renderMode={renderMode} />
    </>
  );
}

interface CanvasBoundaryProps {
  children: ReactNode;
}
interface CanvasBoundaryState {
  error: Error | null;
}
class CanvasErrorBoundary extends Component<
  CanvasBoundaryProps,
  CanvasBoundaryState
> {
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
        <div className="error">viewport error: {this.state.error.message}</div>
      );
    }
    return this.props.children;
  }
}

/** Body index owning the B-Rep face with this idHash, or `null`. Scans the
 *  kernel's bodies — cheap (face lists are tiny) and only runs on the
 *  escalation click (ticket evq2). */
function findBodyIndexForFace(
  kernel: {
    bodyCount(): number;
    bodyBrepFaces(i: number): { idHash: string }[];
  },
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

/** Camera ray through the mouse position, or null without camera/layout. */
function rayAtMouse(
  event: React.MouseEvent<HTMLDivElement>,
  host: HTMLDivElement | null,
): THREE.Ray | null {
  const camera = getPublishedPickCamera();
  if (!host || !camera) return null;
  const rect = host.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return null;
  const cx = ((event.clientX - rect.left) / rect.width) * 2 - 1;
  const cy = -(((event.clientY - rect.top) / rect.height) * 2 - 1);
  const raycaster = new THREE.Raycaster();
  raycaster.setFromCamera(new THREE.Vector2(cx, cy), camera);
  return raycaster.ray;
}

function pickAtMouse(
  event: React.MouseEvent<HTMLDivElement>,
  host: HTMLDivElement | null,
  store: { kernel: { pick(origin: number[], dir: number[]): PickResult } },
): PickResult | null {
  const ray = rayAtMouse(event, host);
  if (!ray) return null;
  const o = ray.origin;
  const d = ray.direction;
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
 *
 * `datumAtRay` resolves the default datum plane under the click lazily
 * (front/top/right, or null). A FACE picker that hits no model geometry
 * picks that datum plane as `qCreatedBy(origin_plane_…)` — without this,
 * an empty project has nothing pickable at all and the sketch-plane
 * field's picker can never complete.
 */
const SNAPSHOT_PICK_REJECT_MSG =
  "Nur Körper aus früheren Features sind auswählbar — ein Feature darf nichts referenzieren, das später im Baum kommt.";

function tryQueryPick(
  click: PickResult | null,
  store: {
    kernel: {
      bodyCount(): number;
      bodyBrepFaces(i: number): { branchId: number; idHash: string }[];
    };
  } | null,
  datumAtRay?: () => string | null,
  onReject?: (msg: string) => void,
  sketchLineAtRay?: () => { sketchId: number; lineId: number } | null,
  // Ticket dhjn: resolve a click on a committed sketch's filled region to the
  // `qRegion(...)` query string for THAT region (or null on a miss). Gated on
  // the picker's `SKETCHREGION` filter — the Revolve/Extrude profile field.
  sketchRegionAtRay?: () => string | null,
): boolean {
  const picker = getActiveQueryPicker();
  if (!picker) return false;

  // Snapshot semantics: only geometry from features BEFORE the one being
  // edited may be picked — you cannot reference a feature that comes later in
  // the tree. `selectableFeatureIds` (undefined = no restriction) holds the
  // allowed owner features; reject picks of any later geometry (and tell the
  // user why, so a dead click isn't silent).
  const anyContributorAllowed = (): boolean => {
    if (!picker.selectableFeatureIds) return true;
    if (!click || !click.hit) return false;
    return click.contributors.some((c: number) =>
      picker.selectableFeatureIds!.has(c),
    );
  };

  // Single snapshot gate: a click on geometry whose features are ALL later
  // than the edited one is rejected up front with a visible reason (the hover
  // overlay already dims such geometry, so this catches the stray click).
  if (click?.hit && picker.selectableFeatureIds && !anyContributorAllowed()) {
    onReject?.(SNAPSHOT_PICK_REJECT_MSG);
    return false;
  }

  const hovered = getHoveredElement();
  if (hovered) {
    const query = mapPickToQuery(hovered, picker.entityFilter);
    if (query) return picker.onPick(query);
  }

  // Sketch-line axis pick (`qSketchLine`): the Revolve axis field accepts a
  // committed sketch line as well as a B-Rep edge. Sketch lines aren't
  // B-Rep elements (no hover overlay / id_hash), so resolve them by ray
  // against the overlay-published per-line geometry when the filter opts in.
  // Runs after the B-Rep hover path so a hovered edge still wins.
  if (sketchLineAtRay && filterAllowsSketchLine(picker.entityFilter)) {
    const hit = sketchLineAtRay();
    if (hit)
      return picker.onPick(`qSketchLine(${hit.sketchId}, ${hit.lineId})`);
  }

  // Sketch-region profile pick (`qRegion`): the Revolve/Extrude profiles field
  // (`SKETCHREGION` filter) accepts a single closed region of a committed
  // sketch. Resolve the clicked region to its `qRegion(...)` query — a hit here
  // takes the picked region; a miss falls through (so a click on empty space or
  // a B-Rep body can still feed the field its whole-sketch fallback). Runs after
  // B-Rep hover so a hovered face still wins where relevant (ticket dhjn).
  if (sketchRegionAtRay && filterAllowsSketchRegion(picker.entityFilter)) {
    const query = sketchRegionAtRay();
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

  // Default datum planes (front/top/right). Model geometry always wins —
  // this only runs when the click hit no body — and the origin marker is
  // a point, not a plane, so it never satisfies a FACE pick.
  if (!click?.hit && datumAtRay && filterAllows("face", picker.entityFilter)) {
    const datum = datumAtRay();
    const query = datum ? DATUM_PLANE_QUERIES[datum] : undefined;
    if (query && picker.onPick(query)) return true;
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
  store: {
    runIK(opts: {
      position: [number, number, number];
      endEffectorPartId: string;
      endEffectorConnectorId: number;
    }): void;
  },
  state: {
    assembly: {
      document: {
        mates: { type: string }[];
        parts: { id: string; mateConnectors: { featureId: number }[] }[];
      };
    };
  },
  click: PickHit,
): void {
  const revMates = state.assembly.document.mates.filter(
    (m) => m.type === "revolute",
  );
  if (revMates.length === 0) return;
  const lastPart =
    state.assembly.document.parts[state.assembly.document.parts.length - 1];
  if (!lastPart || lastPart.mateConnectors.length === 0) return;
  store.runIK({
    position: click.point,
    endEffectorPartId: lastPart.id,
    endEffectorConnectorId: lastPart.mateConnectors[0].featureId,
  });
}

function handleMateClick(store: {
  addMateConnectorAt(
    anchor: string,
    opts?: {
      t?: number;
      secondary?: string;
      reorient?: number;
      flip?: boolean;
    },
  ): void;
  showTransientError(msg: string): void;
}): void {
  // The MateInferenceOverlay snaps a connector to the connection point nearest
  // the cursor (vertex / edge midpoint / face centre) and publishes it to the
  // bridge; placing it just commits that snap.
  const snap = getMateInferenceSnap();
  if (!snap) {
    store.showTransientError(
      "Mate connector needs geometry — hover a vertex, edge, or face and click.",
    );
    return;
  }
  store.addMateConnectorAt(snap.anchorQuery, {
    t: snap.t,
    secondary: snap.secondary ?? "",
    reorient: snap.reorient,
  });
}

function handleFilletClick(
  store: {
    addFilletAtEdge(edgeQuery: string, r?: number): void;
    showTransientError(msg: string): void;
  },
  _click: PickResult | null,
): void {
  // DL-61: a fillet selects a B-Rep EDGE by query. Use the edge the
  // overlay published to the pick bridge and emit `qEdge(<idHash>)` for it
  // — the same element-query path the mate/sketch face clicks use.
  const edge = getHoveredElementOfKind("edge");
  const query = edge ? elementQueryString(edge) : null;
  if (!query) {
    store.showTransientError(
      "Fillet needs an edge — hover a B-Rep edge and click.",
    );
    return;
  }
  store.addFilletAtEdge(query);
}

function handleThickenClick(
  store: {
    addThicken(faceQueries: string[]): number | null;
    showTransientError(msg: string): void;
  },
  _click: PickResult | null,
): void {
  // Thicken selects a B-Rep FACE by query and folds it into a thin wall —
  // one thicken feature per click, the face analogue of the edge-click
  // fillet flow. The kernel rejects planar / cone / partial / tessellated
  // faces; `store.addThicken` surfaces that as a transient error, so a bad
  // pick just shows a message.
  const face = getHoveredElementOfKind("face");
  const query = face ? elementQueryString(face) : null;
  if (!query) {
    store.showTransientError(
      "Thicken needs a curved face — hover a cylinder/sphere/torus face and click.",
    );
    return;
  }
  store.addThicken([query]);
}

function handleMeasureClick(store: {
  addMeasureEntity(el: PickedElement): void;
}): void {
  // stv8 + me9w: Measure picks a B-Rep VERTEX, EDGE or FACE by its stable
  // id_hash. `getHoveredElement` already merges the three overlays by the CAD
  // priority vertex > edge > face, so the most specific element under the
  // cursor wins. The floating MeasurePanel resolves it to geometry and reports
  // length / area / coordinates / distance. A click over nothing is ignored
  // (keeps the current measure set intact).
  const el = getHoveredElement();
  if (el) store.addMeasureEntity(el);
}

function handleSelectionClick(
  // Ticket 7hgr (a): every selection here is a 3D SCENE click — pass
  // origin: "scene" so the mutators never end an active sketch session.
  store: {
    clearSelection(origin?: "list" | "scene"): void;
    selectRangeTo(id: number, origin?: "list" | "scene"): void;
    toggleSelect(id: number, origin?: "list" | "scene"): void;
    selectOnly(id: number, origin?: "list" | "scene"): void;
  },
  click: PickResult | null,
  event: React.MouseEvent,
): void {
  if (!click || !click.hit) {
    if (!event.shiftKey && !event.ctrlKey && !event.metaKey) {
      store.clearSelection("scene");
    }
    invalidateCanvas();
    return;
  }
  const featureId = click.contributors[0];
  if (featureId !== undefined && featureId > 0) {
    if (event.shiftKey) {
      store.selectRangeTo(featureId, "scene");
    } else if (event.ctrlKey || event.metaKey) {
      store.toggleSelect(featureId, "scene");
    } else {
      store.selectOnly(featureId, "scene");
    }
  }
  invalidateCanvas();
}

/**
 * Experimental WebGL render path (mode `webgl`; docs/ADAPTIVE-RENDERING-PLAN.md
 * Achse 3). A SEPARATE R3F `<Canvas>` on the DEFAULT `THREE.WebGLRenderer` — no
 * WebGPU, no node materials — for weak / no-WebGPU-adapter machines. The Canvas
 * sits in an explicitly-sized wrapper: R3F only creates its root once the
 * container measures width>0 && height>0 (react-three-fiber Canvas gate), so a
 * 0×0 container silently renders nothing.
 */
function WebGLViewport({
  controlsPreset,
  brepEdgeMode,
  ndc,
  screenSpaceCsg,
  perspective,
}: {
  controlsPreset: ControlsPresetId;
  brepEdgeMode: BRepEdgeMode;
  // Live cursor NDC + active flag, driven by the parent Viewport's host-pointer
  // handlers (now enabled in WebGL mode too — ticket 814w). Feeds the hover
  // overlays below so B-Rep edge/face/vertex hover publishes to brepPickBridge,
  // which is what makes Measure / element selection / body selection work on
  // this backend exactly like on WebGPU.
  ndc: { x: number; y: number; active: boolean };
  // Opt-in experiment (default off): when true, mount the experimental
  // Screen-Space-CSG A−B pass (docs/ADAPTIVE-RENDERING-PLAN.md, Achse 3,
  // Phase 3). Off → the WebGL path renders exactly as before.
  screenSpaceCsg: boolean;
  // Camera projection: false (default) → orthographic, true → perspective.
  perspective: boolean;
}) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const { state } = useKernelStore();
  const suppressBrepOverlays = state?.gizmoDragSuppressed ?? false;
  // Mirror the WebGPU <Scene>: hide the transform gizmos while a query-by-click
  // picker is armed so they can't occlude the pickable geometry (ticket pqoe).
  const queryPickerActive = useQueryPickerActive();

  // The kernel's Dual-Contouring mesher produces a boolean-correct mesh of every
  // body. When those are available we render them (booleans visible); until the
  // async mesh arrives we fall back to the per-leaf parametric hulls so the view
  // isn't blank (ticket gcvz). Never both at once — no double-draw.
  const hasMeshes = (state?.meshedBodies?.length ?? 0) > 0;
  const stateRef = useRef<{
    gl: THREE.WebGLRenderer;
    scene: THREE.Scene;
    // Live R3F state accessor — `get().camera` always returns the CURRENT
    // default camera, which ViewportCamera swaps when the projection setting
    // flips. Reading it per frame (instead of caching the camera) keeps the
    // clip-plane / pick / ViewCube publishes pointed at the active camera.
    get: () => { camera: THREE.Camera };
    // R3F's per-root loop driver. Calling it runs every `useFrame` subscriber
    // in THIS root (the B-Rep hover overlays — ticket 814w) and then renders.
    advance: (timestamp: number, runGlobalEffects?: boolean) => void;
  } | null>(null);

  // (The react-use-measure resize nudge lives in the parent Viewport, keyed on
  // render mode, so it covers both the WebGPU and this WebGL canvas.)

  // Drive rendering ourselves. R3F's shared global loop doesn't tick this
  // standalone second root (the WebGPU canvas owns the scheduler), so the Canvas
  // runs `frameloop="never"` and we advance THIS root's loop each animation
  // frame. Ticket 814w: we call R3F's per-root `advance()` (not a bare
  // `gl.render`) so every `useFrame` subscriber in this root runs first — that
  // is what ticks the B-Rep hover overlays (edge/face/vertex hover → pick
  // bridge), making Measure / element / body selection work on this backend.
  useEffect(() => {
    let raf = 0;
    const tick = (timestamp: number) => {
      const st = stateRef.current;
      if (st) {
        // The active camera (ViewportCamera may have swapped ortho⇄persp since
        // mount) — read it live each frame, never the cached one.
        const camera = st.get().camera;
        // The export-preview solid lives on PREVIEW_LAYER; enable it on the
        // WebGL camera so it renders in this root's single scene pass
        // (transparent → crossfades over the body meshes).
        camera.layers.enable(PREVIEW_LAYER);
        // Adapt the clip planes to the scene each frame (ticket lbzq) — the
        // WebGL root bypasses <CameraBridgePublisher>'s useFrame.
        adaptClipPlanes(camera);
        // useFrame doesn't tick this manual-loop root, so feed the ViewCube
        // the live WebGL camera orientation here (ticket r8b4).
        publishCameraQuat(camera.quaternion);
        // Ticket sqif: the WebGL root has no <CameraBridgePublisher> (that
        // lives only in the WebGPU <Scene>), so getPublishedPickCamera() was
        // null on this backend. rayAtMouse() then returned null, and the
        // query-picker datum branch in handleClick could never build a ray —
        // hovering a datum still highlighted it (R3F's own canvas raycaster),
        // but clicking it never fired picker.onPick. Publish the live WebGL
        // camera here so the host-div click path rays through the SAME camera.
        publishPickCamera(camera);
        // Ticket x941: the WebGL root has no <CameraBridgePublisher> either, so
        // the 2D sketch overlay (SketchCanvas2d) never received a sketch-camera
        // publish — its bridge width/height/zoom/version stayed stale and its
        // rAF redraw gate never tripped, so a drawing drag (line/circle/…) only
        // appeared on mouse-up (the committed 3D geometry), never as a live
        // preview during the drag. Mirror the WebGPU publisher here: publish the
        // live camera dimensions and redraw the overlay in lockstep each frame.
        const canvas = st.gl.domElement;
        publishSketchCamera({
          width: canvas.clientWidth,
          height: canvas.clientHeight,
          devicePixelRatio: st.gl.getPixelRatio(),
        });
        requestOverlayRedraw();
        // Run this root's useFrame subscribers (hover overlays) + render.
        // `advance` expects seconds; rAF gives milliseconds. `runGlobalEffects:
        // false` — only THIS root's local frame loop; the WebGPU root (when it
        // exists) owns the global effects, so we must not double-fire them.
        st.advance(timestamp / 1000, false);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      // Ticket sqif: drop the published pick camera so a later switch to the
      // WebGPU backend doesn't briefly ray through this stale WebGL camera
      // before <CameraBridgePublisher> republishes its own.
      publishPickCamera(null);
    };
  }, []);

  return (
    // No own pointer handlers: the parent Viewport's host-pointer handlers
    // (onClick/onPointerMove/… on the .fd-viewport ancestor) now drive
    // selection, hover, measure and query picks in WebGL mode too — the same
    // renderer-independent `kernel.pick` / pick-camera ray path the WebGPU
    // viewport uses (ticket 814w). Events bubble up to that ancestor.
    <div ref={wrapRef} style={{ position: "absolute", inset: 0 }}>
      <Canvas
        frameloop="never"
        camera={{ position: [200, 200, 200], fov: 45, near: 1, far: 4000 }}
        onCreated={(st) => {
          stateRef.current = {
            gl: st.gl as unknown as THREE.WebGLRenderer,
            scene: st.scene,
            get: st.get as unknown as () => { camera: THREE.Camera },
            advance: st.advance,
          };
        }}
      >
        {/* Owns the active camera — swaps ortho (default) ⇄ perspective when
            the setting flips, on this WebGL root too. */}
        <ViewportCamera perspective={perspective} />
        {/* ticket xqp3: Szene ist weiß, nicht grau. */}
        <color attach="background" args={["#ffffff"]} />
        {/* Boolean-correct DC meshes once meshed; per-leaf hulls while the
            async mesh is still pending (ticket gcvz). Both carry their own
            lights, so exactly one renders at a time. */}
        {/* Ticket 55ea: once DC meshes exist, render single parametric solids
            (helix/sweep/revolve/extrude/primitives) from their fine analytic
            hull and keep the boolean-correct DC mesh only for composited
            bodies — a helix DC-meshes far too coarsely. Shared lights so the
            two paths don't double the scene lighting. Before meshing finishes,
            the hull fallback still draws every body. */}
        {hasMeshes ? (
          <>
            <WebGLSceneLights />
            {/* SS-CSG an: Difference-Bodys NICHT als DC-Mesh zeichnen — die kommen
                ausschließlich aus dem ScreenSpaceCsgPass. Flag aus → unverändert. */}
            <MeshPreview
              parametricSplit
              lights={false}
              skipSubtractionBodies={screenSpaceCsg}
            />
            <WebGLBodies onlyParametric lights={false} />
          </>
        ) : (
          <WebGLBodies skipSubtractionBodies={screenSpaceCsg} />
        )}
        {/* Export preview solid — on PREVIEW_LAYER (which the WebGL camera
            enables), transparent so the crossfade slider blends it over the
            body meshes; returns null when no export preview is published. */}
        <ExportPreviewMesh />
        {/* Debug ▸ SDF-Plane overlay (on OVERLAY_LAYER); null unless the
            dialog published a plane. */}
        <SdfPlaneOverlay />
        <MesherOverlay />
        <PartAabbOverlay />
        {/* Opt-in experiment (default off): Screen-Space-CSG Stage 1
            (docs/ADAPTIVE-RENDERING-PLAN.md, Achse 3, Phase 3). Mounts an
            extra A−B composite pass over the parametric hulls. When the flag is
            off this is NOT rendered, so the WebGL path is byte-for-byte
            unchanged. VISUELL UNVERIFIZIERT — Experiment. */}
        {screenSpaceCsg && <ScreenSpaceCsgPass />}
        {/* Default datum planes/origin (ticket 59c2) + committed sketches
            (ticket jjmv) — all plain-material meshes, so they render on the
            WebGL path just like in WebGPU. */}
        <DatumOverlay />
        <SketchOverlay3D />
        {/* Ticket uzhr: the ACTIVE sketch session (in-edit geometry, HUD,
            labels) must also render on the WebGL fallback — it was only
            mounted in the WebGPU Scene, so drawing in WebGL mode showed
            nothing. SketchSession3D now uses classic materials on this
            backend (also uzhr). */}
        <SketchSession3D />
        <SketchHud3D />
        <SketchLabels3D />
        {/* Ticket jnej: B-Rep edges render on the WebGL fallback (classic
            LineSegments; the fat-line path is WebGPU-only). Ticket 814w: the
            live `ndc` is now fed in so the edge-hover useFrame publishes the
            hovered edge to brepPickBridge — which is what Measure reads. The
            selection/hover overlays below complete the picture (face/vertex
            hover + persistent selection), all driven by `kernel.pick` /
            screen-space projection, so they work renderer-independently.
            Hidden during a suppressed gizmo drag, like the WebGPU path. */}
        {!suppressBrepOverlays && (
          <>
            <BRepEdgeOverlay ndc={ndc} mode={brepEdgeMode} />
            <BRepSelectionOverlay ndc={ndc} />
            <BRepSelectedOverlay />
            <BodyHighlightOverlay />
          </>
        )}
        {/* Mate-connector triads + the inference preview render on the WebGL
            fallback too (classic materials); the user's WebGPU runs in
            Compatibility mode where WebGL is the reliable path. */}
        <MateConnectorOverlay />
        {state?.mateMode && <MateInferenceOverlay ndc={ndc} />}
        {/* Ticket 7hk3: the Move/transform gizmos must render on the WebGL
            fallback too. They were only mounted in the WebGPU <Scene>, so in
            WebGL mode arming Move + selecting a body (via scene click → host
            pointer `transformBody`, or Parts-list click) materialised the
            transform feature but drew NO gizmo. SelectionTransformGizmo is
            renderer-independent (useThree + classic ThreeTransformControls),
            and this root's manual rAF advance() keeps it interactive without
            relying on invalidate(). Gated on the query picker like WebGPU. */}
        {!queryPickerActive && (
          <>
            <SelectionTransformGizmo />
            <ExtrudeDepthGizmo />
            <SweepPathGizmo />
          </>
        )}
        {/* Ticket owia: use the SAME preset-aware controls as the WebGPU path
            (CameraRig applies the active controlsPreset's mouseButtons). A bare
            <OrbitControls> defaulted to LEFT=rotate / MIDDLE=dolly / RIGHT=pan,
            ignoring the Onshape preset (MIDDLE=pan, RIGHT=rotate) — so the mouse
            felt wrong in the WebGL fallback, including in sketch mode. */}
        <CameraRig preset={controlsPreset} />
        <CameraPoseSync />
        {/* Ticket 10cl: the view-command consumer (ViewCube clicks, standard
            views, zoom-to-fit, view-normal) was mounted ONLY in the WebGPU
            <Canvas>. In WebGL mode the shared HTML <ViewCube> still rendered and
            its onClick called requestView(...), but nothing consumed the pending
            command — so the camera never moved. Mounting it here moves THIS
            root's WebGL camera; this root's manual rAF advance() ticks its
            useFrame every frame, so the consume happens without an explicit
            invalidate. Only one of the two roots is ever mounted (isWebGL gate),
            so the module-singleton registerInvalidate never collides. */}
        <ViewCommandConsumer />
      </Canvas>
    </div>
  );
}

/** A reason the WebGPU viewport could not start, shown to the user instead of
 *  a broken canvas. `title` is the headline, `detail` the actionable hint. */
interface WebGpuError {
  title: string;
  detail: string;
}

// Linux is where WebGPU most often fails despite capable hardware: the API is
// gated behind a flag and needs a Vulkan driver under a Wayland session.
const WEBGPU_HINT_LINUX =
  "On Linux: enable chrome://flags/#enable-unsafe-webgpu, use a Wayland session, and install a Vulkan driver (verify with `vulkaninfo`).";

const WEBGPU_NO_API: WebGpuError = {
  title: "This browser doesn’t support WebGPU.",
  detail:
    "FieldDraft needs a WebGPU-capable browser (current Chrome or Edge, or Safari 17+) served over a secure context (HTTPS or localhost).",
};

// The API exists but no usable GPU adapter could be acquired — or three fell
// back to WebGL2, which can’t run FieldDraft’s compute/storage-buffer SDF
// pipeline. Same user-facing remedy for both.
const WEBGPU_NO_ADAPTER: WebGpuError = {
  title: "No WebGPU graphics adapter is available on this system.",
  detail: `Your browser exposes WebGPU but couldn’t initialize a GPU, and FieldDraft has no WebGL fallback. ${WEBGPU_HINT_LINUX}`,
};

export function Viewport({
  renderMode,
  brepEdgeMode,
  frameLoop,
  showEdgePoints,
  showOctreeCells,
  screenSpaceCsg,
  perspective,
  controlsPreset,
}: {
  renderMode: RenderMode;
  brepEdgeMode: BRepEdgeMode;
  frameLoop: FrameLoopMode;
  showEdgePoints: boolean;
  showOctreeCells: boolean;
  /** Opt-in experiment (default off): mount the Screen-Space-CSG pass in the
   *  WebGL render path. Off → exact current behaviour. */
  screenSpaceCsg: boolean;
  /** Camera projection: false (default) → orthographic, true → perspective. */
  perspective: boolean;
  controlsPreset: ControlsPresetId;
}) {
  const { store, state } = useKernelStore();
  const suppressBrepOverlays = state?.gizmoDragSuppressed ?? false;
  const hostRef = useRef<HTMLDivElement | null>(null);
  const rendererRef = useRef<{ dispose(): void } | null>(null);
  const [webgpuError, setWebgpuError] = useState<WebGpuError | null>(null);
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

  // Right-click context menu. Tracked separately from the pointer handlers
  // because the right button also drives orbit/pan in most controls presets —
  // we only pop the menu on a stationary right-click, not after a drag.
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number } | null>(null);
  // The context menu opens on the right-button pointerUP, and only when the
  // press did NOT turn into an orbit/pan drag. We can't decide this in the
  // `contextmenu` handler: on Linux/X11 Chrome that event fires at press time
  // (before any movement), so a right-DRAG would open the menu before we ever
  // see the drag. Instead we watch the whole right-button gesture via window
  // listeners (capture phase, so OrbitControls' stopPropagation can't hide it).
  const trackRightDown = (e: ReactPointerEvent) => {
    if (e.button !== 2) return;
    const down = { x: e.clientX, y: e.clientY };
    let moved = false;
    const onMove = (ev: PointerEvent) => {
      if (
        Math.abs(ev.clientX - down.x) > 4 ||
        Math.abs(ev.clientY - down.y) > 4
      )
        moved = true;
    };
    const cleanup = () => {
      window.removeEventListener("pointermove", onMove, true);
      window.removeEventListener("pointerup", onUp, true);
      window.removeEventListener("pointercancel", cleanup, true);
    };
    const onUp = (ev: PointerEvent) => {
      if (ev.button !== 2) return;
      cleanup();
      if (!moved) setCtxMenu({ x: ev.clientX, y: ev.clientY });
    };
    window.addEventListener("pointermove", onMove, true);
    window.addEventListener("pointerup", onUp, true);
    window.addEventListener("pointercancel", cleanup, true);
  };
  const handleContextMenu = (e: ReactMouseEvent) => {
    // Always suppress the native browser menu over the 3D scene; the app menu
    // is driven from the right-button pointerUp (see trackRightDown).
    e.preventDefault();
  };
  const inSketch = state?.sketchSession.active ?? false;
  // "View Normal to Face" (ticket z0vk): only for a single, PLANAR selected
  // face — a curved face has no single normal to look down. Resolve the face's
  // world geometry up front so the menu both gates on planarity and captures
  // the normal/centroid to drive the same camera command sketches use.
  const selectedFaces = state?.selectedElements ?? [];
  const singleFace =
    !inSketch && selectedFaces.length === 1 && selectedFaces[0].kind === "face"
      ? selectedFaces[0]
      : null;
  const faceGeom =
    singleFace && store
      ? resolveFaceWorldGeometry(store.kernel, singleFace.idHash)
      : null;
  const planarFace = faceGeom?.surfaceKind === "plane" ? faceGeom : null;
  const contextMenuItems: ContextMenuItem[] = [
    ...(inSketch
      ? [
          {
            label: "Confirm Sketch",
            testId: "ctx-confirm-sketch",
            onSelect: () => {
              // Ticket rkez: mirror the ✓ button — commit AND clear the
              // selection, otherwise the just-committed sketch stays selected
              // and its Detail dialog reads as still-open.
              store?.commitSketch();
              store?.clearSelection();
            },
          },
        ]
      : []),
    {
      label: "Zoom to fit",
      testId: "ctx-zoom-fit",
      onSelect: () => requestViewFit(),
    },
    ...(inSketch
      ? [
          {
            label: "View Normal to Sketch plane",
            testId: "ctx-view-normal",
            onSelect: () => requestViewNormal(),
          },
        ]
      : []),
    ...(planarFace
      ? [
          {
            label: "View Normal to Face",
            testId: "ctx-view-normal-face",
            onSelect: () =>
              requestViewNormalToFace(planarFace.normal, planarFace.centroid),
          },
        ]
      : []),
  ];

  // ── Box select (rectangle drag from empty space) ──────────────────
  // Armed on pointerdown over empty space in presets with `boxSelect`;
  // becomes a visible rectangle once the pointer travels a few px. The
  // mouseup that ends a drag also fires a click — `suppressClickRef`
  // swallows exactly that one so it can't clear the fresh selection.
  const boxDragRef = useRef<{ startX: number; startY: number } | null>(null);
  // The live rectangle lives in a REF (read synchronously by pointerup —
  // a fast drag finishes before React commits state) and is mirrored into
  // state purely to render the overlay.
  const boxRectRef = useRef<{
    x0: number;
    y0: number;
    x1: number;
    y1: number;
  } | null>(null);
  const [boxRect, setBoxRect] = useState<{
    x0: number;
    y0: number;
    x1: number;
    y1: number;
  } | null>(null);
  const suppressClickRef = useRef(false);

  const handlePointerMove = (
    event: ReactPointerEvent<HTMLDivElement>,
  ): void => {
    const host = hostRef.current;
    if (!host) return;
    const rect = host.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;
    const x = ((event.clientX - rect.left) / rect.width) * 2 - 1;
    const y = -(((event.clientY - rect.top) / rect.height) * 2 - 1);
    setNdc({ x, y, active: true });

    // fs23: while a query field that accepts sketch lines is armed, publish the
    // sketch line under the cursor so the committed-sketch overlay highlights
    // it — the "potential target" affordance. A hovered B-Rep element wins only
    // when it is itself a VALID target for this filter (e.g. a hovered edge for
    // an EDGE,SKETCHLINE axis) — a face the filter rejects must not block the
    // sketch line behind it. Cleared whenever no picker/line applies.
    const picker = getActiveQueryPicker();
    if (picker && filterAllowsSketchLine(picker.entityFilter)) {
      const hov = getHoveredElement();
      const brepWins =
        hov !== null && filterAllows(hov.kind, picker.entityFilter);
      const ray = brepWins
        ? null
        : rayAtMouse(
            event as unknown as React.MouseEvent<HTMLDivElement>,
            host,
          );
      const camera = getPublishedPickCamera();
      setHoveredSketchLine(
        ray ? pickSketchLineAtRay(ray, camera ?? undefined, rect.height) : null,
      );
    } else {
      setHoveredSketchLine(null);
    }

    const drag = boxDragRef.current;
    if (drag) {
      const px = event.clientX - rect.left;
      const py = event.clientY - rect.top;
      if (
        boxRectRef.current !== null ||
        Math.hypot(px - drag.startX, py - drag.startY) > 4
      ) {
        boxRectRef.current = {
          x0: drag.startX,
          y0: drag.startY,
          x1: px,
          y1: py,
        };
        setBoxRect(boxRectRef.current);
      }
    }
  };

  const handlePointerLeave = (): void => {
    setNdc((prev) => ({ ...prev, active: false }));
    setPickResult(null);
    setHoveredSketchLine(null); // fs23: drop the sketch-line hover highlight
  };

  const handlePointerDown = (
    event: ReactPointerEvent<HTMLDivElement>,
  ): void => {
    if (!store || event.button !== 0) return;
    const preset = CONTROLS_PRESETS[controlsPreset];
    const selection = preset.selection;
    if (!selection.boxSelect) return;
    // Box select never starts inside another interaction mode…
    if (
      state?.sketchSession.active ||
      state?.measureMode ||
      state?.filletMode ||
      state?.thickenMode ||
      state?.mateMode ||
      state?.ikMode ||
      getActiveQueryPicker()
    )
      return;
    if (preset.camera.shiftLeftBoxSelect) {
      // Three.js preset: LMB orbits, so the box is gated on Shift (the rig has
      // freed LMB while Shift is down). It may start anywhere — a Shift+click
      // (no drag) still falls through to additive selection; only a Shift+drag
      // grows into a box.
      if (!event.shiftKey) return;
    } else {
      // Free-LMB presets: the box starts only from EMPTY space — nothing
      // hovered, nothing under the ray — so it never eats a plain pick.
      if (getHoveredElement()) return;
      const click = pickAtMouse(
        event as unknown as React.MouseEvent<HTMLDivElement>,
        hostRef.current,
        store,
      );
      if (click?.hit) return;
    }
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
      if (!ndcZVisible(camera, v.z)) return null;
      return { x: v.x, y: v.y };
    };
    // Onshape's implicit multi-select accumulates box selections too; the
    // other presets replace unless their additive modifier is held.
    const additive =
      selection.implicitMultiSelect || isAdditiveClick(selection, event);
    if (selection.mode === "element") {
      store.applyBoxSelection(
        elementsInRect(store.kernel, project, ndcRect, mode),
        additive,
      );
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

    const datumAtClick = (): string | null => {
      const ray = rayAtMouse(event, hostRef.current);
      if (!ray) return null;
      return pickDatumAtRay(
        ray.origin,
        ray.direction,
        state?.hiddenDatums ?? new Set<string>(),
      );
    };
    const sketchLineAtClick = (): {
      sketchId: number;
      lineId: number;
    } | null => {
      const ray = rayAtMouse(event, hostRef.current);
      if (!ray) return null;
      // fs23: zoom-aware tolerance — pass the live camera + viewport height so
      // a sketch line is pickable at any model scale (the old fixed 0.5 mm
      // radius was sub-pixel on metre-scale models).
      const camera = getPublishedPickCamera();
      const h = hostRef.current?.getBoundingClientRect().height ?? 0;
      return pickSketchLineAtRay(ray, camera ?? undefined, h);
    };
    // Ticket dhjn: resolve a click on a committed sketch's filled region to the
    // `qRegion(...)` query for that ONE region. The kernel runs its own region
    // detection (same flood-fill order as `listSketchRegions`) on the picked
    // sketch-space point, so the index it returns addresses the same region.
    const sketchRegionAtClick = (): string | null => {
      const ray = rayAtMouse(event, hostRef.current);
      if (!ray) return null;
      const hit = pickSketchAtRay(ray);
      if (!hit) return null;
      try {
        const q = store.kernel.regionQueryAtPoint(
          hit.sketchId,
          hit.uv[0],
          hit.uv[1],
        );
        return q && q.length > 0 ? q : null;
      } catch {
        return null;
      }
    };
    if (
      tryQueryPick(
        click,
        store,
        datumAtClick,
        (msg) => store.showTransientError(msg),
        sketchLineAtClick,
        sketchRegionAtClick,
      )
    )
      return;
    // fs23: a query field is armed and the click did NOT resolve to a valid
    // pick (missed the target, or the filter rejected it). That is a no-op —
    // NOT a feature-deselect, which would clear the selection and close the
    // Detail view the picker lives in. Swallow the click so the field stays
    // armed for the next attempt.
    if (getActiveQueryPicker()) {
      invalidateCanvas();
      return;
    }

    // Ticket 4anf: context-dependent scene-click routing while a sketch is
    // open. The two contexts that MAY pick scene geometry both run before this
    // point: the sketch-PLANE field's pick via `tryQueryPick` + the armed-
    // picker swallow above, and the PROJECT tool's pick on the 2D overlay
    // (pointer-events: auto, so its clicks never reach here). What remains is
    // the rule that in EVERY OTHER sketch context the 3D scene is INERT: no
    // feature / element / datum selection, no mode activation — so no other
    // feature can be activated and the sketch ends only via Accept (✓) /
    // Cancel (✗). A click that reaches here during a session therefore either
    // missed the Project guides or had no armed picker; routing says inert and
    // we swallow it. Only the `default` routing (no session) falls through to
    // the normal selection / mode branches below.
    const sketchRouting = routeSketchSceneClick({
      sketchActive: state?.sketchSession.active ?? false,
      // The plane-pick context was fully consumed above; a surviving picker was
      // already handled by the swallow. We still report it so the routing keeps
      // its own invariant (`plane-pick` ⇒ not a fall-through to selection).
      planeQueryArmed: getActiveQueryPicker() !== null,
      projectToolActive: state?.sketchSession.selectedTool === "sketch_project",
    });
    if (sketchRouting !== "default") {
      invalidateCanvas();
      return;
    }

    if (state?.ikMode && state?.assembly.active && click?.hit) {
      handleIkClick(store, state, click);
      return;
    }
    if (state?.mateMode) {
      // The tool owns every click while active: place at the inferred snap
      // (which can sit on a corner/edge just off the silhouette, where the
      // kernel raycast reports no `hit`), else hint.
      handleMateClick(store);
      return;
    }
    if (state?.filletMode) {
      handleFilletClick(store, click);
      return;
    }
    if (state?.thickenMode) {
      handleThickenClick(store, click);
      return;
    }
    if (state?.measureMode) {
      handleMeasureClick(store);
      return;
    }

    // Ticket D6/320t: Move mode armed (transform toolbar button) — a click on a
    // body materialises (or re-selects) its transform feature and shows the
    // gizmo, instead of a plain face/feature select. A click on empty space
    // just disarms.
    if (state?.gizmoArmed) {
      const fid = click?.hit ? click.contributors[0] : undefined;
      if (fid !== undefined && fid > 0) {
        store.transformBody(fid);
      } else {
        store.disarmGizmo();
      }
      invalidateCanvas();
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
    // A click that misses every B-Rep body but lands on a committed sketch's
    // region (or outline) selects THAT sketch feature — so it can then be
    // extruded — taking priority over the datum plane behind it (ticket md1e).
    // `store.kernel.pick()` only knows B-Rep bodies, so committed sketch
    // overlays never produce a `click.hit`; we ray-cast against the world-space
    // geometry the overlay published to `sketchPickBridge`. Only on a plain
    // click (no armed query picker — that path already ran above).
    if (!getActiveQueryPicker() && !click?.hit) {
      const ray = rayAtMouse(event, hostRef.current);
      const sketchHit = ray ? pickSketchAtRay(ray) : null;
      if (sketchHit) {
        // Ticket 7hgr (a): scene-origin — must not exit an active session.
        store.selectOnly(sketchHit.sketchId, "scene");
        invalidateCanvas();
        return;
      }
    }

    // Default geometry is selectable in element-selection presets (Onshape
    // UX, ticket r36f): a click that hits no B-Rep element but lands on a
    // visible datum plane (or the origin marker) selects that datum. Runs
    // AFTER element resolution so model geometry always wins over the
    // planes slicing through it.
    if (selection.mode === "element" && !click?.hit) {
      const datum = datumAtClick();
      if (datum) {
        store.selectDatum(datum);
        invalidateCanvas();
        return;
      }
    }

    store.clearElementSelection();
    if (state?.highlightedBodyIndex != null) store.highlightBody(null);

    handleSelectionClick(store, click, event);
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
      type PrimName =
        | "box"
        | "sphere"
        | "cylinder"
        | "torus"
        | "cone"
        | "capsule"
        | "hex_prism"
        | "ellipsoid";
      const valid: PrimName[] = [
        "box",
        "sphere",
        "cylinder",
        "torus",
        "cone",
        "capsule",
        "hex_prism",
        "ellipsoid",
      ];
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

  const isWebGL = renderMode === "webgl";
  // Ticket 814w: the WebGL render path now uses the SAME host-pointer handlers
  // as the WebGPU path. They are renderer-independent — every pick rays through
  // `getPublishedPickCamera()` (which WebGLViewport publishes) into
  // `kernel.pick`, and hover/selection flow through the shared brepPickBridge /
  // store. Only a hard WebGPU init error still disables the host pointers
  // (nothing to interact with then).
  const noHostPointer = webgpuError !== null;

  // Nudge R3F's react-use-measure. The <Canvas> (WebGPU OR WebGL) doesn't
  // render until its ResizeObserver reports a non-zero container size, and that
  // first measure is missed on mount here — leaving the canvas blank until a
  // manual window resize (a known R3F issue; affects BOTH backends, not just
  // StrictMode). A few spaced resize events on mount / render-mode change force
  // the measure so the canvas comes up immediately.
  useEffect(() => {
    let n = 0;
    const id = setInterval(() => {
      window.dispatchEvent(new Event("resize"));
      if (++n >= 8) clearInterval(id);
    }, 120);
    return () => clearInterval(id);
  }, [renderMode]);

  return (
    <div
      ref={hostRef}
      className="fd-viewport"
      data-testid="fielddraft-canvas"
      onPointerMove={noHostPointer ? undefined : handlePointerMove}
      onPointerLeave={noHostPointer ? undefined : handlePointerLeave}
      onPointerDown={noHostPointer ? undefined : handlePointerDown}
      onPointerUp={noHostPointer ? undefined : handlePointerUp}
      onPointerCancel={noHostPointer ? undefined : handlePointerCancel}
      onClick={noHostPointer ? undefined : handleClick}
      onPointerDownCapture={trackRightDown}
      onContextMenu={handleContextMenu}
      onDragOver={noHostPointer ? undefined : handleDragOver}
      onDrop={webgpuError ? undefined : handleDrop}
    >
      {webgpuError && (
        <div className="error">
          <div className="webgpu-error">
            <strong>{webgpuError.title}</strong>
            <p>{webgpuError.detail}</p>
          </div>
        </div>
      )}
      {boxRect && (
        <div
          className={
            "select-rect" +
            (CONTROLS_PRESETS[controlsPreset].selection.boxSelect ===
              "window-crossing" && boxRect.x1 < boxRect.x0
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
      {isWebGL && (
        <WebGLViewport
          controlsPreset={controlsPreset}
          brepEdgeMode={brepEdgeMode}
          ndc={ndc}
          screenSpaceCsg={screenSpaceCsg}
          perspective={perspective}
        />
      )}
      {!isWebGL && (
        <CanvasErrorBoundary>
          <Canvas
            shadows={false}
            // ticket 7kht: disable R3F's default ACES tone mapping. The SDF
            // viewport is flat-shaded (every material sets `toneMapped=false`),
            // so ACES did nothing to the geometry — but it DID tone-map the
            // white clear colour down to grey (ACES(1.0)≈0.8), so the scene
            // background regressed to grey. `flat` → NoToneMapping keeps the
            // background (and all overlay colours) exactly as authored.
            flat
            frameloop={frameLoop}
            camera={{ position: [350, 280, 350], fov: 45, near: 1, far: 4000 }}
            gl={async (props) => {
              const { WebGPURenderer } = await import("three/webgpu");

              // WebGPU capability gate. `navigator.gpu` only proves the *API*
              // exists; a machine can expose it yet have no usable adapter
              // (WebGPU disabled by flag, missing Vulkan driver, blocklisted
              // GPU). In that case WebGPURenderer.init() silently falls back to a
              // WebGL2 backend that can't run our compute/storage-buffer SDF
              // pipeline — the user then sees a storm of shader errors instead of
              // a clear message. So we probe for an adapter up front and, belt
              // and braces, verify the live backend after init.
              if (!navigator.gpu) {
                setWebgpuError(WEBGPU_NO_API);
                throw new Error(
                  "WebGPU unavailable: navigator.gpu is undefined",
                );
              }
              let adapter: Awaited<
                ReturnType<typeof navigator.gpu.requestAdapter>
              > = null;
              try {
                adapter = await navigator.gpu.requestAdapter({
                  powerPreference: "high-performance",
                });
              } catch {
                adapter = null;
              }
              if (!adapter) {
                setWebgpuError(WEBGPU_NO_ADAPTER);
                throw new Error(
                  "WebGPU unavailable: no adapter (navigator.gpu.requestAdapter returned null)",
                );
              }

              // Telemetry: capture the GPU identity (vendor/architecture) and a
              // couple of limits so a session can be bucketed into a hardware
              // tier. Best-effort — never let it block renderer creation.
              try {
                const info = (adapter as unknown as { info?: Partial<GpuInfo> })
                  .info;
                setGpuInfo({
                  vendor: info?.vendor ?? "",
                  architecture: info?.architecture ?? "",
                  device: info?.device ?? "",
                  description: info?.description ?? "",
                  isFallback:
                    (adapter as unknown as { isFallbackAdapter?: boolean })
                      .isFallbackAdapter ?? false,
                  maxStorageBufferBindingSize:
                    adapter.limits.maxStorageBufferBindingSize,
                  maxComputeInvocationsPerWorkgroup:
                    adapter.limits.maxComputeInvocationsPerWorkgroup,
                });
              } catch {
                // adapter.info / limits unavailable on this implementation.
              }

              const renderer = new WebGPURenderer({
                ...(props as object),
                antialias: true,
              });
              await renderer.init();

              // Guard against three's silent WebGL2 fallback: if the live backend
              // isn't WebGPU, our SDF pipeline can't run — surface the same
              // message rather than render a broken scene.
              const backend = (
                renderer as unknown as {
                  backend?: { isWebGPUBackend?: boolean };
                }
              ).backend;
              if (backend?.isWebGPUBackend !== true) {
                renderer.dispose();
                setWebgpuError(WEBGPU_NO_ADAPTER);
                throw new Error(
                  "WebGPU unavailable: backend fell back to WebGL",
                );
              }

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
                <ViewportCamera perspective={perspective} />
                <ClearColorSync />
                <Scene
                  ndc={ndc}
                  onPick={setPickResult}
                  renderMode={renderMode}
                  brepEdgeMode={brepEdgeMode}
                />
                <ReferenceMesh />
                {/* Export dialog open + meshed: the exact export solid,
                    rendered in place of the suppressed SDF body pass. */}
                <ExportPreviewMesh />
                {/* Debug ▸ SDF-Plane overlay (OVERLAY_LAYER); null unless a
                    plane is published by the dialog. */}
                <SdfPlaneOverlay />
                <MesherOverlay />
                <PartAabbOverlay />
                {/* All B-Rep overlays hide during a suppressed gizmo drag:
                  the kernel serves no fresh weave then, and even cached
                  edges only refresh per 50ms commit — they'd trail the
                  drag. They remount (and refetch) on release. */}
                {!suppressBrepOverlays && (
                  <>
                    <BRepEdgeOverlay ndc={ndc} mode={brepEdgeMode} />
                    {showEdgePoints && <BRepEdgePointsOverlay />}
                    {showOctreeCells && <OctreeCellOverlay />}
                    <BRepSelectionOverlay ndc={ndc} />
                    <BRepSelectedOverlay />
                    <BodyHighlightOverlay />
                  </>
                )}
                <MateConnectorOverlay />
                {state?.mateMode && <MateInferenceOverlay ndc={ndc} />}
                <AssemblyParts />
                <CameraRig preset={controlsPreset} />
                <CameraPoseSync />
                <ViewCommandConsumer />
                <CameraQuaternionBridge />
              </>
            )}
          </Canvas>
        </CanvasErrorBoundary>
      )}
      {/* The cube stays up during a sketch session — the camera is the
          normal 3D camera there too (sketch-in-3D), and "look straight at
          the plane" is the single most wanted view action while drawing. */}
      <ViewCube />
      <ToolHint />
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
      <MeasurePanel />
      <PartAabbPanel />
      {state?.mateMode && (
        <div className="face-pick-hud" data-testid="mate-mode-hud">
          Hover a vertex, edge, or face — click to place a mate connector · R:
          reorient · Shift: lock · K: hide connectors
        </div>
      )}
      {state?.ikMode && (
        <div className="face-pick-hud" data-testid="ik-mode-hud">
          Click to set IK target position
        </div>
      )}
      {ctxMenu && (
        <ContextMenu
          x={ctxMenu.x}
          y={ctxMenu.y}
          items={contextMenuItems}
          onClose={() => setCtxMenu(null)}
          testId="viewport-context-menu"
        />
      )}
    </div>
  );
}
