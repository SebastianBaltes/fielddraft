/**
 * projectPersistence — the async persistence sink + load boundary (Phase 1b/1c).
 *
 * A thin wiring layer over the P1a building blocks
 * ({@link ./assetStore | assetStore} + {@link ./assetExternalize |
 * assetExternalize}) that finally hooks content-addressed assets into the
 * live persistence path (docs/PROJECT-ASSET-STORAGE-PLAN §5.2 / §5.3):
 *
 *  - {@link persistProjectDoc}: the autosave SINK. Externalizes a serialized
 *    Project's embedded meshes to immutable blobs, content-addresses every
 *    referenced uploaded font's bytes to blobs too, and writes a lean
 *    document ENVELOPE + all blobs to the {@link AssetStore} (default:
 *    IndexedDB).
 *  - {@link loadProjectDoc}: the LOAD boundary. Reads the envelope back,
 *    re-inlines the mesh blobs, and restores each uploaded font's bytes into
 *    the in-memory font cache so the result feeds the boot path unchanged
 *    (`ProjectStore.bootstrap` → `kernel.loadJson`) *and* text re-renders.
 *  - {@link migrateFromLocalStorage}: a one-way move of the legacy
 *    `fielddraft.project.v2` localStorage row into the AssetStore (its embedded
 *    meshes become blobs on the way in).
 *
 * ── Why this is project-aware (and not a bare `externalizeAssets(fullJson)`) ─
 * A Project serializes each part tab's kernel document as a nested JSON
 * *string* (`PartTab.documentJson` in project.ts), so the mesh payloads
 * (`__positions`/`__indices`, emitted by `imported_mesh`) live INSIDE that
 * string — invisible to a top-level object walk. We therefore run the P1a
 * transform on each part tab's `documentJson` and re-stringify. The transforms
 * (`externalizeAssets`/`inlineAssets`) stay untouched — only the orchestration
 * lives here. A non-Project input (a bare kernel document) is externalized
 * directly as a fallback.
 *
 * ── Fonts (P1c) ───────────────────────────────────────────────────────────
 * Uploaded font bytes never live in the document — a `sketch_path` only carries
 * `text.fontId` (an `uploaded:` id); the raw bytes sit in an in-memory cache
 * (sketchTextFont.ts) that a reload wipes. So a mesh survived a working-store
 * reload but an uploaded font did NOT: the doc kept the id, the bytes were gone,
 * and text rendered blank. P1c closes that: on persist we scan the referenced
 * `fontId`s (same walk as `ProjectStore.collectProjectAssets`), content-address
 * each font's bytes to a blob, and record a `fontId → hash` manifest in the
 * envelope; on load we resolve each manifest entry and push the bytes back into
 * the font cache. Fonts are purely additive to the mesh path.
 *
 * ── Storage envelope ──────────────────────────────────────────────────────
 * The `doc` store slot holds a v2 envelope `{ v: 2, doc, fonts }` where `doc`
 * is the lean project-json string and `fonts` is the `fontId → hash` manifest.
 * A legacy slot (a raw project-json string, as P1b wrote) is tolerated on load
 * as `{ doc: <string>, fonts: {} }` — no migration, no crash (dev data).
 *
 * `serializeProject()` stays SYNCHRONOUS and keeps emitting the full document
 * with inline meshes; the externalization happens only here, at the async sink,
 * and the inlining only at the load boundary.
 */

import { IndexedDbAssetStore, sha256Hex, type AssetStore } from "./assetStore";
import { externalizeAssets, inlineAssets } from "./assetExternalize";
import { loadPersisted, clearPersisted } from "./persistence";
import {
  UPLOADED_FONT_PREFIX,
  getUploadedFontBytes,
  registerUploadedFontFromBytes,
} from "./sketchTextFont";

/** Module-singleton store. IndexedDB in the browser; a quiet no-op where
 *  `indexedDB` is absent (SSR / vitest jsdom). Swappable for tests. */
let store: AssetStore = new IndexedDbAssetStore();

/** Test seam: replace the module-singleton {@link AssetStore} (e.g. an
 *  in-memory fake). Returns the previous store so a test can restore it. */
export function __setAssetStoreForTest(next: AssetStore): AssetStore {
  const prev = store;
  store = next;
  return prev;
}

/**
 * The uploaded-font byte cache, behind a tiny interface so tests can inject a
 * fake (the real setter parses bytes with opentype, which rejects the fake
 * font bytes a unit test uses). `get` yields the retained bytes for a persisted
 * `fontId`; `set` re-registers bytes on load (parse → cache → retain).
 */
interface FontByteCache {
  get(fontId: string): Uint8Array | undefined;
  set(fontId: string, bytes: Uint8Array): void;
}

let fontCache: FontByteCache = {
  get: getUploadedFontBytes,
  set: registerUploadedFontFromBytes,
};

/** Test seam: replace the module-singleton {@link FontByteCache}. Returns the
 *  previous cache so a test can restore it. */
export function __setFontCacheForTest(next: FontByteCache): FontByteCache {
  const prev = fontCache;
  fontCache = next;
  return prev;
}

/** The minimal shape we read off a parsed Project without importing the full
 *  `Project` type (this module stays kernel/three-free). */
interface JsonPartTabish {
  kind?: unknown;
  documentJson?: unknown;
}
interface JsonProjectish {
  tabs?: unknown;
}

function isPartTab(tab: unknown): tab is JsonPartTabish & { documentJson: string } {
  return (
    typeof tab === "object" &&
    tab !== null &&
    (tab as JsonPartTabish).kind === "part" &&
    typeof (tab as JsonPartTabish).documentJson === "string"
  );
}

/**
 * The blob subset of {@link AssetStore} — the ONLY surface the externalize/
 * inline path needs. Generalizing over it lets the exact same mesh+font
 * content-addressing run against IndexedDB (local autosave) and the server
 * (cloud save, {@link ./serverBlobStore | ServerBlobStore}) with one code path.
 */
export interface BlobBackend {
  /** Whether a blob with this hash is already stored (skip re-upload). */
  hasBlob(hash: string): Promise<boolean>;
  /** Store a blob. Idempotent — a present hash is a no-op (immutable). */
  writeBlob(hash: string, bytes: Uint8Array): Promise<void>;
  /** Bytes of a blob by hash, or `null` when absent. */
  readBlob(hash: string): Promise<Uint8Array | null>;
}

/**
 * Externalize every embedded mesh in a serialized Project. Runs the P1a
 * transform on each part tab's inner `documentJson` (see the module note),
 * merging the per-tab blob maps (dedup by hash). Falls back to a direct
 * transform for a non-Project input.
 */
async function externalizeMeshes(
  fullJson: string,
): Promise<{ doc: string; blobs: Map<string, Uint8Array> }> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(fullJson);
  } catch {
    // Not JSON — nothing to externalize; persist verbatim.
    return { doc: fullJson, blobs: new Map() };
  }
  const project = parsed as JsonProjectish;
  if (!Array.isArray(project.tabs)) {
    // Not a Project blob (e.g. a bare kernel document) — externalize directly.
    return externalizeAssets(fullJson);
  }
  const blobs = new Map<string, Uint8Array>();
  for (const tab of project.tabs) {
    if (!isPartTab(tab)) continue;
    const res = await externalizeAssets(tab.documentJson);
    tab.documentJson = res.doc;
    for (const [hash, bytes] of res.blobs) {
      if (!blobs.has(hash)) blobs.set(hash, bytes);
    }
  }
  return { doc: JSON.stringify(project), blobs };
}

/** Inverse of {@link externalizeMeshes}: re-inline each part tab's blobs from
 *  `backend` so the result feeds `kernel.loadJson` unchanged. */
async function inlineMeshes(slimJson: string, backend: BlobBackend): Promise<string> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(slimJson);
  } catch {
    return slimJson;
  }
  const project = parsed as JsonProjectish;
  if (!Array.isArray(project.tabs)) {
    return inlineAssets(slimJson, (hash) => backend.readBlob(hash));
  }
  for (const tab of project.tabs) {
    if (!isPartTab(tab)) continue;
    tab.documentJson = await inlineAssets(tab.documentJson, (hash) =>
      backend.readBlob(hash),
    );
  }
  return JSON.stringify(project);
}

// ── fonts (P1c) ─────────────────────────────────────────────────────────────

/** The `fontId → blob-hash` manifest recorded alongside the document. */
type FontManifest = Record<string, string>;

/** The persisted document envelope (see the module note). */
interface DocEnvelope {
  v: 2;
  /** The lean project-json string (meshes already externalized). */
  doc: string;
  /** Uploaded fonts referenced by the project: id → content-address. */
  fonts: FontManifest;
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** Collect every uploaded `fontId` referenced anywhere in a parsed kernel
 *  document (mirrors `ProjectStore.collectProjectAssets`'s walk). */
function collectFontIdsFromDoc(docJson: string, ids: Set<string>): void {
  let doc: unknown;
  try {
    doc = JSON.parse(docJson);
  } catch {
    return;
  }
  const walk = (value: unknown): void => {
    if (Array.isArray(value)) {
      for (const v of value) walk(v);
      return;
    }
    if (isObject(value)) {
      const fontId = value.fontId;
      if (typeof fontId === "string" && fontId.startsWith(UPLOADED_FONT_PREFIX)) {
        ids.add(fontId);
      }
      for (const v of Object.values(value)) walk(v);
    }
  };
  walk(doc);
}

/** Every uploaded font id referenced by a serialized Project (across all part
 *  tabs), or by a bare kernel document as a fallback. */
function scanFontIds(fullJson: string): Set<string> {
  const ids = new Set<string>();
  let parsed: unknown;
  try {
    parsed = JSON.parse(fullJson);
  } catch {
    return ids;
  }
  const project = parsed as JsonProjectish;
  if (!Array.isArray(project.tabs)) {
    collectFontIdsFromDoc(fullJson, ids);
    return ids;
  }
  for (const tab of project.tabs) {
    if (isPartTab(tab)) collectFontIdsFromDoc(tab.documentJson, ids);
  }
  return ids;
}

/** Content-address every referenced uploaded font's retained bytes into
 *  `blobs` (dedup by hash) and return the `fontId → hash` manifest. Fonts
 *  with no retained bytes (never uploaded this session) are silently skipped —
 *  matching `collectProjectAssets`. */
async function collectFontBlobs(
  fullJson: string,
  blobs: Map<string, Uint8Array>,
): Promise<FontManifest> {
  const manifest: FontManifest = {};
  for (const fontId of scanFontIds(fullJson)) {
    const bytes = fontCache.get(fontId);
    if (!bytes) continue;
    const hash = await sha256Hex(bytes);
    if (!blobs.has(hash)) blobs.set(hash, bytes);
    manifest[fontId] = hash;
  }
  return manifest;
}

/** Restore each manifested font's bytes into the font cache, reading blobs from
 *  `backend`. A missing blob is a warning, never a crash (P1a inline-assets
 *  semantics). */
async function restoreFonts(fonts: FontManifest, backend: BlobBackend): Promise<void> {
  for (const [fontId, hash] of Object.entries(fonts)) {
    const bytes = await backend.readBlob(hash);
    if (!bytes) {
      console.warn(
        `fielddraft: font blob missing (${hash}) for "${fontId}" — text may not render`,
      );
      continue;
    }
    try {
      fontCache.set(fontId, bytes);
    } catch (err) {
      console.warn(`fielddraft: font restore failed for "${fontId}"`, err);
    }
  }
}

/** Unwrap the stored `doc` slot into its inner document + font manifest.
 *  Tolerant of a legacy raw project-json string (P1b): it has no numeric `v`,
 *  so it becomes `{ doc: <slot>, fonts: {} }` — no migration needed. */
function parseEnvelope(slot: string): { doc: string; fonts: FontManifest } {
  try {
    const parsed: unknown = JSON.parse(slot);
    if (isObject(parsed) && typeof parsed.v === "number" && typeof parsed.doc === "string") {
      const fonts = isObject(parsed.fonts) ? (parsed.fonts as FontManifest) : {};
      return { doc: parsed.doc, fonts };
    }
  } catch {
    // Not JSON — treat the slot verbatim as the document.
  }
  return { doc: slot, fonts: {} };
}

// ── generalized externalize / inline (backend-agnostic) ─────────────────────

/**
 * Externalize a serialized Project into a lean document ENVELOPE against an
 * arbitrary {@link BlobBackend}: move every embedded mesh and referenced
 * uploaded-font's bytes into content-addressed blobs, UPLOAD each new blob to
 * `backend` (upload-if-missing — a hash the backend already holds is skipped,
 * saving a server round-trip), and return the `{ v, doc, fonts }` envelope as a
 * JSON string. The one code path shared by IndexedDB autosave
 * ({@link persistProjectDoc}) and the server cloud-save
 * (`ProjectStore.syncToServer`).
 */
export async function externalizeProject(
  fullJson: string,
  backend: BlobBackend,
): Promise<string> {
  const { doc, blobs } = await externalizeMeshes(fullJson);
  const fonts = await collectFontBlobs(fullJson, blobs);
  // Write blobs BEFORE the document so a stored doc never references a blob the
  // backend lacks. Immutable/content-addressed ⇒ skip what's already present.
  for (const [hash, bytes] of blobs) {
    if (!(await backend.hasBlob(hash))) await backend.writeBlob(hash, bytes);
  }
  const envelope: DocEnvelope = { v: 2, doc, fonts };
  return JSON.stringify(envelope);
}

/**
 * Inverse of {@link externalizeProject}: unwrap the `slot` (envelope OR a legacy
 * raw/full project-json — {@link parseEnvelope} tolerates both), restore any
 * manifested fonts from `backend`, and re-inline every mesh blob so the result
 * feeds `kernel.loadJson` unchanged. A legacy full document (no envelope, no
 * `__asset` refs) round-trips as a NO-OP — nothing to fetch — so an old
 * server-side document still loads and is externalized on the next save.
 */
export async function inlineProject(
  slot: string,
  backend: BlobBackend,
): Promise<string> {
  const { doc, fonts } = parseEnvelope(slot);
  await restoreFonts(fonts, backend);
  return inlineMeshes(doc, backend);
}

// ── sink / load boundary (IndexedDB) ────────────────────────────────────────

/**
 * Persist a serialized Project to the module {@link AssetStore} (IndexedDB):
 * externalize meshes + fonts to blobs (uploaded via the store's blob methods)
 * and write the lean document ENVELOPE. The autosave sink (fire-and-forget from
 * the debounce timer).
 */
export async function persistProjectDoc(fullJson: string): Promise<void> {
  const envelope = await externalizeProject(fullJson, store);
  await store.writeDoc(envelope);
}

/**
 * Load the persisted Project (lean document + re-inlined mesh blobs + restored
 * fonts) from the module {@link AssetStore}, or `null` when nothing is stored.
 * The boot path feeds the result straight into `ProjectStore.bootstrap`.
 */
export async function loadProjectDoc(): Promise<string | null> {
  const slot = await store.readDoc();
  if (slot === null) return null;
  return inlineProject(slot, store);
}

/**
 * One-way migration of the legacy `fielddraft.project.v2` localStorage row
 * into the {@link AssetStore}. Idempotent and safe to call on every boot:
 *
 *  - No legacy row  → no-op.
 *  - Store already has a document → the legacy row is stale; drop it (and keep
 *    the store's document, which is newer/already-migrated).
 *  - Otherwise → persist the legacy (mesh-embedded) document through
 *    {@link persistProjectDoc} (externalizing its meshes to blobs), then delete
 *    the localStorage row.
 *
 * No-op when neither localStorage nor indexedDB is available (both degrade
 * quietly).
 */
export async function migrateFromLocalStorage(): Promise<void> {
  const legacy = loadPersisted();
  if (legacy === null) return;
  const existing = await store.readDoc();
  if (existing !== null) {
    // Already migrated (or a newer working doc exists) — drop the stale row.
    clearPersisted();
    return;
  }
  await persistProjectDoc(legacy);
  clearPersisted();
}
