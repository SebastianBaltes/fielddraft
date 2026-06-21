/**
 * Sketch sub-call list (ticket yx6a, Phase 1 of the Sketch-in-3D
 * rework — docs/SKETCH-IN-3D-PLAN.md).
 *
 * Extracted from the Outliner's session panel so the same list
 * renders in two places:
 *   - the ACTIVE session panel (interactive: row click selects the
 *     geometry, construction toggle, delete), and
 *   - the detail view of a COMMITTED sketch (read-only — the
 *     geometry is owned by the reentry session; handlers are
 *     simply omitted).
 *
 * The `.row` count contract stays geometry + constraints in one
 * list; section headers are non-`.row` list items.
 */

import type { SketchSessionState } from "./store";
import type { SketchDiagnostics } from "./solver/types";
import {
  constraintRefIds,
  idsOfSubCall,
  summarizeDiagnostics,
} from "./sketchSession";
import { FEATURE_TYPE_META } from "./widgets/FeatureDialog";

export const SKETCH_SUB_LABEL: Record<string, string> = {
  sketch_point: "Point",
  sketch_line: "Line",
  sketch_rectangle: "Rect",
  sketch_circle: "Circle",
  sketch_polygon: "Polygon",
  sketch_arc: "Arc",
  sketch_mirror: "Mirror",
  sketch_trim: "Trim",
  sketch_offset: "Offset",
  sketch_constraint_horizontal: "Horizontal",
  sketch_constraint_vertical: "Vertical",
  sketch_constraint_coincident: "Coincident",
  sketch_constraint_distance: "Distance",
  sketch_constraint_angle: "Angle",
  sketch_constraint_equal: "Equal",
  sketch_constraint_parallel: "Parallel",
  sketch_constraint_collinear: "Collinear",
  sketch_constraint_perpendicular: "Perpendicular",
  sketch_constraint_tangent: "Tangent",
  sketch_constraint_midpoint: "Midpoint",
  sketch_constraint_symmetric: "Symmetric",
  sketch_constraint_radius: "Radius",
  sketch_constraint_diameter: "Diameter",
};

function subCallGlyph(type: string): string {
  return FEATURE_TYPE_META[type]?.glyph ?? "?";
}

export interface SketchSubCallsProps {
  subCalls: SketchSessionState["subCalls"];
  diagnostics: SketchDiagnostics;
  /** Solver-entity ids currently selected — rows whose ids are all
   *  contained light up. Omit for the read-only detail view. */
  selection?: ReadonlyArray<number>;
  /** Row click → select the row's solver entities. Omitting it
   *  renders non-interactive rows. */
  onSelect?: (refIds: number[]) => void;
  /** Construction toggle per geometry row; omit to hide the button. */
  onToggleConstruction?: (index: number) => void;
  /** Delete per row; omit to hide the button. */
  onDelete?: (index: number) => void;
}

export function SketchSubCalls({
  subCalls,
  diagnostics,
  selection,
  onSelect,
  onToggleConstruction,
  onDelete,
}: SketchSubCallsProps) {
  if (subCalls.length === 0) return null;

  const selected = new Set(selection ?? []);
  const geometry: number[] = [];
  const constraints: number[] = [];
  subCalls.forEach((call, i) => {
    if (call.type.startsWith("sketch_constraint")) constraints.push(i);
    else geometry.push(i);
  });

  // F20: which constraint rows the last solve flagged.
  const diag = summarizeDiagnostics(subCalls, diagnostics);

  const renderRow = (i: number) => {
    const call = subCalls[i];
    const isConstraint = call.type.startsWith("sketch_constraint");
    const isConstruction = Boolean(call.params.construction);
    const refIds = isConstraint
      ? constraintRefIds(subCalls, i)
      : idsOfSubCall(subCalls, i);
    const isActive =
      refIds.length > 0 && refIds.every((id) => selected.has(id));
    const selectRow = onSelect ? () => onSelect(refIds) : undefined;
    return (
      <li
        key={i}
        className={
          "row" +
          (isConstraint ? " is-constraint" : "") +
          (isConstruction ? " is-construction" : "") +
          (isActive ? " is-active" : "")
        }
        role={selectRow ? "button" : undefined}
        tabIndex={selectRow ? 0 : undefined}
        aria-pressed={selectRow ? isActive : undefined}
        data-testid={`sketch-element-row-${i}`}
        onClick={selectRow}
        onKeyDown={
          selectRow
            ? (e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  selectRow();
                }
              }
            : undefined
        }
      >
        <span className="kind">
          {subCallGlyph(call.type)}{" "}
          {SKETCH_SUB_LABEL[call.type] ?? call.type.replace("sketch_", "")}
        </span>
        {isConstraint && diag.conflicts.has(i) ? (
          <span
            className="badge is-conflict"
            title="This constraint conflicts with the others — delete one of them to resolve the over-constrained sketch."
            data-testid={`sketch-constraint-conflict-${i}`}
          >
            conflict
          </span>
        ) : isConstraint && diag.redundants.has(i) ? (
          <span
            className="badge is-redundant"
            title="This constraint is redundant — it is already implied by the others and can be removed."
            data-testid={`sketch-constraint-redundant-${i}`}
          >
            redundant
          </span>
        ) : null}
        {!isConstraint && onToggleConstruction ? (
          <button
            type="button"
            className={"row-construction" + (isConstruction ? " is-active" : "")}
            title={
              isConstruction
                ? "Construction geometry (reference only) — click to make it solid"
                : "Make this construction geometry (reference only, not extruded)"
            }
            aria-pressed={isConstruction}
            data-testid={`sketch-sub-row-construction-${i}`}
            onClick={(e) => {
              e.stopPropagation();
              onToggleConstruction(i);
            }}
          >
            ⊘
          </button>
        ) : null}
        {onDelete ? (
          <button
            type="button"
            className="row-delete"
            title="Delete"
            data-testid={`sketch-sub-row-delete-${i}`}
            onClick={(e) => {
              e.stopPropagation();
              onDelete(i);
            }}
          >
            ×
          </button>
        ) : null}
      </li>
    );
  };

  return (
    <ul className="sub-spine elements" data-testid="sketch-sub-spine">
      {geometry.length > 0 ? (
        <li className="elements-header" data-testid="sketch-elements-geometry">
          Geometry ({geometry.length})
        </li>
      ) : null}
      {geometry.map(renderRow)}
      {constraints.length > 0 ? (
        <li
          className="elements-header"
          data-testid="sketch-elements-constraints"
        >
          Constraints ({constraints.length})
        </li>
      ) : null}
      {diag.conflictCount > 0 || diag.redundantCount > 0 ? (
        <li
          className={
            "diagnostics-summary" +
            (diag.conflictCount > 0 ? " has-conflict" : "")
          }
          data-testid="sketch-diagnostics-summary"
        >
          ⚠ {diag.conflictCount} conflicting, {diag.redundantCount} redundant
        </li>
      ) : null}
      {constraints.map(renderRow)}
    </ul>
  );
}
