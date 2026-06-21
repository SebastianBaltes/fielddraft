/**
 * Snap a cursor / dragged endpoint onto a circle or arc (ticket fkig).
 *
 * The 2D sketcher already snaps onto points, line segments and B-Rep edges
 * (see `resolveSnap` in SketchCanvas2d). Circles and arcs were missing: while
 * drawing a line you could not land an endpoint on a circle's rim, so a
 * line-to-circle relationship had to be added by hand afterwards.
 *
 * This module is the pure-geometry half of the fix. Given the resolved
 * circles/arcs of the sketch and a cursor position it returns the nearest
 * interesting snap point within a radius, classified so the commit path can
 * emit the matching persistent constraint:
 *
 *   - `"rim"`     → the cursor projected radially onto the circle/arc edge.
 *                   Commits to a point-on-object constraint (slides along).
 *   - `"center"`  → the circle/arc centre. Commits to a coincident constraint
 *                   against the centre point.
 *   - `"quadrant"`→ one of the four axis-aligned rim points (0°, 90°, 180°,
 *                   270°). Geometrically still on the rim, so it also commits
 *                   to point-on-object, but it snaps with priority over a plain
 *                   rim projection because it's a recognisable feature point.
 *
 * Priority within the radius: centre > quadrant > rim. A centre / quadrant is a
 * crisp feature the user is usually aiming for, so it wins ties against the much
 * larger continuum of plain rim points. Distances are compared in sketch
 * millimetres; this module never touches the DOM or the camera.
 */

export type Vec2 = [number, number];

/** A circle or arc in resolved sketch coordinates. `center` is the resolved
 *  centre point (not an entity id). For an arc, `start`/`sweep` bound the live
 *  span in radians (sweep may be negative); omit them for a full circle. */
export interface SnapCircle {
  /** Entity id of the curve itself (circle/arc), for point-on-object refs. */
  id: number;
  /** Entity id of the centre point, for a coincident-to-centre snap. */
  centerId: number;
  center: Vec2;
  radius: number;
  start?: number;
  sweep?: number;
}

export type CircleSnapKind = "rim" | "center" | "quadrant";

export interface CircleSnap {
  /** Where to place the snapped cursor. */
  at: Vec2;
  kind: CircleSnapKind;
  /** The circle/arc the snap belongs to. */
  curve: SnapCircle;
}

const TAU = Math.PI * 2;

function dist(a: Vec2, b: Vec2): number {
  return Math.hypot(a[0] - b[0], a[1] - b[1]);
}

/** Wrap into [0, TAU). */
function norm2pi(a: number): number {
  let x = a % TAU;
  if (x < 0) x += TAU;
  return x;
}

/** Is angle `theta` within the arc's live span? Full circles (no span) always
 *  pass. Handles either sweep direction and the ±π seam. */
function angleOnArc(c: SnapCircle, theta: number): boolean {
  if (c.start === undefined || c.sweep === undefined) return true;
  const span = Math.abs(c.sweep);
  if (span >= TAU - 1e-9) return true;
  const travelled =
    c.sweep >= 0 ? norm2pi(theta - c.start) : norm2pi(c.start - theta);
  return travelled <= span + 1e-9;
}

/** Point on the circle/arc at angle `theta`. */
function rimPoint(c: SnapCircle, theta: number): Vec2 {
  return [c.center[0] + c.radius * Math.cos(theta), c.center[1] + c.radius * Math.sin(theta)];
}

/**
 * Best circle/arc snap for `cursor` within `tolMm`, or null when no circle is
 * close enough. Pure: pass the already-resolved circles/arcs of the sketch.
 *
 * The draft curve being drawn must be excluded by the caller (don't snap a
 * line's endpoint onto a circle that doesn't exist yet) — this function snaps
 * onto whatever it's given.
 */
export function snapToCircles(
  circles: ReadonlyArray<SnapCircle>,
  cursor: Vec2,
  tolMm: number,
): CircleSnap | null {
  let best: CircleSnap | null = null;
  // Lower number = higher priority.
  let bestRank = Infinity;
  let bestDist = Infinity;

  const consider = (cand: CircleSnap, rank: number, d: number) => {
    if (d > tolMm) return;
    if (rank < bestRank || (rank === bestRank && d < bestDist)) {
      best = cand;
      bestRank = rank;
      bestDist = d;
    }
  };

  for (const c of circles) {
    if (!(c.radius > 1e-9)) continue;

    // Centre snap (rank 0).
    consider({ at: c.center, kind: "center", curve: c }, 0, dist(cursor, c.center));

    // Angle of the cursor about the centre.
    const theta = Math.atan2(cursor[1] - c.center[1], cursor[0] - c.center[0]);

    // Quadrant snaps (rank 1): the four axis points that lie on the live span.
    for (const q of [0, Math.PI / 2, Math.PI, (3 * Math.PI) / 2]) {
      if (!angleOnArc(c, q)) continue;
      const at = rimPoint(c, q);
      consider({ at, kind: "quadrant", curve: c }, 1, dist(cursor, at));
    }

    // Rim projection (rank 2): only when the cursor's angle is on the span.
    if (angleOnArc(c, theta)) {
      const at = rimPoint(c, theta);
      consider({ at, kind: "rim", curve: c }, 2, dist(cursor, at));
    }
  }

  return best;
}
