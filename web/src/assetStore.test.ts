// Phase 1a: content-addressed asset store — hashing + the IndexedDB backend's
// degraded (no-`indexedDB`) fallback. vitest's jsdom env ships no IndexedDB
// and fake-indexeddb is not a dependency, so we exercise the fallback path and
// (when a real IndexedDB happens to be present) a round-trip.
import { describe, expect, it } from "vitest";
import { IndexedDbAssetStore, sha256Hex } from "./assetStore";

const enc = (s: string) => new TextEncoder().encode(s);

describe("sha256Hex", () => {
  it("matches known NIST vectors (lowercase hex)", async () => {
    expect(await sha256Hex(new Uint8Array())).toBe(
      "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
    expect(await sha256Hex(enc("abc"))).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
  });

  it("is deterministic for identical bytes", async () => {
    const a = await sha256Hex(enc("fielddraft"));
    const b = await sha256Hex(enc("fielddraft"));
    expect(a).toBe(b);
    expect(a).not.toBe(await sha256Hex(enc("fielddraft!")));
  });

  it("honours a view's byte range, not its whole backing buffer", async () => {
    const backing = enc("XXXabcYYY");
    const view = backing.subarray(3, 6); // "abc"
    expect(await sha256Hex(view)).toBe(await sha256Hex(enc("abc")));
  });
});

describe("IndexedDbAssetStore", () => {
  const idbPresent = typeof indexedDB !== "undefined" && indexedDB !== null;

  if (!idbPresent) {
    it("degrades to no-ops when indexedDB is absent", async () => {
      const store = new IndexedDbAssetStore("fielddraft-assets-test");
      // Writes must not throw…
      await store.writeDoc('{"schema_version":1}');
      await store.writeBlob("deadbeef", enc("bytes"));
      await store.deleteBlob("deadbeef");
      // …and reads resolve to empty.
      expect(await store.readDoc()).toBeNull();
      expect(await store.readBlob("deadbeef")).toBeNull();
      expect(await store.hasBlob("deadbeef")).toBe(false);
      expect(await store.listBlobs()).toEqual([]);
    });
  } else {
    it("round-trips a doc + immutable, dedup'd blobs (real IndexedDB)", async () => {
      const store = new IndexedDbAssetStore(
        `fielddraft-assets-test-${Math.random().toString(36).slice(2)}`,
      );
      await store.writeDoc('{"schema_version":1}');
      expect(await store.readDoc()).toBe('{"schema_version":1}');

      const hash = await sha256Hex(enc("hello"));
      await store.writeBlob(hash, enc("hello"));
      expect(await store.hasBlob(hash)).toBe(true);
      expect(new TextDecoder().decode((await store.readBlob(hash))!)).toBe(
        "hello",
      );
      // Immutable: a re-write with different bytes is a no-op.
      await store.writeBlob(hash, enc("tampered"));
      expect(new TextDecoder().decode((await store.readBlob(hash))!)).toBe(
        "hello",
      );
      expect(await store.listBlobs()).toEqual([hash]);

      await store.deleteBlob(hash);
      expect(await store.hasBlob(hash)).toBe(false);
      expect(await store.listBlobs()).toEqual([]);
    });
  }
});
