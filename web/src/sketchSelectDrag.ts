/**
 * Ticket matf — Select-mode drag planning.
 *
 * Plain Select mode (no tool active) lets the user grab geometry and drag it.
 * Two distinct gestures share the same pointer-down:
 *
 *  - **grip drag** — grab a precise *handle* (a free point, a line endpoint, a
 *    polygon/spline vertex, a circle/arc centre or rim) and reshape the entity
 *    by moving just that degree of freedom. This is the ticket-t11x behaviour.
 *
 *  - **selection translate** — grab the *body* of an entity (a line segment, a
 *    circle/arc rim away from a handle, a region fill) or grab anything while a
 *    MULTI-selection is live, and rigidly translate every selected entity by
 *    the drag delta. The live solver still has the final say: constraints that
 *    forbid the motion pull the geometry back (ticket matf).
 *
 * This module is the pure decision layer: given what the pointer-down landed on
 * (a picked grip handle and/or a hit entity id) plus the current selection, it
 * returns which gesture to start and which entity ids the translate should
 * carry. Keeping it DOM-free lets Vitest exercise every branch.
 */

export type SelectDragPlan =
  | { kind: "none" }
  /** Reshape one handle's degree of freedom (ticket t11x). `selectId` is the
   *  entity to mark selected so its grips/highlight show during the drag. */
  | { kind: "grip"; selectId: number | null }
  /** Rigidly translate `entityIds` together (ticket matf). `selectIds` is the
   *  selection to set before the drag so the moved set highlights. */
  | { kind: "translate"; entityIds: number[]; selectIds: number[] };

export interface SelectDragInput {
  /** The current committed-entity selection (entity ids). */
  selection: ReadonlyArray<number>;
  /** Entity id of a precise handle directly under the pointer, or null. A
   *  handle is a point/vertex/centre — a deliberate reshape target.
   *
   *  Note: for a curve whose vertices are their OWN entities (a line's
   *  endpoints are point entities distinct from the line), this resolves to
   *  the most-specific handle — the endpoint POINT, not the owning line. That
   *  differs from the selection a user makes by clicking the curve body, hence
   *  {@link handleOnSelection} below. */
  handleEntityId: number | null;
  /** Entity id of whatever the broad entity hit-test found under the pointer
   *  (curve body, region, point), or null. */
  hitEntityId: number | null;
  /** True when the grabbed handle is a grip of a currently-selected entity —
   *  i.e. it was found among the selection's own grips. Grabbing the endpoint
   *  of the selected line reshapes it even though the endpoint is a distinct
   *  point entity from the line (`handleEntityId` won't equal the selection). */
  handleOnSelection: boolean;
}

/**
 * Decide the Select-mode drag gesture for a pointer-down.
 *
 * Rules, in priority order:
 *  1. A handle under the cursor while the selection is empty or is just that
 *     handle's own entity → **grip** reshape (precise single-DoF edit wins, so
 *     dragging a lone point/vertex still tweaks it, exactly as before).
 *  2. The cursor is over an entity that is part of a live MULTI-selection →
 *     **translate** the whole selection (drag the group as one).
 *  3. The cursor is over an entity body (no precise handle, or a handle whose
 *     entity isn't the sole selection) → **translate** that single entity,
 *     replacing the selection with it.
 *  4. Nothing under the cursor → **none** (caller starts a marquee / clears).
 *
 * The translate path always carries the dragged entity even if it wasn't
 * pre-selected, so a first click-drag on a line moves that line.
 */
export function planSelectDrag(input: SelectDragInput): SelectDragPlan {
  const { selection, handleEntityId, hitEntityId, handleOnSelection } = input;

  // The entity actually under the cursor: a precise handle wins as the drag
  // target identity, else the broad hit.
  const targetId = handleEntityId ?? hitEntityId;

  // Dragging inside a live multi-selection moves the whole group — even when
  // the grab landed on a handle (the user grabbed the group, not one DoF).
  if (
    selection.length > 1 &&
    ((targetId !== null && selection.includes(targetId)) || handleOnSelection)
  ) {
    return { kind: "translate", entityIds: [...selection], selectIds: [...selection] };
  }

  // A precise handle, with no competing multi-selection: reshape that DoF.
  if (handleEntityId !== null) {
    const soleSelf =
      selection.length === 0 ||
      (selection.length === 1 &&
        (selection[0] === handleEntityId || handleOnSelection));
    // Keep the current selection when reshaping a handle that belongs to the
    // already-selected curve (grabbing a line's endpoint shouldn't reselect
    // the bare endpoint point); otherwise mark the handle's own entity.
    if (soleSelf) {
      const selectId =
        selection.length === 1 && selection[0] !== handleEntityId
          ? null
          : handleEntityId;
      return { kind: "grip", selectId };
    }
  }

  // An entity body (curve / region) under the cursor: translate just it,
  // making it the new selection.
  if (hitEntityId !== null) {
    // If it's already part of the (single-item) selection, keep the selection;
    // otherwise replace it with the freshly grabbed entity.
    const selIds =
      selection.includes(hitEntityId) && selection.length > 0
        ? [...selection]
        : [hitEntityId];
    return { kind: "translate", entityIds: [hitEntityId], selectIds: selIds };
  }

  return { kind: "none" };
}
