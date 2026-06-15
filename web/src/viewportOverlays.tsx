import { useEffect, useMemo, useRef, useState } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import { TransformControls as ThreeTransformControls } from "three/examples/jsm/controls/TransformControls.js";
import { LineSegments2 } from "three/examples/jsm/lines/webgpu/LineSegments2.js";
import { LineSegmentsGeometry } from "three/examples/jsm/lines/LineSegmentsGeometry.js";
import { Line2NodeMaterial, PointsNodeMaterial, MeshBasicNodeMaterial, SpriteNodeMaterial } from "three/webgpu";
import * as THREE from "three";
import { useKernelStore, type StoreState } from "./store";
import {
  FACE_INDEX_SENTINEL,
  localToWorld,
  type BRepEdge,
  type OctreeCells,
  type BRepFace,
  type BRepVertex,
  type FeatureSummary,
  type HybridLeafInfo,
  type Placement,
  type PickResult,
} from "./kernel";
import type { BRepEdgeMode } from "./SettingsMenu";
import { PerfProbe } from "./perfMonitor";
import { OVERLAY_LAYER } from "./viewportSdfOverlays";
import { resolveMaterials, type MaterialProps } from "./materialBridge";
import {
  BODY_COLORS,
  applyLeafTransform,
  createPrimitiveGeometry,
  buildWarpedLeafGeometry,
} from "./sdfDeferredRenderer";
import { triangulatePlanarFace } from "./faceFill";
import { collectAnalyticBodyMeshes, type BrepMeshKernel } from "./brepMesh";
import { resolvePlaneFrame } from "./sketchPlaneFrame";
// Extracted to sketchOutline.ts (DOM-free) — shared with the extrude
// gizmo's pending-sketch-extrude anchor.
import {
  collectSketchLineSegments,
  collectSketchSessionGeometry,
  collectSketchRegionFill,
  type SketchSubCall,
} from "./sketchOutline";
import { buildDraftSketchState } from "./sketchSession";
import type { SketchState } from "./solver/types";
import { displayDegreesOfFreedom } from "./solver/dofPreflight";
import { gripsForSelection } from "./sketchGrips";
import { planeToWorld, type PlaneFrame } from "./sketchPlaneFrame";
import { getSketchPlaneFrame } from "./sketchCameraBridge";
import {
  readSketchHud,
  setHudInvalidate,
  clearSketchHud,
} from "./sketchHudBridge";
import {
  readSketchLabels,
  setLabelInvalidate,
  clearSketchLabels,
  type SketchLabel,
} from "./sketchLabelBridge";
import { getHoveredElementOfKind, setHoveredElementOfKind } from "./brepPickBridge";
import { filterAllows } from "./elementQuery";
import { getActive as getActiveQueryPicker } from "./queryPickerBridge";

/** Feature kinds that are themselves a transform of another feature — the
 *  gizmo edits THEIR params in place instead of appending a new transform
 *  call onto a geometry feature (ticket p3vh). */
const TRANSFORM_FEATURE_KINDS = new Set(["translate", "rotate", "scale"]);

export function findSingleSelectedFeature(state: StoreState | null | undefined): FeatureSummary | undefined {
  if (!state || state.selection.size !== 1) return undefined;
  const first = state.selection.values().next().value;
  if (first === undefined) return undefined;
  return state.features.find((f) => f.id === first);
}


export function SketchOverlay3D() {
  const { store, state } = useKernelStore();
  const visibleSketches = state?.visibleSketches;

  const geometries = useMemo(() => {
    if (!store || !visibleSketches || visibleSketches.length === 0) return [];
    const out: {
      id: number;
      geometry: THREE.BufferGeometry;
    }[] = [];
    for (const sketchId of visibleSketches) {
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
        const points = collectSketchLineSegments(params.sub_calls ?? [], frame);
        if (points.length === 0) continue;
        const geometry = new THREE.BufferGeometry();
        geometry.setAttribute(
          "position",
          new THREE.Float32BufferAttribute(points, 3),
        );
        out.push({ id: sketchId, geometry });
      } catch {
        // Skip invalid sketches
      }
    }
    return out;
  }, [store, visibleSketches, state?.features]);

  useEffect(() => {
    return () => {
      for (const g of geometries) g.geometry.dispose();
    };
  }, [geometries]);

  if (geometries.length === 0) return null;

  return (
    <group>
      {geometries.map(({ id, geometry }) => (
        <lineSegments key={id} geometry={geometry}>
          <lineBasicMaterial color="#2563eb" linewidth={2} />
        </lineSegments>
      ))}
    </group>
  );
}

// DOF status colours (mirror SketchCanvas2d's COLOR_UNDER/FULL/OVER): blue =
// underconstrained, green = fully constrained, red = over/conflicting.
const SKETCH_COLOR_UNDER = 0x2563eb;
const SKETCH_COLOR_FULL = 0x16a34a;
const SKETCH_COLOR_OVER = 0xdc2626;
// Fixed (non-DOF) sketch colours, mirroring the canvas tokens in SketchCanvas2d.
const SKETCH_CONSTRUCTION_COLOR = 0xb07820; // COLOR_CONSTRUCTION
const SKETCH_GUIDE_COLOR = 0x64748b; // COLOR_GUIDE (rgba .6)
const SKETCH_LEG_COLOR = 0x7c3aed; // COLOR_TANGENT_LEG
const SKETCH_FILL_COLOR = 0x94a3b8; // COLOR_REGION_FILL (rgba .18)
const SKETCH_AXIS_COLOR = 0x000000; // origin axes (rgba .25)
const SKETCH_SELECTED_COLOR = 0xf97316; // COLOR_SELECTED

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
          const next: [number, number] = [c[0] + r * Math.cos(t), c[1] + r * Math.sin(t)];
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
  const session = state?.sketchSession;
  const active = session?.active ?? false;

  // Resolve the plane frame from the session's own plane query rather than
  // reading the camera bridge — avoids a one-frame race with the overlay's
  // resolve effect on a plane change.
  const frame = useMemo(() => {
    if (!store || !active || !session) return null;
    return (
      resolvePlaneFrame(session.planeQuery ?? "qCreatedBy(origin_plane_xy)", store.kernel) ??
      resolvePlaneFrame("qCreatedBy(origin_plane_xy)", store.kernel)
    );
  }, [store, active, session?.planeQuery]);

  // Solved entity state — shared by the DOF colour and the selection
  // highlight (same id space as session.selection).
  const skState = useMemo(() => {
    if (!active || !session) return null;
    try {
      return buildDraftSketchState(session.subCalls);
    } catch {
      return null;
    }
  }, [active, session?.subCalls]);

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
      return dof > 0 ? SKETCH_COLOR_UNDER : dof === 0 ? SKETCH_COLOR_FULL : SKETCH_COLOR_OVER;
    } catch {
      return SKETCH_COLOR_UNDER;
    }
  }, [active, skState, session?.diagnostics]);

  const geom = useMemo(() => {
    if (!active || !frame || !session) return null;
    return collectSketchSessionGeometry(session.subCalls as SketchSubCall[], frame);
  }, [active, frame, session?.subCalls]);

  const selection = useMemo(() => {
    if (!active || !frame || !session || !skState || session.selection.length === 0) {
      return null;
    }
    const g = collectSelectionGeometry(skState, session.selection, session.subCalls, frame);
    return g.highlight.length || g.markers.length ? g : null;
  }, [active, frame, skState, session?.selection, session?.subCalls]);

  const fill = useMemo(() => {
    if (!active || !frame || !session) return null;
    const tri = (poly: [number, number][]): number[][] => {
      const contour = poly.map((q) => new THREE.Vector2(q[0], q[1]));
      return THREE.ShapeUtils.triangulateShape(contour, []);
    };
    const pos = collectSketchRegionFill(session.subCalls as SketchSubCall[], frame, tri);
    return pos.length ? pos : null;
  }, [active, frame, session?.subCalls]);

  const axes = useMemo(() => {
    if (!active || !frame) return null;
    const L = 1e5; // long enough to cross the viewport at any usable zoom
    return [
      ...planeToWorld(-L, 0, frame), ...planeToWorld(L, 0, frame),
      ...planeToWorld(0, -L, frame), ...planeToWorld(0, L, frame),
    ];
  }, [active, frame]);

  // Materials persist across rebuilds (only the DOF-driven colours mutate);
  // the WebGPU backend handles a material colour change fine but never a
  // geometry mutation, so the objects are rebuilt fresh on every change.
  const matsRef = useRef<{
    solid: Line2NodeMaterial;
    construction: Line2NodeMaterial;
    guide: Line2NodeMaterial;
    leg: Line2NodeMaterial;
    axis: Line2NodeMaterial;
    point: PointsNodeMaterial;
    handle: PointsNodeMaterial;
    selLine: Line2NodeMaterial;
    selPoint: PointsNodeMaterial;
    fill: MeshBasicNodeMaterial;
  } | null>(null);
  if (!matsRef.current) {
    const fatLine = (c: number, lw: number, extra: Record<string, unknown> = {}) =>
      new Line2NodeMaterial({
        color: c, linewidth: lw, worldUnits: false,
        depthTest: false, depthWrite: false, transparent: true, ...extra,
      });
    const dots = (c: number, size: number) =>
      new PointsNodeMaterial({
        color: c, size, sizeAttenuation: false,
        depthTest: false, depthWrite: false, transparent: true,
      });
    matsRef.current = {
      solid: fatLine(color, 2),
      construction: fatLine(SKETCH_CONSTRUCTION_COLOR, 2, { dashed: true, dashSize: 3, gapSize: 2 }),
      guide: fatLine(SKETCH_GUIDE_COLOR, 1, { opacity: 0.6, dashed: true, dashSize: 3, gapSize: 2 }),
      leg: fatLine(SKETCH_LEG_COLOR, 2),
      axis: fatLine(SKETCH_AXIS_COLOR, 1, { opacity: 0.22 }),
      point: dots(color, 7), // overlay POINT_PX radius 3 → ~7px dot
      handle: dots(color, 6),
      selLine: fatLine(SKETCH_SELECTED_COLOR, 4),
      selPoint: dots(SKETCH_SELECTED_COLOR, 10),
      fill: new MeshBasicNodeMaterial({
        color: SKETCH_FILL_COLOR, transparent: true, opacity: 0.18,
        depthTest: false, depthWrite: false, side: THREE.DoubleSide,
      }),
    };
  }
  const M = matsRef.current;
  useEffect(() => {
    M.solid.color.setHex(color);
    M.point.color.setHex(color);
    M.handle.color.setHex(color);
    invalidate();
  }, [color, invalidate, M]);
  // Dispose materials when the component unmounts (session closed).
  useEffect(() => () => { for (const m of Object.values(M)) m.dispose(); }, [M]);

  const objects = useMemo(() => {
    const objs: THREE.Object3D[] = [];
    const addLines = (
      pos: number[] | null,
      mat: Line2NodeMaterial,
      order: number,
      dashed: boolean,
    ) => {
      if (!pos || pos.length === 0) return;
      const g = new LineSegmentsGeometry();
      g.setPositions(new Float32Array(pos));
      const o = new LineSegments2(g, mat);
      if (dashed) o.computeLineDistances();
      o.frustumCulled = false;
      o.renderOrder = order;
      o.layers.enable(OVERLAY_LAYER);
      objs.push(o);
    };
    const addPoints = (pos: number[] | null, mat: PointsNodeMaterial, order: number) => {
      if (!pos || pos.length === 0) return;
      const g = new THREE.BufferGeometry();
      g.setAttribute("position", new THREE.Float32BufferAttribute(pos, 3));
      const o = new THREE.Points(g, mat);
      o.frustumCulled = false;
      o.renderOrder = order;
      o.layers.enable(OVERLAY_LAYER);
      objs.push(o);
    };
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
      addPoints(geom.points, M.point, 998);
      addPoints(geom.handles, M.handle, 998);
    }
    if (selection) {
      addLines(selection.highlight, M.selLine, 999, false);
      addPoints(selection.markers, M.selPoint, 1000);
    }
    return objs;
  }, [geom, fill, axes, selection, M]);

  useEffect(() => {
    invalidate();
    return () => {
      for (const o of objects) (o as { geometry?: THREE.BufferGeometry }).geometry?.dispose();
    };
  }, [objects, invalidate]);

  if (!active) return null;
  return (
    <group>
      {objects.map((o, i) => (
        <primitive key={i} object={o} dispose={null} />
      ))}
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
  const invalidate = useThree((s) => s.invalidate);
  const groupRef = useRef<THREE.Group>(null);
  const lastVer = useRef(-1);
  const objsRef = useRef<THREE.Object3D[]>([]);

  const matsRef = useRef<{
    line: Line2NodeMaterial;
    guide: Line2NodeMaterial;
    projected: Line2NodeMaterial;
    point: PointsNodeMaterial;
    snap: PointsNodeMaterial;
  } | null>(null);
  if (!matsRef.current) {
    const fatLine = (c: number, lw: number, extra: Record<string, unknown> = {}) =>
      new Line2NodeMaterial({
        color: c, linewidth: lw, worldUnits: false,
        depthTest: false, depthWrite: false, transparent: true, ...extra,
      });
    matsRef.current = {
      line: fatLine(SKETCH_PREVIEW_COLOR, 2),
      guide: fatLine(SKETCH_GUIDE_COLOR, 1, { opacity: 0.6, dashed: true, dashSize: 3, gapSize: 2 }),
      projected: fatLine(SKETCH_GUIDE_COLOR, 1.5, { opacity: 0.5, dashed: true, dashSize: 4, gapSize: 4 }),
      point: new PointsNodeMaterial({
        color: SKETCH_PREVIEW_COLOR, size: 7, sizeAttenuation: false,
        depthTest: false, depthWrite: false, transparent: true,
      }),
      snap: new PointsNodeMaterial({
        color: SKETCH_SNAP_COLOR, size: 13, sizeAttenuation: false,
        depthTest: false, depthWrite: false, transparent: true,
      }),
    };
  }
  const M = matsRef.current;

  useEffect(() => {
    setHudInvalidate(invalidate);
    return () => {
      setHudInvalidate(null);
      clearSketchHud();
    };
  }, [invalidate]);

  useFrame(() => {
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

    const frame = getSketchPlaneFrame();
    const w = (uv: [number, number]) => planeToWorld(uv[0], uv[1], frame);
    const addLines = (pos: number[], mat: Line2NodeMaterial, order: number, dashed: boolean) => {
      if (pos.length === 0) return;
      const g = new LineSegmentsGeometry();
      g.setPositions(new Float32Array(pos));
      const o = new LineSegments2(g, mat);
      if (dashed) o.computeLineDistances();
      o.frustumCulled = false;
      o.renderOrder = order;
      o.layers.enable(OVERLAY_LAYER);
      group.add(o);
      objsRef.current.push(o);
    };
    const addPoints = (pos: number[], mat: PointsNodeMaterial, order: number) => {
      if (pos.length === 0) return;
      const g = new THREE.BufferGeometry();
      g.setAttribute("position", new THREE.Float32BufferAttribute(pos, 3));
      const o = new THREE.Points(g, mat);
      o.frustumCulled = false;
      o.renderOrder = order;
      o.layers.enable(OVERLAY_LAYER);
      group.add(o);
      objsRef.current.push(o);
    };

    // Projected B-Rep edge guides (dashed, behind everything).
    const proj: number[] = [];
    for (const [a, b] of hud.projected) proj.push(...w(a), ...w(b));
    addLines(proj, M.projected, 993, true);

    // Drag preview shapes (via the shared flattener) + extra segments.
    const prev = collectSketchSessionGeometry(hud.previewCalls as SketchSubCall[], frame);
    const lines = [...prev.solid, ...prev.legs];
    for (const [a, b] of hud.previewLines) lines.push(...w(a), ...w(b));
    addLines(lines, M.line, 996, false);
    const guides: number[] = [];
    for (const [a, b] of hud.previewGuides) guides.push(...w(a), ...w(b));
    addLines(guides, M.guide, 996, true);

    // Preview marker points + hover.
    const pts = [...prev.points];
    for (const p of hud.previewPoints) pts.push(...w(p));
    if (hud.hover) pts.push(...w(hud.hover));
    addPoints(pts, M.point, 997);

    // Snap ring (on top).
    if (hud.snap) addPoints([...w(hud.snap)], M.snap, 1001);

    invalidate();
  });

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
  x: number, y: number, w: number, h: number, r: number,
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
  const fontPx = label.style === "constraint" ? CONSTRAINT_FONT_PX : LABEL_FONT_PX;
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
  mat: SpriteNodeMaterial;
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
        const mat = new SpriteNodeMaterial();
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
        itemsRef.current.push({ sprite, tex, mat, anchorUv: label.anchorUv, cssW, cssH });
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

const NO_GIZMO_KINDS: ReadonlySet<string> = new Set([
  // extrude: owns its depth-arrow gizmo (ExtrudeDepthGizmo, ticket qqgo)
  "boolean", "sketch", "fillet_edge", "extrude",
]);

export function SelectionTransformGizmo() {
  const { store, state } = useKernelStore();
  const meshRef = useRef<THREE.Mesh>(null);
  const tcRef = useRef<ThreeTransformControls | null>(null);
  const syncedTranslateRef = useRef<[number, number, number] | null>(null);
  const gizmoThrottleRef = useRef<ReturnType<typeof setTimeout>>(undefined);
  const gizmoPendingRef = useRef(false);

  const camera = useThree((s) => s.camera);
  const gl = useThree((s) => s.gl);
  const scene = useThree((s) => s.scene);
  const defaultControls = useThree((s) => s.controls);
  const invalidate = useThree((s) => s.invalidate);

  const selectedFeature = useMemo(() => findSingleSelectedFeature(state), [state]);

  const gizmoMode = state?.gizmoMode ?? "translate";

  useEffect(() => {
    const mesh = meshRef.current;
    if (!mesh || !selectedFeature) {
      syncedTranslateRef.current = null;
      return;
    }
    const [tx, ty, tz] = selectedFeature.translate;
    const prev = syncedTranslateRef.current;
    if (!prev || prev[0] !== tx || prev[1] !== ty || prev[2] !== tz) {
      mesh.position.set(tx, ty, tz);
      syncedTranslateRef.current = [tx, ty, tz];
    }
    const [rx, ry, rz, rw] = selectedFeature.rotation;
    mesh.quaternion.set(rx, ry, rz, rw);
    const [scx, scy, scz] = selectedFeature.scale;
    mesh.scale.set(scx, scy, scz);
  }, [selectedFeature]);

  const commitRef = useRef<() => void>(() => {});
  commitRef.current = () => {
    const mesh = meshRef.current;
    if (!mesh || !store || !selectedFeature) return;
    gizmoPendingRef.current = false;
    const p = mesh.position;
    const q = mesh.quaternion;
    let sx = mesh.scale.x, sy = mesh.scale.y, sz = mesh.scale.z;
    if (gizmoMode === "translate") {
      syncedTranslateRef.current = [p.x, p.y, p.z];
    }
    if (gizmoMode === "scale" && selectedFeature.kind === "sphere") {
      const u = Math.max(Math.abs(sx), Math.abs(sy), Math.abs(sz));
      sx = u; sy = u; sz = u;
      mesh.scale.set(u, u, u);
    }
    // A selected TRANSFORM feature (Translate/Rotate/Scale row in the
    // outliner) edits ITS OWN params: `applyGizmo*` targets a geometry
    // feature and appends/merges a transform onto it, but a transform
    // feature never appears in the kernel's decoded spine, so the call
    // was a silent no-op (ticket p3vh — dragging the sphere's translate
    // gizmo moved nothing). Only the matching mode commits; dragging a
    // rotate handle on a Translate feature has no parameter to write.
    if (TRANSFORM_FEATURE_KINDS.has(selectedFeature.kind)) {
      let current: Record<string, unknown>;
      try {
        current = store.kernel.getFeatureCallParams(selectedFeature.id);
      } catch {
        return;
      }
      if (gizmoMode === "translate" && selectedFeature.kind === "translate") {
        store.updateFeatureParams(selectedFeature.id, {
          ...current,
          translate: [p.x, p.y, p.z],
        });
      } else if (gizmoMode === "rotate" && selectedFeature.kind === "rotate") {
        const e = new THREE.Euler().setFromQuaternion(q, "XYZ");
        store.updateFeatureParams(selectedFeature.id, {
          ...current,
          rotation: [e.x, e.y, e.z],
        });
      } else if (gizmoMode === "scale" && selectedFeature.kind === "scale") {
        store.updateFeatureParams(selectedFeature.id, {
          ...current,
          scale: [sx, sy, sz],
        });
      }
      return;
    }
    if (gizmoMode === "translate") {
      store.applyGizmoTranslate(selectedFeature.id, p.x, p.y, p.z);
    } else if (gizmoMode === "rotate") {
      store.applyGizmoRotate(selectedFeature.id, q.x, q.y, q.z, q.w);
    } else {
      store.applyGizmoScale(selectedFeature.id, sx, sy, sz);
    }
  };

  useEffect(() => {
    const mesh = meshRef.current;
    if (!mesh || !selectedFeature || !store) {
      if (tcRef.current) {
        tcRef.current.detach();
        scene.remove(tcRef.current.getHelper() as THREE.Object3D);
        tcRef.current.dispose();
        tcRef.current = null;
      }
      return;
    }

    const tc = new ThreeTransformControls(camera, gl.domElement);
    const helper = tc.getHelper() as THREE.Object3D;
    tc.attach(mesh);
    tc.setMode(gizmoMode);
    tc.setSize(0.5);
    helper.traverse((child: THREE.Object3D) => child.layers.enable(OVERLAY_LAYER));

    const onDragging = (e: { value: boolean }) => {
      if (defaultControls) (defaultControls as any).enabled = !e.value;
      // "B-Rep rebuild while dragging" (default off): suppress the
      // per-commit weave for the whole drag, catch up once on release.
      if (e.value) store.beginGizmoDrag();
      else store.endGizmoDrag();
    };
    const onChange = () => invalidate();
    const onObjectChange = () => {
      gizmoPendingRef.current = true;
      if (gizmoThrottleRef.current !== undefined) return;
      commitRef.current();
      gizmoThrottleRef.current = setTimeout(() => {
        gizmoThrottleRef.current = undefined;
        if (gizmoPendingRef.current) commitRef.current();
      }, 50);
    };

    tc.addEventListener("dragging-changed", onDragging as any);
    tc.addEventListener("change", onChange);
    tc.addEventListener("objectChange", onObjectChange);
    scene.add(helper);
    tcRef.current = tc;
    invalidate();

    return () => {
      clearTimeout(gizmoThrottleRef.current);
      gizmoThrottleRef.current = undefined;
      tc.removeEventListener("dragging-changed", onDragging as any);
      tc.removeEventListener("change", onChange);
      tc.removeEventListener("objectChange", onObjectChange);
      tc.detach();
      scene.remove(helper);
      tc.dispose();
      tcRef.current = null;
      // Unmounting mid-drag (feature deselected/deleted) must lift an
      // active B-Rep suppression; a no-op otherwise (idempotent).
      store.endGizmoDrag();
    };
  }, [selectedFeature?.id, gizmoMode, camera, gl, scene, defaultControls, invalidate, store]);

  useEffect(() => () => clearTimeout(gizmoThrottleRef.current), []);

  if (!selectedFeature || !store || NO_GIZMO_KINDS.has(selectedFeature.kind)) return null;

  return (
    <mesh ref={meshRef} visible={false}>
      <boxGeometry args={[1, 1, 1]} />
    </mesh>
  );
}

export function Invalidator({ ndc }: { ndc: { x: number; y: number; active: boolean } }) {
  const invalidate = useThree((s) => s.invalidate);
  const { state } = useKernelStore();
  const meshedBodies = state?.meshedBodies;
  const sketchActive = state?.sketchSession.active ?? false;
  const sketchSubCalls = state?.sketchSession.subCalls;
  const selection = state?.selection;
  const features = state?.features;
  const visibleSketches = state?.visibleSketches;

  useEffect(() => {
    invalidate();
  }, [
    invalidate,
    meshedBodies,
    sketchActive,
    sketchSubCalls,
    selection,
    features,
    visibleSketches,
    ndc.x,
    ndc.y,
    ndc.active,
  ]);

  return null;
}

/** One analytic B-Rep edge: a world-space line-segment chain plus its
 *  canonical colliding-branch signature, used to match the hovered edge. */
interface AnalyticEdge {
  a: number;
  b: number;
  /** Stable decimal id_hash of this edge, or `undefined` until the kernel
   *  wasm exporting it has been regenerated (defensive — Brocken B-K). */
  idHash?: string;
  /** Flat `LineSegments` positions (pairs of points) for this edge. */
  positions: Float32Array;
}

/** Build the per-edge `LineSegments` positions from a body-relative polyline. */
function polylineToSegments(points: number[]): Float32Array {
  const n = points.length / 3;
  if (n < 2) return new Float32Array(0);

  // Collinear decimation (ticket mcvp): the fat-line shader renders every
  // segment as its own round-capped quad, so a STRAIGHT edge sampled into
  // many sub-segments shows a small notch at every interior joint once the
  // stroke is 4px wide. Greedily extend each output segment as long as all
  // skipped sample points stay within a hair of the chord — a straight edge
  // collapses to ONE segment, curved edges keep their samples (the chord
  // tolerance is far below a pixel at any usable zoom).
  const px = (i: number) => points[i * 3];
  const py = (i: number) => points[i * 3 + 1];
  const pz = (i: number) => points[i * 3 + 2];
  const keep: number[] = [0];
  let anchor = 0;
  for (let cand = 2; cand < n; cand++) {
    const acx = px(cand) - px(anchor);
    const acy = py(cand) - py(anchor);
    const acz = pz(cand) - pz(anchor);
    const acLen2 = acx * acx + acy * acy + acz * acz;
    const tol2 = Math.max(1e-8 * acLen2, 1e-12);
    // O(1) probes instead of re-checking the whole span: the mid sample
    // is where a monotone arc deviates most from the chord, and the
    // sample just before the candidate catches sharp local kinks.
    let straight = acLen2 > 0;
    const probes = [(anchor + cand) >> 1, cand - 1];
    for (const m of probes) {
      if (!straight || m <= anchor || m >= cand) continue;
      const abx = px(m) - px(anchor);
      const aby = py(m) - py(anchor);
      const abz = pz(m) - pz(anchor);
      // squared distance of M from the chord anchor→cand
      const cx = aby * acz - abz * acy;
      const cy = abz * acx - abx * acz;
      const cz = abx * acy - aby * acx;
      straight = (cx * cx + cy * cy + cz * cz) / acLen2 <= tol2;
    }
    if (!straight) {
      anchor = cand - 1;
      keep.push(anchor);
    }
  }
  keep.push(n - 1);

  const seg = new Float32Array((keep.length - 1) * 6);
  for (let s = 0; s < keep.length - 1; s++) {
    const a = keep[s];
    const b = keep[s + 1];
    seg[s * 6] = px(a);
    seg[s * 6 + 1] = py(a);
    seg[s * 6 + 2] = pz(a);
    seg[s * 6 + 3] = px(b);
    seg[s * 6 + 4] = py(b);
    seg[s * 6 + 5] = pz(b);
  }
  return seg;
}

/** NDC pick band for edges — the cursor snaps to an edge within this radius
 *  (in normalised device coords, like {@link VERTEX_PICK_NDC_RADIUS}). A touch
 *  tighter than the vertex radius so a corner still wins over its edges. */
const EDGE_PICK_NDC_RADIUS = 0.022;

/**
 * Draws the analytic B-Rep edges (from the woven, cached kernel B-Rep — *not*
 * the mesh) as a dark wireframe overlay, and highlights the edge the cursor is
 * over. The hovered edge is found by projecting every analytic polyline to the
 * screen and snapping the cursor to the nearest one within
 * {@link EDGE_PICK_NDC_RADIUS} — the same screen-space approach
 * {@link BRepSelectionOverlay} uses for faces/vertices. (The kernel sphere-trace
 * pick almost always reports the face *behind* the thin edge, so it can't drive
 * edge hover on its own.) The matched edge carries the stable `id_hash` that
 * STEP export emits, so the lit edge is exactly the one a `qEdge` query targets.
 *
 * Rendered on both the default layer (mesh render mode) and the overlay layer
 * (composited over the SDF render pass), so edges are visible in every mode.
 */
export function BRepEdgeOverlay({
  ndc,
  mode,
}: {
  ndc: { x: number; y: number; active: boolean };
  mode: BRepEdgeMode;
}) {
  const { store, state } = useKernelStore();
  const camera = useThree((s) => s.camera);
  const invalidate = useThree((s) => s.invalidate);
  // Base edges render as fat lines too (ticket 8f5z: plain LineBasicMaterial
  // rasterises 1px hairlines on WebGPU that read broken/washed-out up close —
  // "Kanten wirken nicht voll"). 2.5px through the instanced screen-space
  // line shader gives the solid Onshape-style wireframe; hover (3px) and
  // selection (4px) stay heavier.
  const baseMaterialRef = useRef(
    new Line2NodeMaterial({
      color: 0x1e293b,
      linewidth: 2.5,
      worldUnits: false,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    }),
  );
  // Edge hover paints in a LIGHTER tint of the element-selection colour
  // (ticket m0yw: same hue as the selected edge, just brighter). It renders
  // through the same fat-line path as the *selected* edge (LineSegments2 +
  // Line2NodeMaterial) because that is the one line path proven to composite
  // over the SDF passes on the WebGPU backend — a plain LineSegments hover
  // overlay never showed up there. 3px wide (ticket bqvz: 1px drowned in
  // the base wireframe); the bolder 4px stays reserved for the
  // selection, so hover and selection share the hue but differ in brightness
  // and weight.
  const hoverMaterialRef = useRef(
    new Line2NodeMaterial({
      color: ELEMENT_HOVER_COLOR,
      linewidth: ELEMENT_HOVER_LINEWIDTH,
      worldUnits: false,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    }),
  );

  // "visible" mode hides edges occluded by faces, so the line materials must
  // depth-test. "xray" (default) and "none" keep depthTest off. Depth comes
  // from the meshes in mesh render mode; the SDF overlays additionally write
  // depth and skip the depth-clear in "visible" mode (see renderOverlayLayer).
  useEffect(() => {
    const depthTest = mode === "visible";
    for (const m of [baseMaterialRef.current, hoverMaterialRef.current]) {
      if (m.depthTest !== depthTest) {
        m.depthTest = depthTest;
        m.needsUpdate = true;
      }
    }
  }, [mode]);

  // Body fingerprint (count + per-body id.version). Bumps on any geometry edit
  // and on a placement move (the body version increments), so the edge cache
  // re-reads exactly when — and only when — the bodies actually change.
  //
  // Deliberately NOT driven by `meshedBodies`: DC meshing is enabled only in
  // mesh render mode (App gates `setDcMeshingEnabled` on it), so in the default
  // SDF modes `meshedBodies` is empty. The analytic B-Rep lives on the
  // main-thread kernel regardless of render mode, so we read it straight from
  // `bodyCount()` — that is what makes the edges appear in SDF modes too.
  const bodyFingerprint = useMemo(() => {
    if (!store) return "";
    const n = store.kernel.bodyCount();
    let fp = String(n);
    for (let i = 0; i < n; i++) fp += `:${store.kernel.bodyId(i)}.${store.kernel.bodyVersion(i)}`;
    return fp;
  }, [store, state]);

  const edges = useMemo<AnalyticEdge[]>(() => {
    if (!store) return [];
    const count = store.kernel.bodyCount();
    const out: AnalyticEdge[] = [];
    for (let i = 0; i < count; i++) {
      let bodyEdges: BRepEdge[];
      try {
        bodyEdges = store.kernel.bodyBrepEdges(i);
      } catch {
        continue;
      }
      for (const e of bodyEdges) {
        const positions = polylineToSegments(e.points);
        if (positions.length > 0) out.push({ a: e.a, b: e.b, idHash: e.idHash, positions });
      }
    }
    return out;
  }, [store, bodyFingerprint]);

  // One fresh LineSegments2 for ALL base edges per geometry change — the
  // WebGPU renderer never re-uploads a LineSegmentsGeometry mutated in place
  // (ticket twxw), so the merged buffer gets a brand-new geometry + object.
  const [baseObj, setBaseObj] = useState<LineSegments2 | null>(null);
  useEffect(() => {
    if (edges.length === 0) {
      setBaseObj(null);
      return;
    }
    let total = 0;
    for (const e of edges) total += e.positions.length;
    const merged = new Float32Array(total);
    let off = 0;
    for (const e of edges) {
      merged.set(e.positions, off);
      off += e.positions.length;
    }
    const geom = new LineSegmentsGeometry();
    geom.setPositions(merged);
    const obj = new LineSegments2(geom, baseMaterialRef.current);
    obj.frustumCulled = false;
    obj.renderOrder = 998;
    obj.layers.enable(OVERLAY_LAYER);
    setBaseObj(obj);
    return () => geom.dispose();
  }, [edges]);

  // Screen-space edge hover. The kernel sphere-trace pick reports the face
  // *behind* the thin edge (1 contributor → "face"), so it can never light up
  // an edge on its own. Instead we project every analytic polyline to NDC each
  // frame and snap the cursor to the nearest segment within
  // EDGE_PICK_NDC_RADIUS — front-most wins on near-ties. Mirrors how
  // BRepSelectionOverlay screen-space-picks vertices.
  const [hoveredIdx, setHoveredIdx] = useState(-1);
  const projA = useRef(new THREE.Vector3());
  const projB = useRef(new THREE.Vector3());

  // Occlusion probe (ticket twvd): a point is hidden when the kernel's
  // analytic raycast from the camera hits a surface clearly in FRONT of it.
  // The slack absorbs sphere-trace tolerance on the faces meeting AT the
  // probed edge — only a genuinely nearer surface rejects a candidate.
  const isOccludedPoint = (w: [number, number, number]): boolean => {
    if (!store) return false;
    const o = camera.position;
    const dx = w[0] - o.x;
    const dy = w[1] - o.y;
    const dz = w[2] - o.z;
    const dist = Math.hypot(dx, dy, dz);
    if (dist < 1e-6) return false;
    const hit = store.kernel.pick(
      [o.x, o.y, o.z],
      [dx / dist, dy / dist, dz / dist],
    );
    return hit.hit && hit.t < dist - Math.max(1.0, dist * 0.02);
  };

  useFrame(() => {
    if (!ndc.active || edges.length === 0) {
      if (hoveredIdx !== -1) {
        setHoveredIdx(-1);
        invalidate();
      }
      return;
    }
    const a = projA.current;
    const b = projB.current;
    // Collect the best segment per edge within the pick band; the winner is
    // resolved afterwards so non-x-ray modes can skip occluded candidates.
    const candidates: {
      ei: number;
      d: number;
      depth: number;
      world: [number, number, number];
    }[] = [];
    for (let ei = 0; ei < edges.length; ei++) {
      const pos = edges[ei].positions;
      let bestD = EDGE_PICK_NDC_RADIUS;
      let bestDepth = Infinity;
      let bestWorld: [number, number, number] | null = null;
      for (let i = 0; i + 5 < pos.length; i += 6) {
        a.set(pos[i], pos[i + 1], pos[i + 2]).project(camera);
        b.set(pos[i + 3], pos[i + 4], pos[i + 5]).project(camera);
        // Skip segments that cross or sit behind the near plane — projecting
        // those wraps the NDC coords and yields bogus distances.
        if (a.z < -1 || a.z > 1 || b.z < -1 || b.z > 1) continue;
        // Closest point on the segment in NDC; `t` doubles as the lerp
        // parameter for the world-space occlusion probe point below (the
        // perspective error over one short tessellation segment is nil).
        const sx = b.x - a.x;
        const sy = b.y - a.y;
        const len2 = sx * sx + sy * sy;
        let t = len2 > 0 ? ((ndc.x - a.x) * sx + (ndc.y - a.y) * sy) / len2 : 0;
        t = Math.max(0, Math.min(1, t));
        const d = Math.hypot(ndc.x - (a.x + t * sx), ndc.y - (a.y + t * sy));
        if (d > EDGE_PICK_NDC_RADIUS) continue;
        const depth = Math.min(a.z, b.z);
        if (d < bestD - 1e-4 || (d <= bestD + 1e-4 && depth < bestDepth)) {
          bestD = d;
          bestDepth = depth;
          bestWorld = [
            pos[i] + t * (pos[i + 3] - pos[i]),
            pos[i + 1] + t * (pos[i + 4] - pos[i + 1]),
            pos[i + 2] + t * (pos[i + 5] - pos[i + 2]),
          ];
        }
      }
      if (bestWorld) candidates.push({ ei, d: bestD, depth: bestDepth, world: bestWorld });
    }
    // Nearest cursor distance wins; near-ties resolve to the front-most edge.
    candidates.sort((p, q) =>
      Math.abs(p.d - q.d) > 1e-4 ? p.d - q.d : p.depth - q.depth,
    );
    let best = -1;
    if (mode === "xray") {
      best = candidates.length > 0 ? candidates[0].ei : -1;
    } else {
      // Outside x-ray mode, edges hidden behind geometry are neither hovered
      // nor selectable (ticket twvd): probe candidates front-to-back until one
      // is actually visible. Capped — rarely more than a couple under the
      // cursor — so the per-frame raycast cost stays bounded.
      const limit = Math.min(candidates.length, 8);
      for (let c = 0; c < limit; c++) {
        if (!isOccludedPoint(candidates[c].world)) {
          best = candidates[c].ei;
          break;
        }
      }
    }
    if (best !== hoveredIdx) {
      setHoveredIdx(best);
      invalidate();
    }
  });

  // Publish the hovered edge's stable id_hash to the pick bridge so a click
  // in query-picker mode can emit `qEdge(<idHash>)`. A missing idHash (kernel
  // wasm not yet regenerated) clears it — the edge isn't selectable yet.
  useEffect(() => {
    const e = hoveredIdx >= 0 ? edges[hoveredIdx] : undefined;
    if (e?.idHash !== undefined) {
      setHoveredElementOfKind("edge", { kind: "edge", idHash: e.idHash });
      return () => setHoveredElementOfKind("edge", null);
    }
    return undefined;
  }, [hoveredIdx, edges]);

  // The hovered edge gets a BRAND-NEW LineSegments2 + geometry per change —
  // the WebGPU renderer does not re-upload a line geometry mutated in place
  // (same caveat as the selected-edge fat line, ticket twxw), so reusing one
  // object would freeze the highlight on whatever edge it was first built for.
  const [hoverObj, setHoverObj] = useState<LineSegments2 | null>(null);
  useEffect(() => {
    if (hoveredIdx < 0 || hoveredIdx >= edges.length) {
      setHoverObj(null);
      return;
    }
    const geom = new LineSegmentsGeometry();
    geom.setPositions(edges[hoveredIdx].positions);
    const obj = new LineSegments2(geom, hoverMaterialRef.current);
    obj.frustumCulled = false;
    obj.renderOrder = 999;
    obj.layers.enable(OVERLAY_LAYER);
    setHoverObj(obj);
    return () => geom.dispose();
  }, [hoveredIdx, edges]);

  useEffect(() => {
    const bm = baseMaterialRef.current;
    const hm = hoverMaterialRef.current;
    return () => {
      bm.dispose();
      hm.dispose();
    };
  }, []);

  if (mode === "none" || !baseObj) return null;

  return (
    <>
      {/* dispose={null}: object identity changes per rebuild; the previous
          geometry is disposed by the effect above, the material is shared. */}
      <primitive object={baseObj} dispose={null} />
      {hoverObj && <primitive object={hoverObj} dispose={null} />}
    </>
  );
}

/** Fingerprint that bumps on any body geometry edit or placement move — count
 *  plus per-body `id.version`. Shared by the two debug overlays below so their
 *  kernel reads re-run exactly when (and only when) the bodies change. Mirrors
 *  {@link BRepEdgeOverlay}'s own `bodyFingerprint`. */
function useBodyFingerprint(): string {
  const { store, state } = useKernelStore();
  return useMemo(() => {
    if (!store) return "";
    const n = store.kernel.bodyCount();
    let fp = String(n);
    for (let i = 0; i < n; i++) fp += `:${store.kernel.bodyId(i)}.${store.kernel.bodyVersion(i)}`;
    return fp;
  }, [store, state]);
}

/**
 * Debug overlay: renders every B-Rep edge's **structural** points — the few
 * control points that actually define the curve, NOT the dense tessellation.
 * Three layers per edge:
 *   - **on-curve nodes** (red, large): the span joins that lie *on* the curve,
 *     at every `degree`-th control point. Count them to see how few points a
 *     NURBS edge is built from (e.g. an exact circle = a 4-span rational
 *     quadratic → 5 nodes).
 *   - **off-curve handles** (amber, small): the interior Bézier control points
 *     that bend each span; absent on straight/polyline edges.
 *   - **control polygon** (faint grey lines): connects consecutive control
 *     points, making the per-span quadratic/cubic structure visible.
 *
 * Reads the same cached analytic B-Rep (`controlPoints`/`degree`, world space)
 * as {@link BRepEdgeOverlay}, independent of mesh resolution and render mode.
 * Toggle via Settings → "Edge sample points".
 */
const EDGE_NODE_COLOR = 0xdc2626;
const EDGE_HANDLE_COLOR = 0xf59e0b;
const EDGE_POLYGON_COLOR = 0x94a3b8;

export function BRepEdgePointsOverlay() {
  const { store } = useKernelStore();
  const fingerprint = useBodyFingerprint();
  const nodeGeomRef = useRef(new THREE.BufferGeometry());
  const handleGeomRef = useRef(new THREE.BufferGeometry());
  const polygonGeomRef = useRef(new THREE.BufferGeometry());
  const nodeMatRef = useRef(
    new THREE.PointsMaterial({
      color: EDGE_NODE_COLOR, size: 9, sizeAttenuation: false,
      depthTest: false, depthWrite: false, transparent: true,
    }),
  );
  const handleMatRef = useRef(
    new THREE.PointsMaterial({
      color: EDGE_HANDLE_COLOR, size: 6, sizeAttenuation: false,
      depthTest: false, depthWrite: false, transparent: true,
    }),
  );
  const polygonMatRef = useRef(
    new THREE.LineBasicMaterial({
      color: EDGE_POLYGON_COLOR, depthTest: false, depthWrite: false,
      transparent: true, opacity: 0.6,
    }),
  );

  const buffers = useMemo(() => {
    if (!store) return null;
    const count = store.kernel.bodyCount();
    const nodes: number[] = [];
    const handles: number[] = [];
    const polygon: number[] = [];
    for (let i = 0; i < count; i++) {
      let bodyEdges: BRepEdge[];
      try {
        bodyEdges = store.kernel.bodyBrepEdges(i);
      } catch {
        continue;
      }
      for (const e of bodyEdges) {
        const cps = e.controlPoints;
        const d = Math.max(1, e.degree);
        const nCps = Math.floor(cps.length / 3);
        for (let p = 0; p < nCps; p++) {
          const x = cps[p * 3], y = cps[p * 3 + 1], z = cps[p * 3 + 2];
          // On-curve nodes sit at every `degree`-th control point; the rest
          // are off-curve Bézier handles.
          if (p % d === 0) nodes.push(x, y, z);
          else handles.push(x, y, z);
          // Control polygon: a segment from this control point to the next.
          if (p + 1 < nCps) {
            polygon.push(x, y, z, cps[(p + 1) * 3], cps[(p + 1) * 3 + 1], cps[(p + 1) * 3 + 2]);
          }
        }
      }
    }
    return {
      nodes: new Float32Array(nodes),
      handles: new Float32Array(handles),
      polygon: new Float32Array(polygon),
    };
  }, [store, fingerprint]);

  const [ready, setReady] = useState(false);
  useEffect(() => {
    if (!buffers || buffers.nodes.length === 0) {
      setReady(false);
      return;
    }
    nodeGeomRef.current.setAttribute("position", new THREE.BufferAttribute(buffers.nodes, 3));
    nodeGeomRef.current.computeBoundingSphere();
    handleGeomRef.current.setAttribute("position", new THREE.BufferAttribute(buffers.handles, 3));
    handleGeomRef.current.computeBoundingSphere();
    polygonGeomRef.current.setAttribute("position", new THREE.BufferAttribute(buffers.polygon, 3));
    polygonGeomRef.current.computeBoundingSphere();
    setReady(true);
  }, [buffers]);

  useEffect(() => {
    return () => {
      nodeGeomRef.current.dispose();
      handleGeomRef.current.dispose();
      polygonGeomRef.current.dispose();
      nodeMatRef.current.dispose();
      handleMatRef.current.dispose();
      polygonMatRef.current.dispose();
    };
  }, []);

  if (!ready || !buffers) return null;
  const enableOverlay = (obj: THREE.Object3D | null) => { if (obj) obj.layers.enable(OVERLAY_LAYER); };
  return (
    <>
      <lineSegments
        ref={enableOverlay}
        geometry={polygonGeomRef.current}
        material={polygonMatRef.current}
        frustumCulled={false}
        renderOrder={999}
      />
      {buffers.handles.length > 0 && (
        <points
          ref={enableOverlay}
          geometry={handleGeomRef.current}
          material={handleMatRef.current}
          frustumCulled={false}
          renderOrder={1000}
        />
      )}
      <points
        ref={enableOverlay}
        geometry={nodeGeomRef.current}
        material={nodeMatRef.current}
        frustumCulled={false}
        renderOrder={1001}
      />
    </>
  );
}

/** Per-class wireframe colour for the octree-cell overlay, indexed by the
 *  kernel's `class` byte: 0=Face (blue), 1=Edge (amber), 2=Vertex (red).
 *  RGB in 0..1 for a vertex-colour `LineBasicMaterial`. */
const OCTREE_CLASS_COLORS: [number, number, number][] = [
  [0.23, 0.51, 0.96],
  [0.96, 0.62, 0.04],
  [0.86, 0.15, 0.15],
];

/** Push the 12 wireframe edges of the axis-aligned box `[min,max]` into the
 *  `pos`/`col` arrays (24 vertices, each carrying `rgb`). */
function pushBoxWireframe(
  pos: number[],
  col: number[],
  min: [number, number, number],
  max: [number, number, number],
  rgb: [number, number, number],
) {
  const c: [number, number, number][] = [
    [min[0], min[1], min[2]], [max[0], min[1], min[2]],
    [max[0], max[1], min[2]], [min[0], max[1], min[2]],
    [min[0], min[1], max[2]], [max[0], min[1], max[2]],
    [max[0], max[1], max[2]], [min[0], max[1], max[2]],
  ];
  const edges: [number, number][] = [
    [0, 1], [1, 2], [2, 3], [3, 0], // bottom face
    [4, 5], [5, 6], [6, 7], [7, 4], // top face
    [0, 4], [1, 5], [2, 6], [3, 7], // verticals
  ];
  for (const [a, b] of edges) {
    pos.push(c[a][0], c[a][1], c[a][2], c[b][0], c[b][1], c[b][2]);
    col.push(rgb[0], rgb[1], rgb[2], rgb[0], rgb[1], rgb[2]);
  }
}

/**
 * Debug overlay: draws the topology **octree's leaf cells** as a wireframe
 * grid, colour-coded by class (blue = face, amber = edge, red = vertex). The
 * octree is **feature-adaptive on topology**: `edge`/`vertex` cells refine to
 * the fine `min_cell_size` (≈ largest-extent / 32) to pin the feature, while
 * `face` cells stop at the coarser `face_cell_size` (FACE_CELL_FACTOR× larger)
 * because a single-branch patch carries no topology to seed and feeds no weave
 * consumer. So flat/curved face interiors show large blue cells and the grid
 * tightens only along the amber edges and red corners. Cells arrive in the
 * body-local frame plus the body placement, which is applied as a group
 * matrix. Toggle via Settings → "Octree cells".
 */
export function OctreeCellOverlay() {
  const { store } = useKernelStore();
  const fingerprint = useBodyFingerprint();
  const materialRef = useRef(
    new THREE.LineBasicMaterial({
      vertexColors: true,
      depthTest: false,
      depthWrite: false,
      transparent: true,
      opacity: 0.5,
    }),
  );

  const bodies = useMemo(() => {
    if (!store) return [];
    const count = store.kernel.bodyCount();
    const out: { id: string; geometry: THREE.BufferGeometry; placement: OctreeCells }[] = [];
    for (let i = 0; i < count; i++) {
      let dump: OctreeCells;
      try {
        dump = store.kernel.bodyOctreeCells(i);
      } catch {
        continue;
      }
      if (dump.cells.length === 0) continue;
      const pos: number[] = [];
      const col: number[] = [];
      for (const cell of dump.cells) {
        const rgb = OCTREE_CLASS_COLORS[cell.class] ?? OCTREE_CLASS_COLORS[0];
        pushBoxWireframe(pos, col, cell.min, cell.max, rgb);
      }
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute("position", new THREE.BufferAttribute(new Float32Array(pos), 3));
      geometry.setAttribute("color", new THREE.BufferAttribute(new Float32Array(col), 3));
      out.push({ id: `${store.kernel.bodyId(i)}`, geometry, placement: dump });
    }
    return out;
  }, [store, fingerprint]);

  // Dispose the previous frame's geometries when `bodies` is rebuilt.
  useEffect(() => {
    return () => {
      for (const b of bodies) b.geometry.dispose();
    };
  }, [bodies]);

  useEffect(() => {
    return () => materialRef.current.dispose();
  }, []);

  if (bodies.length === 0) return null;
  return (
    <>
      {bodies.map((b) => (
        <group
          key={b.id}
          position={[b.placement.translate[0], b.placement.translate[1], b.placement.translate[2]]}
          quaternion={[
            b.placement.rotation[0],
            b.placement.rotation[1],
            b.placement.rotation[2],
            b.placement.rotation[3],
          ]}
          scale={b.placement.scale}
        >
          <lineSegments
            ref={(obj: THREE.LineSegments | null) => { if (obj) obj.layers.enable(OVERLAY_LAYER); }}
            geometry={b.geometry}
            material={materialRef.current}
            frustumCulled={false}
            renderOrder={997}
          />
        </group>
      ))}
    </>
  );
}

// ── B-Rep face / vertex selection overlay (Brocken B-W) ──────────────
//
// Token-driven highlight colours. THREE material colours can't live in CSS
// (they feed the WebGPU pipeline), so — like BRepEdgeOverlay's inline hex —
// they're module constants here. FACE_HOVER mirrors tokens.css
// --color-accent (#2f5fa8); VERTEX mirrors --color-warning (#b07820).
const FACE_HOVER_COLOR = 0x2f5fa8;
const VERTEX_COLOR = 0xb07820;
/** Persistent element-selection colour: a saturated gold-orange (Onshape-like
 *  selection highlight), distinct from the blue picker-highlight. */
const ELEMENT_SELECTION_COLOR = 0xff7a00;
/** Hover colour for B-Rep elements: the SAME gold-orange hue as
 *  {@link ELEMENT_SELECTION_COLOR}, lightened (ticket m0yw, brightened again
 *  in ticket bqvz) — hover previews the selection it would make, selection
 *  then deepens the tone. */
const ELEMENT_HOVER_COLOR = 0xffc080;
/** Stroke width of hovered-element fat lines (ticket bqvz: the old 1px hover
 *  was barely distinguishable from the base wireframe). Clearly wider
 *  than the 2.5px base edges, clearly slimmer than the 4px selection stroke. */
const ELEMENT_HOVER_LINEWIDTH = 3;
/** Vertex markers size in SCREEN pixels (ticket 7zs8): the selected edge
 *  draws as a 4px fat line, the hovered edge as a 3px fat line. World-space
 *  spheres looked huge at modelling zoom levels. */
const VERTEX_SELECT_DIAMETER_PX = 4;
const VERTEX_HOVER_DIAMETER_PX = 3;
/** Screen-space radius (in NDC units) within which a vertex marker counts as
 *  hovered. ~3.5% of the viewport half-extent. */
const VERTEX_PICK_NDC_RADIUS = 0.035;

/**
 * A point marker with a CONSTANT screen-space diameter (ticket 7zs8): a unit
 * sphere rescaled every frame to `px` pixels at its current depth, so the dot
 * neither balloons when zooming in nor vanishes when zooming out.
 */
function ScreenSpacePoint({
  position,
  color,
  px,
  renderOrder = 1000,
  opacity = 1,
}: {
  position: [number, number, number];
  color: number;
  px: number;
  renderOrder?: number;
  opacity?: number;
}) {
  const camera = useThree((s) => s.camera);
  const size = useThree((s) => s.size);
  const meshRef = useRef<THREE.Mesh>(null);
  useFrame(() => {
    const m = meshRef.current;
    if (!m) return;
    // World units per CSS pixel at the marker's depth.
    let wpp: number;
    if ((camera as THREE.PerspectiveCamera).isPerspectiveCamera) {
      const cam = camera as THREE.PerspectiveCamera;
      const dist = m.getWorldPosition(scratchVec3).distanceTo(cam.position);
      wpp = (2 * dist * Math.tan(THREE.MathUtils.degToRad(cam.fov) / 2)) / size.height;
    } else {
      const cam = camera as THREE.OrthographicCamera;
      wpp = (cam.top - cam.bottom) / cam.zoom / size.height;
    }
    m.scale.setScalar(Math.max((px * wpp) / 2, 1e-9));
  });
  return (
    <mesh
      position={position}
      ref={(obj: THREE.Mesh | null) => {
        meshRef.current = obj;
        if (obj) obj.layers.enable(OVERLAY_LAYER);
      }}
      renderOrder={renderOrder}
    >
      <sphereGeometry args={[1, 12, 12]} />
      <meshBasicMaterial color={color} depthTest={false} transparent opacity={opacity} />
    </mesh>
  );
}

const scratchVec3 = new THREE.Vector3();

/** One body's selectable B-Rep data, resolved to world space for picking. */
interface BodyPickData {
  /** World-space triangle mesh (positions + indices), for face raycasts.
   *  `null` when the main-thread kernel holds no mesh for this body (any SDF
   *  render mode with worker meshing) — faces are then resolved through the
   *  kernel's analytic `pick()` raycast instead (ticket evq2). */
  geometry: THREE.BufferGeometry | null;
  /** Persistent raycast target wrapping {@link geometry} (reused per frame). */
  pickMesh: THREE.Mesh | null;
  /** Per-triangle B-Rep face arena index (= `bodyFaceIndexView`). */
  faceIndexView: Uint32Array | null;
  faces: BRepFace[];
  /** Vertices with world-space positions folded through the placement. */
  vertices: { idHash: string; world: [number, number, number] }[];
  /** Analytic edge polylines (world space) keyed for face-outline highlights.
   *  `faces` carries the FACE-precise arena association (== faceIndex,
   *  ticket b91c); the `(a, b)` branch signature stays as the fallback for
   *  picks that only know the branch (coplanar split faces share one). */
  edges: { a: number; b: number; idHash: string; points: number[]; faces: number[] }[];
}

function buildBodyPickData(
  kernel: {
    bodyCount(): number;
    bodyVertexCount(i: number): number;
    bodyPositionsView(i: number): Float32Array;
    bodyIndicesView(i: number): Uint32Array;
    bodyFaceIndexView(i: number): Uint32Array;
    bodyBrepFaces(i: number): BRepFace[];
    bodyBrepVertices(i: number): BRepVertex[];
    bodyBrepEdges(i: number): BRepEdge[];
    bodyPlacement(i: number): Placement;
  },
): BodyPickData[] {
  const out: BodyPickData[] = [];
  const count = kernel.bodyCount();
  for (let i = 0; i < count; i++) {
    let faces: BRepFace[];
    let verts: BRepVertex[];
    let edges: BRepEdge[];
    let placement: Placement;
    try {
      faces = kernel.bodyBrepFaces(i);
      verts = kernel.bodyBrepVertices(i);
      edges = kernel.bodyBrepEdges(i);
      placement = kernel.bodyPlacement(i);
    } catch {
      continue;
    }
    // The triangle pick mesh exists only when this kernel instance actually
    // meshed the body (mesh render mode / sync meshing). The analytic B-Rep
    // data above is render-mode-independent, so faces and vertices stay
    // pickable without it.
    let geometry: THREE.BufferGeometry | null = null;
    let faceIndexView: Uint32Array | null = null;
    if (kernel.bodyVertexCount(i) > 0) {
      try {
        // Copy out of WASM linear memory — the views alias buffers that any
        // later mutating kernel call could relocate.
        faceIndexView = kernel.bodyFaceIndexView(i).slice();
        const positions = kernel.bodyPositionsView(i).slice();
        const indices = kernel.bodyIndicesView(i).slice();
        geometry = new THREE.BufferGeometry();
        geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
        geometry.setIndex(new THREE.BufferAttribute(indices, 1));
        geometry.computeBoundingSphere();
      } catch {
        geometry = null;
        faceIndexView = null;
      }
    }
    const vertices = verts.map((v) => ({
      idHash: v.idHash,
      world: localToWorld(v.positionLocal, placement),
    }));
    out.push({
      geometry,
      pickMesh: geometry ? new THREE.Mesh(geometry) : null,
      faceIndexView,
      faces,
      vertices,
      edges: edges.map((e) => ({
        a: e.a,
        b: e.b,
        idHash: e.idHash,
        points: e.points,
        faces: e.faces,
      })),
    });
  }
  return out;
}

/** Flat LineSegments positions (6 floats per segment) for the boundary of
 *  the face with packed branch id `branchId`: every analytic edge whose
 *  signature contains the branch. Mesh-free face highlight (ticket evq2). */
function faceOutlineSegments(bodies: BodyPickData[], branchId: number): number[] {
  const segs: number[] = [];
  for (const body of bodies) {
    for (const e of body.edges) {
      if (e.a !== branchId && e.b !== branchId) continue;
      segs.push(...polylineToSegments(e.points));
    }
  }
  return segs;
}

/**
 * Hoverable + selectable B-Rep **faces** and **vertices** in the viewport.
 *
 * Faces: an invisible per-body pick mesh (built straight from the kernel's
 * world-space body buffers, independent of render mode — same trick as
 * `BRepEdgeOverlay`) is raycast each frame. THREE reports
 * `intersection.faceIndex` = the **triangle** index; indexing
 * `bodyFaceIndexView(i)` with it gives the B-Rep **face arena index**, and
 * `bodyBrepFaces(i)[arena].idHash` the stable id. All triangles sharing that
 * arena index are lit as the hover highlight.
 *
 * Vertices: rendered as small token-coloured point markers (positions folded
 * body-LOCAL → world via `localToWorld`), nearest-to-cursor in screen space
 * is the hover.
 *
 * The hovered element is published to `brepPickBridge` so the Viewport click
 * handler can emit `qFace`/`qVertex`. Highlighting + markers only light up
 * when an element-level query picker is active and its `entityFilter` allows
 * that kind — so this never adds clutter during normal modelling.
 */
export function BRepSelectionOverlay({
  ndc,
}: {
  ndc: { x: number; y: number; active: boolean };
}) {
  const { store, state } = useKernelStore();
  const camera = useThree((s) => s.camera);
  const invalidate = useThree((s) => s.invalidate);
  const raycaster = useRef(new THREE.Raycaster());

  const faceHighlightGeom = useRef(new THREE.BufferGeometry());
  const faceHighlightMat = useRef(
    new THREE.MeshBasicMaterial({
      color: FACE_HOVER_COLOR,
      transparent: true,
      opacity: 0.4,
      depthTest: false,
      depthWrite: false,
      side: THREE.DoubleSide,
    }),
  );

  // Same body fingerprint trick as BRepEdgeOverlay: rebuild the pick data only
  // when bodies actually change (count or per-body id.version).
  const bodyFingerprint = useMemo(() => {
    if (!store) return "";
    const n = store.kernel.bodyCount();
    let fp = String(n);
    for (let i = 0; i < n; i++) fp += `:${store.kernel.bodyId(i)}.${store.kernel.bodyVersion(i)}`;
    return fp;
  }, [store, state]);

  const bodies = useMemo<BodyPickData[]>(() => {
    if (!store) return [];
    return buildBodyPickData(store.kernel);
  }, [store, bodyFingerprint]);

  useEffect(() => {
    return () => {
      for (const b of bodies) b.geometry?.dispose();
    };
  }, [bodies]);

  useEffect(() => {
    const geom = faceHighlightGeom.current;
    const mat = faceHighlightMat.current;
    return () => {
      geom.dispose();
      mat.dispose();
    };
  }, []);

  const [faceHover, setFaceHover] = useState(false);
  const [vertexHoverIdx, setVertexHoverIdx] = useState<[number, number] | null>(null);

  // Build the highlight triangle list for one arena face index. Bodies
  // without a triangle mesh (SDF render modes) contribute no triangles —
  // their hover feedback is the boundary outline below.
  const highlightFace = (body: BodyPickData, arena: number | null): void => {
    const tris: number[] = [];
    if (body.geometry && body.faceIndexView && arena !== null) {
      const idx = body.geometry.getIndex();
      const pos = body.geometry.getAttribute("position");
      if (idx && pos) {
        const triCount = body.faceIndexView.length;
        for (let t = 0; t < triCount; t++) {
          if (body.faceIndexView[t] !== arena) continue;
          for (let k = 0; k < 3; k++) {
            const vi = idx.getX(t * 3 + k);
            tris.push(pos.getX(vi), pos.getY(vi), pos.getZ(vi));
          }
        }
      }
    }
    faceHighlightGeom.current.setAttribute(
      "position",
      new THREE.Float32BufferAttribute(tris, 3),
    );
    faceHighlightGeom.current.computeBoundingSphere();
  };

  // Boundary-outline hover highlight for the hovered face (ticket evq2) —
  // every analytic edge bounding the face lights up exactly like a hovered
  // edge. This is the ONLY face-hover feedback in the SDF render modes (no
  // triangle fill there), and it must go through the fat-line path
  // (LineSegments2 + Line2NodeMaterial): the previous plain LineBasicMaterial
  // lineSegments never composited on the WebGPU backend, so hovering a face
  // gave no edge feedback at all (ticket bqvz).
  const outlineMatRef = useRef(
    new Line2NodeMaterial({
      color: ELEMENT_HOVER_COLOR,
      linewidth: ELEMENT_HOVER_LINEWIDTH,
      worldUnits: false,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    }),
  );
  useEffect(() => {
    const mat = outlineMatRef.current;
    return () => mat.dispose();
  }, []);
  // Hovered face, published from useFrame; the outline gets a BRAND-NEW
  // LineSegments2 + geometry per change — the WebGPU renderer does not
  // re-upload a line geometry mutated in place (ticket twxw). The functional
  // setter bails on identical (body, arena, branchId) so the per-frame
  // publish never re-renders.
  type HoverFace = { body: BodyPickData; arena: number | null; branchId: number };
  const [hoverFace, setHoverFace] = useState<HoverFace | null>(null);
  const publishHoverFace = (next: HoverFace | null): void => {
    setHoverFace((prev) => {
      if (next === null) return prev === null ? prev : null;
      if (
        prev !== null &&
        prev.body === next.body &&
        prev.arena === next.arena &&
        prev.branchId === next.branchId
      )
        return prev;
      return next;
    });
  };
  const [outlineObj, setOutlineObj] = useState<LineSegments2 | null>(null);
  useEffect(() => {
    if (hoverFace === null) {
      setOutlineObj(null);
      return;
    }
    // Face-precise outline when the triangle pick resolved an arena index
    // (edge.faces association, ticket b91c) — coplanar split faces share a
    // branch signature, so the branch fallback (analytic SDF pick, no
    // arena) lights the whole coplanar family.
    let segs: number[];
    if (hoverFace.arena !== null) {
      segs = [];
      for (const e of hoverFace.body.edges) {
        if (!e.faces.includes(hoverFace.arena)) continue;
        segs.push(...polylineToSegments(e.points));
      }
    } else {
      segs = faceOutlineSegments(bodies, hoverFace.branchId);
    }
    if (segs.length === 0) {
      setOutlineObj(null);
      return;
    }
    const geom = new LineSegmentsGeometry();
    geom.setPositions(new Float32Array(segs));
    const obj = new LineSegments2(geom, outlineMatRef.current);
    obj.frustumCulled = false;
    obj.renderOrder = 999;
    obj.layers.enable(OVERLAY_LAYER);
    setOutlineObj(obj);
    invalidate();
    return () => geom.dispose();
  }, [hoverFace, bodies, invalidate]);

  useFrame(() => {
    const picker = getActiveQueryPicker();
    // Hover detection runs whenever the cursor is over the viewport — tool-free
    // element selection (chunk 3a) needs the hovered element published even
    // with no active picker. When a picker IS active we honour its
    // entityFilter; with no picker, all kinds are pickable.
    if (!ndc.active || bodies.length === 0) {
      if (faceHover) { setFaceHover(false); invalidate(); }
      if (vertexHoverIdx) { setVertexHoverIdx(null); invalidate(); }
      publishHoverFace(null);
      setHoveredElementOfKind("face", null);
      setHoveredElementOfKind("vertex", null);
      return;
    }
    const filter = picker?.entityFilter;
    // Snapshot semantics: while a picker is active, faces of features that
    // come AFTER the one being edited are not selectable — don't light them on
    // hover (the click handler also rejects them with a message).
    const faceAllowed = (featureId: number): boolean =>
      !picker?.selectableFeatureIds || picker.selectableFeatureIds.has(featureId);
    // An edge inside its pick band beats the face it bounds — the bridge
    // merge already resolves clicks that way (vertex > edge > face), so the
    // visuals must agree: with the face outline now drawn in the same hover
    // style (ticket bqvz), keeping both lit would mask WHICH edge a click
    // selects. A face-only picker keeps the face highlight alive instead.
    const edgeWins =
      getHoveredElementOfKind("edge") !== null &&
      (!picker || filterAllows("edge", filter));
    const wantFace = !edgeWins && (picker ? filterAllows("face", filter) : true);
    const wantVertex = picker ? filterAllows("vertex", filter) : true;

    raycaster.current.setFromCamera(new THREE.Vector2(ndc.x, ndc.y), camera);

    // ── Faces ──
    // Two resolution paths (ticket evq2): the triangle pick mesh where this
    // kernel instance holds one (mesh render mode), and the kernel's analytic
    // `pick()` raycast everywhere else (SDF modes keep main-thread meshes
    // empty, which used to make faces and vertices unhoverable). The analytic
    // hit maps its packed branch ids onto `BRepFace.branchId`.
    let resolvedFace:
      | { body: BodyPickData; arena: number | null; branchId: number; idHash: string; dist: number }
      | null = null;
    if (wantFace) {
      for (const body of bodies) {
        if (!body.pickMesh || !body.faceIndexView) continue;
        const hits = raycaster.current.intersectObject(body.pickMesh, false);
        for (const h of hits) {
          if (h.faceIndex === undefined || h.faceIndex === null) continue;
          const arena = body.faceIndexView[h.faceIndex];
          if (arena === undefined || arena === FACE_INDEX_SENTINEL) continue;
          const face = body.faces.find((f) => f.faceIndex === arena);
          if (!face) continue;
          if (!faceAllowed(face.featureId)) continue;
          if (!resolvedFace || h.distance < resolvedFace.dist) {
            resolvedFace = { body, arena, branchId: face.branchId, idHash: face.idHash, dist: h.distance };
          }
          break;
        }
      }
      if (!resolvedFace && store) {
        const o = raycaster.current.ray.origin;
        const d = raycaster.current.ray.direction;
        const hit = store.kernel.pick([o.x, o.y, o.z], [d.x, d.y, d.z]);
        if (hit.hit) {
          outer: for (const body of bodies) {
            for (const face of body.faces) {
              if (!hit.branches.includes(face.branchId)) continue;
              if (!faceAllowed(face.featureId)) continue;
              resolvedFace = { body, arena: null, branchId: face.branchId, idHash: face.idHash, dist: hit.t };
              break outer;
            }
          }
        }
      }
    }

    // ── Vertices (nearest in screen space) ──
    // A vertex within its snap radius wins over the face under the cursor —
    // clicking near a corner selects the corner (ticket evq2) — unless the
    // face hit is clearly in FRONT of it (occluded back-side corner).
    let resolvedVertex:
      | { bi: number; vi: number; idHash: string; ndcDist: number }
      | null = null;
    if (wantVertex) {
      const v = new THREE.Vector3();
      const camPos = camera.position;
      for (let bi = 0; bi < bodies.length; bi++) {
        const verts = bodies[bi].vertices;
        for (let vi = 0; vi < verts.length; vi++) {
          const w = verts[vi].world;
          v.set(w[0], w[1], w[2]);
          const camDist = v.distanceTo(camPos);
          if (resolvedFace && camDist > resolvedFace.dist * 1.05 + 1e-3) continue;
          v.project(camera);
          if (v.z < -1 || v.z > 1) continue;
          const d = Math.hypot(v.x - ndc.x, v.y - ndc.y);
          if (d > VERTEX_PICK_NDC_RADIUS) continue;
          if (!resolvedVertex || d < resolvedVertex.ndcDist) {
            resolvedVertex = { bi, vi, idHash: verts[vi].idHash, ndcDist: d };
          }
        }
      }
    }
    if (resolvedVertex) resolvedFace = null;

    if (resolvedFace) {
      highlightFace(resolvedFace.body, resolvedFace.arena);
      publishHoverFace({
        body: resolvedFace.body,
        arena: resolvedFace.arena,
        branchId: resolvedFace.branchId,
      });
      if (!faceHover) { setFaceHover(true); invalidate(); }
      if (vertexHoverIdx) { setVertexHoverIdx(null); invalidate(); }
      setHoveredElementOfKind("face", { kind: "face", idHash: resolvedFace.idHash });
      setHoveredElementOfKind("vertex", null);
    } else if (resolvedVertex) {
      if (faceHover) { setFaceHover(false); invalidate(); }
      publishHoverFace(null);
      const next: [number, number] = [resolvedVertex.bi, resolvedVertex.vi];
      if (!vertexHoverIdx || vertexHoverIdx[0] !== next[0] || vertexHoverIdx[1] !== next[1]) {
        setVertexHoverIdx(next);
        invalidate();
      }
      setHoveredElementOfKind("face", null);
      setHoveredElementOfKind("vertex", { kind: "vertex", idHash: resolvedVertex.idHash });
    } else {
      if (faceHover) { setFaceHover(false); invalidate(); }
      if (vertexHoverIdx) { setVertexHoverIdx(null); invalidate(); }
      publishHoverFace(null);
      setHoveredElementOfKind("face", null);
      setHoveredElementOfKind("vertex", null);
    }
  });

  const picker = getActiveQueryPicker();
  const showVertexMarkers =
    picker !== null && filterAllows("vertex", picker.entityFilter);
  // Tool-free hovered vertex: render ITS marker even without a picker so a
  // click near a corner has visible feedback before the click (ticket evq2).
  const hoveredVertexWorld =
    !showVertexMarkers && vertexHoverIdx !== null
      ? bodies[vertexHoverIdx[0]]?.vertices[vertexHoverIdx[1]]?.world ?? null
      : null;

  return (
    <>
      {faceHover && (
        <mesh
          ref={(obj: THREE.Mesh | null) => { if (obj) obj.layers.enable(OVERLAY_LAYER); }}
          geometry={faceHighlightGeom.current}
          material={faceHighlightMat.current}
          frustumCulled={false}
          renderOrder={997}
        />
      )}
      {/* dispose={null}: fresh object per hovered face; the previous geometry
          is disposed by the outline effect, the material is shared. */}
      {outlineObj && <primitive object={outlineObj} dispose={null} />}
      {hoveredVertexWorld && (
        <ScreenSpacePoint
          position={hoveredVertexWorld}
          color={ELEMENT_HOVER_COLOR}
          px={VERTEX_HOVER_DIAMETER_PX}
        />
      )}
      {showVertexMarkers &&
        bodies.map((b, bi) =>
          b.vertices.map((v, vi) => {
            const hovered =
              vertexHoverIdx !== null && vertexHoverIdx[0] === bi && vertexHoverIdx[1] === vi;
            return (
              <ScreenSpacePoint
                key={`v-${bi}-${vi}`}
                position={v.world}
                color={hovered ? ELEMENT_HOVER_COLOR : VERTEX_COLOR}
                px={VERTEX_HOVER_DIAMETER_PX}
                opacity={hovered ? 1 : 0.85}
              />
            );
          }),
        )}
    </>
  );
}

/**
 * Persistent B-Rep element-selection highlight (chunk 3a).
 *
 * Renders `state.selectedElement` — a single face, edge, or vertex picked
 * tool-free in the viewport — in the distinct teal selection colour. Unlike
 * the hover highlights in {@link BRepSelectionOverlay}/{@link BRepEdgeOverlay}
 * this is NOT picker-gated and persists after the pointer leaves; it clears
 * only when the store's `selectedElement` is reset.
 *
 * The highlight geometry is resolved from the kernel by `idHash`:
 *   - face → all triangles whose face-arena index matches, in world space;
 *   - edge → the matching analytic polyline, body-local → world;
 *   - vertex → a sphere marker at the world position.
 */
export function BRepSelectedOverlay() {
  const { store, state } = useKernelStore();
  const selected = state?.selectedElement ?? null;
  // Full multi-select set: a face primary highlights every selected face
  // (the coplanar extrude gather); edges/vertices stay single-primary.
  const selectedList = state?.selectedElements ?? [];
  const selectedKey = selectedList.map((e) => `${e.kind}:${e.idHash}`).join(",");

  const faceGeomRef = useRef(new THREE.BufferGeometry());
  const faceMatRef = useRef(
    new THREE.MeshBasicMaterial({
      color: ELEMENT_SELECTION_COLOR,
      transparent: true,
      opacity: 0.5,
      depthTest: false,
      depthWrite: false,
      side: THREE.DoubleSide,
    }),
  );
  // Selected edges use a *fat* line (LineSegments2 + Line2NodeMaterial). A
  // plain LineBasicMaterial silently ignores `linewidth` on WebGPU/WebGL — it
  // always rasterises 1px — so a visible stroke only materialises through the
  // instanced screen-space line shader. The WebGPU variant derives its pixel
  // scale from the built-in viewport/DPR nodes, so unlike the WebGL
  // LineMaterial there is no `resolution` uniform to keep in sync.
  // 4 px matches Onshape's chunky selected-edge stroke (ticket 6btt).
  const edgeMatRef = useRef(
    new Line2NodeMaterial({
      color: ELEMENT_SELECTION_COLOR,
      linewidth: 4,
      worldUnits: false,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    }),
  );
  // A selected FULL-PRIMITIVE face (a sphere/torus face — no mesh, no
  // boundary edges, not planar) tints the matching hybrid leaf primitive,
  // like the body highlight does (ticket yiis).
  const leafGroupRef = useRef<THREE.Group | null>(null);
  const disposeLeafGroup = () => {
    leafGroupRef.current?.traverse((o) => {
      const m = o as THREE.Mesh;
      if (m.geometry) m.geometry.dispose();
    });
    leafGroupRef.current = null;
  };
  // The selected edge gets a BRAND-NEW LineSegments2 + geometry per selection,
  // built in the edge branch of `resolved` below — never an in-place mutation
  // of one persistent geometry. The WebGPU renderer caches a
  // LineSegmentsGeometry's instanced GPU buffers and does NOT re-upload them
  // after `setPositions` overwrites them, so reusing one geometry froze the
  // highlight on the first edge ever selected (ticket twxw: "immer dieselbe
  // Kante"). Handing the renderer a fresh object forces fresh buffers. The ref
  // only tracks the current object so the resolver can dispose the previous.
  const edgeObjRef = useRef<LineSegments2 | null>(null);

  useEffect(() => {
    const fg = faceGeomRef.current;
    const fm = faceMatRef.current;
    const em = edgeMatRef.current;
    return () => {
      fg.dispose();
      fm.dispose();
      edgeObjRef.current?.geometry.dispose();
      em.dispose();
      disposeLeafGroup();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Body fingerprint: re-resolve the highlight when bodies actually change
  // (same trick as the hover overlays), since a regen invalidates idHashes.
  const bodyFingerprint = useMemo(() => {
    if (!store) return "";
    const n = store.kernel.bodyCount();
    let fp = String(n);
    for (let i = 0; i < n; i++) fp += `:${store.kernel.bodyId(i)}.${store.kernel.bodyVersion(i)}`;
    return fp;
  }, [store, state]);

  // Resolve the WHOLE multi-select set to renderable data (box select can
  // put many faces, edges and vertices in it at once): face triangles go
  // into the shared face geometry, face outlines + selected edges into one
  // fresh fat-line object, vertices into world positions for the dot
  // markers.
  const resolved = useMemo<
    | { hasFace: boolean; hasLines: boolean; hasLeafFill: boolean; vertices: [number, number, number][] }
    | null
  >(() => {
    if (!store || selectedList.length === 0) return null;
    const k = store.kernel;
    const count = k.bodyCount();

    const faceHashes = new Set(
      selectedList.filter((e) => e.kind === "face").map((e) => e.idHash),
    );
    const edgeHashes = new Set(
      selectedList.filter((e) => e.kind === "edge").map((e) => e.idHash),
    );
    const vertexHashes = new Set(
      selectedList.filter((e) => e.kind === "vertex").map((e) => e.idHash),
    );

    const tris: number[] = [];
    const vertices: [number, number, number][] = [];
    // Selected face ARENA indices per body (edge.faces association,
    // ticket b91c) — matching by branch signature would light the whole
    // coplanar family, since split coplanar faces share one branch id.
    const wantedArenasByBody = new Map<number, Set<number>>();
    const leafFills: HybridLeafInfo[] = [];

    // Pass 1 — faces (triangles where this kernel holds a mesh; in SDF render
    // modes the fill stays empty and the outline below is the highlight,
    // ticket evq2) and vertices.
    for (let i = 0; i < count; i++) {
      let placement: Placement;
      try {
        placement = k.bodyPlacement(i);
      } catch {
        continue;
      }
      if (faceHashes.size > 0) {
        let faces: BRepFace[];
        try {
          faces = k.bodyBrepFaces(i);
        } catch {
          faces = [];
        }
        const wantedFaces = faces.filter((f) => faceHashes.has(f.idHash));
        if (wantedFaces.length > 0) {
          wantedArenasByBody.set(i, new Set(wantedFaces.map((f) => f.faceIndex)));
        }
        if (wantedFaces.length > 0 && k.bodyVertexCount(i) > 0) {
          try {
            const faceIndexView = k.bodyFaceIndexView(i).slice();
            const positions = k.bodyPositionsView(i).slice();
            const indices = k.bodyIndicesView(i).slice();
            const wanted = new Set(wantedFaces.map((f) => f.faceIndex));
            for (let t = 0; t < faceIndexView.length; t++) {
              if (!wanted.has(faceIndexView[t])) continue;
              for (let c = 0; c < 3; c++) {
                const vi = indices[t * 3 + c];
                const local: [number, number, number] = [
                  positions[vi * 3],
                  positions[vi * 3 + 1],
                  positions[vi * 3 + 2],
                ];
                const w = localToWorld(local, placement);
                tris.push(w[0], w[1], w[2]);
              }
            }
          } catch {
            // mesh views unavailable mid-regen — outline still renders
          }
        } else if (wantedFaces.length > 0) {
          // Mesh-free (SDF render modes): fill PLANAR faces by triangulating
          // their boundary edges (ticket por8) — Onshape-style full-face
          // highlight instead of outline-only. Curved faces keep the outline.
          let edges: BRepEdge[] = [];
          try {
            edges = k.bodyBrepEdges(i);
          } catch {
            // mid-regen — outline still renders
          }
          for (const f of wantedFaces) {
            if (f.surfaceKind === "sphere" || f.surfaceKind === "torus") {
              // A full-primitive face: no boundary edges to outline and no
              // planar profile to fill — tint the nearest hybrid leaf of the
              // same kind, exactly like the body highlight (ticket yiis).
              try {
                const leaves = k.bodyHybridInfo(i).leaves.filter(
                  (l) => l.kind === f.surfaceKind && l.role !== "skip",
                );
                const c = localToWorld(f.centroidLocal, placement);
                let best: (typeof leaves)[number] | null = null;
                let bestD = Infinity;
                for (const l of leaves) {
                  const d =
                    (l.transform[0] - c[0]) ** 2 +
                    (l.transform[1] - c[1]) ** 2 +
                    (l.transform[2] - c[2]) ** 2;
                  if (d < bestD) {
                    best = l;
                    bestD = d;
                  }
                }
                if (best) leafFills.push(best);
              } catch {
                // no hybrid info — face stays un-highlighted
              }
              continue;
            }
            if (f.surfaceKind !== "plane") continue;
            const boundary = edges
              .filter((e) => e.faces.includes(f.faceIndex))
              .map((e) => e.points);
            if (boundary.length === 0) continue;
            const originW = localToWorld(f.centroidLocal, placement);
            const [qx, qy, qz, qw] = placement.rotation;
            const nl = f.normalLocal;
            const tx = 2 * (qy * nl[2] - qz * nl[1]);
            const ty = 2 * (qz * nl[0] - qx * nl[2]);
            const tz = 2 * (qx * nl[1] - qy * nl[0]);
            const normalW: [number, number, number] = [
              nl[0] + qw * tx + qy * tz - qz * ty,
              nl[1] + qw * ty + qz * tx - qx * tz,
              nl[2] + qw * tz + qx * ty - qy * tx,
            ];
            tris.push(...triangulatePlanarFace(boundary, originW, normalW));
          }
        }
      }
      if (vertexHashes.size > 0) {
        try {
          for (const v of k.bodyBrepVertices(i)) {
            if (vertexHashes.has(v.idHash))
              vertices.push(localToWorld(v.positionLocal, placement));
          }
        } catch {
          // mid-regen
        }
      }
    }

    // Pass 2 — line work: every analytic edge that bounds a selected face
    // (outline, by face ARENA index — ticket b91c) or IS a selected edge.
    // BRepEdge.points are already world space (kernel.ts) — folding them
    // through the placement again would double-transform (see the hover
    // overlay).
    const segs: number[] = [];
    for (let i = 0; i < count; i++) {
      let edges: BRepEdge[];
      try {
        edges = k.bodyBrepEdges(i);
      } catch {
        continue;
      }
      const wantedArenas = wantedArenasByBody.get(i);
      for (const e of edges) {
        const boundsSelectedFace =
          wantedArenas !== undefined && e.faces.some((fi) => wantedArenas.has(fi));
        if (!boundsSelectedFace && !edgeHashes.has(e.idHash)) continue;
        segs.push(...polylineToSegments(e.points));
      }
    }

    if (
      tris.length === 0 &&
      segs.length === 0 &&
      vertices.length === 0 &&
      leafFills.length === 0
    ) {
      disposeLeafGroup();
      return null;
    }

    // Full-primitive face fills (sphere/torus, ticket yiis): fresh group per
    // selection, same dispose pattern as the edge object.
    disposeLeafGroup();
    if (leafFills.length > 0) {
      const group = new THREE.Group();
      for (const leaf of leafFills) {
        const geo = createPrimitiveGeometry(leaf);
        if (!geo) continue;
        const mesh = new THREE.Mesh(geo, faceMatRef.current);
        applyLeafTransform(mesh, leaf);
        mesh.frustumCulled = false;
        mesh.renderOrder = 1001;
        mesh.layers.enable(OVERLAY_LAYER);
        group.add(mesh);
      }
      if (group.children.length > 0) leafGroupRef.current = group;
    }

    faceGeomRef.current.setAttribute(
      "position",
      new THREE.Float32BufferAttribute(tris, 3),
    );
    faceGeomRef.current.computeBoundingSphere();

    if (segs.length > 0) {
      // Fresh fat-line object per selection — the WebGPU backend does not
      // re-upload a LineSegmentsGeometry mutated in place (ticket twxw).
      const geom = new LineSegmentsGeometry();
      geom.setPositions(segs);
      const obj = new LineSegments2(geom, edgeMatRef.current);
      obj.frustumCulled = false;
      obj.renderOrder = 1002;
      obj.layers.enable(OVERLAY_LAYER);
      edgeObjRef.current?.geometry.dispose();
      edgeObjRef.current = obj;
    } else {
      edgeObjRef.current?.geometry.dispose();
      edgeObjRef.current = null;
    }

    return {
      hasFace: tris.length > 0,
      hasLines: segs.length > 0,
      hasLeafFill: leafGroupRef.current !== null,
      vertices,
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, selected, selectedKey, bodyFingerprint]);

  if (!resolved) return null;

  return (
    <>
      {resolved.hasFace && (
        <mesh
          ref={(obj: THREE.Mesh | null) => { if (obj) obj.layers.enable(OVERLAY_LAYER); }}
          geometry={faceGeomRef.current}
          material={faceMatRef.current}
          frustumCulled={false}
          renderOrder={1001}
        />
      )}
      {/* `dispose={null}`: the object identity changes per selection, so R3F
          remounts this primitive each time. Without it R3F would auto-dispose
          the outgoing object on unmount — including the *shared* material —
          breaking every later highlight. The previous geometry is disposed in
          the memo above. */}
      {resolved.hasLines && edgeObjRef.current && (
        <primitive object={edgeObjRef.current} dispose={null} />
      )}
      {resolved.hasLeafFill && leafGroupRef.current && (
        <primitive object={leafGroupRef.current} dispose={null} />
      )}
      {resolved.vertices.map((w, i) => (
        <ScreenSpacePoint
          key={`sv-${i}`}
          position={w}
          color={ELEMENT_SELECTION_COLOR}
          px={VERTEX_SELECT_DIAMETER_PX}
          renderOrder={1003}
        />
      ))}
    </>
  );
}

/**
 * Whole-body selection highlight (ticket evq2): when a body is selected —
 * second click on an already-selected face, or a Parts-list row — every
 * analytic B-Rep edge of `state.highlightedBodyIndex` lights up as a fat
 * line in the element-selection colour. Mesh-free, so it works in every
 * render mode, exactly like the edge/face outlines. A body WITHOUT any
 * B-Rep edge (a bare sphere/torus) tints its analytic leaf primitives
 * instead — the selection reads as the round body itself, Onshape-style,
 * not as its AABB wireframe (ticket por8).
 */
export function BodyHighlightOverlay() {
  const { store, state } = useKernelStore();
  const bodyIndex = state?.highlightedBodyIndex ?? null;

  const matRef = useRef(
    new Line2NodeMaterial({
      color: ELEMENT_SELECTION_COLOR,
      linewidth: 2,
      worldUnits: false,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    }),
  );
  const fillMatRef = useRef(
    new THREE.MeshBasicMaterial({
      color: ELEMENT_SELECTION_COLOR,
      transparent: true,
      opacity: 0.65,
      depthTest: false,
      depthWrite: false,
    }),
  );
  const objRef = useRef<THREE.Object3D | null>(null);
  const disposeObj = () => {
    objRef.current?.traverse((o) => {
      const m = o as THREE.Mesh;
      if (m.geometry) m.geometry.dispose();
    });
    objRef.current = null;
  };
  useEffect(() => {
    const mat = matRef.current;
    const fillMat = fillMatRef.current;
    return () => {
      disposeObj();
      mat.dispose();
      fillMat.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const bodyFingerprint = useMemo(() => {
    if (!store) return "";
    const n = store.kernel.bodyCount();
    let fp = String(n);
    for (let i = 0; i < n; i++) fp += `:${store.kernel.bodyId(i)}.${store.kernel.bodyVersion(i)}`;
    return fp;
  }, [store, state]);

  const ready = useMemo(() => {
    if (!store || bodyIndex === null) return false;
    const k = store.kernel;
    if (bodyIndex >= k.bodyCount()) return false;
    let edges: BRepEdge[];
    try {
      edges = k.bodyBrepEdges(bodyIndex);
    } catch {
      return false;
    }
    const segs: number[] = [];
    for (const e of edges) segs.push(...polylineToSegments(e.points));
    if (segs.length === 0) {
      // Bodies without any B-Rep edge (a bare sphere/torus): tint the body's
      // analytic leaf primitives — the SAME hull shapes the deferred renderer
      // rasterises — so the selection is the round body itself, not its AABB
      // box (ticket por8). Leaf transforms are already world space.
      try {
        const hybrid = k.bodyHybridInfo(bodyIndex);
        const group = new THREE.Group();
        for (const leaf of hybrid.leaves) {
          if (leaf.role === "skip") continue;
          const warped = leaf.warp ? buildWarpedLeafGeometry(leaf) : null;
          const geo = warped ?? createPrimitiveGeometry(leaf);
          if (!geo) continue;
          const mesh = new THREE.Mesh(geo, fillMatRef.current);
          if (!warped) applyLeafTransform(mesh, leaf); // warped geo is world-space
          mesh.frustumCulled = false;
          mesh.renderOrder = 1001;
          mesh.layers.enable(OVERLAY_LAYER);
          group.add(mesh);
        }
        if (group.children.length > 0) {
          disposeObj();
          objRef.current = group;
          return true;
        }
      } catch {
        // no hybrid info — fall through to the AABB wireframe
      }
      // Last resort (no analytic leaves either, e.g. a warped TPMS body):
      // the 12 edges of the body's world AABB from the compiled SDF.
      try {
        const aabb = k.bodySdfAabb(bodyIndex);
        if (aabb.length >= 6) {
          const c: [number, number, number][] = [
            [aabb[0], aabb[1], aabb[2]], [aabb[3], aabb[1], aabb[2]],
            [aabb[3], aabb[4], aabb[2]], [aabb[0], aabb[4], aabb[2]],
            [aabb[0], aabb[1], aabb[5]], [aabb[3], aabb[1], aabb[5]],
            [aabb[3], aabb[4], aabb[5]], [aabb[0], aabb[4], aabb[5]],
          ];
          const boxEdges = [
            [0, 1], [1, 2], [2, 3], [3, 0],
            [4, 5], [5, 6], [6, 7], [7, 4],
            [0, 4], [1, 5], [2, 6], [3, 7],
          ];
          for (const [a, b] of boxEdges) segs.push(...c[a], ...c[b]);
        }
      } catch {
        // no compiled SDF either — nothing to draw
      }
    }
    if (segs.length === 0) return false;
    // Fresh geometry + object per change — the WebGPU backend does not
    // re-upload a LineSegmentsGeometry mutated in place (ticket twxw).
    const geom = new LineSegmentsGeometry();
    geom.setPositions(segs);
    const obj = new LineSegments2(geom, matRef.current);
    obj.frustumCulled = false;
    obj.renderOrder = 1002;
    obj.layers.enable(OVERLAY_LAYER);
    disposeObj();
    objRef.current = obj;
    return true;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, bodyIndex, bodyFingerprint]);

  if (!ready || !objRef.current) return null;
  return <primitive object={objRef.current} dispose={null} />;
}

function MeshPreviewBody({ body, mat, bodyIndex }: {
  body: { positions: Float32Array; normals: Float32Array; indices: Uint32Array };
  mat: MaterialProps | undefined;
  bodyIndex: number;
}) {
  const geometry = useMemo(() => {
    const geom = new THREE.BufferGeometry();
    geom.setAttribute("position", new THREE.BufferAttribute(body.positions, 3));
    geom.setAttribute("normal", new THREE.BufferAttribute(body.normals, 3));
    geom.setIndex(new THREE.BufferAttribute(body.indices, 1));
    geom.computeBoundingSphere();
    return geom;
  }, [body.positions, body.normals, body.indices]);

  useEffect(() => {
    return () => geometry.dispose();
  }, [geometry]);

  const fallback = BODY_COLORS[bodyIndex % BODY_COLORS.length];
  const color = mat
    ? new THREE.Color(mat.color[0], mat.color[1], mat.color[2])
    : new THREE.Color(fallback[0], fallback[1], fallback[2]);
  const emissive = mat
    ? new THREE.Color(mat.emissive[0], mat.emissive[1], mat.emissive[2])
    : undefined;

  return (
    <mesh geometry={geometry}>
      <meshStandardMaterial
        color={color}
        roughness={mat?.roughness ?? 0.6}
        metalness={mat?.metalness ?? 0.0}
        emissive={emissive}
        opacity={mat?.opacity ?? 1}
        transparent={mat != null && mat.opacity < 1}
        wireframe={mat?.wireframe ?? false}
        side={THREE.DoubleSide}
      />
    </mesh>
  );
}

export function MeshPreview() {
  const { store, state } = useKernelStore();
  const bodies = state?.meshedBodies;
  const features = state?.features;

  const materials = useMemo(() => {
    if (!store || !features) return new Map<number, MaterialProps>();
    return resolveMaterials(store.kernel, features);
  }, [store, features]);

  if (!bodies || bodies.length === 0) return null;

  return (
    <>
      <ambientLight intensity={0.5} />
      <directionalLight position={[200, 400, 300]} intensity={0.8} />
      <directionalLight position={[-100, 200, -200]} intensity={0.3} />
      {bodies.map((body, i) => (
        <MeshPreviewBody key={body.id} body={body} mat={materials.get(i)} bodyIndex={i} />
      ))}
    </>
  );
}

/**
 * Hybrid-Renderer, Phase 1 (`brep` render mode; docs/ADAPTIVE-RENDERING-PLAN.md
 * Achse 2): render every body whose analytic B-Rep surface is fully PLANAR
 * (boxes, prisms, planar boolean results) as an exact rasterized mesh — sharp
 * edges, exact normals, ZERO raymarching. Bodies with curved/implicit faces are
 * omitted here (they still show their analytic wireframe via BRepEdgeOverlay);
 * curved-face tessellation + SDF-residual compositing are the next phases.
 *
 * The triangulation is pure + unit-tested (`brepMesh.ts`); this component only
 * lifts it onto the GPU, rebuilding on the body fingerprint like the other
 * B-Rep overlays.
 */
export function BRepMeshPreview() {
  const { store } = useKernelStore();
  const fingerprint = useBodyFingerprint();

  const geometries = useMemo(() => {
    if (!store) return [] as { bodyIndex: number; geometry: THREE.BufferGeometry }[];
    let meshes;
    try {
      meshes = collectAnalyticBodyMeshes(store.kernel as unknown as BrepMeshKernel);
    } catch {
      return [];
    }
    return meshes.map((m) => {
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute("position", new THREE.BufferAttribute(m.positions, 3));
      geometry.setAttribute("normal", new THREE.BufferAttribute(m.normals, 3));
      geometry.computeBoundingSphere();
      return { bodyIndex: m.bodyIndex, geometry };
    });
    // fingerprint drives the rebuild; store identity is stable within a session.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, fingerprint]);

  useEffect(() => {
    return () => {
      for (const g of geometries) g.geometry.dispose();
    };
  }, [geometries]);

  if (geometries.length === 0) return null;

  return (
    <>
      <ambientLight intensity={0.5} />
      <directionalLight position={[200, 400, 300]} intensity={0.8} />
      <directionalLight position={[-100, 200, -200]} intensity={0.3} />
      {geometries.map(({ bodyIndex, geometry }) => (
        <mesh key={bodyIndex} geometry={geometry}>
          <meshStandardMaterial color="#94a3b8" side={THREE.DoubleSide} />
        </mesh>
      ))}
    </>
  );
}

/**
 * WebGL render path body meshes (mode `webgl`; docs/ADAPTIVE-RENDERING-PLAN.md
 * Achse 3). Renders each body's PARAMETRIC hull meshes — box/sphere/cylinder/
 * cone/torus/capsule + extrude/revolve via `createPrimitiveGeometry` — with
 * classic `MeshStandardMaterial`, so the scene draws on a plain WebGLRenderer
 * (no WebGPU, no node materials). Bodies are shown individually (no boolean
 * compositing yet — that is the screen-space-CSG phase); helix/twist parametric
 * meshes and the B-Rep tessellation slot in here later.
 */
export function WebGLBodies() {
  const { store, state } = useKernelStore();
  const fingerprint = useBodyFingerprint();
  const matRef = useRef<THREE.MeshStandardMaterial | null>(null);
  if (!matRef.current) {
    matRef.current = new THREE.MeshStandardMaterial({
      color: "#94a3b8",
      side: THREE.DoubleSide,
      metalness: 0.1,
      roughness: 0.7,
    });
  }

  const meshes = useMemo(() => {
    const out: THREE.Mesh[] = [];
    if (!store) return out;
    let count = 0;
    try {
      count = store.kernel.bodyCount();
    } catch {
      return out;
    }
    for (let i = 0; i < count; i++) {
      let info: { leaves: HybridLeafInfo[] };
      try {
        info = store.kernel.bodyHybridInfo(i);
      } catch {
        continue;
      }
      for (const leaf of info.leaves) {
        if (leaf.role === "skip") continue;
        if (leaf.warp) {
          // Twist/Bend: geometry is pre-warped into world space → identity.
          const geo = buildWarpedLeafGeometry(leaf);
          if (!geo) continue;
          out.push(new THREE.Mesh(geo, matRef.current!));
          continue;
        }
        const geo = createPrimitiveGeometry(leaf);
        if (!geo) continue;
        const mesh = new THREE.Mesh(geo, matRef.current!);
        applyLeafTransform(mesh, leaf);
        out.push(mesh);
      }
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, fingerprint, state?.features]);

  useEffect(() => {
    return () => {
      for (const m of meshes) m.geometry.dispose();
    };
  }, [meshes]);
  useEffect(() => () => matRef.current?.dispose(), []);

  if (meshes.length === 0) return null;
  return (
    <>
      <ambientLight intensity={0.5} />
      <directionalLight position={[200, 400, 300]} intensity={0.8} />
      <directionalLight position={[-100, 200, -200]} intensity={0.3} />
      {meshes.map((m, i) => (
        <primitive key={i} object={m} dispose={null} />
      ))}
    </>
  );
}

function AssemblyPartBody({ body, isSelected }: {
  body: { positions: Float32Array; normals: Float32Array; indices: Uint32Array };
  isSelected: boolean;
}) {
  const geometry = useMemo(() => {
    const geom = new THREE.BufferGeometry();
    geom.setAttribute("position", new THREE.BufferAttribute(body.positions, 3));
    geom.setAttribute("normal", new THREE.BufferAttribute(body.normals, 3));
    geom.setIndex(new THREE.BufferAttribute(body.indices, 1));
    geom.computeBoundingSphere();
    return geom;
  }, [body.positions, body.normals, body.indices]);

  useEffect(() => {
    return () => geometry.dispose();
  }, [geometry]);

  return (
    <mesh geometry={geometry}>
      <meshStandardMaterial
        color={isSelected ? "#6366f1" : "#94a3b8"}
        side={THREE.DoubleSide}
      />
    </mesh>
  );
}

export function AssemblyParts() {
  const { state } = useKernelStore();
  const assembly = state?.assembly;
  if (!assembly?.active || assembly.document.parts.length === 0) return null;

  return (
    <>
      {assembly.document.parts.map((part) => {
        const rot = part.transform.rotate.map((d: number) => (d * Math.PI) / 180) as [number, number, number];
        return (
          <group
            key={part.id}
            position={part.transform.translate}
            rotation={rot}
          >
            {part.meshBodies.map((body, i: number) => (
              <AssemblyPartBody
                key={`${part.id}-body-${i}`}
                body={body}
                isSelected={part.id === assembly.selectedPartId}
              />
            ))}
          </group>
        );
      })}
    </>
  );
}

export function MateConnectorOverlay() {
  const { store, state } = useKernelStore();
  const groupRef = useRef<THREE.Group>(null);

  useEffect(() => {
    const group = groupRef.current;
    if (!group || !state) return;
    while (group.children.length > 0) {
      const child = group.children[0];
      group.remove(child);
      if (child instanceof THREE.LineSegments) {
        child.geometry.dispose();
        (child.material as THREE.Material).dispose();
      }
    }
    if (!store) return;
    // C2: the kernel resolves each connector's CSYS from its face query
    // during regeneration; we just render the triads it hands back.
    const connectors = store.kernel.resolvedMateConnectors();
    if (connectors.length === 0) return;
    for (const c of connectors) {
      try {
        const origin = c.origin;
        const size = 15;

        // CSYS: Z = face normal, X = the kernel's in-plane basis vector,
        // Y = Z × X (right-handed).
        const nLen = Math.hypot(c.normal[0], c.normal[1], c.normal[2]) || 1;
        const nz = [c.normal[0] / nLen, c.normal[1] / nLen, c.normal[2] / nLen];
        const xLen = Math.hypot(c.x_axis[0], c.x_axis[1], c.x_axis[2]) || 1;
        const nx = [c.x_axis[0] / xLen, c.x_axis[1] / xLen, c.x_axis[2] / xLen];
        const ny = [
          nz[1] * nx[2] - nz[2] * nx[1],
          nz[2] * nx[0] - nz[0] * nx[2],
          nz[0] * nx[1] - nz[1] * nx[0],
        ];

        const axes: [number[], number][] = [
          [nx, 0xe74c3c],
          [ny, 0x2ecc71],
          [nz, 0x3498db],
        ];
        for (const [dir, color] of axes) {
          const positions = new Float32Array([
            origin[0], origin[1], origin[2],
            origin[0] + dir[0] * size,
            origin[1] + dir[1] * size,
            origin[2] + dir[2] * size,
          ]);
          const geo = new THREE.BufferGeometry();
          geo.setAttribute("position", new THREE.BufferAttribute(positions, 3));
          const mat = new THREE.LineBasicMaterial({ color });
          const line = new THREE.LineSegments(geo, mat);
          group.add(line);
        }
      } catch {
        // Skip if a connector can't be rendered.
      }
    }
    return () => {
      if (!group) return;
      while (group.children.length > 0) {
        const child = group.children[0];
        group.remove(child);
        if (child instanceof THREE.LineSegments) {
          child.geometry.dispose();
          (child.material as THREE.Material).dispose();
        }
      }
    };
  }, [store, state?.features]);

  return <group ref={groupRef} />;
}

const DATUM_PLANE_SIZE = 200;
const DATUM_PLANE_OPACITY = 0.08;
const DATUM_EDGE_OPACITY = 0.35;
const DATUM_COLOR = 0x3b82f6;

const DATUM_PLANES: {
  id: string;
  rotation: [number, number, number];
  label: string;
}[] = [
  { id: "front", rotation: [0, 0, 0],                     label: "Front" },
  { id: "top",   rotation: [-Math.PI / 2, 0, 0],          label: "Top" },
  { id: "right", rotation: [0, Math.PI / 2, 0],           label: "Right" },
];

const DATUM_LABEL_POS: [number, number, number] = [
  -DATUM_PLANE_SIZE / 2 + 22,
  DATUM_PLANE_SIZE / 2 - 10,
  0.5,
];

function makeLabelMaterial(text: string): THREE.MeshBasicMaterial {
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d")!;
  const font = "bold 64px sans-serif";
  ctx.font = font;
  const metrics = ctx.measureText(text);
  const w = Math.ceil(metrics.width) + 8;
  const h = 80;
  canvas.width = w;
  canvas.height = h;
  ctx.font = font;
  ctx.fillStyle = "#3b82f6";
  ctx.globalAlpha = 0.6;
  ctx.textBaseline = "top";
  ctx.fillText(text, 0, 4);
  const tex = new THREE.CanvasTexture(canvas);
  tex.minFilter = THREE.LinearFilter;
  tex.magFilter = THREE.LinearFilter;
  return new THREE.MeshBasicMaterial({ map: tex, transparent: true, depthTest: false, side: THREE.DoubleSide });
}

const datumLabelCache: Record<string, { material: THREE.MeshBasicMaterial; aspect: number }> = {};
function getDatumLabel(text: string) {
  if (!datumLabelCache[text]) {
    const mat = makeLabelMaterial(text);
    const aspect = (mat.map as THREE.CanvasTexture).image.width / (mat.map as THREE.CanvasTexture).image.height;
    datumLabelCache[text] = { material: mat, aspect };
  }
  return datumLabelCache[text];
}

const datumEdgesSource = new THREE.PlaneGeometry(DATUM_PLANE_SIZE, DATUM_PLANE_SIZE);
const datumEdgesGeometry = new THREE.EdgesGeometry(datumEdgesSource);
datumEdgesSource.dispose();

/** Pick radius (mm) around the origin marker — generously larger than the
 *  1.5 mm sphere so the origin stays clickable. */
const DATUM_ORIGIN_PICK_RADIUS = 6;

/**
 * Datum under a viewport ray (ticket r36f): "origin" when the ray passes
 * within {@link DATUM_ORIGIN_PICK_RADIUS} of the world origin, else the
 * front-most visible default plane the ray hits inside its rendered quad.
 * Returns null when the ray misses everything (or the datums are hidden).
 * The plane quads live on the world axes: front = z 0, top = y 0,
 * right = x 0, each ±DATUM_PLANE_SIZE/2.
 */
export function pickDatumAtRay(
  origin: THREE.Vector3,
  dir: THREE.Vector3,
  hidden: ReadonlySet<string>,
): string | null {
  const o = [origin.x, origin.y, origin.z];
  const d = [dir.x, dir.y, dir.z];
  if (!hidden.has("origin")) {
    const t = -(o[0] * d[0] + o[1] * d[1] + o[2] * d[2]);
    if (t > 0) {
      const dist = Math.hypot(o[0] + t * d[0], o[1] + t * d[1], o[2] + t * d[2]);
      if (dist <= DATUM_ORIGIN_PICK_RADIUS) return "origin";
    }
  }
  const half = DATUM_PLANE_SIZE / 2;
  const planes: { id: string; axis: number }[] = [
    { id: "front", axis: 2 },
    { id: "top", axis: 1 },
    { id: "right", axis: 0 },
  ];
  let best: string | null = null;
  let bestT = Infinity;
  for (const { id, axis } of planes) {
    if (hidden.has(id)) continue;
    if (Math.abs(d[axis]) < 1e-9) continue;
    const t = -o[axis] / d[axis];
    if (t <= 0 || t >= bestT) continue;
    const u = o[(axis + 1) % 3] + t * d[(axis + 1) % 3];
    const v = o[(axis + 2) % 3] + t * d[(axis + 2) % 3];
    if (Math.abs(u) > half || Math.abs(v) > half) continue;
    best = id;
    bestT = t;
  }
  return best;
}

export function DatumOverlay() {
  const { state } = useKernelStore();
  const hidden = state?.hiddenDatums ?? new Set<string>();
  const selectedDatum = state?.selectedDatum ?? null;
  const invalidate = useThree((s) => s.invalidate);

  const originVisible = !hidden.has("origin");
  const visiblePlanes = DATUM_PLANES.filter((p) => !hidden.has(p.id));

  useEffect(() => { invalidate(); }, [hidden, selectedDatum, invalidate]);

  if (!originVisible && visiblePlanes.length === 0) return null;

  return (
    <group>
      {originVisible && (
        <mesh>
          <sphereGeometry args={[1.5, 16, 16]} />
          <meshBasicMaterial
            color={selectedDatum === "origin" ? ELEMENT_SELECTION_COLOR : DATUM_COLOR}
            transparent
            opacity={selectedDatum === "origin" ? 1 : 0.6}
            depthTest={false}
          />
        </mesh>
      )}
      {visiblePlanes.map((plane) => (
        <group key={plane.id} rotation={plane.rotation}>
          <mesh>
            <planeGeometry args={[DATUM_PLANE_SIZE, DATUM_PLANE_SIZE]} />
            <meshBasicMaterial
              color={selectedDatum === plane.id ? ELEMENT_SELECTION_COLOR : DATUM_COLOR}
              transparent
              opacity={selectedDatum === plane.id ? DATUM_PLANE_OPACITY * 2 : DATUM_PLANE_OPACITY}
              side={THREE.DoubleSide}
              depthWrite={false}
            />
          </mesh>
          <lineSegments geometry={datumEdgesGeometry}>
            <lineBasicMaterial
              color={selectedDatum === plane.id ? ELEMENT_SELECTION_COLOR : DATUM_COLOR}
              transparent
              opacity={selectedDatum === plane.id ? 0.9 : DATUM_EDGE_OPACITY}
            />
          </lineSegments>
          <mesh
            position={DATUM_LABEL_POS}
            material={getDatumLabel(plane.label).material}
            scale={[12 * getDatumLabel(plane.label).aspect, 12, 1]}
          >
            <planeGeometry args={[1, 1]} />
          </mesh>
        </group>
      ))}
    </group>
  );
}

export function WorldGrid() {
  const grid = useMemo(() => {
    const g = new THREE.GridHelper(600, 60, 0x999999, 0xdddddd);
    g.position.y = -0.1;
    (g.material as THREE.Material).transparent = true;
    (g.material as THREE.Material).opacity = 0.5;
    return g;
  }, []);
  return <primitive object={grid} />;
}

function MeshOverlayEntry({ data }: { data: { positions: Float32Array; normals: Float32Array; indices: Uint32Array } }) {
  const geometry = useMemo(() => new THREE.BufferGeometry(), []);

  useEffect(() => {
    return () => { geometry.dispose(); };
  }, [geometry]);

  useEffect(() => {
    geometry.setAttribute("position", new THREE.BufferAttribute(data.positions, 3));
    geometry.setAttribute("normal", new THREE.BufferAttribute(data.normals, 3));
    geometry.setIndex(new THREE.BufferAttribute(data.indices, 1));
    geometry.computeBoundingBox();
    geometry.computeBoundingSphere();
  }, [geometry, data]);

  return (
    <mesh geometry={geometry}>
      <meshStandardMaterial
        color="#8888cc"
        transparent
        opacity={0.4}
        depthWrite={false}
        side={THREE.DoubleSide}
      />
    </mesh>
  );
}

export function ReferenceMesh() {
  const { store, state } = useKernelStore();

  const overlays = useMemo(() => {
    if (!store || !state) return [];
    return store.getMeshOverlayFeatures();
  }, [store, state?.features]);

  const refMesh = state?.referenceMesh;

  if (overlays.length === 0 && !refMesh) return null;

  return (
    <>
      {refMesh ? <MeshOverlayEntry data={refMesh} /> : null}
      {overlays.map((o) => (
        <MeshOverlayEntry key={o.id} data={o.data} />
      ))}
    </>
  );
}

// Invisible pick-state probe. The on-screen face/edge id readout was a
// debug overlay and is gone; this element stays in the DOM (visually
// hidden) only as a programmatic hook for the hover-pick e2e smoke test.
export function PickHud({ result }: { result: PickResult | null }) {
  return (
    <div
      className="pick-hud"
      data-testid="fielddraft-pick-hud"
      data-pick-hit={result?.hit ? "true" : "false"}
      aria-hidden="true"
    />
  );
}

export { PerfProbe };
