/**
 * Debug ▸ Part-AABB — pure helpers.
 *
 * Maps the current feature selection to the axis-aligned bounding box(es) of
 * the selected part(s). A "part" is a body; several selected features may live
 * in the same body, so the boxes are de-duplicated by body. Kept free of React
 * and three.js so the mapping is unit-testable.
 */

export interface PartAabb {
  min: [number, number, number];
  max: [number, number, number];
  /** `max − min` per axis. */
  size: [number, number, number];
}

/** The slice of the kernel this module needs — one lookup call. */
export interface AabbKernel {
  /** `[minx,miny,minz, maxx,maxy,maxz]` of the body containing `featureId`,
   *  or `[]` when the feature is in no body. */
  bodyAabbForFeature(featureId: number): number[];
}

/** Stable key for de-duplicating boxes that resolve to the same body. */
function boxKey(min: readonly number[], max: readonly number[]): string {
  return [...min, ...max].map((c) => c.toFixed(4)).join(",");
}

/**
 * Resolve the selected feature ids to their parts' bounding boxes. Features of
 * the same body collapse to one box; features in no body (or with an empty
 * bound) are skipped. Order follows first appearance in `selection`.
 */
export function selectedPartAabbs(
  kernel: AabbKernel,
  selection: Iterable<number>,
): PartAabb[] {
  const out: PartAabb[] = [];
  const seen = new Set<string>();
  for (const id of selection) {
    const a = kernel.bodyAabbForFeature(id);
    if (a.length !== 6) continue;
    const min: [number, number, number] = [a[0], a[1], a[2]];
    const max: [number, number, number] = [a[3], a[4], a[5]];
    const key = boxKey(min, max);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push({
      min,
      max,
      size: [max[0] - min[0], max[1] - min[1], max[2] - min[2]],
    });
  }
  return out;
}

/**
 * The 12 edges of an AABB as a flat `[x,y,z, …]` segment buffer (24 vertices =
 * 12 line segments), ready for a `LineSegments`/`LineSegments2` geometry.
 */
export function aabbEdgeSegments(
  min: readonly [number, number, number],
  max: readonly [number, number, number],
): Float32Array {
  const [x0, y0, z0] = min;
  const [x1, y1, z1] = max;
  // 8 corners.
  const c = [
    [x0, y0, z0],
    [x1, y0, z0],
    [x1, y1, z0],
    [x0, y1, z0],
    [x0, y0, z1],
    [x1, y0, z1],
    [x1, y1, z1],
    [x0, y1, z1],
  ];
  // 12 edges as corner-index pairs: bottom loop, top loop, 4 verticals.
  const e = [
    [0, 1],
    [1, 2],
    [2, 3],
    [3, 0],
    [4, 5],
    [5, 6],
    [6, 7],
    [7, 4],
    [0, 4],
    [1, 5],
    [2, 6],
    [3, 7],
  ];
  const buf = new Float32Array(e.length * 6);
  let o = 0;
  for (const [a, b] of e) {
    buf[o++] = c[a][0];
    buf[o++] = c[a][1];
    buf[o++] = c[a][2];
    buf[o++] = c[b][0];
    buf[o++] = c[b][1];
    buf[o++] = c[b][2];
  }
  return buf;
}
