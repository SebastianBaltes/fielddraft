/**
 * Debug ▸ Part-AABB bridge.
 *
 * A tiny module-scoped singleton (like `mesherDebugStore`) holding one flag:
 * whether the axis-aligned bounding box of the currently selected part(s) is
 * drawn in the viewport. Toggled from the Debug menu; the overlay
 * (`PartAabbOverlay`) and the readout (`PartAabbPanel`) both read it. Persisted
 * to localStorage so it survives reloads.
 *
 * `useSyncExternalStore`-friendly subscribe/getSnapshot pair.
 */

const KEY = "fielddraft.debug.partAabb";

function load(): boolean {
  try {
    return localStorage.getItem(KEY) === "true";
  } catch {
    return false;
  }
}

let visible = load();
const listeners = new Set<() => void>();

export function getPartAabbVisible(): boolean {
  return visible;
}

export function setPartAabbVisible(next: boolean): void {
  if (visible === next) return;
  visible = next;
  try {
    localStorage.setItem(KEY, next ? "true" : "false");
  } catch {
    // Storage unavailable (private mode) — keep the in-memory flag.
  }
  for (const l of listeners) l();
}

export function togglePartAabbVisible(): void {
  setPartAabbVisible(!visible);
}

export function subscribePartAabb(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
