/**
 * Debug (#350) — fillet-zone visualisation.
 *
 * Which `fillet_edge` features currently show their fillet-zone overlay: the
 * masked capsule/pill around the rounded edge (`Kernel.filletZones()`). Toggled
 * per fillet from its detail panel in the Outliner; read by `FilletZoneOverlay`.
 *
 * Module-scoped singleton with a `useSyncExternalStore`-friendly
 * subscribe/getSnapshot pair (like `partAabbStore`). The snapshot is a stable
 * `Set` reference that only changes on a real toggle, so subscribers re-render
 * exactly when the shown set changes.
 */

let shownIds: ReadonlySet<number> = new Set();
const listeners = new Set<() => void>();

export function getFilletZoneIds(): ReadonlySet<number> {
  return shownIds;
}

export function isFilletZoneShown(featureId: number): boolean {
  return shownIds.has(featureId);
}

export function setFilletZoneShown(featureId: number, shown: boolean): void {
  if (shown === shownIds.has(featureId)) return;
  const next = new Set(shownIds);
  if (shown) next.add(featureId);
  else next.delete(featureId);
  shownIds = next;
  listeners.forEach((l) => l());
}

export function subscribeFilletZones(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
