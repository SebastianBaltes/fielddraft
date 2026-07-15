/**
 * Debug ▸ Voxel bridge (#369).
 *
 * Module-scoped singleton with a `useSyncExternalStore`-friendly
 * subscribe/getSnapshot pair (like `mesherDebugStore`). It publishes the voxel
 * DISPLAY state the viewport (`VoxelOverlay`) reads:
 *
 *   · `solidActive` — the Voxel dialog (`VoxelDialog`) is open ⇒ draw the
 *     current scene's occupancy as solid cubes (the overlay voxelises live,
 *     reacting to geometry changes). Set on mount / cleared on unmount by the
 *     dialog itself (#379), mirroring how `mesherDebugStore`/`lichenDebugStore`
 *     scope their overlays to their dialog's lifetime — NOT a render mode.
 *   · `diff` — the dialog / console published a before/after diff ⇒ draw the
 *     added voxels green and the removed voxels red (unchanged optionally grey).
 *
 * The requested resolution is persisted so it has a value even before the
 * dialog is ever opened.
 */

const RES_KEY = "fielddraft.debug.voxel.resolution";
export const DEFAULT_VOXEL_RESOLUTION = 48;
export const MIN_VOXEL_RESOLUTION = 4;
export const MAX_VOXEL_RESOLUTION = 256;

/** A published diff to render as green (added) / red (removed) voxels. Carries
 *  the shared grid geometry plus per-voxel masks and the summary counts. */
export interface VoxelDiffView {
  dims: [number, number, number];
  origin: [number, number, number];
  spacing: number;
  addedMask: Uint8Array;
  removedMask: Uint8Array;
  unchangedMask: Uint8Array;
  added: number;
  removed: number;
}

export interface VoxelDisplay {
  /** Voxel render mode active → draw the current scene's solid occupancy. */
  solidActive: boolean;
  /** Resolution (voxels along the longest AABB axis) for the solid fill. */
  resolution: number;
  /** Published diff overlay, or null. Takes visual priority over the solid. */
  diff: VoxelDiffView | null;
  /** Also draw the unchanged voxels (faint grey) in a diff. Default off. */
  showUnchanged: boolean;
  /** Cube edge as a fraction of the voxel spacing (a gap reads as a grid). */
  fillScale: number;
}

function loadResolution(): number {
  try {
    const raw = Number(localStorage.getItem(RES_KEY));
    if (Number.isFinite(raw) && raw >= MIN_VOXEL_RESOLUTION) {
      return Math.min(MAX_VOXEL_RESOLUTION, Math.round(raw));
    }
  } catch {
    // no localStorage (tests) — fall through to default
  }
  return DEFAULT_VOXEL_RESOLUTION;
}

let display: VoxelDisplay = {
  solidActive: false,
  resolution: loadResolution(),
  diff: null,
  showUnchanged: false,
  fillScale: 0.92,
};

const listeners = new Set<() => void>();

function publish(next: Partial<VoxelDisplay>): void {
  display = { ...display, ...next };
  for (const l of listeners) l();
}

export function getVoxelDisplay(): VoxelDisplay {
  return display;
}

export function subscribeVoxelDisplay(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Current resolution (voxels along the longest scene-AABB axis). Read
 *  imperatively by the console API and the solid overlay. */
export function getVoxelResolution(): number {
  return display.resolution;
}

export function setVoxelResolution(n: number): void {
  const clamped = Math.min(
    MAX_VOXEL_RESOLUTION,
    Math.max(MIN_VOXEL_RESOLUTION, Math.round(n)),
  );
  if (clamped === display.resolution) return;
  try {
    localStorage.setItem(RES_KEY, String(clamped));
  } catch {
    // non-persistent is fine
  }
  publish({ resolution: clamped });
}

/** The Voxel dialog's open state toggles this (set true on mount, false on
 *  unmount) — the overlay then draws the current scene's occupancy as solid
 *  cubes for as long as the dialog stays open. */
export function setVoxelSolidActive(active: boolean): void {
  if (active === display.solidActive) return;
  publish({ solidActive: active });
}

export function setVoxelDiff(view: VoxelDiffView | null): void {
  publish({ diff: view });
}

export function setVoxelShowUnchanged(show: boolean): void {
  if (show === display.showUnchanged) return;
  publish({ showUnchanged: show });
}
