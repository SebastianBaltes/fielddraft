/**
 * F24-full: lay a string of FILLED text out into sketch geometry from a
 * real parsed font.
 *
 * Pure and DOM-free (it only touches a parsed `opentype.Font`). Given a
 * font, a string, a placement origin and a cap/em height, it walks the
 * characters left to right and turns each glyph's OUTLINE into one
 * `sketch_path` sub-call. A glyph's outline can have several contours —
 * an outer ring plus inner holes (the bowl of "O", the counters of "A",
 * "B", "8", …). They all go into the one `sketch_path`'s `contours`, and
 * the kernel's even-odd nesting fills the outer ring while subtracting
 * the holes for free. No backward-compat single-stroke path remains.
 *
 * ── Coordinates ─────────────────────────────────────────────────────────
 *  opentype reports glyph paths in FONT UNITS, y-UP (a capital's apex has
 *  a larger y than its baseline) — which matches the sketch's y-up frame,
 *  so NO y-flip is needed. We scale by `height / unitsPerEm` (so `height`
 *  is the em height in mm, as the cap height was before) and translate by
 *  the running pen position + origin. The pen advances by the glyph's
 *  `advanceWidth · scale` (+ optional `letterSpacing`). A space (and any
 *  glyph with no outline) advances but emits nothing; a codepoint missing
 *  from the font is skipped but still advances by the font's `.notdef`
 *  width.
 *
 * ── Curves ──────────────────────────────────────────────────────────────
 *  opentype path commands are M/L/Q/C/Z. Q maps 1:1 to a quad segment; C
 *  (cubic) is degree-reduced to quad segments by the SAME `cubicToQuads`
 *  used for SVG import (reused, not duplicated); L becomes a line; Z marks
 *  the contour closed. Each M starts a fresh contour.
 */

import type { Font, PathCommand } from "opentype.js";
import {
  CUBIC_TOL,
  cubicToQuads,
  type PathContour,
  type PathSeg,
  type SubCall,
} from "./importVector";

export type { SubCall } from "./importVector";

type Pt = [number, number];

export interface TextLayoutOptions {
  /** Lower-left (baseline) of the first glyph, in sketch millimetres. */
  origin: [number, number];
  /** Em height in sketch millimetres (font units scale to this). */
  height: number;
  /** Extra gap (mm) added to each glyph's advance. Default 0. */
  letterSpacing?: number;
}

/**
 * Convert one glyph's outline `commands` into `sketch_path` contours in
 * sketch millimetres. `scale` maps font units → mm; `(tx, ty)` is the
 * glyph origin (pen + sketch origin) in mm. Returns one `PathContour`
 * per M…(Z) subpath; contours with no segments are dropped.
 */
function glyphContours(
  commands: PathCommand[],
  scale: number,
  tx: number,
  ty: number,
): PathContour[] {
  const xf = (x: number, y: number): Pt => [tx + x * scale, ty + y * scale];
  const contours: PathContour[] = [];
  let active: PathContour | null = null;
  // Current point in RAW font units (cubicToQuads works in raw space; the
  // resulting segments are then transformed — quads are affine-invariant).
  let cur: Pt = [0, 0];

  for (const cmd of commands) {
    switch (cmd.type) {
      case "M": {
        active = { start: xf(cmd.x, cmd.y), segments: [], closed: false };
        contours.push(active);
        cur = [cmd.x, cmd.y];
        break;
      }
      case "L": {
        if (!active) break;
        active.segments.push({ kind: "line", to: xf(cmd.x, cmd.y) });
        cur = [cmd.x, cmd.y];
        break;
      }
      case "Q": {
        if (!active) break;
        active.segments.push({
          kind: "quad",
          ctrl: xf(cmd.x1, cmd.y1),
          to: xf(cmd.x, cmd.y),
        });
        cur = [cmd.x, cmd.y];
        break;
      }
      case "C": {
        if (!active) break;
        const p0: Pt = cur;
        const p1: Pt = [cmd.x1, cmd.y1];
        const p2: Pt = [cmd.x2, cmd.y2];
        const p3: Pt = [cmd.x, cmd.y];
        // cubicToQuads runs in RAW font units, so convert the mm flatness
        // tolerance into font units (CUBIC_TOL / scale). Using the mm value
        // directly here would treat 0.05 font units as the target and always
        // subdivide to the depth cap — fine for TrueType (quad) fonts where C
        // never appears, but it bloats uploaded OTF (cubic) glyphs.
        const tolFontUnits = scale > 0 ? CUBIC_TOL / scale : CUBIC_TOL;
        for (const q of cubicToQuads(p0, p1, p2, p3, tolFontUnits)) {
          active.segments.push(xfSeg(q, scale, tx, ty));
        }
        cur = p3;
        break;
      }
      case "Z": {
        if (active) active.closed = true;
        active = null;
        break;
      }
    }
  }

  return contours.filter((c) => c.segments.length > 0);
}

/** Transform a raw-font-unit segment's points into sketch mm. Quads are
 *  affine-invariant, so scaling+translating control + end yields the exact
 *  transformed quad. */
function xfSeg(seg: PathSeg, scale: number, tx: number, ty: number): PathSeg {
  const xf = (p: Pt): Pt => [tx + p[0] * scale, ty + p[1] * scale];
  if (seg.kind === "line") return { kind: "line", to: xf(seg.to) };
  return { kind: "quad", ctrl: xf(seg.ctrl), to: xf(seg.to) };
}

/**
 * Convert `text` into a list of `sketch_path` sub-calls laid out from
 * `origin` at the given em `height`, using the parsed `font`. Each drawn
 * glyph becomes ONE `sketch_path` whose contours carry the glyph's outer
 * ring + holes. Returns `[]` for an empty string (or a string that
 * produces no geometry, e.g. all spaces). Characters missing from the
 * font are skipped but still advance the pen.
 */
export function textToSubCalls(
  font: Font,
  text: string,
  opts: TextLayoutOptions,
): SubCall[] {
  const { origin, height } = opts;
  const letterSpacing = opts.letterSpacing ?? 0;
  const [ox, oy] = origin;
  const scale = height / font.unitsPerEm;

  const out: SubCall[] = [];
  let penX = 0; // running x offset (mm) from the origin

  // Iterating the string with for..of yields full Unicode code points
  // (surrogate pairs stay intact), so astral glyphs lay out correctly.
  for (const ch of text) {
    const glyph = font.charToGlyph(ch);
    // `advanceWidth` is in font units; fall back to a sane default.
    const advance = (glyph.advanceWidth ?? font.unitsPerEm * 0.5) * scale;
    // A codepoint the font doesn't cover resolves to `.notdef` (glyph
    // index 0) — typically a "tofu" box. We SKIP its outline (emit
    // nothing) but still advance the pen, so a missing/unsupported
    // character leaves a gap rather than a box.
    const commands = glyph.index === 0 ? [] : glyph.path.commands;
    if (commands.length > 0) {
      const contours = glyphContours(commands, scale, ox + penX, oy);
      if (contours.length > 0) {
        out.push({ type: "sketch_path", params: { contours } });
      }
    }
    penX += advance + letterSpacing;
  }

  return out;
}
