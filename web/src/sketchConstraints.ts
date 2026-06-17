/**
 * Explicit-constraint palette logic (P0.1 / P0.2).
 *
 * Given the entities the user picked in select mode, decide which
 * geometric constraints can be applied and build the matching
 * `sketch_constraint_*` sub-call. Only constraint/entity combinations
 * the planegcs solver actually supports are offered, so applying one
 * can never throw inside the live solve. Dimensional constraints
 * (P0.2) — Length/Distance and Angle — seed their value from the
 * geometry's current measurement so applying them never makes the
 * sketch jump; the value is editable afterwards via
 * `store.setSketchConstraintValue`. (Radius needs a kernel tool that
 * doesn't exist yet, so it isn't offered.)
 *
 * Pure and DOM-free so the rules are unit-tested directly.
 */

import type { Entity, SketchState } from "./solver/types";

/**
 * zz1r: the unified Dimension tool's `selectedTool` sentinel. It has no
 * fixed constraint type — it resolves the most-specific dimensional
 * constraint that fits the current selection (radius/diameter > angle >
 * distance) at apply time.
 */
export const CONSTRAINT_DIMENSION_TOOL = "sketch_constraint_dimension";

/**
 * Dimensional constraint types, most- to least-specific, so the unified
 * Dimension tool can pick the right one from the current selection. zz1r
 * (moved here from Toolbar so the store can resolve it too).
 */
export const DIMENSION_TYPES_BY_SPECIFICITY = [
  "sketch_constraint_radius",
  "sketch_constraint_diameter",
  "sketch_constraint_angle",
  "sketch_constraint_distance",
] as const;

/**
 * zz1r: a constraint TOOL is any `sketch_constraint_*` value used as the
 * session's `selectedTool` (including the Dimension sentinel above). With
 * a constraint tool active the canvas turns committed-entity clicks into a
 * running selection and applies the constraint the moment it fits — the
 * tool-first constrain workflow (pick the tool, then click the lines).
 */
export function isConstraintTool(tool: string | null | undefined): boolean {
  return typeof tool === "string" && tool.startsWith("sketch_constraint_");
}

export interface ConstraintOption {
  /** Sub-call type, e.g. `sketch_constraint_parallel`. */
  type: string;
  /** Short label for the palette button. */
  label: string;
  /** Single-glyph icon (mirrors FeatureDialog meta). */
  glyph: string;
  /** Tooltip describing what the constraint does. */
  title: string;
}

type Kind =
  | "point"
  | "line"
  | "circle"
  | "arc"
  | "ellipse"
  | "bspline"
  | "origin";

function kindOf(state: SketchState, id: number): Kind | null {
  const e = state.entities.find((x) => x.id === id);
  return e ? (e.kind as Kind) : null;
}

const isPointish = (k: Kind | null): boolean => k === "point" || k === "origin";
// `isCurvish` gates the constraints that only make sense on a circle /
// arc — radius, diameter, equal-radii, concentric. Ellipses are
// deliberately excluded here: PlaneGCS has no plain radius/equal-radii
// for them in our wiring.
const isCurvish = (k: Kind | null): boolean => k === "circle" || k === "arc";
// `isConicCurve` is the broader set that can host a tangent or
// point-on-curve constraint — circle, arc, AND ellipse.
const isConicCurve = (k: Kind | null): boolean =>
  k === "circle" || k === "arc" || k === "ellipse";
// `isOnCurve` is the set a point can be constrained ONTO: the conics
// plus B-splines (point-on-spline). B-splines stay out of
// `isConicCurve` because the line-tangent constraint needs an interior
// knot index we don't wire yet, so it isn't offered for them.
const isOnCurve = (k: Kind | null): boolean =>
  isConicCurve(k) || k === "bspline";

function entityOf(state: SketchState, id: number): Entity | undefined {
  return state.entities.find((x) => x.id === id);
}

/** World position of a point-like entity. The origin sits at (0, 0)
 *  and carries no `at`, so it's special-cased. Returns null for
 *  non-point entities. */
function pointPos(state: SketchState, id: number): [number, number] | null {
  const e = entityOf(state, id);
  if (!e) return null;
  if (e.kind === "origin") return [0, 0];
  if (e.kind === "point") return [e.at.x, e.at.y];
  return null;
}

/** Radius of a circle or arc entity, or null for anything else. */
function curveRadius(state: SketchState, id: number): number | null {
  const e = entityOf(state, id);
  if (!e || (e.kind !== "circle" && e.kind !== "arc")) return null;
  return e.radius;
}

/** The two endpoint ids of a line entity, or null. */
function lineEndpoints(
  state: SketchState,
  id: number,
): [number, number] | null {
  const e = entityOf(state, id);
  if (!e || e.kind !== "line") return null;
  return [e.start, e.end];
}

/** Direction angle (radians) of a line, start→end, matching the
 *  planegcs `l2l_angle_ll` convention (atan2 of the endpoint delta). */
function lineAngle(state: SketchState, id: number): number | null {
  const ends = lineEndpoints(state, id);
  if (!ends) return null;
  const a = pointPos(state, ends[0]);
  const b = pointPos(state, ends[1]);
  if (!a || !b) return null;
  return Math.atan2(b[1] - a[1], b[0] - a[0]);
}

/** Wrap an angle in degrees to (−180, 180]. */
function wrapDeg(deg: number): number {
  let d = deg % 360;
  if (d > 180) d -= 360;
  if (d <= -180) d += 360;
  return d;
}

/** Trim floating-point noise from a measured dimension so the value
 *  shown in the editable field is clean and a no-op blur doesn't
 *  re-trigger the solver. 1e-4 mm / deg is far below any meaningful
 *  geometric tolerance. */
function roundDim(n: number): number {
  return Math.round(n * 1e4) / 1e4;
}

/**
 * Which constraints can apply to the current selection. Order is the
 * palette display order. Empty when the selection fits nothing.
 */
export function applicableConstraints(
  state: SketchState,
  selection: ReadonlyArray<number>,
): ConstraintOption[] {
  const kinds = selection.map((id) => kindOf(state, id));
  if (kinds.some((k) => k === null)) return [];
  const n = kinds.length;
  const lines = kinds.filter((k) => k === "line").length;
  const points = kinds.filter(isPointish).length;
  const curves = kinds.filter(isCurvish).length;
  // Curves eligible for tangent (circle, arc, ellipse).
  const conicCurves = kinds.filter(isConicCurve).length;
  // Curves a point can be constrained onto (conics + B-splines).
  const onCurves = kinds.filter(isOnCurve).length;
  const out: ConstraintOption[] = [];

  if (n === 1 && lines === 1) {
    out.push({
      type: "sketch_constraint_horizontal",
      label: "Horizontal",
      glyph: "—",
      title: "Make this line horizontal",
    });
    out.push({
      type: "sketch_constraint_vertical",
      label: "Vertical",
      glyph: "|",
      title: "Make this line vertical",
    });
    // P0.2: a length dimension on a single line drives the distance
    // between its two endpoints.
    out.push({
      type: "sketch_constraint_distance",
      label: "Length",
      glyph: "↔",
      title: "Set the length of this line",
    });
  }

  if (n === 2 && lines === 2) {
    out.push({
      type: "sketch_constraint_parallel",
      label: "Parallel",
      glyph: "∥",
      title: "Make the two lines parallel",
    });
    out.push({
      type: "sketch_constraint_perpendicular",
      label: "Perpendicular",
      glyph: "⊥",
      title: "Make the two lines perpendicular",
    });
    // P0.2: an angle dimension between two lines.
    out.push({
      type: "sketch_constraint_angle",
      label: "Angle",
      glyph: "∠",
      title: "Set the angle between the two lines",
    });
  }

  if (n === 2 && points === 2) {
    out.push({
      type: "sketch_constraint_coincident",
      label: "Coincident",
      glyph: "⊙",
      title: "Make the two points coincident",
    });
    // P0.2: a distance dimension between two points.
    out.push({
      type: "sketch_constraint_distance",
      label: "Distance",
      glyph: "↔",
      title: "Set the distance between the two points",
    });
  }

  // A single point pins at its current position via a fix constraint.
  if (n === 1 && points === 1) {
    out.push({
      type: "sketch_constraint_fix",
      label: "Fix",
      glyph: "⌖",
      title: "Fix this point at its position",
    });
  }

  if (n === 1 && curves === 1) {
    // A single circle or arc takes a radius or diameter dimension.
    out.push({
      type: "sketch_constraint_radius",
      label: "Radius",
      glyph: "⟜",
      title: "Set the radius of this circle or arc",
    });
    out.push({
      type: "sketch_constraint_diameter",
      label: "Diameter",
      glyph: "⌀",
      title: "Set the diameter of this circle or arc",
    });
  }

  if (n === 2 && curves === 2) {
    out.push({
      type: "sketch_constraint_equal",
      label: "Equal",
      glyph: "=",
      title: "Make the two radii equal",
    });
    out.push({
      type: "sketch_constraint_concentric",
      label: "Concentric",
      glyph: "⊚",
      title: "Make the two circles/arcs concentric",
    });
  }

  if (n === 2 && lines === 1 && conicCurves === 1) {
    out.push({
      type: "sketch_constraint_tangent",
      label: "Tangent",
      glyph: "⌒",
      title: "Make the line tangent to the curve",
    });
  }

  // A point plus a line pins the point at the line's midpoint.
  if (n === 2 && points === 1 && lines === 1) {
    out.push({
      type: "sketch_constraint_midpoint",
      label: "Midpoint",
      glyph: "◎",
      title: "Pin the point at the midpoint of the line",
    });
  }

  // A point plus a line/circle/arc constrains the point onto that
  // curve. Additive to Midpoint for a point+line; the sole option for
  // a point+circle/arc.
  if (n === 2 && points === 1 && (lines === 1 || onCurves === 1)) {
    out.push({
      type: "sketch_constraint_point_on_object",
      label: "On Curve",
      glyph: "⊢",
      title: "Constrain the point onto the line/circle/arc/ellipse/spline",
    });
  }

  // Two points plus a line make the points mirror across that line.
  if (n === 3 && points === 2 && lines === 1) {
    out.push({
      type: "sketch_constraint_symmetric",
      label: "Symmetric",
      glyph: "⇆",
      title: "Make the two points symmetric across the line",
    });
  }

  return out;
}

/**
 * Build the sub-call for `type` from the current selection, mapping
 * entity ids onto the kernel's per-constraint param names. Returns
 * `null` when the selection doesn't match the constraint (the UI
 * gates this via `applicableConstraints`, but the guard keeps the
 * builder safe to call directly).
 */
export function buildConstraintCall(
  type: string,
  selection: ReadonlyArray<number>,
  state: SketchState,
): { type: string; params: Record<string, unknown> } | null {
  const kinds = selection.map((id) => kindOf(state, id));
  if (kinds.some((k) => k === null)) return null;

  switch (type) {
    case "sketch_constraint_horizontal":
    case "sketch_constraint_vertical":
      if (selection.length !== 1 || kinds[0] !== "line") return null;
      return { type, params: { line: selection[0] } };

    case "sketch_constraint_parallel":
    case "sketch_constraint_perpendicular":
      if (
        selection.length !== 2 ||
        kinds[0] !== "line" ||
        kinds[1] !== "line"
      ) {
        return null;
      }
      return { type, params: { line_a: selection[0], line_b: selection[1] } };

    case "sketch_constraint_coincident":
      if (
        selection.length !== 2 ||
        !isPointish(kinds[0]) ||
        !isPointish(kinds[1])
      ) {
        return null;
      }
      return { type, params: { a: selection[0], b: selection[1] } };

    case "sketch_constraint_equal":
      if (
        selection.length !== 2 ||
        !isCurvish(kinds[0]) ||
        !isCurvish(kinds[1])
      ) {
        return null;
      }
      return {
        type,
        params: { entity_a: selection[0], entity_b: selection[1] },
      };

    case "sketch_constraint_concentric":
      if (
        selection.length !== 2 ||
        !isCurvish(kinds[0]) ||
        !isCurvish(kinds[1])
      ) {
        return null;
      }
      return { type, params: { a: selection[0], b: selection[1] } };

    case "sketch_constraint_distance": {
      // Two shapes drive a p2p_distance: a single line (its two
      // endpoints) or two picked points. Either way the initial
      // value is the current measured distance, so applying the
      // dimension never makes the geometry jump.
      let pa: number;
      let pb: number;
      if (selection.length === 1 && kinds[0] === "line") {
        const ends = lineEndpoints(state, selection[0]);
        if (!ends) return null;
        [pa, pb] = ends;
      } else if (
        selection.length === 2 &&
        isPointish(kinds[0]) &&
        isPointish(kinds[1])
      ) {
        [pa, pb] = [selection[0], selection[1]];
      } else {
        return null;
      }
      const a = pointPos(state, pa);
      const b = pointPos(state, pb);
      if (!a || !b) return null;
      const value_mm = roundDim(Math.hypot(b[0] - a[0], b[1] - a[1]));
      return {
        type,
        params: { entity_a: pa, entity_b: pb, value_mm },
      };
    }

    case "sketch_constraint_angle": {
      if (
        selection.length !== 2 ||
        kinds[0] !== "line" ||
        kinds[1] !== "line"
      ) {
        return null;
      }
      const angA = lineAngle(state, selection[0]);
      const angB = lineAngle(state, selection[1]);
      if (angA === null || angB === null) return null;
      const value_deg = roundDim(wrapDeg(((angB - angA) * 180) / Math.PI));
      return {
        type,
        params: { line_a: selection[0], line_b: selection[1], value_deg },
      };
    }

    case "sketch_constraint_tangent": {
      if (selection.length !== 2) return null;
      // Order-independent: find the line and the curve.
      const lineIdx = kinds.findIndex((k) => k === "line");
      const curveIdx = kinds.findIndex(isConicCurve);
      if (lineIdx === -1 || curveIdx === -1 || lineIdx === curveIdx)
        return null;
      return {
        type,
        params: { line: selection[lineIdx], curve: selection[curveIdx] },
      };
    }

    case "sketch_constraint_fix": {
      if (selection.length !== 1 || !isPointish(kinds[0])) return null;
      return { type, params: { point: selection[0] } };
    }

    case "sketch_constraint_point_on_object": {
      if (selection.length !== 2) return null;
      // Order-independent: exactly one point-like, and exactly one of
      // {line, circle, arc} as the host curve.
      const pointIdx = kinds.findIndex(isPointish);
      const curveIdx = kinds.findIndex((k) => k === "line" || isOnCurve(k));
      if (pointIdx === -1 || curveIdx === -1 || pointIdx === curveIdx) {
        return null;
      }
      return {
        type,
        params: { point: selection[pointIdx], curve: selection[curveIdx] },
      };
    }

    case "sketch_constraint_radius": {
      if (selection.length !== 1 || !isCurvish(kinds[0])) return null;
      const r = curveRadius(state, selection[0]);
      if (r === null) return null;
      // Seed with the current radius so applying it never makes the
      // geometry jump; editable afterwards in the constraint manager.
      return { type, params: { entity: selection[0], value_mm: roundDim(r) } };
    }

    case "sketch_constraint_diameter": {
      if (selection.length !== 1 || !isCurvish(kinds[0])) return null;
      const r = curveRadius(state, selection[0]);
      if (r === null) return null;
      // Stored value is the diameter (2·radius).
      return {
        type,
        params: { entity: selection[0], value_mm: roundDim(2 * r) },
      };
    }

    case "sketch_constraint_midpoint": {
      if (selection.length !== 2) return null;
      // Order-independent: one point, one line.
      const pointIdx = kinds.findIndex(isPointish);
      const lineIdx = kinds.findIndex((k) => k === "line");
      if (pointIdx === -1 || lineIdx === -1) return null;
      return {
        type,
        params: { point: selection[pointIdx], line: selection[lineIdx] },
      };
    }

    case "sketch_constraint_symmetric": {
      if (selection.length !== 3) return null;
      // Order-independent: two points and the axis line.
      const lineIdx = kinds.findIndex((k) => k === "line");
      if (lineIdx === -1) return null;
      const pts = selection.filter((_, i) => isPointish(kinds[i]));
      if (pts.length !== 2) return null;
      return {
        type,
        params: {
          entity_a: pts[0],
          entity_b: pts[1],
          axis: selection[lineIdx],
        },
      };
    }

    default:
      return null;
  }
}
