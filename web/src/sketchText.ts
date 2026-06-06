/**
 * F24: lay a string of single-stroke text out into sketch geometry.
 *
 * Pure and DOM-free. Given a string, a placement origin, a cap height and
 * options, it walks the characters left to right, looks each one up in the
 * built-in single-stroke font (see [`./sketchTextFont`]), scales the glyph's
 * normalized [0,1] strokes by `height`, translates them to the running pen
 * position plus `origin`, and emits one `sketch_line` sub-call per consecutive
 * pair of points in each stroke. The pen advances by `advance·height +
 * letterSpacing` after every character (space just advances, emitting nothing;
 * unknown characters are skipped but still advance by the default width).
 *
 * The result is a flat list of ordinary `sketch_line` sub-calls — the kernel
 * needs no text primitive, so the strokes extrude/constrain like any other
 * lines. This is SINGLE-STROKE (engraving-style) text; filled outlines are a
 * future enhancement.
 *
 * Lowercase is folded to uppercase before lookup — the font only authors
 * capital letterforms.
 */

import {
  ADVANCE,
  DEFAULT_ADVANCE,
  GLYPHS,
  type Glyph,
} from "./sketchTextFont";

export interface SubCall {
  type: string;
  params: Record<string, unknown>;
}

export interface TextLayoutOptions {
  /** Lower-left of the first glyph, in sketch millimetres. */
  origin: [number, number];
  /** Cap height in sketch millimetres (the [0,1] cell scales to this). */
  height: number;
  /** Emit construction lines instead of solid ones. Default false. */
  construction?: boolean;
  /** Extra gap (mm) added to each glyph's advance. Default 0. */
  letterSpacing?: number;
}

/** Look up a glyph, folding lowercase to uppercase. */
function glyphFor(ch: string): Glyph | undefined {
  return GLYPHS[ch] ?? GLYPHS[ch.toUpperCase()];
}

/** Advance width (cell-relative) for a character, falling back to the default. */
function advanceFor(ch: string): number {
  const up = ch.toUpperCase();
  return ADVANCE[ch] ?? ADVANCE[up] ?? DEFAULT_ADVANCE;
}

/**
 * Convert `text` into a flat list of `sketch_line` sub-calls laid out from
 * `origin` at the given cap `height`. Each glyph stroke becomes a chain of
 * line segments; a stroke of N points yields N−1 lines. Returns `[]` for an
 * empty string. Characters with no glyph (and no uppercase fallback) are
 * skipped without throwing, but still advance the pen by the default width.
 */
export function textToSubCalls(
  text: string,
  opts: TextLayoutOptions,
): SubCall[] {
  const { origin, height } = opts;
  const construction = opts.construction ?? false;
  const letterSpacing = opts.letterSpacing ?? 0;
  const [ox, oy] = origin;

  const out: SubCall[] = [];
  let penX = 0; // running x offset (mm) from the origin

  for (const ch of text) {
    const glyph = glyphFor(ch);
    if (glyph) {
      for (const stroke of glyph) {
        for (let i = 0; i + 1 < stroke.length; i++) {
          const [ax, ay] = stroke[i];
          const [bx, by] = stroke[i + 1];
          out.push({
            type: "sketch_line",
            params: {
              start: [ox + penX + ax * height, oy + ay * height],
              end: [ox + penX + bx * height, oy + by * height],
              construction,
            },
          });
        }
      }
    }
    // Space, unknown chars and drawn glyphs all advance the pen.
    penX += advanceFor(ch) * height + letterSpacing;
  }

  return out;
}
