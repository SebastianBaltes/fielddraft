/**
 * Pure geometry for the interactive arc tool (UI gap #1).
 *
 * The arc tool is a three-click *centre → start → end* gesture:
 *   1. centre point C
 *   2. start point P1  → fixes radius = |P1−C| and start_angle = ∠(P1−C)
 *   3. end point P2    → fixes the swept angle
 *
 * All angles are in sketch space (radians, CCW from +x, Y-up) — the same
 * convention the grip code (`sketchGrips.ts`) and the kernel `SketchArcDef`
 * use, so a stored arc renders through the points the user actually clicked.
 * Rendering samples the arc with {@link sampleArc} rather than `ctx.arc`,
 * which would mis-interpret these angles on the Y-down canvas.
 *
 * Kept free of React/DOM so the math is unit-testable in isolation.
 */

export type Vec2 = [number, number];

/** Sketch-space angle (radians, CCW from +x) of `p` about `center`. */
export function angleAbout(center: Vec2, p: Vec2): number {
  return Math.atan2(p[1] - center[1], p[0] - center[0]);
}

/** Normalize an angle into (-π, π]. */
function normalizePi(a: number): number {
  let d = a;
  while (d <= -Math.PI) d += 2 * Math.PI;
  while (d > Math.PI) d -= 2 * Math.PI;
  return d;
}

/**
 * Signed sweep from `startAngle` to `endAngle`, unwrapped so it stays
 * continuous with `prevSweep`. This lets a live gesture cross the ±180°
 * seam and wind up toward a full turn instead of snapping to the short
 * way round. Clamped to ±2π (one full revolution).
 */
export function unwrapSweep(
  startAngle: number,
  endAngle: number,
  prevSweep: number,
): number {
  const base = normalizePi(endAngle - startAngle);
  const k = Math.round((prevSweep - base) / (2 * Math.PI));
  let sweep = base + k * 2 * Math.PI;
  const TAU = 2 * Math.PI;
  if (sweep > TAU) sweep = TAU;
  if (sweep < -TAU) sweep = -TAU;
  return sweep;
}

/** Minimum radius (mm) and sweep (rad) below which an arc is degenerate. */
export const MIN_ARC_RADIUS_MM = 1e-2;
export const MIN_ARC_SWEEP_RAD = 1e-3;

export interface ArcSubCall {
  type: "sketch_arc";
  params: {
    center: Vec2;
    radius: number;
    start_angle: number;
    sweep_angle: number;
  };
}

/**
 * Build a `sketch_arc` sub-call from a centre–start–end gesture, or `null`
 * when the result would be degenerate (radius or sweep too small). `prevSweep`
 * is the sweep accumulated during the live drag so the committed arc matches
 * the previewed one across the ±180° seam.
 */
export function buildArcSubCall(
  center: Vec2,
  start: Vec2,
  end: Vec2,
  prevSweep: number,
): ArcSubCall | null {
  const radius = Math.hypot(start[0] - center[0], start[1] - center[1]);
  const startAngle = angleAbout(center, start);
  const endAngle = angleAbout(center, end);
  const sweep = unwrapSweep(startAngle, endAngle, prevSweep);
  if (radius < MIN_ARC_RADIUS_MM || Math.abs(sweep) < MIN_ARC_SWEEP_RAD) {
    return null;
  }
  return {
    type: "sketch_arc",
    params: {
      center: [center[0], center[1]],
      radius,
      start_angle: startAngle,
      sweep_angle: sweep,
    },
  };
}

/**
 * Sample points along an arc in sketch space for polyline rendering. The
 * step is capped at `maxStepRad` so even large arcs stay visually smooth;
 * always returns at least the two endpoints.
 */
export function sampleArc(
  center: Vec2,
  radius: number,
  startAngle: number,
  sweep: number,
  maxStepRad: number = Math.PI / 32,
): Vec2[] {
  const steps = Math.max(2, Math.ceil(Math.abs(sweep) / maxStepRad));
  const pts: Vec2[] = [];
  for (let i = 0; i <= steps; i++) {
    const a = startAngle + (sweep * i) / steps;
    pts.push([
      center[0] + radius * Math.cos(a),
      center[1] + radius * Math.sin(a),
    ]);
  }
  return pts;
}
