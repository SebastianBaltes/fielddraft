import { useEffect, useMemo, useRef, useSyncExternalStore } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import { LineSegments2 } from "three/examples/jsm/lines/webgpu/LineSegments2.js";
import { LineSegmentsGeometry } from "three/examples/jsm/lines/LineSegmentsGeometry.js";
import {
  Line2NodeMaterial,
  MeshBasicNodeMaterial,
  SpriteNodeMaterial,
} from "three/webgpu";
import * as THREE from "three";
import { isWebGPURenderer } from "./overlayLineMaterials";
import { useKernelStore } from "./store";
import { OVERLAY_LAYER } from "./viewportSdfOverlays";
import { disposeAfterGpuIdle } from "./sdfPipelineCommon";
import {
  resolvePlaneFrame,
  planeToWorld,
  type PlaneFrame,
} from "./sketchPlaneFrame";
// Extracted to sketchOutline.ts (DOM-free) — shared with the extrude
// gizmo's pending-sketch-extrude anchor.
import {
  collectSketchLineSegments,
  collectSketchSessionGeometry,
  collectSketchRegionFill,
  collectSketchLinePickables,
  type SketchSubCall,
  type SketchLinePickable,
} from "./sketchOutline";
import { buildDraftSketchState } from "./sketchSession";
import {
  makeLiveEdgeLookup,
  resolveProjectedSubCalls,
} from "./sketchProject";
import {
  getHoveredSketchEntity,
  subscribeHoveredSketchEntity,
} from "./sketchEntityHoverBridge";
import { SketchDots, makeSketchDots } from "./sketchDots";
import type { SketchState } from "./solver/types";
import { displayDegreesOfFreedom } from "./solver/dofPreflight";
import { gripsForSelection } from "./sketchGrips";
import { getSketchPlaneFrame } from "./sketchCameraBridge";
import {
  readSketchHud,
  setHudInvalidate,
  clearSketchHud,
  subscribeSketchHud,
} from "./sketchHudBridge";
import {
  readSketchLabels,
  setLabelInvalidate,
  clearSketchLabels,
  type SketchLabel,
} from "./sketchLabelBridge";
import { filterAllows } from "./elementQuery";
import { getActive as getActiveQueryPicker } from "./queryPickerBridge";
import {
  publishSketchPickables,
  clearSketchPickables,
} from "./sketchPickBridge";
import {
  publishSketchLinePickables,
  clearSketchLinePickables,
  getHoveredSketchLine,
  subscribeHoveredSketchLine,
} from "./sketchLinePickBridge";

// ── Sketch scene overlays ────────────────────────────────────────────
//
// Everything the viewport draws FOR SKETCHES as real 3D scene geometry:
// the committed (visible, non-edited) sketches, the active edit session,
// the transient interaction HUD and the billboarded text labels.

export function SketchOverlay3D() {
  const { store, state } = useKernelStore();
  const visibleSketches = state?.visibleSketches;
  // While a sketch is actively being edited, its OWN geometry is drawn by the
  // editable session overlay (`SketchSession3D`). The committed overlay must
  // skip exactly that sketch — otherwise its loaded geometry is double-drawn on
  // top of the editable copy AND published to `sketchPickBridge`, so md1e's
  // "click sketch → select feature" intercepts clicks meant to select/delete a
  // session entity inside the edited sketch (the loaded circle/line couldn't be
  // picked there at all). Other visible (non-edited) sketches stay rendered +
  // pickable; when no session is active nothing is excluded (ticket o2y3).
  const session = state?.sketchSession;
  const editingSketchId = session?.active ? session.editingFeatureId : null;
  // The committed overlay must look EXACTLY like the active sketch session
  // (ticket jjmv). The session uses fat node-material lines — those only run on
  // the WebGPU backend, so the primary path gets identical strokes here; the
  // WebGL fallback (separate classic-renderer Canvas) draws thin classic lines.
  const gl = useThree((s) => s.gl);
  const isWebGPU =
    (gl as unknown as { isWebGPURenderer?: boolean }).isWebGPURenderer === true;

  const sketches = useMemo(() => {
    if (!store || !visibleSketches || visibleSketches.length === 0) return [];
    const out: {
      id: number;
      linePos: number[];
      pointPos: number[];
      fillPos: number[] | null;
      linePick: SketchLinePickable[];
      // The sketch's plane frame, so a region click can map world→uv for the
      // kernel's region query (ticket dhjn).
      frame: {
        origin: [number, number, number];
        xAxis: [number, number, number];
        yAxis: [number, number, number];
      };
    }[] = [];
    const tri = (poly: [number, number][]): number[][] => {
      const contour = poly.map((q) => new THREE.Vector2(q[0], q[1]));
      return THREE.ShapeUtils.triangulateShape(contour, []);
    };
    // jcb7: live edge lookup, shared across all committed sketches in this
    // pass, so a `sketch_project` sub-call resolves to the body's CURRENT
    // geometry rather than its frozen document snapshot. Built lazily inside
    // the lookup, so sketches without projections pay nothing.
    const liveEdges = makeLiveEdgeLookup(store.kernel);
    for (const sketchId of visibleSketches) {
      // Skip the actively-edited sketch — the session overlay owns it (o2y3).
      if (sketchId === editingSketchId) continue;
      try {
        const params = store.kernel.getFeatureCallParams(sketchId) as {
          plane?: string;
          sub_calls?: Array<{ type: string; params: Record<string, unknown> }>;
        };
        // Sketch-in-3D Phase 2 (ticket bxoo): resolve the real plane
        // frame (datum OR face plane incl. origin offset) instead of
        // the old hardcoded origin-plane axes.
        const frame =
          resolvePlaneFrame(
            params.plane ?? "qCreatedBy(origin_plane_xy)",
            store.kernel,
          ) ?? resolvePlaneFrame("qCreatedBy(origin_plane_xy)", store.kernel)!;
        // jcb7: rewrite `sketch_project` sub-calls to live `sketch_line`s so
        // the projection tracks the body — then every collector below stays
        // projection-agnostic.
        const subCalls = resolveProjectedSubCalls(
          (params.sub_calls ?? []) as SketchSubCall[],
          frame,
          liveEdges,
        );
        const linePos = collectSketchLineSegments(subCalls, frame);
        if (linePos.length === 0) continue;
        // Endpoint/vertex dots like the active sketch session (ticket n6hg):
        // reuse the session geometry collector so every entity's natural
        // vertices (line endpoints, circle centre, polygon/rect corners, arc
        // endpoints, …) render identically to the in-edit overlay.
        const pointPos = collectSketchSessionGeometry(
          subCalls as SketchSubCall[],
          frame,
        ).points;
        // Closed regions filled like in sketch mode — so a region reads as a
        // face and is a pickable profile target.
        const fp = collectSketchRegionFill(
          subCalls as SketchSubCall[],
          frame,
          tri,
        );
        // Per-line pickables (incl. construction lines) for the Revolve
        // axis picker — `qSketchLine(sketchId, lineId)`.
        const linePick = collectSketchLinePickables(
          subCalls as SketchSubCall[],
          frame,
        );
        out.push({
          id: sketchId,
          linePos,
          pointPos,
          fillPos: fp.length ? fp : null,
          linePick,
          frame: {
            origin: frame.origin,
            xAxis: frame.xAxis,
            yAxis: frame.yAxis,
          },
        });
      } catch {
        // Skip invalid sketches
      }
    }
    return out;
  }, [store, visibleSketches, editingSketchId, state?.features]);

  // Fat-line material (WebGPU only) — mirrors the session's solid stroke exactly
  // (same colour + SKETCH_STROKE_WIDTH). Persistent so geometry rebuilds don't
  // churn it.
  const lineMatRef = useRef<Line2NodeMaterial | null>(null);
  if (isWebGPU && !lineMatRef.current) {
    lineMatRef.current = new Line2NodeMaterial({
      color: SKETCH_COLOR_COMMITTED,
      linewidth: SKETCH_STROKE_WIDTH,
      worldUnits: false,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    });
  }
  useEffect(
    () => () => {
      lineMatRef.current?.dispose();
    },
    [],
  );

  // WebGPU: fat LineSegments2 (geometry type must match the node material).
  const fatLines = useMemo(() => {
    if (!isWebGPU || !lineMatRef.current) return null;
    const map = new Map<number, LineSegments2>();
    for (const s of sketches) {
      const g = new LineSegmentsGeometry();
      g.setPositions(new Float32Array(s.linePos));
      const o = new LineSegments2(g, lineMatRef.current);
      o.frustumCulled = false;
      o.renderOrder = 997;
      // Draw in the overlay-only pass (`camera.layers.set(OVERLAY_LAYER)`),
      // like the session lines and the dots — without this the committed
      // sketch's lines never render in WebGPU (only the dots showed; the
      // SDF base pass doesn't draw regular meshes). Ticket fdzj.
      o.layers.enable(OVERLAY_LAYER);
      map.set(s.id, o);
    }
    return map;
  }, [sketches, isWebGPU]);

  // WebGL fallback: thin classic line geometries.
  const classicLines = useMemo(() => {
    if (isWebGPU) return null;
    const map = new Map<number, THREE.BufferGeometry>();
    for (const s of sketches) {
      const g = new THREE.BufferGeometry();
      g.setAttribute(
        "position",
        new THREE.Float32BufferAttribute(s.linePos, 3),
      );
      map.set(s.id, g);
    }
    return map;
  }, [sketches, isWebGPU]);

  // Endpoint/vertex dots as billboarded screen-constant circle MESHES (ticket
  // tdq2 — the old THREE.Points dots from n6hg/b78p never rendered in the
  // overlay pass). Mesh-based, so one path serves both backends; mirrors the
  // session's `M.point` (same colour + SKETCH_POINT_SIZE) for pqoe parity.
  const dots = useMemo(() => {
    const map = new Map<number, SketchDots>();
    for (const s of sketches) {
      // 998: above fills (996) + lines (997), like the session.
      const d = makeSketchDots(
        s.pointPos,
        SKETCH_COLOR_COMMITTED,
        SKETCH_POINT_SIZE,
        998,
      );
      if (d) map.set(s.id, d);
    }
    return map;
  }, [sketches]);

  // Region fills — classic MeshBasicMaterial renders on either backend.
  const fills = useMemo(() => {
    const map = new Map<number, THREE.BufferGeometry>();
    for (const s of sketches) {
      if (!s.fillPos) continue;
      const g = new THREE.BufferGeometry();
      g.setAttribute(
        "position",
        new THREE.Float32BufferAttribute(s.fillPos, 3),
      );
      map.set(s.id, g);
    }
    return map;
  }, [sketches]);

  // fs23: hover highlight — the sketch line the Viewport published as the
  // current query target (e.g. the Revolve axis under the cursor). Subscribe to
  // the bridge and recolour that one segment in the selected colour, so a
  // sketch line reads as a "potential target" on mouseover just like a B-Rep
  // edge does. (B-Rep edges already highlight via their own hover overlay.)
  const hoveredLine = useSyncExternalStore(
    subscribeHoveredSketchLine,
    getHoveredSketchLine,
    () => null,
  );
  const hoverSeg = useMemo<number[] | null>(() => {
    if (!hoveredLine) return null;
    const sk = sketches.find((s) => s.id === hoveredLine.sketchId);
    const ln = sk?.linePick.find((l) => l.lineId === hoveredLine.lineId);
    return ln ? [...ln.a, ...ln.b] : null;
  }, [hoveredLine, sketches]);

  // A dedicated highlight material (WebGPU fat line): selected colour, a touch
  // thicker than the base stroke so it stands out under the cursor.
  const hoverMatRef = useRef<Line2NodeMaterial | null>(null);
  if (isWebGPU && !hoverMatRef.current) {
    hoverMatRef.current = new Line2NodeMaterial({
      color: SKETCH_SELECTED_COLOR,
      linewidth: SKETCH_STROKE_WIDTH * 1.6,
      worldUnits: false,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    });
  }
  useEffect(
    () => () => {
      hoverMatRef.current?.dispose();
    },
    [],
  );

  const hoverFat = useMemo<LineSegments2 | null>(() => {
    if (!isWebGPU || !hoverSeg || !hoverMatRef.current) return null;
    const g = new LineSegmentsGeometry();
    g.setPositions(new Float32Array(hoverSeg));
    const o = new LineSegments2(g, hoverMatRef.current);
    o.frustumCulled = false;
    o.renderOrder = 999; // above the base lines (997) + dots (998)
    o.layers.enable(OVERLAY_LAYER);
    return o;
  }, [isWebGPU, hoverSeg]);

  const hoverClassic = useMemo<THREE.BufferGeometry | null>(() => {
    if (isWebGPU || !hoverSeg) return null;
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", new THREE.Float32BufferAttribute(hoverSeg, 3));
    return g;
  }, [isWebGPU, hoverSeg]);

  // Ticket o6hx / fg9o: these geometries may still be referenced by an
  // in-flight GPU submit when their memo recomputes and tears the previous set
  // down. Destroying them synchronously mid-submit triggers "[Buffer] used in
  // submit while destroyed" and can wedge the device, so defer disposal until
  // the GPU queue has drained (falls back to sync on the WebGL backend).
  //
  // CRUCIAL: one effect PER memo, each keyed on its OWN value. A single bundled
  // effect over all six sets re-ran whenever ANY of them changed — so hovering
  // the Revolve-axis sketch line (which only recomputes `hoverFat`) tore down
  // the still-live base lines/dots/fills, making the hovered edge vanish and
  // spamming the buffer-destroyed error (ticket y2xm).
  useEffect(() => {
    if (!fatLines) return;
    return () =>
      disposeAfterGpuIdle(gl, () =>
        fatLines.forEach((o) => o.geometry.dispose()),
      );
  }, [gl, fatLines]);
  useEffect(() => {
    if (!classicLines) return;
    return () =>
      disposeAfterGpuIdle(gl, () => classicLines.forEach((g) => g.dispose()));
  }, [gl, classicLines]);
  useEffect(() => {
    return () => disposeAfterGpuIdle(gl, () => dots.forEach((d) => d.dispose()));
  }, [gl, dots]);
  useEffect(() => {
    return () =>
      disposeAfterGpuIdle(gl, () => fills.forEach((g) => g.dispose()));
  }, [gl, fills]);
  useEffect(() => {
    if (!hoverFat) return;
    return () => disposeAfterGpuIdle(gl, () => hoverFat.geometry.dispose());
  }, [gl, hoverFat]);
  useEffect(() => {
    if (!hoverClassic) return;
    return () => disposeAfterGpuIdle(gl, () => hoverClassic.dispose());
  }, [gl, hoverClassic]);

  // Publish the world-space pick geometry per visible committed sketch so a
  // plain click in the viewport can select the sketch — taking priority over
  // the datum plane behind it (ticket md1e). `linePos`/`fillPos` are already
  // world-space (both go through `planeToWorld` in `sketchOutline`).
  useEffect(() => {
    if (sketches.length === 0) {
      clearSketchPickables();
      return;
    }
    publishSketchPickables(
      sketches.map((s) => ({
        sketchId: s.id,
        fill: s.fillPos ? new Float32Array(s.fillPos) : null,
        lines: s.linePos.length ? new Float32Array(s.linePos) : null,
        frame: s.frame,
      })),
    );
    return () => clearSketchPickables();
  }, [sketches]);

  // Publish per-line pickables so the Revolve axis picker can ray-cast a
  // single sketch line and emit `qSketchLine`. Separate from the
  // whole-sketch pickables above, which select a sketch as a profile.
  useEffect(() => {
    const withLines = sketches.filter((s) => s.linePick.length > 0);
    if (withLines.length === 0) {
      clearSketchLinePickables();
      return;
    }
    publishSketchLinePickables(
      withLines.map((s) => ({ sketchId: s.id, lines: s.linePick })),
    );
    return () => clearSketchLinePickables();
  }, [sketches]);

  // A click on a filled region selects the whole sketch as a profile, but only
  // when a profile-style picker (no filter, or one that accepts faces) is
  // armed — a BODY/edge picker must ignore sketch regions (ticket jjmv).
  const pickRegion =
    (sketchId: number) => (e: { stopPropagation: () => void }) => {
      const picker = getActiveQueryPicker();
      if (!picker) return;
      if (picker.entityFilter && !filterAllows("face", picker.entityFilter))
        return;
      e.stopPropagation();
      picker.onPick(`qCreatedBy(${sketchId})`);
    };

  // The committed sketch's fill + classic lines must draw in the overlay-only
  // pass (`camera.layers.set(OVERLAY_LAYER)`), like the fat lines and dots —
  // otherwise they never render in WebGPU. Ticket fdzj.
  const enableOverlayLayer = (o: THREE.Object3D | null) => {
    if (o) o.layers.enable(OVERLAY_LAYER);
  };

  if (sketches.length === 0) return null;

  return (
    <group>
      {sketches.map(({ id }) => (
        <group key={id}>
          {fills.get(id) && (
            <mesh
              geometry={fills.get(id)}
              renderOrder={996}
              ref={enableOverlayLayer}
              onClick={pickRegion(id)}
              onPointerDown={pickRegion(id)}
            >
              <meshBasicMaterial
                color={SKETCH_FILL_COLOR}
                transparent
                opacity={SKETCH_FILL_OPACITY}
                side={THREE.DoubleSide}
                depthTest={false}
                depthWrite={false}
              />
            </mesh>
          )}
          {isWebGPU
            ? fatLines?.get(id) && (
                <primitive object={fatLines.get(id)!} dispose={null} />
              )
            : classicLines?.get(id) && (
                <lineSegments
                  geometry={classicLines.get(id)}
                  renderOrder={997}
                  ref={enableOverlayLayer}
                >
                  <lineBasicMaterial
                    color={SKETCH_COLOR_COMMITTED}
                    transparent
                    depthTest={false}
                  />
                </lineSegments>
              )}
          {/* Endpoint/vertex dots (tickets n6hg/b78p/tdq2) — above fills +
              lines, as a billboarded screen-constant circle mesh. */}
          {dots.get(id) && (
            <primitive object={dots.get(id)!.mesh} dispose={null} />
          )}
        </group>
      ))}
      {/* fs23: the hovered query-target sketch line, recoloured on top. */}
      {isWebGPU
        ? hoverFat && <primitive object={hoverFat} dispose={null} />
        : hoverClassic && (
            <lineSegments
              geometry={hoverClassic}
              renderOrder={999}
              ref={enableOverlayLayer}
            >
              <lineBasicMaterial
                color={SKETCH_SELECTED_COLOR}
                transparent
                depthTest={false}
              />
            </lineSegments>
          )}
    </group>
  );
}

// DOF status colours (mirror SketchCanvas2d's COLOR_UNDER/FULL/OVER): blue =
// underconstrained, green = fully constrained, red = over/conflicting.
const SKETCH_COLOR_UNDER = 0x2563eb;
const SKETCH_COLOR_FULL = 0x16a34a;
// Committed (visible, NOT being edited) sketches render in slate GRAY, not the
// DOF blue — blue is reserved for the sketch open for editing (ticket imqf).
const SKETCH_COLOR_COMMITTED = 0x64748b;
const SKETCH_COLOR_OVER = 0xdc2626;
// Fixed (non-DOF) sketch colours, mirroring the canvas tokens in SketchCanvas2d.
const SKETCH_CONSTRUCTION_COLOR = 0xb07820; // COLOR_CONSTRUCTION
const SKETCH_GUIDE_COLOR = 0x64748b; // COLOR_GUIDE (rgba .6)
const SKETCH_LEG_COLOR = 0x7c3aed; // COLOR_TANGENT_LEG
const SKETCH_FILL_COLOR = 0x94a3b8; // COLOR_REGION_FILL
// Shared sketch render parameters so the active sketch mode and the committed
// 3D overlay look EXACTLY identical (ticket jjmv): one fill opacity, one stroke
// width. Bumped from the original 0.18 / 2 — the fill was too faint and the
// stroke too thin to read in 3D.
const SKETCH_FILL_OPACITY = 0.32;
const SKETCH_STROKE_WIDTH = 4;
// Endpoint dots are filled circles at DOUBLE the line diameter (ticket b78p),
// derived from the stroke width so they always track it. They render as
// billboarded screen-constant circle meshes via SketchDots (ticket tdq2) — the
// THREE.Points / PointsNodeMaterial path the dots used before never rendered in
// the three/webgpu overlay pass.
const SKETCH_POINT_SIZE = 2 * SKETCH_STROKE_WIDTH;
const SKETCH_AXIS_COLOR = 0x000000; // origin axes (rgba .25)
const SKETCH_SELECTED_COLOR = 0xf97316; // COLOR_SELECTED
// Pre-pick hover highlight (ticket zadt): amber, distinct from both the blue
// edit strokes and the orange committed selection — "a click here picks this".
const SKETCH_HOVER_COLOR = 0xfbbf24;

/** Selection highlight + grip markers (world-space) for the picked sketch
 *  entities, mirroring drawOverlay's `drawSelectionHighlight` + `drawGrips`.
 *  Lines/circles/arcs become thick outline segments; points/origin and the
 *  drag grips become marker dots. */
function collectSelectionGeometry(
  state: SketchState,
  selection: ReadonlyArray<number>,
  subCalls: ReadonlyArray<{ type: string; params: Record<string, unknown> }>,
  frame: PlaneFrame,
): { highlight: number[]; markers: number[] } {
  const highlight: number[] = [];
  const markers: number[] = [];
  const w = (x: number, y: number) => planeToWorld(x, y, frame);
  const pointAt = (id: number): [number, number] | null => {
    const e = state.entities.find((x) => x.id === id);
    if (!e) return null;
    if (e.kind === "point") return [e.at.x, e.at.y];
    if (e.kind === "origin") return [0, 0];
    return null;
  };
  for (const id of selection) {
    const e = state.entities.find((x) => x.id === id);
    if (!e) continue;
    if (e.kind === "point" || e.kind === "origin") {
      const at = e.kind === "origin" ? [0, 0] : [e.at.x, e.at.y];
      markers.push(...w(at[0], at[1]));
    } else if (e.kind === "line") {
      const a = pointAt(e.start);
      const b = pointAt(e.end);
      if (a && b) highlight.push(...w(a[0], a[1]), ...w(b[0], b[1]));
    } else if (e.kind === "circle" || e.kind === "arc") {
      const c = pointAt(e.center);
      const r = (e as { radius?: number }).radius;
      if (c && typeof r === "number" && r > 0) {
        const steps = 64;
        let prev: [number, number] = [c[0] + r, c[1]];
        for (let i = 1; i <= steps; i++) {
          const t = (i / steps) * Math.PI * 2;
          const next: [number, number] = [
            c[0] + r * Math.cos(t),
            c[1] + r * Math.sin(t),
          ];
          highlight.push(...w(prev[0], prev[1]), ...w(next[0], next[1]));
          prev = next;
        }
      }
    }
  }
  for (const g of gripsForSelection(subCalls, selection)) {
    markers.push(...w(g.pos[0], g.pos[1]));
  }
  return { highlight, markers };
}

/**
 * ALL committed geometry of the active sketch session, rendered as real 3D
 * scene geometry on the sketch plane — fat lines, vertex points, the
 * closed-region fill and the origin axes. The GPU transforms everything
 * with the same camera matrix as the rest of the scene in the same frame,
 * so nothing lags behind / desyncs from the model during a camera pan or
 * orbit (the 2D overlay's separate rAF loop ran a frame behind the
 * demand-mode WebGPU render — the original "ruckelt nach" bug).
 *
 * `collectSketchSessionGeometry` splits the geometry into the categories
 * each material draws (solid / construction-dashed / guide-dashed / tangent
 * legs / points / off-curve handles); `drawOverlay` no longer strokes ANY
 * committed geometry — it owns only the transient HUD (hover/snap/drag
 * previews, selection highlight + grips, dimension/constraint text labels,
 * marquee), which appear during pointer interaction, not camera motion.
 */
export function SketchSession3D() {
  const { store, state } = useKernelStore();
  const invalidate = useThree((s) => s.invalidate);
  // Backend detection (ticket uzhr): the fat node materials below
  // (Line2NodeMaterial / MeshBasicNodeMaterial) only render on the WebGPU
  // backend, so while sketching in the classic THREE.WebGLRenderer fallback
  // nothing showed up. Mirror SketchOverlay3D's branch and draw classic
  // materials on the WebGL path so the in-edit geometry is VISIBLE there too.
  const gl = useThree((s) => s.gl);
  const isWebGPU =
    (gl as unknown as { isWebGPURenderer?: boolean }).isWebGPURenderer === true;
  const session = state?.sketchSession;
  const active = session?.active ?? false;

  // Resolve the plane frame from the session's own plane query rather than
  // reading the camera bridge — avoids a one-frame race with the overlay's
  // resolve effect on a plane change.
  const frame = useMemo(() => {
    if (!store || !active || !session) return null;
    return (
      resolvePlaneFrame(
        session.planeQuery ?? "qCreatedBy(origin_plane_xy)",
        store.kernel,
      ) ?? resolvePlaneFrame("qCreatedBy(origin_plane_xy)", store.kernel)
    );
  }, [store, active, session?.planeQuery]);

  // jcb7: resolve the session's `sketch_project` sub-calls to live
  // `sketch_line`s against the CURRENT body edges, so the in-edit overlay
  // tracks the body exactly like the committed overlay. A `sketch_project`
  // resolves to one `sketch_line` (2 points + 1 line) — the same allocation
  // the kernel's `SketchProjectTool::apply` does — so the entity-id mirror
  // (`buildDraftSketchState`) stays in lockstep. Falls back to raw sub-calls
  // when there's no frame yet (one-frame plane-resolve race).
  const sessionSubCalls = useMemo(() => {
    if (!session) return [];
    const raw = session.subCalls as SketchSubCall[];
    if (!store || !frame) return raw;
    return resolveProjectedSubCalls(
      raw,
      frame,
      makeLiveEdgeLookup(store.kernel),
    );
  }, [store, frame, session?.subCalls]);

  // Solved entity state — shared by the DOF colour and the selection
  // highlight (same id space as session.selection).
  const skState = useMemo(() => {
    if (!active || !session) return null;
    try {
      return buildDraftSketchState(sessionSubCalls);
    } catch {
      return null;
    }
  }, [active, session, sessionSubCalls]);

  // 3-colour DOF status — same derivation as drawOverlay's committedColor.
  const color = useMemo(() => {
    if (!active || !session) return SKETCH_COLOR_UNDER;
    try {
      if (
        session.diagnostics.conflicting.length > 0 ||
        session.diagnostics.malformed.length > 0
      ) {
        return SKETCH_COLOR_OVER;
      }
      if (!skState) return SKETCH_COLOR_UNDER;
      const dof = displayDegreesOfFreedom(skState, session.diagnostics);
      return dof > 0
        ? SKETCH_COLOR_UNDER
        : dof === 0
          ? SKETCH_COLOR_FULL
          : SKETCH_COLOR_OVER;
    } catch {
      return SKETCH_COLOR_UNDER;
    }
  }, [active, skState, session?.diagnostics]);

  const geom = useMemo(() => {
    if (!active || !frame || !session) return null;
    return collectSketchSessionGeometry(sessionSubCalls, frame);
  }, [active, frame, session, sessionSubCalls]);

  const selection = useMemo(() => {
    if (
      !active ||
      !frame ||
      !session ||
      !skState ||
      session.selection.length === 0
    ) {
      return null;
    }
    const g = collectSelectionGeometry(
      skState,
      session.selection,
      sessionSubCalls,
      frame,
    );
    return g.highlight.length || g.markers.length ? g : null;
  }, [active, frame, skState, session?.selection, sessionSubCalls]);

  // Pre-pick hover highlight (ticket zadt): the entity under the cursor while
  // in select mode, published by the 2D interaction canvas. Drawn like the
  // selection but amber and one layer below it — an already-selected entity
  // keeps the orange selection look, so hover never doubles up on it.
  const hoveredEntity = useSyncExternalStore(
    subscribeHoveredSketchEntity,
    getHoveredSketchEntity,
    () => null,
  );
  const hover = useMemo(() => {
    if (
      !active ||
      !frame ||
      !session ||
      !skState ||
      hoveredEntity === null ||
      session.selectedTool !== null ||
      session.selection.includes(hoveredEntity)
    ) {
      return null;
    }
    const g = collectSelectionGeometry(
      skState,
      [hoveredEntity],
      sessionSubCalls,
      frame,
    );
    return g.highlight.length || g.markers.length ? g : null;
  }, [
    active,
    frame,
    skState,
    hoveredEntity,
    sessionSubCalls,
    session?.selectedTool,
    session?.selection,
    session?.subCalls,
  ]);

  const fill = useMemo(() => {
    if (!active || !frame || !session) return null;
    const tri = (poly: [number, number][]): number[][] => {
      const contour = poly.map((q) => new THREE.Vector2(q[0], q[1]));
      return THREE.ShapeUtils.triangulateShape(contour, []);
    };
    const pos = collectSketchRegionFill(
      session.subCalls as SketchSubCall[],
      frame,
      tri,
    );
    return pos.length ? pos : null;
  }, [active, frame, session?.subCalls]);

  const axes = useMemo(() => {
    if (!active || !frame) return null;
    const L = 1e5; // long enough to cross the viewport at any usable zoom
    return [
      ...planeToWorld(-L, 0, frame),
      ...planeToWorld(L, 0, frame),
      ...planeToWorld(0, -L, frame),
      ...planeToWorld(0, L, frame),
    ];
  }, [active, frame]);

  // Materials persist across rebuilds (only the DOF-driven colours mutate);
  // the WebGPU backend handles a material colour change fine but never a
  // geometry mutation, so the objects are rebuilt fresh on every change.
  //
  // Two parallel material sets (ticket uzhr): the WebGPU path keeps the fat
  // node materials (Line2NodeMaterial / MeshBasicNodeMaterial) for the exact
  // unchanged look; the classic WebGLRenderer fallback uses thin classic
  // THREE.Line(Dashed)Material + MeshBasicMaterial — which is what
  // SketchOverlay3D draws on its WebGL branch (plain LineSegments +
  // lineBasicMaterial), so committed + session strokes look consistent there.
  type LineMat =
    | Line2NodeMaterial
    | THREE.LineBasicMaterial
    | THREE.LineDashedMaterial;
  type FillMat = MeshBasicNodeMaterial | THREE.MeshBasicMaterial;
  const matsRef = useRef<{
    solid: LineMat;
    construction: LineMat;
    guide: LineMat;
    leg: LineMat;
    axis: LineMat;
    selLine: LineMat;
    hovLine: LineMat;
    fill: FillMat;
  } | null>(null);
  if (!matsRef.current) {
    if (isWebGPU) {
      const fatLine = (
        c: number,
        lw: number,
        extra: Record<string, unknown> = {},
      ) =>
        new Line2NodeMaterial({
          color: c,
          linewidth: lw,
          worldUnits: false,
          depthTest: false,
          depthWrite: false,
          transparent: true,
          ...extra,
        });
      matsRef.current = {
        solid: fatLine(color, SKETCH_STROKE_WIDTH),
        construction: fatLine(SKETCH_CONSTRUCTION_COLOR, 2, {
          dashed: true,
          dashSize: 3,
          gapSize: 2,
        }),
        guide: fatLine(SKETCH_GUIDE_COLOR, 1, {
          opacity: 0.6,
          dashed: true,
          dashSize: 3,
          gapSize: 2,
        }),
        leg: fatLine(SKETCH_LEG_COLOR, 2),
        axis: fatLine(SKETCH_AXIS_COLOR, 1, { opacity: 0.22 }),
        selLine: fatLine(SKETCH_SELECTED_COLOR, 4),
        hovLine: fatLine(SKETCH_HOVER_COLOR, 4),
        fill: new MeshBasicNodeMaterial({
          color: SKETCH_FILL_COLOR,
          transparent: true,
          opacity: SKETCH_FILL_OPACITY,
          depthTest: false,
          depthWrite: false,
          side: THREE.DoubleSide,
        }),
      };
    } else {
      // Classic WebGL fallback: thin but VISIBLE lines (LineBasicMaterial's
      // linewidth is ignored on WebGL — fat lines would need LineMaterial with
      // a per-frame `.resolution`, which SketchOverlay3D deliberately skips, so
      // we skip it too for the same simple, consistent look). Dashed strokes
      // use LineDashedMaterial (needs computeLineDistances() on the geometry).
      const solidLine = (c: number, opacity = 1) =>
        new THREE.LineBasicMaterial({
          color: c,
          transparent: true,
          opacity,
          depthTest: false,
          depthWrite: false,
        });
      const dashedLine = (c: number, opacity = 1) =>
        new THREE.LineDashedMaterial({
          color: c,
          transparent: true,
          opacity,
          depthTest: false,
          depthWrite: false,
          dashSize: 3,
          gapSize: 2,
        });
      matsRef.current = {
        solid: solidLine(color),
        construction: dashedLine(SKETCH_CONSTRUCTION_COLOR),
        guide: dashedLine(SKETCH_GUIDE_COLOR, 0.6),
        leg: solidLine(SKETCH_LEG_COLOR),
        axis: solidLine(SKETCH_AXIS_COLOR, 0.22),
        selLine: solidLine(SKETCH_SELECTED_COLOR),
        hovLine: solidLine(SKETCH_HOVER_COLOR),
        fill: new THREE.MeshBasicMaterial({
          color: SKETCH_FILL_COLOR,
          transparent: true,
          opacity: SKETCH_FILL_OPACITY,
          depthTest: false,
          depthWrite: false,
          side: THREE.DoubleSide,
        }),
      };
    }
  }
  const M = matsRef.current;
  useEffect(() => {
    M.solid.color.setHex(color);
    invalidate();
  }, [color, invalidate, M]);
  // Dispose materials when the component unmounts (session closed).
  useEffect(
    () => () => {
      for (const m of Object.values(M)) m.dispose();
    },
    [M],
  );

  const objects = useMemo(() => {
    const objs: THREE.Object3D[] = [];
    const addLines = (
      pos: number[] | null,
      mat: LineMat,
      order: number,
      dashed: boolean,
    ) => {
      if (!pos || pos.length === 0) return;
      let o: THREE.Object3D;
      if (isWebGPU) {
        // WebGPU: fat LineSegments2 (geometry type must match the node material).
        const g = new LineSegmentsGeometry();
        g.setPositions(new Float32Array(pos));
        const line = new LineSegments2(g, mat as Line2NodeMaterial);
        if (dashed) line.computeLineDistances();
        o = line;
      } else {
        // Classic WebGL fallback (ticket uzhr): plain THREE.LineSegments + a
        // classic Line(Dashed)Material, exactly like SketchOverlay3D's WebGL
        // branch. computeLineDistances() feeds the dashed materials.
        const g = new THREE.BufferGeometry();
        g.setAttribute("position", new THREE.Float32BufferAttribute(pos, 3));
        const line = new THREE.LineSegments(g, mat as THREE.LineBasicMaterial);
        if (dashed) line.computeLineDistances();
        o = line;
      }
      o.frustumCulled = false;
      o.renderOrder = order;
      o.layers.enable(OVERLAY_LAYER);
      objs.push(o);
    };
    // Vertex/handle/selection dots are NOT built here — they live in a
    // persistent pool (`dotPoolRef`, ticket vwzj) so a per-frame geom rebuild
    // (endpoint drag) updates them in place instead of recreating the
    // InstancedMesh and flashing all points. Only the lines/fill rebuild here.
    addLines(axes, M.axis, 994, false);
    if (fill) {
      const g = new THREE.BufferGeometry();
      g.setAttribute("position", new THREE.Float32BufferAttribute(fill, 3));
      const o = new THREE.Mesh(g, M.fill);
      o.frustumCulled = false;
      o.renderOrder = 995;
      o.layers.enable(OVERLAY_LAYER);
      objs.push(o);
    }
    if (geom) {
      addLines(geom.construction, M.construction, 996, true);
      addLines(geom.guide, M.guide, 996, true);
      addLines(geom.legs, M.leg, 996, false);
      addLines(geom.solid, M.solid, 997, false);
    }
    if (hover) {
      addLines(hover.highlight, M.hovLine, 998.5, false);
    }
    if (selection) {
      addLines(selection.highlight, M.selLine, 999, false);
    }
    return { objs };
  }, [geom, fill, axes, hover, selection, color, M, isWebGPU]);

  useEffect(() => {
    invalidate();
    return () => {
      for (const o of objects.objs)
        (o as { geometry?: THREE.BufferGeometry }).geometry?.dispose();
    };
  }, [objects, invalidate]);

  // Persistent vertex/handle/hover/selection dots (ticket vwzj). Updated IN
  // PLACE across rebuilds — an endpoint drag rebuilds `geom` every frame, and
  // recreating the InstancedMesh dots each time made ALL points flash (a fresh
  // InstancedMesh churns for a frame while the baked-in fat lines stay put).
  // Reusing the same mesh per role keeps the scene-graph object stable.
  const dotsGroupRef = useRef<THREE.Group>(null);
  const dotPoolRef = useRef<Map<string, SketchDots>>(new Map());
  useEffect(() => {
    const group = dotsGroupRef.current;
    if (!group) return;
    const pool = dotPoolRef.current;
    const want: Array<{
      role: string;
      pos: number[] | null | undefined;
      color: number;
      size: number;
      order: number;
    }> = [];
    if (geom) {
      want.push({ role: "points", pos: geom.points, color, size: SKETCH_POINT_SIZE, order: 998 });
      want.push({ role: "handles", pos: geom.handles, color, size: 6, order: 998 });
    }
    if (hover) {
      want.push({ role: "hover", pos: hover.markers, color: SKETCH_HOVER_COLOR, size: 10, order: 998.6 });
    }
    if (selection) {
      want.push({ role: "selection", pos: selection.markers, color: SKETCH_SELECTED_COLOR, size: 10, order: 1000 });
    }
    const used = new Set<string>();
    for (const w of want) {
      if (!w.pos || w.pos.length === 0) continue;
      used.add(w.role);
      const existing = pool.get(w.role);
      if (existing && existing.tryUpdate(w.pos, w.color, w.size)) continue;
      if (existing) {
        group.remove(existing.mesh);
        existing.dispose();
        pool.delete(w.role);
      }
      const d = makeSketchDots(w.pos, w.color, w.size, w.order);
      if (d) {
        group.add(d.mesh);
        pool.set(w.role, d);
      }
    }
    for (const [role, d] of [...pool]) {
      if (!used.has(role)) {
        group.remove(d.mesh);
        d.dispose();
        pool.delete(role);
      }
    }
    invalidate();
  }, [geom, hover, selection, color, invalidate]);

  // Dispose all pooled dots when the session closes.
  useEffect(() => {
    const pool = dotPoolRef.current;
    return () => {
      for (const d of pool.values()) d.dispose();
      pool.clear();
    };
  }, []);

  if (!active) return null;
  return (
    <group>
      {objects.objs.map((o, i) => (
        <primitive key={i} object={o} dispose={null} />
      ))}
      {/* Persistent dots (ticket vwzj) — the pool mutates this group's
          children imperatively; never recreated per geom rebuild. */}
      <group ref={dotsGroupRef} />
    </group>
  );
}

const SKETCH_PREVIEW_COLOR = 0x93c5fd; // COLOR_PREVIEW
const SKETCH_SNAP_COLOR = 0xf59e0b; // COLOR_SNAP

/**
 * Transient sketch HUD GEOMETRY (drag/rubber-band previews, hover point,
 * snap ring, projected B-Rep edge guides) as real 3D scene geometry,
 * rebuilt imperatively from `sketchHudBridge` whenever its version changes.
 * Published by `SketchCanvas2d`; the overlay itself draws only the text
 * labels + marquee now. Lives apart from `SketchSession3D` so the per-pointer
 * churn here never re-runs the committed-geometry memos.
 */
export function SketchHud3D() {
  const gl = useThree((s) => s.gl);
  const invalidate = useThree((s) => s.invalidate);
  // ticket x941: on the classic WebGLRenderer fallback the node materials below
  // don't compile AND `useFrame` never ticks (the WebGL root runs
  // `frameloop="never"`), so the live drag preview never showed there. Mirror
  // SketchSession3D: classic Line(Dashed)Material on WebGL, and rebuild from a
  // direct HUD subscription instead of polling the version in useFrame.
  const isWebGPU =
    (gl as unknown as { isWebGPURenderer?: boolean }).isWebGPURenderer === true;
  const groupRef = useRef<THREE.Group>(null);
  const lastVer = useRef(-1);
  const objsRef = useRef<THREE.Object3D[]>([]);
  const dotSetsRef = useRef<SketchDots[]>([]);

  type LineMat =
    | Line2NodeMaterial
    | THREE.LineBasicMaterial
    | THREE.LineDashedMaterial;
  const matsRef = useRef<{
    line: LineMat;
    guide: LineMat;
    projected: LineMat;
  } | null>(null);
  if (!matsRef.current) {
    if (isWebGPU) {
      const fatLine = (
        c: number,
        lw: number,
        extra: Record<string, unknown> = {},
      ) =>
        new Line2NodeMaterial({
          color: c,
          linewidth: lw,
          worldUnits: false,
          depthTest: false,
          depthWrite: false,
          transparent: true,
          ...extra,
        });
      matsRef.current = {
        line: fatLine(SKETCH_PREVIEW_COLOR, 2),
        guide: fatLine(SKETCH_GUIDE_COLOR, 1, {
          opacity: 0.6,
          dashed: true,
          dashSize: 3,
          gapSize: 2,
        }),
        projected: fatLine(SKETCH_GUIDE_COLOR, 1.5, {
          opacity: 0.5,
          dashed: true,
          dashSize: 4,
          gapSize: 4,
        }),
      };
    } else {
      matsRef.current = {
        line: new THREE.LineBasicMaterial({
          color: SKETCH_PREVIEW_COLOR,
          transparent: true,
          depthTest: false,
          depthWrite: false,
        }),
        guide: new THREE.LineDashedMaterial({
          color: SKETCH_GUIDE_COLOR,
          transparent: true,
          opacity: 0.6,
          depthTest: false,
          depthWrite: false,
          dashSize: 3,
          gapSize: 2,
        }),
        projected: new THREE.LineDashedMaterial({
          color: SKETCH_GUIDE_COLOR,
          transparent: true,
          opacity: 0.5,
          depthTest: false,
          depthWrite: false,
          dashSize: 4,
          gapSize: 4,
        }),
      };
    }
  }
  const M = matsRef.current;

  // Rebuild the transient HUD geometry from the bridge. Held in a ref so the
  // single subscription (below) always calls the freshest closure without
  // re-subscribing every render.
  const rebuildRef = useRef<() => void>(() => {});
  rebuildRef.current = () => {
    const hud = readSketchHud();
    if (hud.version === lastVer.current) return;
    lastVer.current = hud.version;
    const group = groupRef.current;
    if (!group) return;
    for (const o of objsRef.current) {
      group.remove(o);
      (o as { geometry?: THREE.BufferGeometry }).geometry?.dispose();
    }
    objsRef.current = [];
    for (const d of dotSetsRef.current) {
      group.remove(d.mesh);
      d.dispose();
    }
    dotSetsRef.current = [];

    const frame = getSketchPlaneFrame();
    const w = (uv: [number, number]) => planeToWorld(uv[0], uv[1], frame);
    const addLines = (
      pos: number[],
      mat: LineMat,
      order: number,
      dashed: boolean,
    ) => {
      if (pos.length === 0) return;
      let o: THREE.Object3D;
      if (isWebGPU) {
        const g = new LineSegmentsGeometry();
        g.setPositions(new Float32Array(pos));
        const line = new LineSegments2(g, mat as Line2NodeMaterial);
        if (dashed) line.computeLineDistances();
        o = line;
      } else {
        const g = new THREE.BufferGeometry();
        g.setAttribute("position", new THREE.Float32BufferAttribute(pos, 3));
        const line = new THREE.LineSegments(g, mat as THREE.LineBasicMaterial);
        if (dashed) line.computeLineDistances();
        o = line;
      }
      o.frustumCulled = false;
      o.renderOrder = order;
      o.layers.enable(OVERLAY_LAYER);
      group.add(o);
      objsRef.current.push(o);
    };
    const addDots = (
      pos: number[],
      colorHex: number,
      diameterPx: number,
      order: number,
    ) => {
      const d = makeSketchDots(pos, colorHex, diameterPx, order);
      if (!d) return;
      group.add(d.mesh);
      dotSetsRef.current.push(d);
    };

    // Projected B-Rep edge guides (dashed, behind everything).
    const proj: number[] = [];
    for (const [a, b] of hud.projected) proj.push(...w(a), ...w(b));
    addLines(proj, M.projected, 993, true);

    // Drag preview shapes (via the shared flattener) + extra segments.
    const prev = collectSketchSessionGeometry(
      hud.previewCalls as SketchSubCall[],
      frame,
    );
    const lines = [...prev.solid, ...prev.legs];
    for (const [a, b] of hud.previewLines) lines.push(...w(a), ...w(b));
    addLines(lines, M.line, 996, false);
    const guides: number[] = [];
    for (const [a, b] of hud.previewGuides) guides.push(...w(a), ...w(b));
    addLines(guides, M.guide, 996, true);

    // Preview marker points + hover (preview colour, 7px).
    const pts = [...prev.points];
    for (const p of hud.previewPoints) pts.push(...w(p));
    if (hud.hover) pts.push(...w(hud.hover));
    addDots(pts, SKETCH_PREVIEW_COLOR, 7, 997);

    // Snap ring (on top, snap colour, 13px).
    if (hud.snap) addDots([...w(hud.snap)], SKETCH_SNAP_COLOR, 13, 1001);

    invalidate();
  };

  useEffect(() => {
    setHudInvalidate(invalidate);
    // Rebuild on every publish (works without useFrame → drives the WebGL
    // fallback root too, ticket x941).
    const unsub = subscribeSketchHud(() => rebuildRef.current());
    rebuildRef.current(); // initial paint
    return () => {
      setHudInvalidate(null);
      unsub();
      clearSketchHud();
      for (const d of dotSetsRef.current) d.dispose();
      dotSetsRef.current = [];
      for (const m of Object.values(M)) m.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [invalidate]);

  return <group ref={groupRef} />;
}

// Mirror SketchCanvas2d's overlay text tokens so the 3D labels read
// identically to the old canvas readouts.
const LABEL_FONT = "600 14px system-ui, -apple-system, sans-serif";
const LABEL_BG = "rgba(246, 246, 244, 0.92)";
const CONSTRAINT_FONT = "bold 11px sans-serif";
const LABEL_FONT_PX = 14;
const CONSTRAINT_FONT_PX = 11;
// Supersample the label canvas so the billboard texture stays crisp on
// hi-dpi displays (the sprite is drawn at a constant CSS-pixel size).
const LABEL_SUPERSAMPLE = 2;

let labelMeasureCtx: CanvasRenderingContext2D | null = null;
function measureLabelWidth(text: string, font: string): number {
  if (!labelMeasureCtx) {
    labelMeasureCtx = document.createElement("canvas").getContext("2d");
  }
  if (!labelMeasureCtx) return text.length * 8;
  labelMeasureCtx.font = font;
  return labelMeasureCtx.measureText(text).width;
}

function roundRectPath(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
): void {
  const rr = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + rr, y);
  ctx.arcTo(x + w, y, x + w, y + h, rr);
  ctx.arcTo(x + w, y + h, x, y + h, rr);
  ctx.arcTo(x, y + h, x, y, rr);
  ctx.arcTo(x, y, x + w, y, rr);
  ctx.closePath();
}

/**
 * Rasterise one label into an offscreen canvas. The canvas holds the world
 * anchor at its CENTRE plus the text box placed `offsetPx` away (+x right,
 * +y up), so a centred sprite pins the anchor to the geometry while the
 * screen nudge is baked into the texture (constant pixels, never zoom-scaled).
 * Returns the canvas and its CSS-pixel size (sprite scale source).
 */
function renderLabelCanvas(label: SketchLabel): {
  canvas: HTMLCanvasElement;
  cssW: number;
  cssH: number;
} {
  const ss = LABEL_SUPERSAMPLE;
  const font = label.style === "constraint" ? CONSTRAINT_FONT : LABEL_FONT;
  const fontPx =
    label.style === "constraint" ? CONSTRAINT_FONT_PX : LABEL_FONT_PX;
  const textW = measureLabelWidth(label.text, font);

  let boxW: number;
  let boxH: number;
  let pillR = 0;
  if (label.style === "snap") {
    boxW = textW + 14;
    boxH = fontPx + 12;
    pillR = 5;
  } else if (label.style === "constraint") {
    boxW = textW + 6;
    boxH = fontPx + 6;
  } else {
    // dimension: extra room for the legibility halo stroke.
    boxW = textW + 8;
    boxH = fontPx + 8;
  }

  const [dx, dy] = label.offsetPx;
  const cssW = boxW + 2 * Math.abs(dx);
  const cssH = boxH + 2 * Math.abs(dy);
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.ceil(cssW * ss));
  canvas.height = Math.max(1, Math.ceil(cssH * ss));
  const ctx = canvas.getContext("2d");
  if (!ctx) return { canvas, cssW, cssH };
  ctx.scale(ss, ss);

  // Box centre: dx right, dy up (canvas Y points down → subtract dy).
  const bcx = cssW / 2 + dx;
  const bcy = cssH / 2 - dy;
  ctx.font = font;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";

  if (label.style === "snap") {
    const x = bcx - boxW / 2;
    const y = bcy - boxH / 2;
    ctx.fillStyle = LABEL_BG;
    ctx.strokeStyle = label.color;
    ctx.lineWidth = 1;
    roundRectPath(ctx, x + 0.5, y + 0.5, boxW - 1, boxH - 1, pillR);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = label.color;
    ctx.fillText(label.text, bcx, bcy);
  } else if (label.style === "dimension") {
    // Thin halo so the number stays legible over the grid (no box).
    ctx.strokeStyle = LABEL_BG;
    ctx.lineWidth = 3;
    ctx.strokeText(label.text, bcx, bcy);
    ctx.fillStyle = label.color;
    ctx.fillText(label.text, bcx, bcy);
  } else {
    ctx.fillStyle = label.color;
    ctx.fillText(label.text, bcx, bcy);
  }
  return { canvas, cssW, cssH };
}

interface LabelSprite {
  sprite: THREE.Sprite;
  tex: THREE.CanvasTexture;
  // SpriteNodeMaterial on WebGPU, classic THREE.SpriteMaterial on the WebGL
  // fallback (ticket n6rs: the node material crashes in resolveIncludes there).
  mat: SpriteNodeMaterial | THREE.SpriteMaterial;
  anchorUv: [number, number];
  cssW: number;
  cssH: number;
}

/**
 * Sketch TEXT LABELS (dimension readouts, constraint glyphs, the snap label)
 * as camera-facing billboard sprites, rebuilt from `sketchLabelBridge` on a
 * version change. Each label is rasterised into a small canvas texture and
 * drawn at a CONSTANT pixel size (`sizeAttenuation: false`) so it reads the
 * same at any zoom while staying pinned to its sketch-plane anchor — no more
 * lagging behind the model on a camera move (the old 2D-overlay text bug).
 * The marquee stays on the overlay: it's a pure screen-space selection box,
 * not anchored to the sketch.
 */
export function SketchLabels3D() {
  const invalidate = useThree((s) => s.invalidate);
  const camera = useThree((s) => s.camera);
  const size = useThree((s) => s.size);
  const gl = useThree((s) => s.gl);
  // SpriteNodeMaterial renders only on WebGPU; the classic WebGLRenderer needs
  // a plain THREE.SpriteMaterial (ticket n6rs — resolveIncludes crash while a
  // sketch dimension/snap label is on screen during a line draw).
  const isWebGPU = isWebGPURenderer(gl);
  const groupRef = useRef<THREE.Group>(null);
  const lastVer = useRef(-1);
  const itemsRef = useRef<LabelSprite[]>([]);

  useEffect(() => {
    setLabelInvalidate(invalidate);
    return () => {
      setLabelInvalidate(null);
      clearSketchLabels();
    };
  }, [invalidate]);

  // Dispose on unmount (session closed).
  useEffect(() => {
    const items = itemsRef.current;
    const group = groupRef.current;
    return () => {
      for (const it of items) {
        group?.remove(it.sprite);
        it.tex.dispose();
        it.mat.dispose();
      }
    };
  }, []);

  useFrame(() => {
    const set = readSketchLabels();
    const group = groupRef.current;
    if (!group) return;

    // Rebuild sprite set only when the labels actually changed.
    if (set.version !== lastVer.current) {
      lastVer.current = set.version;
      for (const it of itemsRef.current) {
        group.remove(it.sprite);
        it.tex.dispose();
        it.mat.dispose();
      }
      itemsRef.current = [];
      for (const label of set.labels) {
        const { canvas, cssW, cssH } = renderLabelCanvas(label);
        const tex = new THREE.CanvasTexture(canvas);
        tex.colorSpace = THREE.SRGBColorSpace;
        tex.minFilter = THREE.LinearFilter;
        tex.generateMipmaps = false;
        const mat = isWebGPU
          ? new SpriteNodeMaterial()
          : new THREE.SpriteMaterial();
        mat.map = tex;
        mat.transparent = true;
        mat.depthTest = false;
        mat.depthWrite = false;
        mat.sizeAttenuation = false; // constant pixel size, depth-independent
        const sprite = new THREE.Sprite(mat);
        sprite.frustumCulled = false;
        sprite.renderOrder = 1002; // above the snap ring (1001)
        sprite.layers.enable(OVERLAY_LAYER);
        group.add(sprite);
        itemsRef.current.push({
          sprite,
          tex,
          mat,
          anchorUv: label.anchorUv,
          cssW,
          cssH,
        });
      }
      invalidate();
    }

    if (itemsRef.current.length === 0) return;
    // Constant pixel size with `sizeAttenuation: false`: a perspective
    // camera's NDC offset per geometry unit equals the sprite's object
    // scale, so 1 CSS px maps to `2·tan(fov/2)/viewportHeightPx` world-scale
    // units. (Runs every rendered frame so a camera move keeps the labels
    // sharp; cheap — only a handful of sprites.)
    const persp = camera as THREE.PerspectiveCamera;
    const k = persp.isPerspectiveCamera
      ? (2 * Math.tan(persp.fov * 0.5 * THREE.MathUtils.DEG2RAD)) / size.height
      : 1 / size.height;
    const frame = getSketchPlaneFrame();
    for (const it of itemsRef.current) {
      const w = planeToWorld(it.anchorUv[0], it.anchorUv[1], frame);
      it.sprite.position.set(w[0], w[1], w[2]);
      it.sprite.scale.set(it.cssW * k, it.cssH * k, 1);
    }
  });

  return <group ref={groupRef} />;
}
