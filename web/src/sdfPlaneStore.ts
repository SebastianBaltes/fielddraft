/**
 * Debug SDF-plane bridge.
 *
 * While the (non-modal) "Debug ▸ SDF-Plane" dialog is open it publishes the
 * plane parameters (a point + a normal + an in-plane half-extent) here. The
 * viewport then samples the scene SDF on that plane and draws it as a
 * colour-coded quad (blue inside / red outside, iso-contour bands, a bright
 * zero-surface line) — a diagnostic window into exactly the field the meshers
 * contour, so grid-sampling artefacts (thin features, spurious crossings) are
 * visible in the raw distance field.
 *
 * Plain module-scoped singleton with a `useSyncExternalStore`-friendly
 * subscribe/getSnapshot pair (like the export-preview bridge): the dialog and
 * the R3F overlay both need cheap synchronous access across the Canvas
 * boundary, and there is only ever one debug plane at a time.
 */

export interface SdfPlane {
  /** World-space point the plane passes through. */
  point: [number, number, number];
  /** Plane normal (need not be unit — the kernel normalises it). */
  normal: [number, number, number];
  /** Half the plane's side length, in mm. */
  halfExtent: number;
  /** Colour-coding sampling resolution (grid is `resolution × resolution`). */
  resolution: number;
  /** Number of iso-distance bands across the half-extent (spacing = he / bands). */
  bands: number;
}

let plane: SdfPlane | null = null;
const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of listeners) listener();
}

/** Publish the debug plane (dialog open), or `null` to hide it (closed). */
export function setSdfPlane(next: SdfPlane | null): void {
  plane = next;
  notify();
}

/** Current debug plane, or `null` when nothing should be drawn. */
export function getSdfPlane(): SdfPlane | null {
  return plane;
}

export function subscribeSdfPlane(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// ── Hover readout ────────────────────────────────────────────────────
// The overlay projects the mouse onto the plane and publishes the SDF value
// there; the dialog subscribes and shows it. Separate slice so a moving cursor
// doesn't re-render the plane geometry.

export interface SdfPlaneHover {
  /** Sampled SDF value at the cursor's projection onto the plane (mm). */
  value: number;
  /** World point on the plane under the cursor. */
  point: [number, number, number];
}

let hover: SdfPlaneHover | null = null;
const hoverListeners = new Set<() => void>();

export function setSdfPlaneHover(next: SdfPlaneHover | null): void {
  // Skip no-op updates so `useSyncExternalStore` stays stable when idle.
  if (next === null && hover === null) return;
  hover = next;
  for (const listener of hoverListeners) listener();
}

export function getSdfPlaneHover(): SdfPlaneHover | null {
  return hover;
}

export function subscribeSdfPlaneHover(listener: () => void): () => void {
  hoverListeners.add(listener);
  return () => {
    hoverListeners.delete(listener);
  };
}
