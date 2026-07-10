/**
 * Phase 2: the backend-agnostic externalize/inline path + the server blob
 * backend. Verifies that {@link externalizeProject}/{@link inlineProject} run
 * over ANY {@link BlobBackend} (a Map-backed fake here, standing in for the
 * server), that a legacy FULL document loads as a no-op, and that
 * {@link ServerBlobStore} maps the blob methods onto `/api/blobs/*`.
 */
import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { sha256Hex } from "./assetStore";
import {
  externalizeProject,
  inlineProject,
  __setFontCacheForTest,
  type BlobBackend,
} from "./projectPersistence";
import { ServerBlobStore } from "./serverBlobStore";

/** Map-backed {@link BlobBackend} — the storage-agnostic path under test, so
 *  this stands in for both IndexedDB and the server. Records upload attempts so
 *  a test can assert upload-if-missing dedup. */
class FakeBlobBackend implements BlobBackend {
  blobs = new Map<string, Uint8Array>();
  writes: string[] = [];
  reads: string[] = [];
  async hasBlob(hash: string) {
    return this.blobs.has(hash);
  }
  async writeBlob(hash: string, bytes: Uint8Array) {
    this.writes.push(hash);
    if (!this.blobs.has(hash)) this.blobs.set(hash, bytes);
  }
  async readBlob(hash: string) {
    this.reads.push(hash);
    return this.blobs.get(hash) ?? null;
  }
}

class FakeFontCache {
  bytes = new Map<string, Uint8Array>();
  get(id: string) {
    return this.bytes.get(id);
  }
  set(id: string, b: Uint8Array) {
    this.bytes.set(id, b);
  }
}

function projectWithMesh(): string {
  const documentJson = JSON.stringify({
    schema_version: 1,
    name: "Part",
    feature_calls: [
      {
        type: "imported_mesh",
        id: "1",
        params: {
          op: "New",
          __positions: [0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1],
          __indices: [0, 1, 2, 0, 1, 3],
          __watertight: true,
        },
      },
    ],
    next_feature_id: 2,
  });
  return JSON.stringify({
    schemaVersion: 1,
    name: "Untitled",
    tabs: [{ id: "tab_1", kind: "part", name: "Part 1", documentJson }],
    activeTabId: "tab_1",
  });
}

function projectWithFont(fontId: string): string {
  const documentJson = JSON.stringify({
    schema_version: 1,
    name: "Part",
    feature_calls: [
      {
        type: "sketch_path",
        id: "1",
        params: { text: { group: "g1", value: "Hi", height: 10, fontId, origin: [0, 0] } },
      },
    ],
    next_feature_id: 2,
  });
  return JSON.stringify({
    schemaVersion: 1,
    name: "Untitled",
    tabs: [{ id: "tab_1", kind: "part", name: "Part 1", documentJson }],
    activeTabId: "tab_1",
  });
}

/** Key-order-independent normalization: parse the Project + each part tab's
 *  inner doc so a deep compare survives the externalize→inline re-append. */
function normalized(projectJson: string): unknown {
  const p = JSON.parse(projectJson) as { tabs: { kind: string; documentJson?: string }[] };
  for (const tab of p.tabs) {
    if (tab.kind === "part" && typeof tab.documentJson === "string") {
      (tab as { documentJson: unknown }).documentJson = JSON.parse(tab.documentJson);
    }
  }
  return p;
}

describe("externalizeProject / inlineProject over a BlobBackend", () => {
  let fonts: FakeFontCache;
  beforeEach(() => {
    fonts = new FakeFontCache();
    __setFontCacheForTest(fonts);
  });

  it("uploads the mesh blob and returns a lean envelope", async () => {
    const backend = new FakeBlobBackend();
    const envelopeJson = await externalizeProject(projectWithMesh(), backend);

    // One blob uploaded; the envelope is lean (no raw payload, an __asset ref).
    expect(backend.blobs.size).toBe(1);
    expect(backend.writes.length).toBe(1);
    const env = JSON.parse(envelopeJson) as { v: number; doc: string; fonts: unknown };
    expect(env.v).toBe(2);
    expect(env.doc).toContain("__asset");
    expect(env.doc).not.toContain("__positions");
    expect(env.doc).not.toContain("__indices");
  });

  it("skips re-uploading a blob the backend already has (upload-if-missing)", async () => {
    const backend = new FakeBlobBackend();
    await externalizeProject(projectWithMesh(), backend);
    backend.writes.length = 0;
    // Second externalize of the same mesh → present ⇒ no write.
    await externalizeProject(projectWithMesh(), backend);
    expect(backend.writes.length).toBe(0);
    expect(backend.blobs.size).toBe(1);
  });

  it("round-trips a project through externalize → inline (server-shaped)", async () => {
    const original = projectWithMesh();
    const backend = new FakeBlobBackend();
    const envelope = await externalizeProject(original, backend);

    const inlined = await inlineProject(envelope, backend);
    expect(inlined).toContain("__positions");
    expect(inlined).not.toContain("__asset");
    expect(normalized(inlined)).toEqual(normalized(original));
  });

  it("uploads a referenced font and restores it on inline", async () => {
    const FONT_ID = "uploaded:MyFont.ttf";
    const FONT_BYTES = new Uint8Array([0x00, 0x01, 0x00, 0x00, 0x42, 0x99]);
    fonts.set(FONT_ID, FONT_BYTES);

    const backend = new FakeBlobBackend();
    const envelopeJson = await externalizeProject(projectWithFont(FONT_ID), backend);
    const hash = await sha256Hex(FONT_BYTES);
    expect(backend.blobs.get(hash)).toEqual(FONT_BYTES);
    const env = JSON.parse(envelopeJson) as { fonts: Record<string, string> };
    expect(env.fonts).toEqual({ [FONT_ID]: hash });

    // Simulate a reload: empty the font cache, then inline restores the bytes.
    fonts.bytes.clear();
    const inlined = await inlineProject(envelopeJson, backend);
    expect(fonts.get(FONT_ID)).toEqual(FONT_BYTES);
    expect(inlined).toContain(FONT_ID);
  });

  it("loads a legacy FULL document as a no-op (no blob fetches)", async () => {
    // A legacy server document: the full Project blob with the mesh INLINE and
    // no envelope. inlineProject must return it unchanged without touching the
    // backend (parseEnvelope tolerates a non-envelope; no __asset refs).
    const full = projectWithMesh();
    const backend = new FakeBlobBackend();
    const out = await inlineProject(full, backend);
    expect(backend.reads.length).toBe(0);
    expect(normalized(out)).toEqual(normalized(full));
  });
});

describe("ServerBlobStore", () => {
  let fetchMock: ReturnType<typeof vi.fn>;
  beforeEach(() => {
    fetchMock = vi.fn();
    globalThis.fetch = fetchMock as unknown as typeof fetch;
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("hasBlob is false when the hash is reported missing, true otherwise", async () => {
    const s = new ServerBlobStore();
    fetchMock.mockResolvedValueOnce(
      new Response(JSON.stringify({ missing: ["abc"] }), { status: 200 }),
    );
    expect(await s.hasBlob("abc")).toBe(false);

    fetchMock.mockResolvedValueOnce(
      new Response(JSON.stringify({ missing: [] }), { status: 200 }),
    );
    expect(await s.hasBlob("abc")).toBe(true);

    // Probes the batch-missing endpoint.
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/blobs/missing");
    expect((init as RequestInit).method).toBe("POST");
  });

  it("writeBlob PUTs the raw bytes to /api/blobs/{hash}", async () => {
    const s = new ServerBlobStore();
    fetchMock.mockResolvedValueOnce(
      new Response(JSON.stringify({ stored: true }), { status: 200 }),
    );
    await s.writeBlob("deadbeef", new Uint8Array([1, 2, 3]));
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/blobs/deadbeef");
    expect((init as RequestInit).method).toBe("PUT");
    expect((init as RequestInit).credentials).toBe("include");
  });

  it("writeBlob throws on a non-ok response", async () => {
    const s = new ServerBlobStore();
    fetchMock.mockResolvedValueOnce(new Response("nope", { status: 400 }));
    await expect(s.writeBlob("x", new Uint8Array([1]))).rejects.toThrow();
  });

  it("readBlob returns bytes on 200 and null on 404", async () => {
    const s = new ServerBlobStore();
    const payload = new Uint8Array([9, 8, 7]);
    fetchMock.mockResolvedValueOnce(
      new Response(payload, { status: 200 }),
    );
    expect(await s.readBlob("h")).toEqual(payload);

    fetchMock.mockResolvedValueOnce(new Response("", { status: 404 }));
    expect(await s.readBlob("missing")).toBeNull();
  });
});
