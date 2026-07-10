/**
 * serverBlobStore — the {@link BlobBackend} over the server blob API (Phase 2).
 *
 * The cloud counterpart to {@link ./assetStore | IndexedDbAssetStore}: instead
 * of IndexedDB, mesh/font blobs are content-addressed on the server
 * (`/api/blobs/*`, plan §5.5) so a server-bound project stores only a LEAN
 * document (features + asset hashes) and the heavy payloads dedup across
 * projects and users.
 *
 * Exposes exactly the blob subset the externalize/inline path needs
 * ({@link ./projectPersistence}'s `externalizeProject`/`inlineProject`):
 *
 *  - {@link hasBlob}  → `POST /api/blobs/missing` (batched existence probe) so a
 *    blob the server already has is never re-uploaded.
 *  - {@link writeBlob} → `PUT /api/blobs/{hash}` (upload-if-missing; the server
 *    verifies `sha256(body) == hash`).
 *  - {@link readBlob}  → `GET /api/blobs/{hash}` (404 ⇒ `null`).
 *
 * All requests are cookie-authenticated (`credentials: "include"`).
 */

import type { BlobBackend } from "./projectPersistence";

export class ServerBlobStore implements BlobBackend {
  async hasBlob(hash: string): Promise<boolean> {
    const res = await fetch(`/api/blobs/missing`, {
      method: "POST",
      credentials: "include",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ hashes: [hash] }),
    });
    if (!res.ok) return false; // unknown ⇒ treat as absent (will attempt upload)
    const data = (await res.json()) as { missing?: string[] };
    return !(data.missing ?? []).includes(hash);
  }

  async writeBlob(hash: string, bytes: Uint8Array): Promise<void> {
    // Copy into a fresh ArrayBuffer so fetch gets a plain-ArrayBuffer BodyInit
    // (never a WASM/heap or SharedArrayBuffer-backed view).
    const body = bytes.slice();
    const res = await fetch(`/api/blobs/${hash}`, {
      method: "PUT",
      credentials: "include",
      headers: { "Content-Type": "application/octet-stream" },
      body,
    });
    if (!res.ok) {
      throw new Error(`fielddraft: blob upload failed (${hash}): ${res.status}`);
    }
  }

  async readBlob(hash: string): Promise<Uint8Array | null> {
    const res = await fetch(`/api/blobs/${hash}`, { credentials: "include" });
    if (res.status === 404) return null;
    if (!res.ok) {
      throw new Error(`fielddraft: blob fetch failed (${hash}): ${res.status}`);
    }
    const buf = await res.arrayBuffer();
    return new Uint8Array(buf);
  }
}

/** Module-singleton — the server blob backend the cloud save/load path uses. */
export const serverBlobStore = new ServerBlobStore();
