/**
 * Voxel snapshot + diff logic (#369) — pure data, no THREE / no React.
 *
 * A snapshot is the scene solid sampled on a regular cube grid (kernel
 * `voxelizeOccupancy` + `voxelGridInfo`). Two snapshots taken at the SAME
 * resolution AND over the same scene AABB share an identical grid, so their
 * occupancy arrays are element-wise comparable — that is what makes a
 * before/after geometry diff quantitative (added / removed / unchanged voxels
 * and the resulting volume delta).
 *
 * This module is the single source of truth for both the Debug ▸ Voxel dialog
 * and the `window.__fdVoxel` console API, and is unit-tested in isolation.
 */

export interface VoxelSnapshot {
  /** Grid dimensions [nx, ny, nz]. */
  dims: [number, number, number];
  /** World min corner of voxel (0,0,0). */
  origin: [number, number, number];
  /** Edge length of one (cubic) voxel, mm. */
  spacing: number;
  /** Row-major occupancy, x fastest then y then z; 1 = inside, 0 = outside. */
  occupancy: Uint8Array;
  /** Number of occupied (=1) voxels. */
  count: number;
}

export interface VoxelDiff {
  /** Occupied in B but not in A (newly added geometry). */
  added: number;
  /** Occupied in A but not in B (removed geometry). */
  removed: number;
  /** Occupied in both. */
  unchanged: number;
  /** Volume of one voxel, mm³ (from the shared grid spacing). */
  voxelVolume: number;
  /** Signed volume change B − A, mm³ (= (added − removed) · voxelVolume). */
  volumeDelta: number;
  /** Per-voxel flag, 1 where a voxel was added (for green rendering). */
  addedMask: Uint8Array;
  /** Per-voxel flag, 1 where a voxel was removed (for red rendering). */
  removedMask: Uint8Array;
}

/** Whether two snapshots share the exact same grid (dims + spacing + origin),
 *  i.e. their occupancy arrays are element-wise comparable. */
export function sameGrid(a: VoxelSnapshot, b: VoxelSnapshot): boolean {
  const eps = 1e-4;
  return (
    a.dims[0] === b.dims[0] &&
    a.dims[1] === b.dims[1] &&
    a.dims[2] === b.dims[2] &&
    Math.abs(a.spacing - b.spacing) < eps &&
    Math.abs(a.origin[0] - b.origin[0]) < eps &&
    Math.abs(a.origin[1] - b.origin[1]) < eps &&
    Math.abs(a.origin[2] - b.origin[2]) < eps
  );
}

/**
 * Element-wise occupancy diff. `a` = reference (before), `b` = current (after).
 * A voxel occupied only in `b` counts as ADDED (green); occupied only in `a`
 * counts as REMOVED (red); occupied in both is UNCHANGED. Throws if the arrays
 * differ in length — callers guard with {@link sameGrid} first.
 */
export function diffOccupancy(
  a: Uint8Array,
  b: Uint8Array,
): {
  added: number;
  removed: number;
  unchanged: number;
  addedMask: Uint8Array;
  removedMask: Uint8Array;
} {
  if (a.length !== b.length) {
    throw new Error(
      `voxel diff: length mismatch (${a.length} vs ${b.length})`,
    );
  }
  const n = a.length;
  const addedMask = new Uint8Array(n);
  const removedMask = new Uint8Array(n);
  let added = 0;
  let removed = 0;
  let unchanged = 0;
  for (let i = 0; i < n; i++) {
    const av = a[i] !== 0;
    const bv = b[i] !== 0;
    if (av && bv) {
      unchanged++;
    } else if (bv) {
      addedMask[i] = 1;
      added++;
    } else if (av) {
      removedMask[i] = 1;
      removed++;
    }
  }
  return { added, removed, unchanged, addedMask, removedMask };
}

/**
 * Diff two full snapshots. Returns `{ error }` when their grids differ (so a
 * snapshot at another resolution / AABB can't silently produce a bogus count),
 * otherwise the quantitative {@link VoxelDiff}.
 */
export function diffSnapshots(
  ref: VoxelSnapshot,
  cur: VoxelSnapshot,
): VoxelDiff | { error: string } {
  if (!sameGrid(ref, cur)) {
    return {
      error:
        "Snapshots haben unterschiedliche Gitter (Auflösung oder Szenen-AABB) — nicht vergleichbar.",
    };
  }
  const { added, removed, unchanged, addedMask, removedMask } = diffOccupancy(
    ref.occupancy,
    cur.occupancy,
  );
  const voxelVolume = cur.spacing ** 3;
  return {
    added,
    removed,
    unchanged,
    voxelVolume,
    volumeDelta: (added - removed) * voxelVolume,
    addedMask,
    removedMask,
  };
}
