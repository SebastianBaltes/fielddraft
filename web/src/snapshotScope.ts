/**
 * Snapshot scope for viewport query pickers (tickets 9wxs / 2ukj).
 *
 * Feature-tree snapshot semantics: while editing one feature, only geometry
 * from features BEFORE it in the tree may be referenced. A feature can never
 * take as input the bodies it (or any later feature) produces — that would be a
 * forward reference (e.g. Extrude 2's profile field must not be able to pick the
 * very B-Reps Extrude 2 itself creates, ticket 2ukj).
 *
 * This is the single pure source of truth shared by `MultiQueryWidget` and
 * `QueryWidget` (via the `useSelectableFeatureIds` hook) and exercised directly
 * by unit tests. It returns the SET of allowed owner-feature ids, or `undefined`
 * when there is no single edited feature (→ no restriction).
 */

/** Minimal shape needed from the kernel store to compute the scope. */
export interface SnapshotScopeState {
  /** The currently selected feature ids (the edited feature is the sole one). */
  selection: ReadonlySet<number>;
  /** The feature tree, in tree order. */
  features: ReadonlyArray<{ id: number }>;
}

/**
 * The set of feature ids whose output a picker may reference while editing the
 * single selected feature, or `undefined` (no restriction) when zero or more
 * than one feature is selected, or the selected feature isn't in the tree.
 *
 * "Allowed" = every feature strictly BEFORE the edited one in tree order. The
 * edited feature itself and everything after it are excluded, so a click on the
 * edited feature's own output body (or a later body) is rejected by the
 * viewport pick guard.
 */
export function computeSelectableFeatureIds(
  state: SnapshotScopeState | null | undefined,
): ReadonlySet<number> | undefined {
  if (!state || state.selection.size !== 1) return undefined;
  const editedId = [...state.selection][0];
  const idx = state.features.findIndex((f) => f.id === editedId);
  if (idx < 0) return undefined;
  return new Set(state.features.slice(0, idx).map((f) => f.id));
}
