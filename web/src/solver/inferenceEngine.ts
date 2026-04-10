/**
 * Auto-constraint inference engine (K5b adapter layer).
 *
 * The InferenceEngine is the "Onshape-feeling" half of the
 * §4.4 sketch-UX split: while the user is drawing, it watches
 * the cursor and the being-drawn geometry and returns a
 * priority-sorted list of candidate constraints that would
 * snap the cursor to an interesting point or auto-infer a
 * relationship with an existing entity. The UI layer renders
 * the top candidate as a visual indicator; when the user
 * clicks, the top-priority candidate gets committed.
 *
 * ## Priority order
 *
 * From the README §4.4 vocabulary:
 *
 * ```
 * Coincident-on-endpoint > Coincident-on-midpoint > Horizontal
 * > Vertical > Parallel > Tangent > Perpendicular > Concentric
 * ```
 *
 * Candidates of the same rank are ordered by distance to the
 * cursor. `suppress` (held by the UI when Shift is down)
 * disables every candidate and returns an empty list — the
 * user explicitly wants no auto-inference.
 *
 * ## Scope in K5b
 *
 * K5b ships the snap-on-point candidates (endpoint, midpoint,
 * origin) plus the orientation candidates for a line being
 * drawn (horizontal, vertical, parallel to an existing line).
 * Tangent, perpendicular, and concentric hooks are reserved
 * for later slices when the curve-geometry pipeline needs
 * them — the candidate ranks are documented so the priority
 * order stays stable as hooks fill in.
 */

import type { Constraint, EntityId, SketchState } from "./types";

/**
 * Screen-space tolerance for a snap match, in sketch
 * millimetres. The UI layer is expected to feed this in
 * translated from pixels, so the engine itself is
 * resolution-independent.
 */
export const DEFAULT_SNAP_DISTANCE_MM = 1.5;

/**
 * Priority tier for a candidate. Lower numbers win. Gaps
 * between tiers leave room for future in-between ranks
 * without breaking stored preferences.
 */
export enum InferencePriority {
  CoincidentEndpoint = 10,
  CoincidentMidpoint = 20,
  Horizontal = 30,
  Vertical = 40,
  Parallel = 50,
  Tangent = 60,
  Perpendicular = 70,
  Concentric = 80,
}

/**
 * One inferred constraint the UI can show to the user and
 * commit on click. `distanceMm` is the geometric distance in
 * sketch millimetres between the cursor and the snap anchor —
 * the UI uses it for tie-breaking when multiple candidates
 * land inside the snap radius.
 */
export interface InferenceCandidate {
  /**
   * The constraint the commit path would synthesise. The
   * `id` field is `-1` — the real id gets assigned when the
   * candidate is committed into the sketch's constraint list.
   */
  constraint: Constraint;
  /** Priority rank — see [`InferencePriority`]. */
  priority: InferencePriority;
  /** Millimetres between cursor and the snap anchor. */
  distanceMm: number;
  /**
   * Short human-readable label for UI rendering (`"Endpoint"`,
   * `"Midpoint"`, `"Horizontal"`, …). Not localised — that's
   * the UI layer's responsibility.
   */
  label: string;
}

/** Query shape the engine responds to. */
export interface InferenceQuery {
  /** The sketch being edited. */
  state: SketchState;
  /** Current cursor position in sketch-local millimetres. */
  cursor: { x: number; y: number };
  /**
   * The half-id + free endpoint of the line currently being
   * drawn, if any. When the user is dragging out a new line
   * segment, the `fromPointId` is the already-placed start
   * point and the cursor is the floating endpoint. With this
   * in hand the engine can propose orientation constraints
   * (horizontal, vertical, parallel to another line) based on
   * the direction vector.
   */
  drawingLineFrom?: EntityId;
  /** Snap radius in sketch millimetres. Defaults to
   * [`DEFAULT_SNAP_DISTANCE_MM`]. */
  snapDistanceMm?: number;
  /**
   * When `true`, the engine returns an empty list. Wire this
   * to a Shift-key check in the UI.
   */
  suppress?: boolean;
}

// ─── public entry point ────────────────────────────────────────

/**
 * Run the inference pass and return a priority-sorted list of
 * candidates that fall within the snap radius. The list is
 * **freshly allocated** on every call and safe for the UI to
 * mutate or sort further.
 */
export function inferCandidates(
  query: InferenceQuery,
): InferenceCandidate[] {
  if (query.suppress) return [];

  const snap = query.snapDistanceMm ?? DEFAULT_SNAP_DISTANCE_MM;
  const { state, cursor } = query;

  const candidates: InferenceCandidate[] = [];

  // Snap candidates (endpoint / midpoint / origin). These all
  // target a freshly-unplaced point: the caller either hasn't
  // fixed the cursor into the sketch yet (hover) or is about
  // to commit a click that creates a new point at the cursor
  // position. The constraint therefore references a synthetic
  // placeholder entity id of -1 which the commit path swaps
  // for the just-allocated point id.
  for (const e of state.entities) {
    switch (e.kind) {
      case "point": {
        const d = distance(cursor, e.at);
        if (d <= snap) {
          candidates.push({
            constraint: {
              kind: "coincident",
              id: -1,
              a: -1,
              b: e.id,
            },
            priority: InferencePriority.CoincidentEndpoint,
            distanceMm: d,
            label: "Endpoint",
          });
        }
        break;
      }
      case "origin": {
        const d = distance(cursor, { x: 0, y: 0 });
        if (d <= snap) {
          candidates.push({
            constraint: {
              kind: "coincident",
              id: -1,
              a: -1,
              b: e.id,
            },
            priority: InferencePriority.CoincidentEndpoint,
            distanceMm: d,
            label: "Origin",
          });
        }
        break;
      }
      case "line": {
        const start = resolvePoint(state, e.start);
        const end = resolvePoint(state, e.end);
        if (!start || !end) break;
        const mid = { x: (start.x + end.x) / 2, y: (start.y + end.y) / 2 };
        const d = distance(cursor, mid);
        if (d <= snap) {
          // Midpoint snap is modelled as a `Midpoint` constraint
          // on the line using a synthetic point id — the
          // committing UI will insert a real point at the
          // midpoint and reference it from the constraint.
          candidates.push({
            constraint: {
              kind: "midpoint",
              id: -1,
              point: -1,
              line: e.id,
            },
            priority: InferencePriority.CoincidentMidpoint,
            distanceMm: d,
            label: "Midpoint",
          });
        }
        break;
      }
      case "circle":
      case "arc":
        // Tangent / concentric snapping hooks belong here but
        // require the curve-geometry evaluator to exist first.
        break;
    }
  }

  // Orientation candidates for a line being drawn. These are
  // scored by the **angular distance** of the current line's
  // direction to the axis or reference direction, projected
  // back to mm at the cursor as an approximation — the UI
  // doesn't need a precise distance for these, just a stable
  // ordering against the snap-on-point set above.
  if (query.drawingLineFrom !== undefined) {
    const start = resolvePoint(state, query.drawingLineFrom);
    if (start) {
      const dx = cursor.x - start.x;
      const dy = cursor.y - start.y;
      const lineLength = Math.hypot(dx, dy);
      // Only emit orientation hints when the line has a
      // non-trivial length; a zero-length line has no
      // direction.
      if (lineLength > snap * 0.5) {
        const horizontalAngleOffset = Math.abs(dy);
        if (horizontalAngleOffset <= snap) {
          candidates.push({
            constraint: {
              kind: "horizontal",
              id: -1,
              line: -1,
            },
            priority: InferencePriority.Horizontal,
            distanceMm: horizontalAngleOffset,
            label: "Horizontal",
          });
        }
        const verticalAngleOffset = Math.abs(dx);
        if (verticalAngleOffset <= snap) {
          candidates.push({
            constraint: {
              kind: "vertical",
              id: -1,
              line: -1,
            },
            priority: InferencePriority.Vertical,
            distanceMm: verticalAngleOffset,
            label: "Vertical",
          });
        }

        // Parallel-to-existing-line candidates. For each line
        // already in the sketch, compute the angle offset
        // between its direction and the current drag vector.
        for (const other of state.entities) {
          if (other.kind !== "line") continue;
          const os = resolvePoint(state, other.start);
          const oe = resolvePoint(state, other.end);
          if (!os || !oe) continue;
          const oLen = Math.hypot(oe.x - os.x, oe.y - os.y);
          if (oLen < 1e-6) continue;
          // Parallel ⇔ cross product of direction vectors is
          // zero. Project the cross product magnitude back to
          // mm by dividing out the other line's length — a
          // rough geometric measure of "how far off parallel
          // are we" at the cursor distance.
          const cross =
            (dx * (oe.y - os.y) - dy * (oe.x - os.x)) / oLen;
          const offset = Math.abs(cross);
          if (offset <= snap) {
            candidates.push({
              constraint: {
                kind: "parallel",
                id: -1,
                a: -1,
                b: other.id,
              },
              priority: InferencePriority.Parallel,
              distanceMm: offset,
              label: "Parallel",
            });
          }
        }
      }
    }
  }

  // Priority first, then distance within the same tier.
  candidates.sort((a, b) => {
    if (a.priority !== b.priority) return a.priority - b.priority;
    return a.distanceMm - b.distanceMm;
  });
  return candidates;
}

// ─── helpers ────────────────────────────────────────────────────

function distance(
  a: { x: number; y: number },
  b: { x: number; y: number },
): number {
  const dx = a.x - b.x;
  const dy = a.y - b.y;
  return Math.hypot(dx, dy);
}

function resolvePoint(
  state: SketchState,
  id: EntityId,
): { x: number; y: number } | null {
  const e = state.entities.find((x) => x.id === id);
  if (!e) return null;
  if (e.kind === "point") return e.at;
  if (e.kind === "origin") return { x: 0, y: 0 };
  return null;
}
