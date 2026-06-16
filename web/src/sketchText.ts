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
import { FD_ID_PARAM, nextFdId } from "./sketchFillet";

export type { SubCall } from "./importVector";

type Pt = [number, number];

// ── Plan item 5: re-edit recipe for baked text (web-only hybrid) ─────
//
// `textToSubCalls` explodes a string into one `sketch_path` PER GLYPH;
// the original string / height / font are NOT recoverable from the
// contours. To make a committed text re-editable in-scene WITHOUT a
// kernel change we attach a `text` recipe param to EVERY glyph sub-call
// of one placement, all sharing a single `group` id. The kernel's
// `sketch_path` parse ignores unknown params (no `deny_unknown_fields`)
// and round-trips them verbatim through `replaceSketch` → store →
// `getSketchSubCalls`, so the recipe survives commit/reopen; only the
// web reads it. Geometry is unchanged — purely additive on the params
// Value. Same scheme as the fillet/chamfer recipes in `sketchFillet` /
// `sketchChamfer`; it reuses their `fd_id` / `nextFdId` vocabulary.

/** The `text` recipe attached to every baked glyph `sketch_path` of one
 *  placement. `group` ties the glyphs of a single text together; the
 *  rest is everything needed to re-derive the glyphs on a value/height/
 *  font change. */
export interface TextRecipe {
  /** Shared id of all glyph sub-calls of this one text placement. */
  group: string;
  /** The laid-out string. */
  value: string;
  /** Em height in sketch millimetres. */
  height: number;
  /** Font id the glyphs were rendered with (a `sketchTextFont` id). */
  fontId: string;
  /** Baseline-left of the first glyph, in sketch mm. */
  origin: [number, number];
}

/** Read the `text` recipe off a `sketch_path` sub-call, or null if the
 *  call isn't a recipe-tagged glyph. Defensive about shape so a
 *  hand-edited / malformed param never throws at pick time. */
export function textRecipeOf(call: SubCall): TextRecipe | null {
  if (call.type !== "sketch_path") return null;
  const r = call.params.text as Partial<TextRecipe> | undefined;
  if (!r || typeof r !== "object") return null;
  if (typeof r.group !== "string" || typeof r.value !== "string") return null;
  if (typeof r.fontId !== "string") return null;
  if (!Number.isFinite(r.height)) return null;
  if (
    !Array.isArray(r.origin) ||
    r.origin.length !== 2 ||
    !Number.isFinite(r.origin[0]) ||
    !Number.isFinite(r.origin[1])
  ) {
    return null;
  }
  return r as TextRecipe;
}

/** Axis-aligned bounding box `{ min, max }` of a `sketch_path`'s contours
 *  (start points + every segment control/end point), or null if the call
 *  carries no usable contour geometry. Used by the tagged-text hit-test to
 *  pick a glyph by clicking anywhere inside its drawn extent — a glyph's
 *  bbox hugs its ink closely enough that this reads as "click the letter".
 *  Quad control points can bulge slightly outside the ink, which only ever
 *  grows the box a hair — never under-selects. */
export function pathBBox(
  call: SubCall,
): { min: [number, number]; max: [number, number] } | null {
  if (call.type !== "sketch_path") return null;
  const contours = call.params.contours as PathContour[] | undefined;
  if (!Array.isArray(contours) || contours.length === 0) return null;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  const acc = (p: Pt): void => {
    if (!Array.isArray(p) || p.length !== 2) return;
    const [x, y] = p;
    if (x < minX) minX = x;
    if (y < minY) minY = y;
    if (x > maxX) maxX = x;
    if (y > maxY) maxY = y;
  };
  for (const c of contours) {
    if (!c || !Array.isArray(c.segments)) continue;
    acc(c.start);
    for (const seg of c.segments) {
      if (seg.kind === "quad") acc(seg.ctrl);
      acc(seg.to);
    }
  }
  if (!Number.isFinite(minX) || !Number.isFinite(minY)) return null;
  return { min: [minX, minY], max: [maxX, maxY] };
}

export interface TextLayoutOptions {
  /** Lower-left (baseline) of the first glyph, in sketch millimetres. */
  origin: [number, number];
  /** Em height in sketch millimetres (font units scale to this). */
  height: number;
  /** Extra gap (mm) added to each glyph's advance. Default 0. */
  letterSpacing?: number;
  /**
   * Plan item 5: tag every emitted glyph `sketch_path` with a re-edit
   * recipe (see [`TextRecipe`]) so the placement is editable in-scene. The
   * font id the glyphs were rendered with — needed to re-derive on edit —
   * is supplied here; the `group` id is generated fresh per call. Omit to
   * emit untagged glyphs (e.g. an imported / non-re-editable text).
   */
  fontId?: string;
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

  // Plan item 5: when a fontId is supplied, every glyph of THIS call shares
  // one fresh `group` id + the placement recipe, so a later click can find
  // them all and re-derive (see `textRecipeOf`). Untagged otherwise.
  const recipe: TextRecipe | null =
    opts.fontId !== undefined
      ? { group: nextFdId(), value: text, height, fontId: opts.fontId, origin }
      : null;

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
        const params: Record<string, unknown> = { contours };
        if (recipe) {
          params.text = recipe;
          params[FD_ID_PARAM] = nextFdId();
        }
        out.push({ type: "sketch_path", params });
      }
    }
    penX += advance + letterSpacing;
  }

  return out;
}

/**
 * Plan item 5: re-derive a tagged text group at a new value / height /
 * font and return the rewritten sub-call list, or null when nothing
 * changes (no glyph carries the group, or the new text produces no
 * geometry — an empty / all-space value). Pure: no solve, no DOM — the
 * store wraps it with `replaceSketchSubCalls` + a re-solve.
 *
 * Removes the OLD group's glyph sub-calls and inserts the freshly laid-out
 * glyphs at the position of the first removed glyph (so the text keeps its
 * place in the spine), each re-tagged with the SAME `group` id and the new
 * value/height/fontId. The placement origin is taken from the recipe (it
 * does not move on a value/height/font edit). `font` must be the parsed
 * font matching `fontId`; the store passes the loaded font (and falls back
 * gracefully if it isn't ready — see `store.updateSketchTextGroup`).
 */
export function reeditTextGroup(
  subCalls: ReadonlyArray<SubCall>,
  font: Font,
  groupId: string,
  next: { value: string; height: number; fontId: string },
): SubCall[] | null {
  // Find every glyph of the group and the recipe (origin) to re-derive from.
  let firstIdx = -1;
  let origin: [number, number] | null = null;
  const groupIdxs: number[] = [];
  for (let i = 0; i < subCalls.length; i++) {
    const r = textRecipeOf(subCalls[i]);
    if (!r || r.group !== groupId) continue;
    if (firstIdx < 0) {
      firstIdx = i;
      origin = r.origin;
    }
    groupIdxs.push(i);
  }
  if (firstIdx < 0 || origin === null) return null;

  // Re-lay the glyphs, REUSING the existing group id so the re-edited text
  // stays one group (textToSubCalls mints a fresh group, so stamp ours back).
  const fresh = textToSubCalls(font, next.value, {
    origin,
    height: next.height,
    fontId: next.fontId,
  });
  if (fresh.length === 0) return null;
  for (const g of fresh) {
    const r = g.params.text as TextRecipe;
    r.group = groupId;
  }

  // Splice: drop the old group's glyphs, insert the fresh ones where the
  // first old glyph sat. The group is contiguous as emitted, but we filter
  // by membership to be robust to any interleaving.
  const removed = new Set(groupIdxs);
  const out: SubCall[] = [];
  let inserted = false;
  for (let i = 0; i < subCalls.length; i++) {
    if (i === firstIdx) {
      out.push(...fresh);
      inserted = true;
    }
    if (removed.has(i)) continue;
    out.push(subCalls[i]);
  }
  if (!inserted) out.push(...fresh); // defensive; firstIdx is always hit
  return out;
}
