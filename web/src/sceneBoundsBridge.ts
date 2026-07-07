/**
 * Scene-bounds bridge.
 *
 * The camera's near/far clip planes are adapted every frame to the scene so
 * bodies never clip when zooming out (ticket lbzq: a fixed `far` of 4000 with a
 * 3000 mm-tall scene made bodies vanish once the camera receded past the far
 * plane). That per-frame adaptation needs the scene's world-space centre and
 * radius — expensive to recompute from the kernel each frame, so the store
 * publishes it here once per regen and the camera reads it cheaply.
 *
 * A plain module-scoped singleton, like the other viewport bridges.
 */

export interface SceneBounds {
  /** World-space centre of the scene AABB. */
  center: [number, number, number];
  /** Bounding-sphere radius (half the AABB diagonal). 0 = empty scene. */
  radius: number;
  /**
   * World-space centre of the *visible* solid's tight AABB — the orbit pivot
   * (ticket f88u). `center` is the conservative CSG bound the clip-plane
   * adaptation needs; but a half-carved sphere reports the whole sphere there,
   * so its centre lands on the phantom original. This is the centre of what is
   * actually drawn (kernel `sceneVisibleCenter`, SDF-sampled). Absent when the
   * kernel can't tighten (empty / unbounded field) → callers use `center`.
   */
  pivot?: [number, number, number];
}

/** Edge length of the world reference volume around the origin. The visible
 *  floor grid is gone, but the clip-plane adaptation still brackets this region
 *  so near/far stay sane for tiny or far-off-origin models. */
export const GRID_WORLD_SIZE = 600;

/** Bounding sphere of the world reference volume: centred on the origin, radius
 *  reaching its far corner. The clip-plane adaptation unions this with the body
 *  bounds so geometry near the origin is never clipped by the near plane when
 *  the camera recedes (ticket xv9c). */
export const GRID_BOUNDS: SceneBounds = {
  center: [0, 0, 0],
  radius: (GRID_WORLD_SIZE / 2) * Math.SQRT2,
};

let bounds: SceneBounds | null = null;

/** Publish the current scene bounds (called by the store on each regen). */
export function publishSceneBounds(next: SceneBounds | null): void {
  bounds = next;
}

/** Current scene bounds, or `null` when the scene is empty / unknown. */
export function getSceneBounds(): SceneBounds | null {
  return bounds;
}
