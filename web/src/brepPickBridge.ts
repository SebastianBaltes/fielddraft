/**
 * B-Rep element hover bridge (Brocken B-W).
 *
 * The viewport overlays resolve which B-Rep element the cursor is over on
 * each frame and publish it here. The Viewport's click handler reads the
 * current hovered element synchronously to emit the matching element-level
 * query string (`qFace/qEdge/qVertex` — see `elementQuery.ts`), without
 * threading React state through the R3F Canvas boundary.
 *
 * Two overlays publish, by kind, so neither clobbers the other:
 *   - `BRepSelectionOverlay` owns FACE + VERTEX (its own raycast/markers).
 *   - `BRepEdgeOverlay` owns EDGE (signature-matched against the Rust pick).
 * `getHoveredElement` merges them by priority vertex > edge > face — the
 * MORE specific element wins (ticket evq2, the usual CAD convention): a
 * cursor near a corner picks the corner, near an edge the edge, and only
 * the face interior picks the face. The face under the cursor is otherwise
 * hit almost everywhere and would shadow both.
 *
 * A plain module-scoped singleton, exactly like `queryPickerBridge`.
 */

import type { ElementKind, PickedElement } from "./elementQuery";

const slots: Record<ElementKind, PickedElement | null> = {
  face: null,
  edge: null,
  vertex: null,
};

/** Publish (or clear, with `null`) the hovered element for one kind. */
export function setHoveredElementOfKind(
  kind: ElementKind,
  el: PickedElement | null,
): void {
  slots[kind] = el && el.kind === kind ? el : null;
}

/**
 * The element the cursor is currently over, or `null`. Priority vertex >
 * edge > face — the most specific element under the cursor wins, so corners
 * and edges stay clickable on top of the face that bounds them.
 */
export function getHoveredElement(): PickedElement | null {
  return slots.vertex ?? slots.edge ?? slots.face ?? null;
}

/** The hovered element of a specific kind, or `null`. Used by pickers that
 * only accept one kind (e.g. the fillet tool wants the EDGE under the
 * cursor, even when a face in front would otherwise win the merge). */
export function getHoveredElementOfKind(
  kind: ElementKind,
): PickedElement | null {
  return slots[kind];
}
