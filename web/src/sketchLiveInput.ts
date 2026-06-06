/**
 * F18: live numeric input while drawing a LINE.
 *
 * Pure, DOM-free geometry + sub-call bundle helpers for the
 * "type-while-drawing" UX (Onshape-style). Factored out of
 * `SketchCanvas2d.tsx` so the commit math is unit-testable without a
 * canvas, kernel, or solver.
 *
 * The committed bundle for a typed line is:
 *   - a `sketch_line` sub-call for [start, end]
 *   - a DRIVEN `sketch_constraint_distance` between the new line's two
 *     endpoints with `value_mm = length` so the line stays parametric.
 *
 * The endpoint ids the distance constraint references are PREDICTED via
 * `predictNextLineIds` (the kernel allocates start, end, line in that
 * order), so the bundle is internally consistent before it is pushed.
 */

import { predictNextLineIds } from "./sketchSession";
import type { SketchSessionState } from "./store";

/** One sketch sub-call (matches the spine entries the session stores). */
export interface SubCall {
  type: string;
  params: Record<string, unknown>;
}

/**
 * End point of a line of `length` mm at `angleDeg` (CCW from +X, the
 * same convention as `Math.atan2(dy, dx)`) starting at `start`.
 *
 *   length 10, angle 0   from (0,0) → (10, 0)
 *   length 10, angle 90  from (0,0) → (0, 10)
 */
export function lineFromLengthAngle(
  start: [number, number],
  length: number,
  angleDeg: number,
): [number, number] {
  const theta = (angleDeg * Math.PI) / 180;
  return [
    start[0] + length * Math.cos(theta),
    start[1] + length * Math.sin(theta),
  ];
}

/** Length (mm) of the segment start→end. */
export function measuredLength(
  start: [number, number],
  end: [number, number],
): number {
  return Math.hypot(end[0] - start[0], end[1] - start[1]);
}

/** Angle (deg, CCW from +X) of the segment start→end. */
export function measuredAngleDeg(
  start: [number, number],
  end: [number, number],
): number {
  return (Math.atan2(end[1] - start[1], end[0] - start[0]) * 180) / Math.PI;
}

/**
 * Parse a HUD text field into a finite number, or `null` when the field
 * is blank / unparseable. Leading + trailing whitespace is ignored; an
 * empty string is treated as "no override" (→ fall back to the live
 * value), NOT as zero.
 */
export function parseField(raw: string): number | null {
  const t = raw.trim();
  if (t === "") return null;
  const n = Number(t);
  return Number.isFinite(n) ? n : null;
}

/**
 * Resolve the effective length + angle for a typed-line commit from the
 * raw HUD fields and the current (live, drag-measured) values. A blank
 * field falls back to its live value; a non-finite typed value is
 * rejected by returning `null` for that field.
 */
export function resolveLengthAngle(
  lengthRaw: string,
  angleRaw: string,
  liveLength: number,
  liveAngleDeg: number,
): { length: number; angleDeg: number } | null {
  const typedLen = parseField(lengthRaw);
  const typedAng = parseField(angleRaw);
  const length = typedLen === null ? liveLength : typedLen;
  const angleDeg = typedAng === null ? liveAngleDeg : typedAng;
  if (!Number.isFinite(length) || !Number.isFinite(angleDeg)) return null;
  return { length, angleDeg };
}

/**
 * Build the commit bundle for a typed line: the `sketch_line` sub-call
 * plus a DRIVEN `sketch_constraint_distance` between the predicted
 * endpoint ids with `value_mm = length`.
 *
 * Returns `null` (→ no commit) when the length is not a finite, strictly
 * positive value — a zero-length or non-finite line is degenerate.
 *
 * `subCalls` is the current session spine, used only to predict the ids
 * the new line will occupy; it is not mutated.
 */
export function buildTypedLineBundle(
  subCalls: SketchSessionState["subCalls"],
  start: [number, number],
  length: number,
  angleDeg: number,
): SubCall[] | null {
  if (!Number.isFinite(length) || length <= 0) return null;
  if (!Number.isFinite(angleDeg)) return null;
  const end = lineFromLengthAngle(start, length, angleDeg);
  const ids = predictNextLineIds(subCalls);
  return [
    {
      type: "sketch_line",
      params: {
        start: [start[0], start[1]] as [number, number],
        end: [end[0], end[1]] as [number, number],
      },
    },
    {
      type: "sketch_constraint_distance",
      params: {
        entity_a: ids.start,
        entity_b: ids.end,
        value_mm: length,
        driven: true,
      },
    },
  ];
}
