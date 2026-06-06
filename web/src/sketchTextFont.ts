/**
 * F24: a compact built-in SINGLE-STROKE vector font for sketch text.
 *
 * This is engraving-style text: each glyph is a set of polyline *strokes*
 * (a stroke = an ordered list of `[x, y]` points), not filled outlines.
 * Rendered, it looks like single-line CNC/pen-plotter lettering. Filled
 * TrueType outlines would be a future enhancement — they'd need a real font
 * loader and a closed-region fill, neither of which exists here.
 *
 * Glyphs live on a normalized cell: x ∈ [0, 1] wide, y ∈ [0, 1] tall, with
 * the baseline at y = 0 and the cap height at y = 1. Letterforms are simple
 * straight-segment approximations; curved letters (O, S, C, G, …) are drawn
 * as short polylines around their arcs, which is plenty legible at the sizes
 * a sketch uses. A few strokes overshoot the cell slightly by design — the
 * sanity test allows a small margin.
 *
 * Coverage: A–Z, 0–9, space, and `. - _` (period, hyphen, underscore).
 * Lowercase is NOT authored separately — the layout in `sketchText.ts`
 * upper-cases the input, so "abc" renders with the A/B/C glyphs.
 */

export type Glyph = number[][][];

/** Default advance width (cell-relative) for a glyph with no explicit metric. */
export const DEFAULT_ADVANCE = 0.7;
/** Advance width for the space character (it emits no strokes). */
export const SPACE_ADVANCE = 0.5;

/**
 * Per-glyph advance width override (cell-relative). Glyphs absent from this
 * map advance by {@link DEFAULT_ADVANCE}. The narrow letters (I, J) and the
 * wide ones (M, W) get bespoke advances so the spacing reads naturally.
 */
export const ADVANCE: Record<string, number> = {
  " ": SPACE_ADVANCE,
  I: 0.3,
  J: 0.5,
  M: 0.85,
  W: 0.9,
  ".": 0.3,
  "-": 0.55,
  _: 0.7,
};

// Short helpers to approximate curves with polylines on the [0,1] cell.
// `arcPts` walks a circular arc; angles in radians, CCW positive.
function arcPts(
  cx: number,
  cy: number,
  rx: number,
  ry: number,
  a0: number,
  a1: number,
  steps = 8,
): number[][] {
  const pts: number[][] = [];
  for (let i = 0; i <= steps; i++) {
    const t = a0 + ((a1 - a0) * i) / steps;
    pts.push([cx + rx * Math.cos(t), cy + ry * Math.sin(t)]);
  }
  return pts;
}

const TAU = Math.PI * 2;

/**
 * The font table: glyph → array of strokes, stroke → array of `[x, y]`
 * points. Coordinates are in the normalized [0,1] cell described above.
 */
export const GLYPHS: Record<string, Glyph> = {
  " ": [],

  A: [
    [
      [0, 0],
      [0.35, 1],
      [0.7, 0],
    ],
    [
      [0.13, 0.36],
      [0.57, 0.36],
    ],
  ],
  B: [
    [
      [0, 0],
      [0, 1],
      [0.45, 1],
      ...arcPts(0.45, 0.75, 0.25, 0.25, Math.PI / 2, -Math.PI / 2, 6),
      [0, 0.5],
      [0.5, 0.5],
      ...arcPts(0.5, 0.25, 0.25, 0.25, Math.PI / 2, -Math.PI / 2, 6),
      [0, 0],
    ],
  ],
  C: [arcPts(0.55, 0.5, 0.55, 0.5, Math.PI / 3, TAU - Math.PI / 3, 12)],
  D: [
    [
      [0, 0],
      [0, 1],
      [0.35, 1],
      ...arcPts(0.35, 0.5, 0.4, 0.5, Math.PI / 2, -Math.PI / 2, 10),
      [0, 0],
    ],
  ],
  E: [
    [
      [0.65, 1],
      [0, 1],
      [0, 0],
      [0.65, 0],
    ],
    [
      [0, 0.5],
      [0.5, 0.5],
    ],
  ],
  F: [
    [
      [0.65, 1],
      [0, 1],
      [0, 0],
    ],
    [
      [0, 0.5],
      [0.5, 0.5],
    ],
  ],
  G: [
    [
      ...arcPts(0.55, 0.5, 0.55, 0.5, Math.PI / 3, TAU - Math.PI / 3, 12),
    ],
    [
      [0.6, 0.45],
      [1.05, 0.45],
      [1.05, 0.0],
    ],
  ],
  H: [
    [
      [0, 1],
      [0, 0],
    ],
    [
      [0.7, 1],
      [0.7, 0],
    ],
    [
      [0, 0.5],
      [0.7, 0.5],
    ],
  ],
  I: [
    [
      [0.15, 0],
      [0.15, 1],
    ],
  ],
  J: [
    [
      [0.5, 1],
      [0.5, 0.25],
      ...arcPts(0.25, 0.25, 0.25, 0.25, 0, -Math.PI, 6),
    ],
  ],
  K: [
    [
      [0, 1],
      [0, 0],
    ],
    [
      [0.6, 1],
      [0, 0.45],
      [0.65, 0],
    ],
  ],
  L: [
    [
      [0, 1],
      [0, 0],
      [0.6, 0],
    ],
  ],
  M: [
    [
      [0, 0],
      [0, 1],
      [0.42, 0.4],
      [0.84, 1],
      [0.84, 0],
    ],
  ],
  N: [
    [
      [0, 0],
      [0, 1],
      [0.7, 0],
      [0.7, 1],
    ],
  ],
  O: [arcPts(0.5, 0.5, 0.5, 0.5, 0, TAU, 16)],
  P: [
    [
      [0, 0],
      [0, 1],
      [0.45, 1],
      ...arcPts(0.45, 0.74, 0.26, 0.26, Math.PI / 2, -Math.PI / 2, 6),
      [0, 0.48],
    ],
  ],
  Q: [
    arcPts(0.5, 0.5, 0.5, 0.5, 0, TAU, 16),
    [
      [0.55, 0.3],
      [0.85, -0.05],
    ],
  ],
  R: [
    [
      [0, 0],
      [0, 1],
      [0.45, 1],
      ...arcPts(0.45, 0.74, 0.26, 0.26, Math.PI / 2, -Math.PI / 2, 6),
      [0, 0.48],
    ],
    [
      [0.3, 0.48],
      [0.7, 0],
    ],
  ],
  S: [
    [
      ...arcPts(0.32, 0.74, 0.32, 0.26, -Math.PI / 4, Math.PI, 8),
      ...arcPts(0.32, 0.26, 0.32, 0.26, 0, -Math.PI - Math.PI / 4, 8),
    ],
  ],
  T: [
    [
      [0, 1],
      [0.7, 1],
    ],
    [
      [0.35, 1],
      [0.35, 0],
    ],
  ],
  U: [
    [
      [0, 1],
      [0, 0.28],
      ...arcPts(0.35, 0.28, 0.35, 0.28, Math.PI, TAU, 8),
      [0.7, 1],
    ],
  ],
  V: [
    [
      [0, 1],
      [0.35, 0],
      [0.7, 1],
    ],
  ],
  W: [
    [
      [0, 1],
      [0.22, 0],
      [0.44, 0.6],
      [0.66, 0],
      [0.88, 1],
    ],
  ],
  X: [
    [
      [0, 1],
      [0.7, 0],
    ],
    [
      [0, 0],
      [0.7, 1],
    ],
  ],
  Y: [
    [
      [0, 1],
      [0.35, 0.5],
      [0.7, 1],
    ],
    [
      [0.35, 0.5],
      [0.35, 0],
    ],
  ],
  Z: [
    [
      [0, 1],
      [0.7, 1],
      [0, 0],
      [0.7, 0],
    ],
  ],

  "0": [
    arcPts(0.35, 0.5, 0.35, 0.5, 0, TAU, 16),
    [
      [0.1, 0.15],
      [0.6, 0.85],
    ],
  ],
  "1": [
    [
      [0.1, 0.8],
      [0.35, 1],
      [0.35, 0],
    ],
    [
      [0.1, 0],
      [0.6, 0],
    ],
  ],
  "2": [
    [
      ...arcPts(0.35, 0.72, 0.35, 0.28, Math.PI, -Math.PI / 4, 8),
      [0, 0],
      [0.7, 0],
    ],
  ],
  "3": [
    [
      ...arcPts(0.32, 0.74, 0.3, 0.26, Math.PI / 1.5, -Math.PI / 2, 8),
      [0.1, 0.5],
      ...arcPts(0.3, 0.26, 0.34, 0.26, Math.PI / 2, -Math.PI / 1.2, 8),
    ],
  ],
  "4": [
    [
      [0.5, 0],
      [0.5, 1],
      [0, 0.3],
      [0.7, 0.3],
    ],
  ],
  "5": [
    [
      [0.6, 1],
      [0.1, 1],
      [0.1, 0.55],
      [0.4, 0.6],
      ...arcPts(0.35, 0.3, 0.35, 0.3, Math.PI / 2, -Math.PI, 10),
    ],
  ],
  "6": [
    [
      ...arcPts(0.35, 0.3, 0.35, 0.3, -Math.PI / 2, TAU - Math.PI / 2, 14),
      [0.0, 0.4],
      [0.45, 1],
    ],
  ],
  "7": [
    [
      [0, 1],
      [0.7, 1],
      [0.25, 0],
    ],
  ],
  "8": [
    arcPts(0.35, 0.74, 0.3, 0.26, 0, TAU, 12),
    arcPts(0.35, 0.26, 0.35, 0.26, 0, TAU, 12),
  ],
  "9": [
    [
      ...arcPts(0.35, 0.7, 0.35, 0.3, Math.PI / 2, TAU + Math.PI / 2, 14),
      [0.7, 0.6],
      [0.25, 0],
    ],
  ],

  ".": [
    [
      [0.1, 0],
      [0.2, 0],
      [0.2, 0.1],
      [0.1, 0.1],
      [0.1, 0],
    ],
  ],
  "-": [
    [
      [0.1, 0.5],
      [0.45, 0.5],
    ],
  ],
  _: [
    [
      [0, 0],
      [0.7, 0],
    ],
  ],
};
