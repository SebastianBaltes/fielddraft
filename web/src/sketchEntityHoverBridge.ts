/**
 * In-edit sketch-entity hover bridge (ticket zadt).
 *
 * While a sketch is open for editing in SELECT mode (no drawing tool active),
 * the 2D interaction canvas hit-tests the entity under the cursor on every
 * pointer-move and publishes its solver-entity id here. `SketchSession3D`
 * subscribes and draws that entity highlighted — the "this is what a click
 * will pick" affordance, mirroring the orange selection highlight.
 *
 * A plain module singleton + listener set, exactly like the hover slice of
 * `sketchLinePickBridge` — neither side threads React state through R3F's
 * Canvas boundary, and a publish is a no-op (no notify) when the id is
 * unchanged so pointer-move doesn't thrash the render.
 */

let hovered: number | null = null;
const listeners = new Set<() => void>();

/** The sketch entity id currently hovered in select mode, or `null`. */
export function getHoveredSketchEntity(): number | null {
  return hovered;
}

/** Publish (or clear with `null`) the hovered sketch entity. No-op — and no
 *  notify — when unchanged, so a stationary cursor over a line doesn't churn. */
export function setHoveredSketchEntity(next: number | null): void {
  if (next === hovered) return;
  hovered = next;
  for (const l of listeners) l();
}

/** Subscribe to hovered-entity changes. Returns an unsubscribe fn. */
export function subscribeHoveredSketchEntity(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
