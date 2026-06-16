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

// A click within this world-space distance of a sketch line's segment
// counts as a hit. Matches the outline proximity band used by the
// whole-sketch fallback in `sketchPickBridge`.
const LINE_PICK_WORLD_RADIUS = 0.5;

/**
 * Nearest sketch line the ray passes within {@link LINE_PICK_WORLD_RADIUS}
 * of, as `{ sketchId, lineId }`, or `null`. Closest hit by distance along
 * the ray wins.
 */
export function pickSketchLineAtRay(
  ray: THREE.Ray,
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
      if (d > LINE_PICK_WORLD_RADIUS) continue;
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
