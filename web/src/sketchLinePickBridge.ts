/**
 * Sketch-line axis-pick bridge.
 *
 * The Revolve tool's `axis` query accepts a sketch line (`qSketchLine`) as
 * well as a B-Rep edge (`qEdge`). B-Rep edges already flow through
 * `brepPickBridge` / `elementQuery`, but committed sketch lines are not
 * B-Rep elements — they live only as 2D sub-calls on a sketch plane. This
 * bridge publishes their per-line world-space segments (tagged with the
 * shared entity id) so the Viewport's query-pick handler can ray-cast
 * against them when an axis picker is active and emit
 * `qSketchLine(sketchId, lineId)` for the nearest line.
 *
 * A plain module-scoped singleton, exactly like `sketchPickBridge` /
 * `queryPickerBridge` — the overlay publishes, the viewport consumes,
 * neither threading React state through R3F's Canvas boundary.
 */

import * as THREE from "three";
import type { SketchLinePickable } from "./sketchOutline";

/** All pickable sketch lines of one committed sketch. */
export interface SketchLinePickables {
  sketchId: number;
  lines: SketchLinePickable[];
}

let pickables: SketchLinePickables[] = [];

/** Replace the set of pickable sketch lines. */
export function publishSketchLinePickables(items: SketchLinePickables[]): void {
  pickables = items;
}

/** Drop all pickables (overlay unmount / no visible sketches). */
export function clearSketchLinePickables(): void {
  pickables = [];
}

/** Read the current pickable set (overlay highlight uses it to find the
 *  hovered line's world segment). */
export function getSketchLinePickables(): SketchLinePickables[] {
  return pickables;
}

// ─── hover highlight (ticket fs23) ──────────────────────────────────
// While a query field that accepts sketch lines is armed, the Viewport's
// pointer-move publishes the sketch line under the cursor here so the
// committed-sketch overlay can highlight it — the "this edge is a potential
// target" affordance the ticket asks for. A plain module singleton + listener
// set, exactly like the pick bridges around it.

let hovered: { sketchId: number; lineId: number } | null = null;
const hoverListeners = new Set<() => void>();

/** The sketch line currently hovered as a query target, or `null`. */
export function getHoveredSketchLine(): {
  sketchId: number;
  lineId: number;
} | null {
  return hovered;
}

/** Publish (or clear with `null`) the hovered sketch line. No-op — and no
 *  notify — when the value is unchanged, so pointer-move doesn't thrash. */
export function setHoveredSketchLine(
  next: { sketchId: number; lineId: number } | null,
): void {
  const same =
    (next === null && hovered === null) ||
    (next !== null &&
      hovered !== null &&
      next.sketchId === hovered.sketchId &&
      next.lineId === hovered.lineId);
  if (same) return;
  hovered = next;
  for (const l of hoverListeners) l();
}

/** Subscribe to hovered-sketch-line changes. Returns unsubscribe. */
export function subscribeHoveredSketchLine(listener: () => void): () => void {
  hoverListeners.add(listener);
  return () => {
    hoverListeners.delete(listener);
  };
}

// Screen-space pick tolerance: a click within this many pixels of a sketch
// line counts as a hit. A FIXED world radius (the old 0.5 mm) is unusable on
// large models — at metre scale 0.5 mm is sub-pixel — so the tolerance is
// converted to a per-segment WORLD radius from the camera at click time
// (ticket fs23). Mirrors `SELECT_PICK_PX` in the 2D sketch canvas.
const LINE_PICK_PX = 8;

/**
 * World-space radius that subtends {@link LINE_PICK_PX} pixels at `point` for
 * the given camera — so the pick tolerance is constant in SCREEN space
 * regardless of zoom or model scale. Handles both perspective and
 * orthographic cameras; falls back to a small constant without a viewport
 * height.
 */
function worldPickRadius(
  camera: THREE.Camera,
  point: THREE.Vector3,
  viewportHeightPx: number,
): number {
  if (viewportHeightPx <= 0) return 0.5;
  const persp = camera as THREE.PerspectiveCamera;
  if (persp.isPerspectiveCamera) {
    const dist = camera.position.distanceTo(point);
    const worldPerPx =
      (2 * Math.tan((persp.fov * Math.PI) / 360) * dist) / viewportHeightPx;
    return LINE_PICK_PX * worldPerPx;
  }
  const ortho = camera as THREE.OrthographicCamera;
  if (ortho.isOrthographicCamera) {
    const worldPerPx =
      (ortho.top - ortho.bottom) / (ortho.zoom || 1) / viewportHeightPx;
    return LINE_PICK_PX * worldPerPx;
  }
  return 0.5;
}

/**
 * Nearest sketch line the ray passes within a screen-constant tolerance of,
 * as `{ sketchId, lineId }`, or `null`. Closest hit by distance along the ray
 * wins. `camera`/`viewportHeightPx` make the tolerance zoom-independent; both
 * are optional so older callers keep a sane fixed fallback.
 */
export function pickSketchLineAtRay(
  ray: THREE.Ray,
  camera?: THREE.Camera,
  viewportHeightPx = 0,
): { sketchId: number; lineId: number } | null {
  let best: { sketchId: number; lineId: number; along: number } | null = null;
  const s0 = new THREE.Vector3();
  const s1 = new THREE.Vector3();
  const out = new THREE.Vector3();
  for (const sketch of pickables) {
    for (const line of sketch.lines) {
      s0.set(line.a[0], line.a[1], line.a[2]);
      s1.set(line.b[0], line.b[1], line.b[2]);
      const d = raySegmentDistance(ray, s0, s1, out);
      const radius = camera
        ? worldPickRadius(camera, out, viewportHeightPx)
        : 0.5;
      if (d > radius) continue;
      const along = ray.origin.distanceTo(out);
      if (!best || along < best.along) {
        best = { sketchId: sketch.sketchId, lineId: line.lineId, along };
      }
    }
  }
  return best ? { sketchId: best.sketchId, lineId: best.lineId } : null;
}

/** Shortest distance between a ray and a segment; writes the point ON the
 *  segment closest to the ray into `out`. (Mirror of the helper in
 *  `sketchPickBridge`.) */
function raySegmentDistance(
  ray: THREE.Ray,
  p0: THREE.Vector3,
  p1: THREE.Vector3,
  out: THREE.Vector3,
): number {
  const u = ray.direction; // unit length
  const v = new THREE.Vector3().subVectors(p1, p0);
  const w0 = new THREE.Vector3().subVectors(ray.origin, p0);
  const a = u.dot(u);
  const b = u.dot(v);
  const cc = v.dot(v);
  const d = u.dot(w0);
  const e = v.dot(w0);
  const denom = a * cc - b * b;
  let sc = 0; // along ray
  let tc = 0; // along segment, clamped to [0,1]
  if (denom > 1e-9) {
    sc = (b * e - cc * d) / denom;
    tc = (a * e - b * d) / denom;
  } else {
    tc = cc > 1e-9 ? e / cc : 0;
  }
  if (sc < 0) sc = 0; // ray, not a line — never behind the origin
  tc = Math.min(1, Math.max(0, tc));
  out.copy(p0).addScaledVector(v, tc);
  const onRay = new THREE.Vector3().copy(ray.origin).addScaledVector(u, sc);
  return onRay.distanceTo(out);
}
