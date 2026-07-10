/**
 * fdraftFile — the self-contained, content-addressed `.fdraft` boundary
 * (docs/PROJECT-ASSET-STORAGE-PLAN §5.4, Phase 3d).
 *
 * A saved `.fdraft` ZIP is now the SAME content-addressed shape as the
 * IndexedDB working store and the server:
 *
 *   project.json     the lean document ENVELOPE (`{ v, doc, fonts }`) from
 *                    {@link externalizeProject} — features + `__asset` mesh
 *                    hashes + a `fontId → hash` manifest.
 *   assets/<hash>    every referenced binary blob (meshes AND fonts),
 *                    sha256hex-named and deduplicated.
 *
 * Instead of writing the third `BlobBackend` from scratch, this reuses the
 * exact `externalizeProject` / `inlineProject` transforms
 * ({@link ./projectPersistence}) that already power local autosave and cloud
 * save — the ONLY new piece is {@link ZipBlobBackend}, a `BlobBackend` whose
 * "store" is an in-memory hash→bytes {@link Map}. On export the map's entries
 * become the `assets/<hash>` ZIP entries; on import the map is seeded FROM
 * those entries before the document is re-inlined.
 *
 * Backward compatibility (LOAD ONLY): an old fully-embedded `.fdraft` (plain
 * JSON, or a `document.json`+manifest ZIP with meshes inline and fonts under
 * `assets/fonts/…`) still opens — {@link readFdraftBytes} returns a `legacy`
 * variant, whose meshes are already inline (nothing to inline) and whose fonts
 * are re-registered here. No new `.fdraft` is written in that shape.
 */

import {
  readFdraftBytes,
  serializeFdraftZip,
  parseProject,
  type Project,
} from "./project";
import {
  externalizeProject,
  inlineProject,
  type BlobBackend,
} from "./projectPersistence";
import { registerUploadedFontFromBytes } from "./sketchTextFont";

/**
 * A {@link BlobBackend} backed by an in-memory hash→bytes map — the store for
 * the self-contained `.fdraft` ZIP. Writing collects blobs (idempotent, since
 * content-addressed blobs are immutable); reading serves them straight back.
 * The map IS the set of `assets/<hash>` ZIP entries.
 */
export class ZipBlobBackend implements BlobBackend {
  constructor(readonly blobs: Map<string, Uint8Array> = new Map()) {}

  async hasBlob(hash: string): Promise<boolean> {
    return this.blobs.has(hash);
  }

  async writeBlob(hash: string, bytes: Uint8Array): Promise<void> {
    if (!this.blobs.has(hash)) this.blobs.set(hash, bytes);
  }

  async readBlob(hash: string): Promise<Uint8Array | null> {
    return this.blobs.get(hash) ?? null;
  }
}

/**
 * Serialize a full (mesh-inline) project JSON into a content-addressed
 * `.fdraft` ZIP: externalize every embedded mesh and referenced uploaded font
 * to blobs in a {@link ZipBlobBackend}, then pack the lean envelope
 * (`project.json`) plus each blob (`assets/<hash>`).
 */
export async function exportFdraftBytes(fullProjectJson: string): Promise<Uint8Array> {
  const backend = new ZipBlobBackend();
  const envelopeJson = await externalizeProject(fullProjectJson, backend);
  return serializeFdraftZip(envelopeJson, backend.blobs);
}

/**
 * Inverse of {@link exportFdraftBytes}: read `.fdraft` bytes back into a full
 * (mesh-inline) {@link Project}, restoring uploaded fonts into the in-memory
 * cache along the way. Returns `null` on malformed input.
 *
 *  - `v2` (content-addressed): seed a {@link ZipBlobBackend} from the ZIP's
 *    `assets/<hash>` entries, then {@link inlineProject} re-inlines the mesh
 *    blobs and restores the font blobs into the cache.
 *  - `legacy` (fully embedded): the project already carries inline meshes;
 *    re-register any manifest-carried uploaded fonts and return it as-is.
 */
export async function readFdraftProject(bytes: Uint8Array): Promise<Project | null> {
  const contents = readFdraftBytes(bytes);
  if (!contents) return null;

  if (contents.kind === "legacy") {
    // Re-register embedded uploaded fonts so text re-renders with them.
    for (const [id, { bytes: fontBytes }] of contents.assets.fonts) {
      try {
        registerUploadedFontFromBytes(id, fontBytes);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.error("fielddraft: embedded font re-register failed", id, err);
      }
    }
    return contents.project;
  }

  // v2: re-inline mesh blobs + restore fonts from the ZIP's blob entries.
  const backend = new ZipBlobBackend(contents.blobs);
  const fullJson = await inlineProject(contents.envelopeJson, backend);
  return parseProject(fullJson);
}
