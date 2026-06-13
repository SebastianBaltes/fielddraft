/**
 * Transient sketch TEXT LABELS — dimension readouts, constraint glyphs and
 * the snap label — published from `SketchCanvas2d`'s overlay draw and rendered
 * as real 3D billboard sprites by `SketchLabels3D`. Like the geometry/HUD that
 * already moved into the scene (`SketchSession3D` / `SketchHud3D`), the labels
 * now ride the camera in lockstep with the model instead of being stroked on
 * the lagging 2D overlay canvas — no more "Nachlaufen" on a pan or orbit.
 *
 * A label lives at a real point in the sketch plane: `anchorUv` is in plane
 * (u, v) millimetres and the consumer lifts it to world space through the
 * active plane frame. The sprite faces the camera and is drawn at a constant
 * pixel size, so the text reads the same at any zoom while still being pinned
 * to its geometry. `offsetPx` nudges the label a constant number of SCREEN
 * pixels off the anchor (+x right, +y up), mirroring the old canvas offsets —
 * the nudge is baked into the sprite texture so it never scales with zoom.
 *
 * The version counter lets the consumer skip rebuilding when nothing changed.
 */

export type SketchLabelStyle = "dimension" | "constraint" | "snap";

export interface SketchLabel {
  /** Anchor in sketch-plane (u, v) millimetres. */
  anchorUv: [number, number];
  /** The glyph/number to draw. */
  text: string;
  /** Visual treatment: haloed number, bare glyph, or bordered pill. */
  style: SketchLabelStyle;
  /** CSS colour of the text. */
  color: string;
  /** Constant screen-pixel nudge from the anchor: +x right, +y up. */
  offsetPx: [number, number];
}

interface SketchLabelSet {
  labels: SketchLabel[];
  version: number;
}

const set: SketchLabelSet = { labels: [], version: 0 };

// R3F invalidate, so a publish during drawing (which need not move the
// camera) still drives a render → SketchLabels3D's useFrame → rebuild under
// the demand frameloop.
let invalidateFn: (() => void) | null = null;
export function setLabelInvalidate(fn: (() => void) | null): void {
  invalidateFn = fn;
}

/** Replace the published labels and bump the version. */
export function publishSketchLabels(labels: SketchLabel[]): void {
  set.labels = labels;
  set.version += 1;
  invalidateFn?.();
}

/** Clear all labels (sketch session ended / overlay gone). */
export function clearSketchLabels(): void {
  if (set.labels.length === 0) return;
  publishSketchLabels([]);
}

export function readSketchLabels(): Readonly<SketchLabelSet> {
  return set;
}
