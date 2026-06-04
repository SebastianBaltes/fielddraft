/**
 * Pure hit-testing for select mode in the 2D sketch editor.
 *
 * Given the rebuilt solver `SketchState` and a cursor position in
 * sketch-plane millimetres, return the id of the committed entity
 * under the cursor — or `null` when nothing is within tolerance.
 *
 * Priority: points (vertices, centres) win over curves, because a
 * vertex is a precise target the user means to pick when they click
 * near a line end. Among curves the nearest within tolerance wins.
 * Keeping this logic free of canvas / camera state lets Vitest
 * exercise every branch without a DOM.
 */

import type { Entity, SketchState } from "./solver/types";

function entityPoint(state: SketchState, id: number): [number, number] | null {
  const e = state.entities.find((x) => x.id === id);
  if (!e) return null;
  if (e.kind === "point") return [e.at.x, e.at.y];
  if (e.kind === "origin") return [0, 0];
  return null;
}

function dist(a: [number, number], b: [number, number]): number {
  return Math.hypot(a[0] - b[0], a[1] - b[1]);
}

/** Shortest distance from `p` to the segment `a`–`b`. */
function distToSegment(
  p: [number, number],
  a: [number, number],
  b: [number, number],
): number {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const lenSq = dx * dx + dy * dy;
  if (lenSq < 1e-12) return dist(p, a);
  let t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / lenSq;
  t = Math.max(0, Math.min(1, t));
  return dist(p, [a[0] + t * dx, a[1] + t * dy]);
}

/**
 * Hit-test a cursor against committed sketch geometry. Returns the
 * picked entity id or `null`. `tolMm` is the pick radius in
 * sketch-plane millimetres (the caller translates pixels → mm via
 * the camera zoom).
 */
export function hitTestSketchEntity(
  state: SketchState,
  cursor: [number, number],
  tolMm: number,
): number | null {
  // Pass 1 — points and the origin take priority.
  let bestPointId: number | null = null;
  let bestPointDist = tolMm;
  for (const e of state.entities) {
    if (e.kind !== "point" && e.kind !== "origin") continue;
    const at: [number, number] = e.kind === "origin" ? [0, 0] : [e.at.x, e.at.y];
    const d = dist(cursor, at);
    if (d < bestPointDist) {
      bestPointDist = d;
      bestPointId = e.id;
    }
  }
  if (bestPointId !== null) return bestPointId;

  // Pass 2 — curves (lines, circles, arcs).
  let bestId: number | null = null;
  let bestDist = tolMm;
  for (const e of state.entities) {
    const d = curveDistance(state, e, cursor);
    if (d !== null && d < bestDist) {
      bestDist = d;
      bestId = e.id;
    }
  }
  return bestId;
}

function curveDistance(
  state: SketchState,
  e: Entity,
  cursor: [number, number],
): number | null {
  if (e.kind === "line") {
    const a = entityPoint(state, e.start);
    const b = entityPoint(state, e.end);
    if (!a || !b) return null;
    return distToSegment(cursor, a, b);
  }
  if (e.kind === "circle") {
    const c = entityPoint(state, e.center);
    if (!c) return null;
    return Math.abs(dist(cursor, c) - e.radius);
  }
  if (e.kind === "arc") {
    const c = entityPoint(state, e.center);
    if (!c) return null;
    // Best-effort: distance to the full circle the arc lies on.
    // Angular clamping is refined once arcs become drawable.
    return Math.abs(dist(cursor, c) - e.radius);
  }
  return null;
}
