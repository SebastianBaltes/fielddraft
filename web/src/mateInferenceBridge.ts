/**
 * Mate-inference hover bridge.
 *
 * `MateInferenceOverlay` resolves the connection point under the cursor each
 * frame (origin + chosen orientation variant) and publishes it here; the
 * Viewport's click handler reads it synchronously to place the connector,
 * without threading React state through the R3F Canvas boundary. A plain
 * module-scoped singleton, exactly like `brepPickBridge` / `queryPickerBridge`.
 */

/** The connector the cursor is currently snapped to, ready to commit. */
export interface MateInferenceSnap {
  /** Anchor element query: `qVertex`/`qEdge`/`qFace(idHash)`. */
  anchorQuery: string;
  /** Edge arc-length param (0.5 = midpoint); 0 for vertex/face. */
  t: number;
  /** Secondary reference FACE query (orients secondary axes / defines a
   *  vertex's +Z), or `null` to let the kernel auto-pick. */
  secondary: string | null;
  /** Reorient steps about +Z (0..3). */
  reorient: number;
  /** World origin, for debugging / readouts. */
  world: [number, number, number];
}

let current: MateInferenceSnap | null = null;

export function setMateInferenceSnap(s: MateInferenceSnap | null): void {
  current = s;
}

export function getMateInferenceSnap(): MateInferenceSnap | null {
  return current;
}
