/**
 * Element-level query mapping (Brocken B-W).
 *
 * The single place that turns a picked B-Rep element + the active query
 * picker's `entityFilter` into a canonical query string. The viewport's
 * click handler and the overlay components all funnel through here so the
 * mapping `(pickedElement, entityFilter) → queryString` lives in exactly
 * one testable spot.
 *
 * The produced strings are the FROZEN element-query contract:
 *   - `qFace(<idHashDecimal>)`
 *   - `qEdge(<idHashDecimal>)`
 *   - `qVertex(<idHashDecimal>)`
 * where `<idHashDecimal>` is the decimal-string `id_hash` the kernel
 * exports. Resolution of these strings is the Rust side's job; we only
 * PRODUCE them.
 */

/** A B-Rep element kind that can be addressed by a stable id_hash. */
export type ElementKind = "face" | "edge" | "vertex";

/** A picked element: its kind plus the stable decimal id_hash, and — when
 *  the kernel provided it — the canonical fingerprint-carrying query string
 *  (PERSISTENT-NAMING-PLAN N2). */
export interface PickedElement {
  kind: ElementKind;
  idHash: string;
  /** Kernel-built `qFace/qEdge/qVertex(<hash> @ <fingerprint>)`. Preferred
   *  over the hash-only fallback: it lets the stored reference heal after
   *  topological edits. The string is opaque to the web layer. */
  query?: string;
}

/**
 * Normalize a schema `entityFilter` (e.g. `"FACE"`, `"face"`, `"EDGE"`,
 * `"VERTEX"`, `"BODY"`, or `undefined`) to the lower-case element kinds it
 * permits. `undefined`/`""` means "no restriction" → all element kinds are
 * pickable. A `"BODY"` filter permits no element kinds (body/feature-level
 * pickers fall back to `qCreatedBy`, handled separately).
 */
export function allowedKinds(entityFilter?: string): ReadonlySet<ElementKind> {
  if (!entityFilter) return ALL_KINDS;
  const out = new Set<ElementKind>();
  for (const token of entityFilter.split(/[,\s|]+/)) {
    switch (token.trim().toUpperCase()) {
      case "FACE":
        out.add("face");
        break;
      case "EDGE":
        out.add("edge");
        break;
      case "VERTEX":
        out.add("vertex");
        break;
      default:
        // BODY (or anything unknown) contributes no element kind.
        break;
    }
  }
  return out;
}

const ALL_KINDS: ReadonlySet<ElementKind> = new Set<ElementKind>([
  "face",
  "edge",
  "vertex",
]);

/** Does the active picker's `entityFilter` allow picking `kind`? */
export function filterAllows(kind: ElementKind, entityFilter?: string): boolean {
  return allowedKinds(entityFilter).has(kind);
}

/**
 * Does the picker's `entityFilter` opt into committed sketch lines
 * (`qSketchLine`)? Sketch lines are NOT B-Rep elements — they have no
 * `id_hash`, so they sit outside {@link allowedKinds} and are addressed by
 * `(sketchId, lineId)` instead. A filter that contains the `SKETCHLINE`
 * token (e.g. the Revolve axis field's `"EDGE,SKETCHLINE"`) makes them
 * pickable; the viewport's sketch-line ray-cast is gated on this.
 */
export function filterAllowsSketchLine(entityFilter?: string): boolean {
  if (!entityFilter) return false;
  return entityFilter
    .split(/[,\s|]+/)
    .some((t) => t.trim().toUpperCase() === "SKETCHLINE");
}

/**
 * Does the picker's `entityFilter` opt into committed sketch REGIONS
 * (`qRegion`/`qRegions`/`qAllRegions`)? The Revolve/Extrude `profiles` field
 * carries the `SKETCHREGION` token (ticket dhjn): with it, a click on one of a
 * sketch's filled closed regions stores just that region instead of the whole
 * sketch. Sketch regions are NOT B-Rep elements (no id_hash), so they sit
 * outside {@link allowedKinds} — like sketch lines.
 */
export function filterAllowsSketchRegion(entityFilter?: string): boolean {
  if (!entityFilter) return false;
  return entityFilter
    .split(/[,\s|]+/)
    .some((t) => t.trim().toUpperCase() === "SKETCHREGION");
}

/**
 * Does a canonical query string satisfy a picker's `entityFilter`? The single
 * chokepoint that keeps a filtered picker from accepting the wrong kind of
 * pick, no matter which code path emits it (viewport, outliner row, datum
 * quad, …). Enforced inside the query widgets' `onPick`.
 *
 * `entityFilter === undefined`/`""` means no restriction → accept anything.
 * Otherwise each query form maps to the filter category it belongs to:
 *   - `qFace` → FACE; `qEdge`/`qEdgeBetween` → EDGE; `qVertex` → VERTEX
 *   - `qSketchLine` → SKETCHLINE
 *   - `qFaceOf`/`qRegion`/`qRegions`/`qAllRegions` → FACE (face-level)
 *   - `qCreatedBy(origin_plane_*)` → FACE (a datum plane is a face-like pick)
 *   - `qCreatedBy(<other>)` → ambiguous region-or-body → FACE or BODY
 * So an `"EDGE,SKETCHLINE"` axis picker accepts only `qEdge`/`qSketchLine` and
 * rejects datum planes (`qCreatedBy(origin_plane_*)`), bodies, faces, etc.,
 * while a FACE/BODY picker keeps its existing reach.
 */
export function queryAcceptedByFilter(query: string, entityFilter?: string): boolean {
  if (!entityFilter) return true;
  const tokens = entityFilter
    .split(/[,\s|]+/)
    .map((t) => t.trim().toUpperCase())
    .filter(Boolean);
  if (tokens.length === 0) return true;
  const has = (t: string) => tokens.includes(t);
  const name = query.trim().match(/^q([A-Za-z]+)\s*\(/)?.[1];
  switch (name) {
    case "Face":
      return has("FACE");
    case "Edge":
    case "EdgeBetween":
      return has("EDGE");
    case "Vertex":
      return has("VERTEX");
    case "SketchLine":
      return has("SKETCHLINE");
    case "FaceOf":
      return has("FACE");
    case "Region":
    case "Regions":
    case "AllRegions":
      // A sketch region satisfies a FACE picker (region = face-level) OR a
      // SKETCHREGION profile picker (ticket dhjn).
      return has("FACE") || has("SKETCHREGION");
    case "CreatedBy":
      // The role-filtered form `qCreatedBy(op, CAP_TOP)` etc. addresses
      // faces of one feature — a FACE-level query, never a body/region.
      if (/,\s*(WALL|CAP_TOP|CAP_BOTTOM|INNER_WALL|BLEND)\s*\)/.test(query)) {
        return has("FACE");
      }
      // A bare `qCreatedBy(sketch)` is the WHOLE-sketch profile — accepted by a
      // SKETCHREGION picker too (it's the "all regions" case).
      if (has("SKETCHREGION") && !/origin_plane_/.test(query)) return true;
      return /origin_plane_/.test(query) ? has("FACE") : has("FACE") || has("BODY");
    default:
      // Unknown / future query forms: don't block them.
      return true;
  }
}

/** Build the canonical element-query string addressing one element. The
 *  kernel-provided fingerprint-carrying form wins; the hash-only fallback
 *  covers picks from a kernel wasm that predates the `query` field. */
export function elementQueryString(el: PickedElement): string {
  if (el.query) return el.query;
  switch (el.kind) {
    case "face":
      return `qFace(${el.idHash})`;
    case "edge":
      return `qEdge(${el.idHash})`;
    case "vertex":
      return `qVertex(${el.idHash})`;
  }
}

/**
 * The core mapper: given a picked element and the active picker's
 * `entityFilter`, return the canonical query string, or `null` when the
 * filter rejects this element kind (the click should be ignored by the
 * picker). `entityFilter === undefined` allows every element kind.
 */
export function mapPickToQuery(
  el: PickedElement,
  entityFilter?: string,
): string | null {
  if (!filterAllows(el.kind, entityFilter)) return null;
  return elementQueryString(el);
}

/** What a tool-free viewport click on a hovered element should do
 *  (ticket evq2). Pure decision logic, exercised by the click handler:
 *  - an additive click (the controls preset's multi-select modifier)
 *    toggles the element in the multi-select set;
 *  - a plain click on the face that is ALREADY the selected element
 *    escalates to selecting the whole body (the Onshape-style
 *    "click again for the body" idiom from the ticket) — presets can
 *    turn this idiom off via `bodyEscalation` (docs/UX-PRESETS.md);
 *  - any other plain click selects the element.
 */
export type ElementClickAction = "toggle" | "select" | "escalate-body";

export function resolveElementClick(
  hovered: PickedElement,
  selected: PickedElement | null,
  additive: boolean,
  bodyEscalation = true,
): ElementClickAction {
  if (additive) return "toggle";
  if (
    bodyEscalation &&
    hovered.kind === "face" &&
    selected !== null &&
    selected.kind === "face" &&
    selected.idHash === hovered.idHash
  ) {
    return "escalate-body";
  }
  return "select";
}
