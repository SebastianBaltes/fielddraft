/**
 * Shared display formatting for `qCreatedBy(...)` query strings.
 *
 * Used by the Outliner (plane labels on sketch/feature rows) and the
 * query widgets (selection-input chips). Kept in one place so the
 * origin-plane labels and the `qCreatedBy(…)` parsing don't drift.
 */

export const QUERY_DISPLAY_LABELS: Record<string, string> = {
  "qCreatedBy(origin_plane_xy)": "XY Plane",
  "qCreatedBy(origin_plane_xz)": "XZ Plane",
  "qCreatedBy(origin_plane_yz)": "YZ Plane",
};

/**
 * Turn a query expression into a human label. Known origin planes map to
 * "XY/XZ/YZ Plane"; a `qCreatedBy(<id>)` whose inner value is a feature id
 * present in `featureNames` resolves to that feature's name; otherwise the
 * inner token is de-underscored. Non-matching input is returned verbatim.
 */
export function formatQueryDisplay(
  raw: string,
  featureNames?: Map<number, string>,
): string {
  if (QUERY_DISPLAY_LABELS[raw]) return QUERY_DISPLAY_LABELS[raw];
  const match = raw.match(/^qCreatedBy\((.+)\)$/);
  if (match) {
    const inner = match[1];
    if (featureNames) {
      const id = Number(inner);
      if (!Number.isNaN(id) && featureNames.has(id)) return featureNames.get(id)!;
    }
    return inner.replace(/_/g, " ");
  }
  return raw;
}
