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
import { bsplinePoints } from "./sketchSpline";
import { curveOfSubCall, distToCurve, type SubCall, type Vec2 } from "./sketchTrim";
import { fdIdOf, filletRecipeOf } from "./sketchFillet";
import { chamferRecipeOf } from "./sketchChamfer";

/** Sample a B-spline's drawn polyline from its pole entities, or null
 *  if any pole is missing. Reuses the same sampler the canvas renders
 *  with, so the pick test matches what the user sees. */
function bsplinePolyline(
  state: SketchState,
  e: Extract<Entity, { kind: "bspline" }>,
): [number, number][] | null {
  const poles: [number, number][] = [];
  for (const id of e.poles) {
    const p = entityPoint(state, id);
    if (!p) return null;
    poles.push(p);
  }
  return bsplinePoints(poles, e.degree, e.periodic, 64);
}

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

/**
 * Ids of every ellipse focus point. Focus points are internal
 * construction geometry — the solver moves them to reshape the
 * ellipse, but the user must never pick or box-select them, so both
 * hit-test passes skip these ids.
 */
function ellipseFocusIds(state: SketchState): Set<number> {
  const ids = new Set<number>();
  for (const e of state.entities) {
    if (e.kind === "ellipse") ids.add(e.focus1);
  }
  return ids;
}

/**
 * Shortest distance from `cursor` to an ellipse outline, sampled.
 * The ellipse is reconstructed from its centre, focus, and semi-minor
 * radius: the major axis points centre→focus with semi-major
 * `a = hypot(radmin, |centre − focus|)`; the minor axis is `radmin`
 * perpendicular to it. Sampling 64 points is ample for a pick test.
 */
function ellipseOutlineDistance(
  center: [number, number],
  focus: [number, number],
  radmin: number,
  cursor: [number, number],
): number {
  const cdx = focus[0] - center[0];
  const cdy = focus[1] - center[1];
  const c = Math.hypot(cdx, cdy);
  const ux = c > 1e-9 ? cdx / c : 1;
  const uy = c > 1e-9 ? cdy / c : 0;
  const a = Math.sqrt(radmin * radmin + c * c);
  let best = Infinity;
  const STEPS = 64;
  for (let i = 0; i < STEPS; i++) {
    const t = (i / STEPS) * Math.PI * 2;
    const ca = a * Math.cos(t);
    const cb = radmin * Math.sin(t);
    // major axis û, minor axis v̂ = (−uy, ux).
    const px = center[0] + ca * ux + cb * -uy;
    const py = center[1] + ca * uy + cb * ux;
    const d = dist(cursor, [px, py]);
    if (d < best) best = d;
  }
  return best;
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
  const focusIds = ellipseFocusIds(state);

  // Pass 1 — points and the origin take priority.
  let bestPointId: number | null = null;
  let bestPointDist = tolMm;
  for (const e of state.entities) {
    if (e.kind !== "point" && e.kind !== "origin") continue;
    if (focusIds.has(e.id)) continue; // internal ellipse focus
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

/**
 * Plan item 4: a clicked, re-editable corner. `kind` selects the
 * editor; `subCallId` is the stable `fd_id` of the recipe-tagged
 * sub-call (passed to `store.setFilletRadiusAt` / `setChamferDistanceAt`);
 * `value` is the current radius / distance to pre-fill; `anchor` is the
 * sketch-plane `(u, v)` to position the editor at (arc midpoint for a
 * fillet, bevel midpoint for a chamfer).
 */
export interface TaggedCornerHit {
  kind: "fillet" | "chamfer";
  subCallId: string;
  value: number;
  anchor: Vec2;
}

/**
 * Hit-test a Select-mode click against re-editable baked corners — the
 * `sketch_arc`s carrying a `fillet` recipe and the `sketch_line`s
 * carrying a `chamfer` recipe (see `sketchFillet` / `sketchChamfer`).
 *
 * Unlike [`hitTestSketchEntity`], this reads the SUB-CALL geometry
 * directly (`curveOfSubCall` + `distToCurve`): a fillet arc isn't a
 * picked solver entity at all (the draft state only allocates its centre
 * point), so an entity-based pick would never reach it. The nearest
 * tagged corner within `tolMm` wins; ties resolve to the later sub-call
 * (the corner drawn last sits on top). Returns null when no tagged
 * corner is under the cursor — the caller then falls back to the normal
 * entity hit-test.
 */
export function hitTestTaggedCorner(
  subCalls: ReadonlyArray<SubCall>,
  cursor: Vec2,
  tolMm: number,
): TaggedCornerHit | null {
  let best: TaggedCornerHit | null = null;
  let bestDist = tolMm;
  for (const call of subCalls) {
    const id = fdIdOf(call);
    if (id === null) continue;
    const cv = curveOfSubCall(call);
    if (!cv) continue;

    const fillet = filletRecipeOf(call);
    if (fillet && cv.kind === "arc") {
      const d = distToCurve(cursor, cv);
      if (d <= bestDist) {
        const a = cv.start + cv.sweep * 0.5;
        bestDist = d;
        best = {
          kind: "fillet",
          subCallId: id,
          value: fillet.radius,
          anchor: [cv.c[0] + cv.r * Math.cos(a), cv.c[1] + cv.r * Math.sin(a)],
        };
      }
      continue;
    }

    const chamfer = chamferRecipeOf(call);
    if (chamfer && cv.kind === "line") {
      const d = distToCurve(cursor, cv);
      if (d <= bestDist) {
        bestDist = d;
        best = {
          kind: "chamfer",
          subCallId: id,
          value: chamfer.distance,
          anchor: [(cv.a[0] + cv.b[0]) / 2, (cv.a[1] + cv.b[1]) / 2],
        };
      }
    }
  }
  return best;
}

/** Is the point `p` inside (or on the border of) the axis-aligned box? */
function pointInBox(
  p: [number, number],
  min: [number, number],
  max: [number, number],
): boolean {
  return p[0] >= min[0] && p[0] <= max[0] && p[1] >= min[1] && p[1] <= max[1];
}

/**
 * Does the segment `a`–`b` intersect (or lie inside) the axis-aligned
 * box `min`/`max`? Returns true if either endpoint is inside the box,
 * or if the segment crosses any of the four box edges. A segment whose
 * endpoints straddle the box but whose body passes through it is caught
 * by the edge-crossing test.
 */
function segmentIntersectsBox(
  a: [number, number],
  b: [number, number],
  min: [number, number],
  max: [number, number],
): boolean {
  if (pointInBox(a, min, max) || pointInBox(b, min, max)) return true;
  const corners: [number, number][] = [
    [min[0], min[1]],
    [max[0], min[1]],
    [max[0], max[1]],
    [min[0], max[1]],
  ];
  for (let i = 0; i < 4; i++) {
    if (segmentsIntersect(a, b, corners[i], corners[(i + 1) % 4])) return true;
  }
  return false;
}

/** Standard orientation-based segment-segment intersection test. */
function segmentsIntersect(
  p1: [number, number],
  p2: [number, number],
  p3: [number, number],
  p4: [number, number],
): boolean {
  const d = (
    a: [number, number],
    b: [number, number],
    c: [number, number],
  ): number => (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
  const d1 = d(p3, p4, p1);
  const d2 = d(p3, p4, p2);
  const d3 = d(p1, p2, p3);
  const d4 = d(p1, p2, p4);
  if (
    ((d1 > 0 && d2 < 0) || (d1 < 0 && d2 > 0)) &&
    ((d3 > 0 && d4 < 0) || (d3 < 0 && d4 > 0))
  ) {
    return true;
  }
  // Collinear-overlap cases: a zero determinant with the point on the
  // segment's span counts as touching.
  const onSeg = (
    a: [number, number],
    b: [number, number],
    c: [number, number],
  ): boolean =>
    Math.min(a[0], b[0]) <= c[0] &&
    c[0] <= Math.max(a[0], b[0]) &&
    Math.min(a[1], b[1]) <= c[1] &&
    c[1] <= Math.max(a[1], b[1]);
  if (d1 === 0 && onSeg(p3, p4, p1)) return true;
  if (d2 === 0 && onSeg(p3, p4, p2)) return true;
  if (d3 === 0 && onSeg(p1, p2, p3)) return true;
  if (d4 === 0 && onSeg(p1, p2, p4)) return true;
  return false;
}

/**
 * Box-select / marquee hit-testing. Returns the ids of every committed
 * entity matching the axis-aligned box (`min`/`max` in sketch-plane
 * millimetres) under the given mode:
 *
 *  - "enclosing" — the entity must lie *entirely* inside the box
 *    (left→right drag, the Onshape/FreeCAD "window" select).
 *  - "crossing"  — the entity merely needs to touch the box
 *    (right→left drag).
 *
 * Per-kind rules:
 *  - point/origin: selected iff the point is inside the box (both
 *    modes). The origin is treated like any other point so the
 *    behaviour is consistent and testable. Border is inclusive.
 *  - line: enclosing = both endpoints inside; crossing = the segment
 *    intersects or lies inside the box.
 *  - circle/arc: enclosing = the whole circle's bounding square is
 *    inside the box. Crossing is APPROXIMATED as "the box overlaps the
 *    circle's bounding square" — i.e. an AABB-vs-AABB overlap on the
 *    circle's [c±r] square. This over-selects circles whose corner
 *    region near the box is empty (the box clips the bounding square
 *    but not the ring), but it never under-selects and needs no
 *    angular ring math. Arcs reuse the same full-circle bounding
 *    square; refining to the swept extent is a later slice.
 *
 * A zero-area or inverted box selects nothing.
 */
export function entitiesInBox(
  state: SketchState,
  min: [number, number],
  max: [number, number],
  mode: "enclosing" | "crossing",
): number[] {
  if (max[0] <= min[0] || max[1] <= min[1]) return [];

  const pointAt = (id: number): [number, number] | null => {
    const e = state.entities.find((x) => x.id === id);
    if (!e) return null;
    if (e.kind === "point") return [e.at.x, e.at.y];
    if (e.kind === "origin") return [0, 0];
    return null;
  };

  const focusIds = ellipseFocusIds(state);

  const result: number[] = [];
  for (const e of state.entities) {
    // The origin is fixed reference geometry, not user-drawn — a marquee
    // shouldn't sweep it up (you'd rarely mean to constrain it), so unlike
    // `hitTestSketchEntity` (a deliberate click) box-select skips it.
    if (e.kind === "origin") continue;
    if (focusIds.has(e.id)) continue; // internal ellipse focus
    if (e.kind === "point") {
      if (pointInBox([e.at.x, e.at.y], min, max)) result.push(e.id);
    } else if (e.kind === "ellipse") {
      const c = pointAt(e.center);
      const f = pointAt(e.focus1);
      if (!c || !f) continue;
      // Axis-aligned bounding box of the (possibly rotated) ellipse.
      const cx = f[0] - c[0];
      const cy = f[1] - c[1];
      const cc = Math.hypot(cx, cy);
      const ux = cc > 1e-9 ? cx / cc : 1;
      const uy = cc > 1e-9 ? cy / cc : 0;
      const a = Math.sqrt(e.radmin * e.radmin + cc * cc);
      const hx = Math.hypot(a * ux, e.radmin * uy);
      const hy = Math.hypot(a * uy, e.radmin * ux);
      const bbMin: [number, number] = [c[0] - hx, c[1] - hy];
      const bbMax: [number, number] = [c[0] + hx, c[1] + hy];
      if (mode === "enclosing") {
        if (
          bbMin[0] >= min[0] &&
          bbMax[0] <= max[0] &&
          bbMin[1] >= min[1] &&
          bbMax[1] <= max[1]
        ) {
          result.push(e.id);
        }
      } else {
        const overlap =
          min[0] <= bbMax[0] &&
          max[0] >= bbMin[0] &&
          min[1] <= bbMax[1] &&
          max[1] >= bbMin[1];
        if (overlap) result.push(e.id);
      }
    } else if (e.kind === "line") {
      const a = pointAt(e.start);
      const b = pointAt(e.end);
      if (!a || !b) continue;
      if (mode === "enclosing") {
        if (pointInBox(a, min, max) && pointInBox(b, min, max)) result.push(e.id);
      } else {
        if (segmentIntersectsBox(a, b, min, max)) result.push(e.id);
      }
    } else if (e.kind === "bspline") {
      // Box-test against the drawn polyline's bounding box (the curve
      // lies within the convex hull of its poles, so the sampled bbox
      // is a faithful, slightly conservative proxy — same approach as
      // circle/ellipse).
      const poly = bsplinePolyline(state, e);
      if (!poly || poly.length === 0) continue;
      let bbMinX = Infinity;
      let bbMinY = Infinity;
      let bbMaxX = -Infinity;
      let bbMaxY = -Infinity;
      for (const [px, py] of poly) {
        if (px < bbMinX) bbMinX = px;
        if (py < bbMinY) bbMinY = py;
        if (px > bbMaxX) bbMaxX = px;
        if (py > bbMaxY) bbMaxY = py;
      }
      if (mode === "enclosing") {
        if (
          bbMinX >= min[0] &&
          bbMaxX <= max[0] &&
          bbMinY >= min[1] &&
          bbMaxY <= max[1]
        ) {
          result.push(e.id);
        }
      } else {
        const overlap =
          min[0] <= bbMaxX &&
          max[0] >= bbMinX &&
          min[1] <= bbMaxY &&
          max[1] >= bbMinY;
        if (overlap) result.push(e.id);
      }
    } else if (e.kind === "circle" || e.kind === "arc") {
      const c = pointAt(e.center);
      if (!c) continue;
      const bbMin: [number, number] = [c[0] - e.radius, c[1] - e.radius];
      const bbMax: [number, number] = [c[0] + e.radius, c[1] + e.radius];
      if (mode === "enclosing") {
        if (
          bbMin[0] >= min[0] &&
          bbMax[0] <= max[0] &&
          bbMin[1] >= min[1] &&
          bbMax[1] <= max[1]
        ) {
          result.push(e.id);
        }
      } else {
        // AABB overlap of the box against the circle's bounding square.
        const overlap =
          min[0] <= bbMax[0] &&
          max[0] >= bbMin[0] &&
          min[1] <= bbMax[1] &&
          max[1] >= bbMin[1];
        if (overlap) result.push(e.id);
      }
    }
  }
  return result;
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
  if (e.kind === "ellipse") {
    const c = entityPoint(state, e.center);
    const f = entityPoint(state, e.focus1);
    if (!c || !f) return null;
    return ellipseOutlineDistance(c, f, e.radmin, cursor);
  }
  if (e.kind === "bspline") {
    const poly = bsplinePolyline(state, e);
    if (!poly || poly.length < 2) return null;
    let best = Infinity;
    for (let i = 0; i + 1 < poly.length; i++) {
      const d = distToSegment(cursor, poly[i], poly[i + 1]);
      if (d < best) best = d;
    }
    return Number.isFinite(best) ? best : null;
  }
  return null;
}
