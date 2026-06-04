/**
 * Explicit-constraint palette logic (P0.1).
 *
 * Given the entities the user picked in select mode, decide which
 * geometric constraints can be applied and build the matching
 * `sketch_constraint_*` sub-call. Only constraint/entity combinations
 * the planegcs solver actually supports are offered, so applying one
 * can never throw inside the live solve. Dimensional constraints
 * (distance, angle, radius) need a numeric value and are handled by
 * the editable-dimensions slice (P0.2), not here.
 *
 * Pure and DOM-free so the rules are unit-tested directly.
 */

import type { SketchState } from "./solver/types";

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

type Kind = "point" | "line" | "circle" | "arc" | "origin";

function kindOf(state: SketchState, id: number): Kind | null {
  const e = state.entities.find((x) => x.id === id);
  return e ? (e.kind as Kind) : null;
}

const isPointish = (k: Kind | null): boolean => k === "point" || k === "origin";
const isCurvish = (k: Kind | null): boolean => k === "circle" || k === "arc";

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
  }

  if (n === 2 && points === 2) {
    out.push({
      type: "sketch_constraint_coincident",
      label: "Coincident",
      glyph: "⊙",
      title: "Make the two points coincident",
    });
  }

  if (n === 2 && curves === 2) {
    out.push({
      type: "sketch_constraint_equal",
      label: "Equal",
      glyph: "=",
      title: "Make the two radii equal",
    });
  }

  if (n === 2 && lines === 1 && curves === 1) {
    out.push({
      type: "sketch_constraint_tangent",
      label: "Tangent",
      glyph: "⌒",
      title: "Make the line tangent to the curve",
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
      if (selection.length !== 2 || kinds[0] !== "line" || kinds[1] !== "line") {
        return null;
      }
      return { type, params: { line_a: selection[0], line_b: selection[1] } };

    case "sketch_constraint_coincident":
      if (selection.length !== 2 || !isPointish(kinds[0]) || !isPointish(kinds[1])) {
        return null;
      }
      return { type, params: { a: selection[0], b: selection[1] } };

    case "sketch_constraint_equal":
      if (selection.length !== 2 || !isCurvish(kinds[0]) || !isCurvish(kinds[1])) {
        return null;
      }
      return { type, params: { entity_a: selection[0], entity_b: selection[1] } };

    case "sketch_constraint_tangent": {
      if (selection.length !== 2) return null;
      // Order-independent: find the line and the curve.
      const lineIdx = kinds.findIndex((k) => k === "line");
      const curveIdx = kinds.findIndex(isCurvish);
      if (lineIdx === -1 || curveIdx === -1 || lineIdx === curveIdx) return null;
      return {
        type,
        params: { line: selection[lineIdx], curve: selection[curveIdx] },
      };
    }

    default:
      return null;
  }
}
