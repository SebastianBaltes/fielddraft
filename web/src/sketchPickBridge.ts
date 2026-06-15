/**
 * Committed-sketch pick bridge.
 *
 * `store.kernel.pick()` only knows the analytic B-Rep bodies, so a click on a
 * committed sketch's filled region (or its outline) reports `hit:false` and the
 * Viewport's click handler would otherwise fall through to the datum-plane
 * fallback — selecting the "Front" plane that sits BEHIND the sketch instead of
 * the sketch itself (ticket md1e). `SketchOverlay3D` already builds the
 * world-space fill triangles + outline segments for every visible committed
 * sketch; it publishes them here so the click handler can ray-cast against the
 * sketch geometry on a plain click and select the sketch feature, taking
 * priority OVER datum planes.
 *
 * A plain module-scoped singleton, exactly like `brepPickBridge` /
 * `queryPickerBridge` — the overlay and the viewport need cheap synchronous
 * access without threading React state through R3F's Canvas boundary.
 */

import * as THREE from "three";

/** One pickable committed sketch: its fill triangles and outline segments,
 *  both flat world-space position arrays (xyz triplets). */
export interface SketchPickable {
  sketchId: number;
  /** Flat triangle vertices (9 floats per triangle), or `null` for an
   *  outline-only sketch without a closed region. World-space. */
  fill: Float32Array | null;
  /** Flat line-segment endpoints (pairs of xyz), world-space. */
  lines: Float32Array | null;
}

let pickables: SketchPickable[] = [];

/** Replace the set of pickable committed sketches. */
export function publishSketchPickables(items: SketchPickable[]): void {
  pickables = items;
}

/** Drop all pickables (overlay unmount / no visible sketches). */
export function clearSketchPickables(): void {
  pickables = [];
}

// Outline proximity band for outline-only sketches (no closed region): a click
// within this world-space distance of an outline segment hits the sketch. Fill
// triangles are the priority — this is the fallback for open profiles.
const LINE_PICK_WORLD_RADIUS = 0.5;

/**
 * Nearest committed sketch the ray crosses, or `null`. Fill triangles win by
 * true ray–triangle distance; outline-only sketches fall back to ray-vs-segment
 * proximity. Returns the closest hit by distance along the ray.
 */
export function pickSketchAtRay(
  ray: THREE.Ray,
): { sketchId: number; distance: number } | null {
  let best: { sketchId: number; distance: number } | null = null;
  const a = new THREE.Vector3();
  const b = new THREE.Vector3();
  const c = new THREE.Vector3();
  const hit = new THREE.Vector3();
  for (const p of pickables) {
    if (p.fill && p.fill.length >= 9) {
      const f = p.fill;
      for (let i = 0; i + 8 < f.length; i += 9) {
        a.set(f[i], f[i + 1], f[i + 2]);
        b.set(f[i + 3], f[i + 4], f[i + 5]);
        c.set(f[i + 6], f[i + 7], f[i + 8]);
        // Sketch fills are double-sided (`THREE.DoubleSide`), so don't cull
        // by winding — a sketch is pickable from either side.
        if (ray.intersectTriangle(a, b, c, false, hit)) {
          const d = ray.origin.distanceTo(hit);
          if (!best || d < best.distance) best = { sketchId: p.sketchId, distance: d };
        }
      }
    } else if (p.lines && p.lines.length >= 6) {
      // Outline-only fallback: closest approach of the ray to each segment.
      const l = p.lines;
      for (let i = 0; i + 5 < l.length; i += 6) {
        a.set(l[i], l[i + 1], l[i + 2]);
        b.set(l[i + 3], l[i + 4], l[i + 5]);
        const d = raySegmentDistance(ray, a, b, hit);
        if (d <= LINE_PICK_WORLD_RADIUS) {
          const along = ray.origin.distanceTo(hit);
          if (!best || along < best.distance) best = { sketchId: p.sketchId, distance: along };
        }
      }
    }
  }
  return best;
}

/** Shortest distance between a ray and a segment; writes the point ON the
 *  segment closest to the ray into `out`. */
function raySegmentDistance(
  ray: THREE.Ray,
  s0: THREE.Vector3,
  s1: THREE.Vector3,
  out: THREE.Vector3,
): number {
  const u = ray.direction; // unit length
  const v = new THREE.Vector3().subVectors(s1, s0);
  const w0 = new THREE.Vector3().subVectors(ray.origin, s0);
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
    tc = e / cc;
  }
  if (sc < 0) sc = 0; // ray, not a line — never behind the origin
  tc = Math.min(1, Math.max(0, tc));
  out.copy(s0).addScaledVector(v, tc);
  const onRay = new THREE.Vector3().copy(ray.origin).addScaledVector(u, sc);
  return onRay.distanceTo(out);
}
