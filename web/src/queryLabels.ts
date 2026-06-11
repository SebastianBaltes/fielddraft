/**
 * Shared display formatting for `qCreatedBy(...)` query strings.
 *
 * Used by the Outliner (plane labels on sketch/feature rows) and the
 * query widgets (selection-input chips). Kept in one place so the
 * origin-plane labels and the `qCreatedBy(…)` parsing don't drift.
 */

export const QUERY_DISPLAY_LABELS: Record<string, string> = {
  // Named like the Default-geometry rows in the outliner and the labels
  // on the viewport datum quads — "Front" is what the user sees in 3D,
  // the axis pair is kept as a hint for query authors.
  "qCreatedBy(origin_plane_xy)": "Front (XY)",
  "qCreatedBy(origin_plane_xz)": "Top (XZ)",
  "qCreatedBy(origin_plane_yz)": "Right (YZ)",
};

/**
 * Query expression for a default datum plane, by outliner/viewport datum
 * id (`"front" | "top" | "right"`). The inverse of the naming above —
 * kept beside it so the two maps can't drift. `"origin"` (the origin
 * marker datum) has no plane query and returns undefined.
 */
export const DATUM_PLANE_QUERIES: Record<string, string> = {
  front: "qCreatedBy(origin_plane_xy)",
  top: "qCreatedBy(origin_plane_xz)",
  right: "qCreatedBy(origin_plane_yz)",
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
  // A picked face plane (ticket yx6a). The hash is meaningless to a human —
  // shorten it; resolving "Face of <feature>" needs a brep lookup the label
  // layer doesn't have.
  const face = raw.match(/^qFace\((\d+)\)$/);
  if (face) return `Face (${face[1].slice(0, 6)}…)`;
  return raw;
}
