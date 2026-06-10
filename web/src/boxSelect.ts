/**
 * Rectangle ("box") selection (ticket onshape-artiges-select-rectangle).
 *
 * A drag from empty viewport space spans a screen rectangle; this module
 * resolves which B-Rep entities it selects. Two industry-standard modes
 * (Onshape "Selection", Fusion "Selection in Fusion" docs):
 *   - "window": only entities COMPLETELY inside the rectangle;
 *   - "crossing": additionally entities merely touching it.
 *
 * Geometry is tested in NDC: edges by their dense tessellation polylines
 * (every point inside = contained; any point inside = crossing — the
 * polylines are dense enough that the approximation is exact for practical
 * rectangles), vertices by their single point. Object-mode presets select
 * whole bodies instead; bodies without any B-Rep edge (a bare sphere) fall
 * back to their world AABB corners.
 *
 * Pure logic — the camera projection is injected as a callback so tests run
 * without three.js.
 */

import type { BRepEdge, BRepVertex, Placement } from "./kernel";
import { localToWorld } from "./kernel";
import type { PickedElement } from "./elementQuery";

/** Screen rectangle in NDC, normalized (min ≤ max). */
export interface NdcRect {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

export type BoxSelectMode = "window" | "crossing";

/** Projects a world point to NDC; null when behind the camera / unusable. */
export type ProjectFn = (
  x: number,
  y: number,
  z: number,
) => { x: number; y: number } | null;

/** The kernel slice box selection reads. */
export interface BoxSelectKernel {
  bodyCount(): number;
  bodyBrepEdges(i: number): BRepEdge[];
  bodyBrepVertices(i: number): BRepVertex[];
  bodyPlacement(i: number): Placement;
  bodySdfAabb(i: number): Float32Array;
}

const inside = (rect: NdcRect, p: { x: number; y: number }): boolean =>
  p.x >= rect.minX && p.x <= rect.maxX && p.y >= rect.minY && p.y <= rect.maxY;

/** Classify a flat [x,y,z,…] world polyline against the rectangle.
 *  Returns "all" / "some" / "none" points inside (off-screen points count
 *  as outside). */
function classifyPolyline(
  points: ArrayLike<number>,
  rect: NdcRect,
  project: ProjectFn,
): "all" | "some" | "none" {
  let anyIn = false;
  let anyOut = false;
  for (let i = 0; i + 2 < points.length; i += 3) {
    const p = project(points[i], points[i + 1], points[i + 2]);
    if (p !== null && inside(rect, p)) anyIn = true;
    else anyOut = true;
    if (anyIn && anyOut) return "some";
  }
  if (!anyIn) return "none";
  return anyOut ? "some" : "all";
}

const matches = (cls: "all" | "some" | "none", mode: BoxSelectMode): boolean =>
  cls === "all" || (mode === "crossing" && cls === "some");

/**
 * B-Rep edges and vertices the rectangle selects (element-mode presets).
 * Window mode keeps only fully-contained entities, crossing mode also the
 * partially covered ones.
 */
export function elementsInRect(
  kernel: BoxSelectKernel,
  project: ProjectFn,
  rect: NdcRect,
  mode: BoxSelectMode,
): PickedElement[] {
  const out: PickedElement[] = [];
  const count = kernel.bodyCount();
  for (let i = 0; i < count; i++) {
    let edges: BRepEdge[];
    let vertices: BRepVertex[];
    let placement: Placement;
    try {
      edges = kernel.bodyBrepEdges(i);
      vertices = kernel.bodyBrepVertices(i);
      placement = kernel.bodyPlacement(i);
    } catch {
      continue;
    }
    for (const e of edges) {
      // Edge polylines are already world space (see BRepEdge docs).
      if (e.idHash === undefined || e.points.length < 6) continue;
      if (matches(classifyPolyline(e.points, rect, project), mode)) {
        out.push({ kind: "edge", idHash: e.idHash });
      }
    }
    for (const v of vertices) {
      const w = localToWorld(v.positionLocal, placement);
      const p = project(w[0], w[1], w[2]);
      if (p !== null && inside(rect, p)) {
        out.push({ kind: "vertex", idHash: v.idHash });
      }
    }
  }
  return out;
}

/**
 * Body indices the rectangle selects (object-mode presets, e.g. TinkerCAD).
 * A body counts by its complete edge set; edge-free bodies (bare sphere /
 * ellipsoid) fall back to the 8 corners of their world AABB.
 */
export function bodiesInRect(
  kernel: BoxSelectKernel,
  project: ProjectFn,
  rect: NdcRect,
  mode: BoxSelectMode,
): number[] {
  const out: number[] = [];
  const count = kernel.bodyCount();
  for (let i = 0; i < count; i++) {
    let edges: BRepEdge[];
    try {
      edges = kernel.bodyBrepEdges(i);
    } catch {
      continue;
    }
    let points: number[] = [];
    for (const e of edges) points = points.concat(e.points);
    if (points.length < 3) {
      let aabb: Float32Array;
      try {
        aabb = kernel.bodySdfAabb(i);
      } catch {
        continue;
      }
      if (aabb.length < 6) continue;
      for (const x of [aabb[0], aabb[3]])
        for (const y of [aabb[1], aabb[4]])
          for (const z of [aabb[2], aabb[5]]) points.push(x, y, z);
    }
    if (matches(classifyPolyline(points, rect, project), mode)) out.push(i);
  }
  return out;
}
