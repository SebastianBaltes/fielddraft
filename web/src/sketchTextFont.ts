/**
 * F24-full: font registry + async loader/cache for filled glyph text.
 *
 * The old single-stroke vector font is gone. Filled text now comes from
 * REAL font outlines parsed with `opentype.js`. This module owns:
 *  - the registry of BUNDLED fonts (shipped under `public/fonts/`),
 *  - an async loader that fetches + parses a bundled font by id (used by
 *    the store when the Text tool is activated or the font changes),
 *  - a parse-from-ArrayBuffer path for user-uploaded `.ttf`/`.otf` files,
 *  - a tiny in-memory cache so a font is fetched + parsed at most once.
 *
 * The layout/glyph→`sketch_path` conversion lives in `sketchText.ts`; this
 * module only deals with *getting a parsed `opentype.Font`*.
 */

import opentype, { type Font } from "opentype.js";

/** One selectable bundled font. `url` is resolved relative to the site
 *  root (Vite serves `public/` from `/`). */
export interface FontEntry {
  /** Stable id used by the store / dropdown `<option value>`. */
  id: string;
  /** Human-readable label for the dropdown. */
  label: string;
  /** Public URL of the font file. */
  url: string;
}

/** The bundled, license-clean fonts (DejaVu — see public/fonts/LICENSE.txt).
 *  The first entry is the default. */
export const BUNDLED_FONTS: readonly FontEntry[] = [
  { id: "dejavu-sans", label: "DejaVu Sans", url: "/fonts/DejaVuSans.ttf" },
  { id: "dejavu-serif", label: "DejaVu Serif", url: "/fonts/DejaVuSerif.ttf" },
];

/** Default font id (DejaVu Sans). */
export const DEFAULT_FONT_ID = BUNDLED_FONTS[0].id;

/** Look up a bundled entry by id. */
export function bundledFont(id: string): FontEntry | undefined {
  return BUNDLED_FONTS.find((f) => f.id === id);
}

/** In-memory cache: font id → parsed Font (or the in-flight promise so
 *  concurrent loads share one fetch). Keyed by id; uploaded fonts use a
 *  synthetic id (see {@link registerUploadedFont}). */
const cache = new Map<string, Font>();
const inflight = new Map<string, Promise<Font>>();

/** wu2v: raw bytes of uploaded fonts, keyed by their synthetic id. The
 *  parsed `Font` in `cache` can't be re-serialized, so we keep the
 *  original file bytes here to embed them in the saved `.fdraft` ZIP and
 *  re-register the font on reload. */
const uploadedBytes = new Map<string, Uint8Array>();

/** Parse a font from a raw ArrayBuffer (uploaded file or fetched bytes). */
export function parseFontBuffer(buffer: ArrayBuffer): Font {
  return opentype.parse(buffer);
}

/**
 * Load a bundled font by id, returning a cached parse when available.
 * Concurrent calls for the same id share a single fetch+parse. Rejects
 * when the id is unknown or the fetch/parse fails.
 */
export async function loadBundledFont(id: string): Promise<Font> {
  const cached = cache.get(id);
  if (cached) return cached;
  const pending = inflight.get(id);
  if (pending) return pending;

  const entry = bundledFont(id);
  if (!entry) {
    return Promise.reject(new Error(`unknown bundled font "${id}"`));
  }

  const promise = (async () => {
    const res = await fetch(entry.url);
    if (!res.ok) {
      throw new Error(`font fetch failed (${res.status}): ${entry.url}`);
    }
    const buf = await res.arrayBuffer();
    const font = parseFontBuffer(buf);
    cache.set(id, font);
    inflight.delete(id);
    return font;
  })();
  inflight.set(id, promise);
  // On failure, clear the in-flight slot so a later retry can try again.
  promise.catch(() => inflight.delete(id));
  return promise;
}

/** Synthetic id prefix for user-uploaded fonts. */
export const UPLOADED_FONT_PREFIX = "uploaded:";

/**
 * Register an already-parsed uploaded font under a synthetic id so the
 * rest of the app treats it like any other cached font. Returns the id.
 *
 * wu2v: pass the original file `bytes` to retain them for persistence —
 * a saved `.fdraft` ZIP embeds these so the uploaded font survives a
 * reload (previously only the id was kept and the bytes were lost).
 */
export function registerUploadedFont(
  name: string,
  font: Font,
  bytes?: Uint8Array,
): string {
  const id = `${UPLOADED_FONT_PREFIX}${name}`;
  cache.set(id, font);
  if (bytes) uploadedBytes.set(id, bytes);
  return id;
}

/** wu2v: raw bytes of a previously-uploaded font, or `undefined` when the
 *  id is unknown or was registered without bytes. */
export function getUploadedFontBytes(id: string): Uint8Array | undefined {
  return uploadedBytes.get(id);
}

/** wu2v: re-register an uploaded font from its persisted bytes (parse →
 *  cache → retain) so it renders again after a project reload. The id is
 *  given verbatim (as stored in the project), not re-derived from a name.
 *  Throws if the bytes aren't a parseable font. */
export function registerUploadedFontFromBytes(id: string, bytes: Uint8Array): void {
  // opentype.parse needs an ArrayBuffer; slice to the exact view bounds.
  const buf = bytes.buffer.slice(
    bytes.byteOffset,
    bytes.byteOffset + bytes.byteLength,
  );
  const font = opentype.parse(buf);
  cache.set(id, font);
  uploadedBytes.set(id, bytes);
}

/** Synchronously read a parsed font from the cache, or `undefined` when it
 *  hasn't been loaded yet. Used by callers that must not block. */
export function cachedFont(id: string): Font | undefined {
  return cache.get(id);
}
