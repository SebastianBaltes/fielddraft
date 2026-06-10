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

/** A picked element: its kind plus the stable decimal id_hash. */
export interface PickedElement {
  kind: ElementKind;
  idHash: string;
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

/** Build the canonical element-query string addressing one element. */
export function elementQueryString(el: PickedElement): string {
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
