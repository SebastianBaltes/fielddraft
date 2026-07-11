/**
 * Sketch snap resolution — extracted verbatim from `SketchCanvas2d.tsx`
 * (architecture review, Befund 2: tool-controller registry).
 *
 * Given the raw cursor position and the committed sketch, resolve where a
 * placed point should land: an inference candidate (endpoint / origin /
 * horizontal / vertical / parallel), a projection onto a committed line
 * (tickets pgyf / ymbp), a circle / arc rim / centre / quadrant (ticket
 * fkig) or a projected B-Rep edge guide (K6b). Also classifies where a
 * committed endpoint landed so the commit path can emit a *persistent*
 * constraint (ticket inej), not just a one-off snapped position.
 *
 * Pure and DOM-free apart from the camera-bridge plane frame used by
 * `projectEdgesToSketchPlane`.
 */

import {
  buildDraftSketchState,
  DRAFT_DRAG_START_ID,
  type SketchSessionState,
} from "./sketchSession";
import { getSketchPlaneFrame } from "./sketchCameraBridge";
import { snapToCircles, type SnapCircle } from "./sketchCircleSnap";
import {
  inferCandidates,
  type InferenceCandidate,
} from "./solver/inferenceEngine";
import type { SketchState } from "./solver/types";

/** Snap radius in CSS pixels. Divided by the camera zoom (px-per-mm) to
 *  get the sketch-plane tolerance, so snapping feels identical at every
 *  zoom level — the way Onshape works. A fixed-mm radius (the old
 *  `SNAP_RADIUS_MM = 10`) collapsed to ~3 px when zoomed out, which made
 *  closing a polygon by snapping onto its start point practically
 *  impossible (ticket zuw9). Generous enough to catch ordinary cursor
 *  jitter, tight enough to still place free points near an entity. */
export const SNAP_RADIUS_PX = 12;

/** Resolve the snap radius from CSS pixels to sketch-plane millimetres
 *  for the current camera zoom (mirrors the `SELECT_PICK_PX` pattern).
 *  `cam.zoom` is px-per-mm, so the on-screen snap radius stays a constant
 *  `SNAP_RADIUS_PX` at every zoom — exported for the regression test. */
export function snapRadiusMmFor(cam: { zoom: number }): number {
  return SNAP_RADIUS_PX / Math.max(cam.zoom, 1e-6);
}

export interface ProjectedEdge {
  p0: [number, number];
  p1: [number, number];
  /** jcb7: the source B-Rep edge's stable id-hash, so the Project tool can
   *  emit an ASSOCIATIVE `sketch_project { edge: qEdge(idHash) }` sub-call
   *  rather than a frozen coordinate copy. Absent for guides whose edge has
   *  no exported id-hash (older bodies) — those fall back to a plain copy. */
  idHash?: string;
}

export function projectEdgesToSketchPlane(
  edges: { p0: number[]; p1: number[]; idHash?: string }[],
): ProjectedEdge[] {
  // Tickets yx6a/bxoo: drop each world point into the resolved plane
  // frame (orthogonal projection along the plane normal).
  const f = getSketchPlaneFrame();
  const uv = (p: number[]): [number, number] => {
    const dx = p[0] - f.origin[0];
    const dy = p[1] - f.origin[1];
    const dz = p[2] - f.origin[2];
    return [
      dx * f.xAxis[0] + dy * f.xAxis[1] + dz * f.xAxis[2],
      dx * f.yAxis[0] + dy * f.yAxis[1] + dz * f.yAxis[2],
    ];
  };
  return edges.map((e) => ({ p0: uv(e.p0), p1: uv(e.p1), idHash: e.idHash }));
}

export function nearestPointOnSegment(
  p: [number, number],
  a: [number, number],
  b: [number, number],
): [number, number] {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const lenSq = dx * dx + dy * dy;
  if (lenSq < 1e-10) return a;
  const t = Math.max(
    0,
    Math.min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / lenSq),
  );
  return [a[0] + t * dx, a[1] + t * dy];
}

export function dist2d(a: [number, number], b: [number, number]): number {
  return Math.hypot(a[0] - b[0], a[1] - b[1]);
}

/**
 * Ticket fkig: collect the sketch's circles and arcs as resolved
 * [`SnapCircle`]s so a drawn endpoint can snap onto their rim / centre /
 * quadrants. Centres and arc endpoints are looked up from the entity list;
 * an arc's live span is recovered from the angles of its start/end points
 * about the centre (PlaneGCS stores arcs CCW from start to end).
 */
export function snapCirclesOf(state: SketchState): SnapCircle[] {
  const pointAt = (id: number): [number, number] | null => {
    const e = state.entities.find((x) => x.id === id);
    if (e?.kind === "point") return [e.at.x, e.at.y];
    if (e?.kind === "origin") return [0, 0];
    return null;
  };
  const out: SnapCircle[] = [];
  for (const e of state.entities) {
    if (e.kind === "circle") {
      const c = pointAt(e.center);
      if (c && e.radius > 1e-9) {
        out.push({ id: e.id, centerId: e.center, center: c, radius: e.radius });
      }
    } else if (e.kind === "arc") {
      const c = pointAt(e.center);
      const s = pointAt(e.start);
      const en = pointAt(e.end);
      if (c && s && en && e.radius > 1e-9) {
        const a0 = Math.atan2(s[1] - c[1], s[0] - c[0]);
        const a1 = Math.atan2(en[1] - c[1], en[0] - c[0]);
        let sweep = a1 - a0;
        while (sweep <= 0) sweep += Math.PI * 2; // CCW span in (0, 2π]
        out.push({
          id: e.id,
          centerId: e.center,
          center: c,
          radius: e.radius,
          start: a0,
          sweep,
        });
      }
    }
  }
  return out;
}

/**
 * Inference result for the current cursor: the raw cursor
 * position, the snapped cursor position (possibly equal), and
 * the inference candidate that drove the snap, if any. Feeds
 * both the preview drawing and the `onPointerUp` commit path.
 */
export interface SnapResolution {
  snapped: [number, number];
  candidate: InferenceCandidate | null;
  /** Anchor in sketch coordinates where the snap indicator
   *  should be drawn. For endpoint / origin snaps this is the
   *  snap target itself; for horizontal / vertical / parallel
   *  hints it's the snapped cursor. `null` skips the ring. */
  anchor: [number, number] | null;
  /** Real entity id the cursor snapped to, for coincident
   *  snaps. Used by the commit path to emit a matching
   *  constraint referencing the kernel-side entity. `null`
   *  when the snap is orientation-only (horizontal/vertical/
   *  parallel) or when there is no snap at all. */
  coincidentTargetId: number | null;
}

/** The "no snap" resolution at `cursor` — cursor passes through unchanged,
 *  no candidate, no indicator ring, no coincident binding. */
export function noSnap(cursor: [number, number]): SnapResolution {
  return {
    snapped: cursor,
    candidate: null,
    anchor: null,
    coincidentTargetId: null,
  };
}

/** Does `constraint` reference any entity id in `exclude`? Scans every
 *  own numeric field except the (always `-1`) candidate `id`, so it stays
 *  correct for every constraint shape the inference engine emits. */
function referencesExcluded(
  constraint: Record<string, unknown>,
  exclude: ReadonlySet<number>,
): boolean {
  for (const [key, value] of Object.entries(constraint)) {
    if (key === "id") continue;
    if (typeof value === "number" && exclude.has(value)) return true;
  }
  return false;
}

export function resolveSnap(
  cursor: [number, number],
  session: SketchSessionState,
  dragStart: [number, number] | null,
  snapRadiusMm: number,
  suppress = false,
  projectedEdges: ProjectedEdge[] = [],
  /** Entity ids to never snap onto — the grip-drag path passes the
   *  dragged point and its owning curve so a handle can't bind to
   *  itself (ticket b26062eb). */
  excludeIds: ReadonlySet<number> = EMPTY_ID_SET,
): SnapResolution {
  if (suppress) {
    return {
      snapped: cursor,
      candidate: null,
      anchor: null,
      coincidentTargetId: null,
    };
  }
  const state: SketchState = buildDraftSketchState(
    session.subCalls,
    dragStart ?? undefined,
  );
  const cands = inferCandidates({
    state,
    cursor: { x: cursor[0], y: cursor[1] },
    drawingLineFrom: dragStart ? DRAFT_DRAG_START_ID : undefined,
    snapDistanceMm: snapRadiusMm,
  }).filter(
    (cand) =>
      !referencesExcluded(
        cand.constraint as unknown as Record<string, unknown>,
        excludeIds,
      ),
  );

  // Tickets pgyf / ymbp: snap onto an existing SKETCH LINE segment (project the
  // cursor onto the nearest committed line within the snap radius). Lower
  // priority than an endpoint/origin coincident snap, so a vertex still wins;
  // used as the fallback when no point-snap candidate applies. This is what
  // lets the Point tool land on a line and a dragged endpoint land on another
  // line. The in-flight draft line isn't a committed entity, so we never snap
  // onto the line being drawn.
  const lineSnap = ((): SnapResolution | null => {
    let bestD = snapRadiusMm;
    let bestPt: [number, number] | null = null;
    for (const e of state.entities) {
      if (e.kind !== "line") continue;
      // Spline tangent legs are guides — never snap-project onto them
      // (ticket 7ad7d649); their endpoints stay point-snappable.
      if (e.guide) continue;
      if (
        excludeIds.has(e.id) ||
        excludeIds.has(e.start) ||
        excludeIds.has(e.end)
      )
        continue;
      const a = state.entities.find((x) => x.id === e.start);
      const b = state.entities.find((x) => x.id === e.end);
      if (!a || !b || a.kind !== "point" || b.kind !== "point") continue;
      const np = nearestPointOnSegment(
        cursor,
        [a.at.x, a.at.y],
        [b.at.x, b.at.y],
      );
      const d = dist2d(cursor, np);
      if (d < bestD) {
        bestD = d;
        bestPt = np;
      }
    }
    return bestPt
      ? {
          snapped: bestPt,
          candidate: null,
          anchor: bestPt,
          coincidentTargetId: null,
        }
      : null;
  })();

  // Ticket fkig: snap onto an existing SKETCH CIRCLE / ARC — rim projection,
  // centre, or quadrant. Like the line snap above it's a fallback below the
  // endpoint/origin coincident snaps (a real vertex still wins), but a circle
  // centre is itself a coincident target so the commit binds to it. The draft
  // curve isn't a committed entity, so we never snap onto what's being drawn.
  const circleSnap = ((): SnapResolution | null => {
    const circles = snapCirclesOf(state).filter(
      (c) => !excludeIds.has(c.id) && !excludeIds.has(c.centerId),
    );
    const hit = snapToCircles(circles, cursor, snapRadiusMm);
    if (!hit) return null;
    // A centre snap is a coincident-to-point relationship; rim / quadrant land
    // ON the curve, so the commit emits point-on-object (classifyLineAnchor).
    return {
      snapped: hit.at,
      candidate: null,
      anchor: hit.at,
      coincidentTargetId: hit.kind === "center" ? hit.curve.centerId : null,
    };
  })();

  // Combined geometry fallback: whichever of the line / circle projection is
  // closer to the cursor (both already passed their own snap radius). A circle
  // centre wins outright — it carries a coincident target the line snap can't.
  const curveSnap: SnapResolution | null = (() => {
    if (circleSnap && circleSnap.coincidentTargetId !== null) return circleSnap;
    if (!circleSnap) return lineSnap;
    if (!lineSnap) return circleSnap;
    const dC = dist2d(cursor, circleSnap.anchor ?? cursor);
    const dL = dist2d(cursor, lineSnap.anchor ?? cursor);
    return dC <= dL ? circleSnap : lineSnap;
  })();

  // K6b: check B-Rep edge snapping before falling through to no-snap.
  // Edge snap is lower priority than entity snap (coincident/H/V) but
  // higher priority than no snap at all.
  if (cands.length === 0 && projectedEdges.length > 0) {
    let bestDist = snapRadiusMm;
    let bestPt: [number, number] | null = null;
    for (const edge of projectedEdges) {
      // Snap to edge endpoints (vertices)
      const d0 = dist2d(cursor, edge.p0);
      if (d0 < bestDist) {
        bestDist = d0;
        bestPt = edge.p0;
      }
      const d1 = dist2d(cursor, edge.p1);
      if (d1 < bestDist) {
        bestDist = d1;
        bestPt = edge.p1;
      }
      // Snap to nearest point on edge segment
      const np = nearestPointOnSegment(cursor, edge.p0, edge.p1);
      const dn = dist2d(cursor, np);
      if (dn < bestDist) {
        bestDist = dn;
        bestPt = np;
      }
    }
    if (bestPt) {
      return {
        snapped: bestPt,
        candidate: null,
        anchor: bestPt,
        coincidentTargetId: null,
      };
    }
  }

  if (cands.length === 0) {
    // Fall back to a sketch line / circle projection snap before giving up
    // (pgyf/ymbp for lines, fkig for circles & arcs).
    if (curveSnap) return curveSnap;
    return {
      snapped: cursor,
      candidate: null,
      anchor: null,
      coincidentTargetId: null,
    };
  }
  const top = cands[0];
  const c = top.constraint;
  // Ticket 3095b041: REAL geometry beats an orientation-only hint. When
  // the cursor is within the snap radius of a committed curve (line
  // projection / circle / arc rim), landing ON that curve — with its
  // persistent point-on-object binding — is what the user is aiming
  // for; a horizontal/vertical/parallel hint would otherwise shadow the
  // curve and the endpoint would fall short of it. Coincident (vertex)
  // candidates still win below by returning before this check is used.
  if (c.kind !== "coincident" && curveSnap) return curveSnap;
  // Endpoint / origin snap: pull the cursor onto the target
  // point's coordinates.
  if (c.kind === "coincident") {
    const target = state.entities.find((e) => e.id === c.b);
    if (target && target.kind === "point") {
      return {
        snapped: [target.at.x, target.at.y],
        candidate: top,
        anchor: [target.at.x, target.at.y],
        coincidentTargetId: target.id,
      };
    }
    if (target && target.kind === "origin") {
      return {
        snapped: [0, 0],
        candidate: top,
        anchor: [0, 0],
        coincidentTargetId: target.id,
      };
    }
  }
  // Horizontal/vertical: lock one axis to the drag-start axis.
  if (c.kind === "horizontal" && dragStart) {
    const snapped: [number, number] = [cursor[0], dragStart[1]];
    return {
      snapped,
      candidate: top,
      anchor: snapped,
      coincidentTargetId: null,
    };
  }
  if (c.kind === "vertical" && dragStart) {
    const snapped: [number, number] = [dragStart[0], cursor[1]];
    return {
      snapped,
      candidate: top,
      anchor: snapped,
      coincidentTargetId: null,
    };
  }
  // Parallel to an existing line: project the cursor onto the
  // parallel ray through the drag start.
  if (c.kind === "parallel" && dragStart) {
    const other = state.entities.find((e) => e.id === c.b);
    if (other && other.kind === "line") {
      const os = state.entities.find((e) => e.id === other.start);
      const oe = state.entities.find((e) => e.id === other.end);
      if (os && oe && os.kind === "point" && oe.kind === "point") {
        const dx = oe.at.x - os.at.x;
        const dy = oe.at.y - os.at.y;
        const len = Math.hypot(dx, dy);
        if (len > 1e-6) {
          const ux = dx / len;
          const uy = dy / len;
          // Project (cursor - start) onto the unit direction.
          const cdx = cursor[0] - dragStart[0];
          const cdy = cursor[1] - dragStart[1];
          const t = cdx * ux + cdy * uy;
          const snapped: [number, number] = [
            dragStart[0] + ux * t,
            dragStart[1] + uy * t,
          ];
          return {
            snapped,
            candidate: top,
            anchor: snapped,
            coincidentTargetId: null,
          };
        }
      }
    }
  }
  // No endpoint / H-V / parallel snap applied — fall back to projecting onto a
  // sketch line (pgyf/ymbp) or circle/arc (fkig) if the cursor is near one.
  if (curveSnap) return curveSnap;
  return {
    snapped: cursor,
    candidate: null,
    anchor: null,
    coincidentTargetId: null,
  };
}

/**
 * Ticket inej: classify where a committed line endpoint landed so the commit
 * can emit a *persistent* constraint, not just a one-off snapped position.
 * Snapping placed the point exactly on existing geometry, so:
 *   - on/at an existing point → coincident (the point IS that vertex);
 *   - on a line's interior     → point-on-object (slides along that line).
 * Without this the endpoint sits on the target visually but is unconstrained,
 * so a later dimension change drifts it off (the reported bug). Points are
 * checked before lines so an endpoint hit reads as coincident, not on-object.
 */
export type AnchorClass =
  | { kind: "point"; id: number }
  | { kind: "object"; id: number }
  | null;

// Tight tolerance: a snapped endpoint lands EXACTLY on its target, so we only
// bind when the point is essentially on the geometry — not merely within the
// (much larger) live snap radius, which would over-constrain a free endpoint
// that just happens to pass near an unrelated line.
const ANCHOR_ON_EPS_MM = 0.1;

const EMPTY_ID_SET: ReadonlySet<number> = new Set();

export function classifyLineAnchor(
  pt: [number, number],
  state: SketchState,
  /** Entity ids never to bind to — the grip-drag drop passes the dragged
   *  point and its owning curve (they'd trivially match at distance 0). */
  excludeIds: ReadonlySet<number> = EMPTY_ID_SET,
): AnchorClass {
  for (const e of state.entities) {
    if (excludeIds.has(e.id)) continue;
    if (e.kind === "point" && dist2d(pt, [e.at.x, e.at.y]) < ANCHOR_ON_EPS_MM) {
      return { kind: "point", id: e.id };
    }
  }
  for (const e of state.entities) {
    if (e.kind !== "line") continue;
    if (e.guide) continue;
    if (excludeIds.has(e.id)) continue;
    const a = state.entities.find((x) => x.id === e.start);
    const b = state.entities.find((x) => x.id === e.end);
    if (!a || !b || a.kind !== "point" || b.kind !== "point") continue;
    const np = nearestPointOnSegment(pt, [a.at.x, a.at.y], [b.at.x, b.at.y]);
    if (dist2d(pt, np) < ANCHOR_ON_EPS_MM) return { kind: "object", id: e.id };
  }
  // Ticket fkig: an endpoint snapped onto a circle / arc rim binds to that
  // curve with point-on-object, so a later radius change keeps it on the rim.
  // The snap already placed it exactly on the radius, so test radial distance.
  for (const c of snapCirclesOf(state)) {
    if (excludeIds.has(c.id)) continue;
    const radial = Math.abs(dist2d(pt, c.center) - c.radius);
    if (radial >= ANCHOR_ON_EPS_MM) continue;
    if (c.start !== undefined && c.sweep !== undefined) {
      // Arc: only bind when the point's angle is on the live span.
      let off = Math.atan2(pt[1] - c.center[1], pt[0] - c.center[0]) - c.start;
      while (off < 0) off += Math.PI * 2;
      if (off > c.sweep + 1e-6) continue;
    }
    return { kind: "object", id: c.id };
  }
  return null;
}

/** Constraint sub-calls binding a new line endpoint to whatever it landed on. */
export function anchorConstraints(
  pointId: number,
  anchor: AnchorClass,
): Array<{ type: string; params: Record<string, unknown> }> {
  if (anchor === null) return [];
  if (anchor.kind === "point") {
    return [
      {
        type: "sketch_constraint_coincident",
        params: { a: pointId, b: anchor.id },
      },
    ];
  }
  return [
    {
      type: "sketch_constraint_point_on_object",
      params: { point: pointId, curve: anchor.id },
    },
  ];
}
