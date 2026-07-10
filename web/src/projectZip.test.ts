// Phase 3d: the pure `.fdraft` byte layer — content-addressed ZIP write/read
// (`serializeFdraftZip` / `readFdraftBytes`) + legacy-format fallbacks. The
// async externalize/inline round-trip lives in `fdraftFile.test.ts`.
import { describe, expect, it } from "vitest";
import { zipSync, unzipSync } from "fflate";
import {
  createDefaultProject,
  serializeProject,
  serializeFdraftZip,
  readFdraftBytes,
} from "./project";

const enc = (s: string) => new TextEncoder().encode(s);
const dec = (b: Uint8Array) => new TextDecoder().decode(b);

describe("serializeFdraftZip / readFdraftBytes (v2, content-addressed)", () => {
  it("writes `project.json` + `assets/<hash>` and reads them back", () => {
    const envelope = JSON.stringify({ v: 2, doc: "{}", fonts: {} });
    const blobs = new Map<string, Uint8Array>([
      ["aaaa", new Uint8Array([1, 2, 3])],
      ["bbbb", new Uint8Array([9, 9])],
    ]);
    const zip = serializeFdraftZip(envelope, blobs);
    // ZIP magic bytes ("PK").
    expect(zip[0]).toBe(0x50);
    expect(zip[1]).toBe(0x4b);

    // The archive carries the envelope under `project.json` and each blob under
    // `assets/<hash>` — no raw document embedding.
    const files = unzipSync(zip);
    expect(dec(files["project.json"])).toBe(envelope);
    expect(Array.from(files["assets/aaaa"])).toEqual([1, 2, 3]);
    expect(Array.from(files["assets/bbbb"])).toEqual([9, 9]);
    expect(files["document.json"]).toBeUndefined();

    const parsed = readFdraftBytes(zip);
    expect(parsed).not.toBeNull();
    expect(parsed!.kind).toBe("v2");
    if (parsed!.kind !== "v2") throw new Error("expected v2");
    expect(parsed!.envelopeJson).toBe(envelope);
    expect(Array.from(parsed!.blobs.get("aaaa")!)).toEqual([1, 2, 3]);
    expect(Array.from(parsed!.blobs.get("bbbb")!)).toEqual([9, 9]);
  });

  it("carries a lean envelope even with no blobs", () => {
    const envelope = JSON.stringify({ v: 2, doc: "{}", fonts: {} });
    const zip = serializeFdraftZip(envelope, new Map());
    const parsed = readFdraftBytes(zip);
    expect(parsed!.kind).toBe("v2");
    if (parsed!.kind !== "v2") throw new Error("expected v2");
    expect(parsed!.blobs.size).toBe(0);
  });
});

describe("readFdraftBytes — legacy fallbacks", () => {
  it("reads legacy plain-JSON bytes (no ZIP) as a project", () => {
    const project = createDefaultProject();
    const jsonBytes = enc(serializeProject(project));
    // Not a ZIP — first byte is '{'.
    expect(jsonBytes[0]).not.toBe(0x50);

    const parsed = readFdraftBytes(jsonBytes);
    expect(parsed).not.toBeNull();
    expect(parsed!.kind).toBe("legacy");
    if (parsed!.kind !== "legacy") throw new Error("expected legacy");
    expect(parsed!.project).toEqual(project);
    expect(parsed!.assets.fonts.size).toBe(0);
  });

  it("reads a legacy embedded ZIP (`document.json` + font manifest)", () => {
    const project = createDefaultProject();
    const fontId = "uploaded:MyFont.ttf";
    const fontPath = "assets/fonts/uploaded_MyFont.ttf.ttf";
    const legacyZip = zipSync({
      "document.json": enc(serializeProject(project)),
      [fontPath]: new Uint8Array([1, 2, 3, 4, 5]),
      "manifest.json": enc(JSON.stringify({ fonts: { [fontId]: fontPath } })),
    });

    const parsed = readFdraftBytes(legacyZip);
    expect(parsed).not.toBeNull();
    expect(parsed!.kind).toBe("legacy");
    if (parsed!.kind !== "legacy") throw new Error("expected legacy");
    expect(parsed!.project).toEqual(project);
    const back = parsed!.assets.fonts.get(fontId);
    expect(back).toBeDefined();
    expect(back!.ext).toBe("ttf");
    expect(Array.from(back!.bytes)).toEqual([1, 2, 3, 4, 5]);
  });

  it("returns null for malformed JSON bytes", () => {
    expect(readFdraftBytes(enc("not json"))).toBeNull();
  });
});
