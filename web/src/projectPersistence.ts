/**
 * projectPersistence — the async persistence sink + load boundary (Phase 1b).
 *
 * A thin wiring layer over the P1a building blocks
 * ({@link ./assetStore | assetStore} + {@link ./assetExternalize |
 * assetExternalize}) that finally hooks content-addressed assets into the
 * live persistence path (docs/PROJECT-ASSET-STORAGE-PLAN §5.2 / §5.3):
 *
 *  - {@link persistProjectDoc}: the autosave SINK. Externalizes a serialized
 *    Project's embedded meshes to immutable blobs and writes the lean document
 *    + blobs to the {@link AssetStore} (default: IndexedDB).
 *  - {@link loadProjectDoc}: the LOAD boundary. Reads the lean document back
 *    and re-inlines the blobs so the result feeds the boot path unchanged
 *    (`ProjectStore.bootstrap` → `kernel.loadJson`).
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
 * `serializeProject()` stays SYNCHRONOUS and keeps emitting the full document
 * with inline meshes; the externalization happens only here, at the async sink,
 * and the inlining only at the load boundary.
 */

import { IndexedDbAssetStore, type AssetStore } from "./assetStore";
import { externalizeAssets, inlineAssets } from "./assetExternalize";
import { loadPersisted, clearPersisted } from "./persistence";

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
 * Externalize every embedded mesh in a serialized Project. Runs the P1a
 * transform on each part tab's inner `documentJson` (see the module note),
 * merging the per-tab blob maps (dedup by hash). Falls back to a direct
 * transform for a non-Project input.
 */
async function externalizeProject(
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

/** Inverse of {@link externalizeProject}: re-inline each part tab's blobs from
 *  the store so the result feeds `kernel.loadJson` unchanged. */
async function inlineProject(slimJson: string): Promise<string> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(slimJson);
  } catch {
    return slimJson;
  }
  const project = parsed as JsonProjectish;
  if (!Array.isArray(project.tabs)) {
    return inlineAssets(slimJson, (hash) => store.readBlob(hash));
  }
  for (const tab of project.tabs) {
    if (!isPartTab(tab)) continue;
    tab.documentJson = await inlineAssets(tab.documentJson, (hash) =>
      store.readBlob(hash),
    );
  }
  return JSON.stringify(project);
}

/**
 * Persist a serialized Project: externalize its embedded meshes to blobs, then
 * write the lean document + every new blob to the {@link AssetStore}. The
 * autosave sink (fire-and-forget from the debounce timer).
 */
export async function persistProjectDoc(fullJson: string): Promise<void> {
  const { doc, blobs } = await externalizeProject(fullJson);
  await store.writeDoc(doc);
  // Blobs are immutable/content-addressed: writeBlob is a no-op when present.
  for (const [hash, bytes] of blobs) await store.writeBlob(hash, bytes);
}

/**
 * Load the persisted Project (lean document + re-inlined blobs), or `null`
 * when nothing is stored. The boot path feeds the result straight into
 * `ProjectStore.bootstrap`.
 */
export async function loadProjectDoc(): Promise<string | null> {
  const slim = await store.readDoc();
  if (slim === null) return null;
  return inlineProject(slim);
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
