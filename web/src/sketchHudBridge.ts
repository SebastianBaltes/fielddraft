/**
 * Transient sketch HUD geometry, published from `SketchCanvas2d` (where the
 * interaction state lives) and rendered as real 3D scene geometry by
 * `SketchHud3D` — so the hover/snap markers, drag/rubber-band previews and
 * projected-edge guides are GPU-transformed with the camera like everything
 * else, instead of being stroked on the lagging 2D canvas.
 *
 * Everything here is in sketch-plane (u, v) millimetres; the 3D consumer
 * lifts it to world space through the active plane frame. The version counter
 * lets the consumer rebuild its (cheap, short-lived) geometry only when the
 * transient state actually changed.
 */

import type { SketchSubCall } from "./sketchOutline";

export type HudVec2 = [number, number];

export interface SketchHudGeometry {
  /** Preview shapes expressed as pseudo sub-calls — tessellated by the
   *  shared flattener and drawn in the preview colour. */
  previewCalls: SketchSubCall[];
  /** Extra preview segments not expressible as a sub-call (radius spokes,
   *  partial rubber-bands, open polygon edges). */
  previewLines: Array<[HudVec2, HudVec2]>;
  /** Extra preview marker points (e.g. a tool's centre). */
  previewPoints: HudVec2[];
  /** Dashed preview guide segments (open-polygon closing edge). */
  previewGuides: Array<[HudVec2, HudVec2]>;
  /** Hover preview point (where a click would land), or null. */
  hover: HudVec2 | null;
  /** Snap anchor (drawn as an open ring), or null. */
  snap: HudVec2 | null;
  /** Projected B-Rep edge guides (dashed), as segment pairs. */
  projected: Array<[HudVec2, HudVec2]>;
  /** Bumped on every publish so the consumer can skip unchanged frames. */
  version: number;
}

const hud: SketchHudGeometry = {
  previewCalls: [],
  previewLines: [],
  previewPoints: [],
  previewGuides: [],
  hover: null,
  snap: null,
  projected: [],
  version: 0,
};

const EMPTY: Omit<SketchHudGeometry, "version"> = {
  previewCalls: [],
  previewLines: [],
  previewPoints: [],
  previewGuides: [],
  hover: null,
  snap: null,
  projected: [],
};

// R3F invalidate, so a publish during drawing (which need not move the
// camera) still drives a render → SketchHud3D's useFrame → preview rebuild
// under the demand frameloop.
let invalidateFn: (() => void) | null = null;
export function setHudInvalidate(fn: (() => void) | null): void {
  invalidateFn = fn;
}

/** Replace the published transient HUD geometry and bump the version. */
export function publishSketchHud(next: Omit<SketchHudGeometry, "version">): void {
  hud.previewCalls = next.previewCalls;
  hud.previewLines = next.previewLines;
  hud.previewPoints = next.previewPoints;
  hud.previewGuides = next.previewGuides;
  hud.hover = next.hover;
  hud.snap = next.snap;
  hud.projected = next.projected;
  hud.version += 1;
  invalidateFn?.();
}

/** Clear all transient HUD geometry (sketch session ended / overlay gone). */
export function clearSketchHud(): void {
  publishSketchHud(EMPTY);
}

export function readSketchHud(): Readonly<SketchHudGeometry> {
  return hud;
}
