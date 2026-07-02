/**
 * F18 (reworked): click-to-edit dimension on the most recently drawn LINE.
 *
 * While a line is being drawn the canvas shows only an unobtrusive
 * numeric readout (no DOM field — see `drawDistanceLabel`). Once the
 * line is placed, clicking that readout turns it into an input field;
 * committing a typed length rewrites the line's endpoint to the exact
 * value and keeps a DRIVEN `sketch_constraint_distance` between its
 * endpoints so the dimension stays attached parametrically.
 *
 * Pure, DOM-free helpers factored out of `SketchCanvas2d.tsx` so the
 * edit math is unit-testable without a canvas, kernel, or solver.
 */

import { idsOfSubCall, type SketchSessionState } from "./sketchSession";

/** One sketch sub-call (matches the spine entries the session stores). */
export interface SubCall {
  type: string;
  params: Record<string, unknown>;
}

/** Length (mm) of the segment start→end. */
export function measuredLength(
  start: [number, number],
  end: [number, number],
): number {
  return Math.hypot(end[0] - start[0], end[1] - start[1]);
}

/**
 * Parse the dimension text field into a finite number, or `null` when
 * the field is blank / unparseable. Leading + trailing whitespace is
 * ignored; an empty string is "no value", NOT zero.
 */
export function parseField(raw: string): number | null {
  const t = raw.trim();
  if (t === "") return null;
  const n = Number(t);
  return Number.isFinite(n) ? n : null;
}

/** The spine edit a typed exact length resolves to. */
export interface LineLengthEdit {
  /** Replacement for the `sketch_line` call at `index`, end point moved
   *  to exactly `length` mm along the line's current direction. */
  line: SubCall;
  /** The DRIVEN distance constraint carrying the typed value. */
  constraint: SubCall;
  /** Spine index of an existing distance constraint between the line's
   *  endpoints to replace, or `null` to append `constraint` instead. */
  existingConstraintIndex: number | null;
}

/**
 * Build the spine edit for "set the committed line at `index` to exactly
 * `length` mm": the line keeps its start and direction, only the end
 * point moves. Returns `null` when the edit is degenerate — `index` is
 * not a `sketch_line`, the line has no direction (zero length), or the
 * typed length is not finite and strictly positive.
 */
export function buildLineLengthEdit(
  subCalls: SketchSessionState["subCalls"],
  index: number,
  length: number,
): LineLengthEdit | null {
  if (!Number.isFinite(length) || length <= 0) return null;
  const call = subCalls[index];
  if (!call || call.type !== "sketch_line") return null;
  const start = toPair(call.params.start);
  const end = toPair(call.params.end);
  if (!start || !end) return null;
  const dx = end[0] - start[0];
  const dy = end[1] - start[1];
  const current = Math.hypot(dx, dy);
  if (current < 1e-9) return null;

  const newEnd: [number, number] = [
    start[0] + (dx / current) * length,
    start[1] + (dy / current) * length,
  ];
  // A line allocates [start, end, line] ids in that order.
  const [startId, endId] = idsOfSubCall(subCalls, index);

  const existing = subCalls.findIndex(
    (c) =>
      c.type === "sketch_constraint_distance" &&
      ((c.params.entity_a === startId && c.params.entity_b === endId) ||
        (c.params.entity_a === endId && c.params.entity_b === startId)),
  );

  return {
    line: {
      type: "sketch_line",
      params: { start: [start[0], start[1]], end: newEnd },
    },
    constraint: {
      type: "sketch_constraint_distance",
      params:
        existing >= 0
          ? { ...subCalls[existing].params, value_mm: length }
          : { entity_a: startId, entity_b: endId, value_mm: length, driven: true },
    },
    existingConstraintIndex: existing >= 0 ? existing : null,
  };
}

function toPair(value: unknown): [number, number] | null {
  if (!Array.isArray(value) || value.length !== 2) return null;
  const [x, y] = value;
  if (typeof x !== "number" || typeof y !== "number") return null;
  return [x, y];
}
