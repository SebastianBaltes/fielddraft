// wu2v: ZIP-container `.fdraft` round-trip + legacy-JSON fallback tests.
import { describe, expect, it } from "vitest";
import {
  createDefaultProject,
  serializeProject,
  serializeProjectToZip,
  parseProjectBytes,
  emptyProjectAssets,
  type Project,
} from "./project";

/** A small project with a part tab whose document references an uploaded
 *  font id via a `sketch_path` `text.fontId`. */
function projectWithUploadedFont(fontId: string): Project {
  const base = createDefaultProject();
  const part = base.tabs.find((t) => t.kind === "part");
  if (!part || part.kind !== "part") throw new Error("no part tab");
  const doc = {
    schema_version: 1,
    name: "Part",
    feature_calls: [
      {
        type: "sketch_path",
        params: { text: { group: "g1", value: "Hi", height: 10, fontId, origin: [0, 0] } },
      },
    ],
    next_feature_id: 2,
  };
  part.documentJson = JSON.stringify(doc);
  return base;
}

describe("serializeProjectToZip / parseProjectBytes", () => {
  it("round-trips a project with no assets to an equal project", () => {
    const project = createDefaultProject();
    const zip = serializeProjectToZip(project, emptyProjectAssets());
    // ZIP magic bytes ("PK").
    expect(zip[0]).toBe(0x50);
    expect(zip[1]).toBe(0x4b);

    const parsed = parseProjectBytes(zip);
    expect(parsed).not.toBeNull();
    expect(parsed!.project).toEqual(project);
    expect(parsed!.assets.fonts.size).toBe(0);
  });

  it("parses legacy plain-JSON bytes (no ZIP) as a project", () => {
    const project = createDefaultProject();
    const jsonBytes = new TextEncoder().encode(serializeProject(project));
    // Not a ZIP — first byte is '{'.
    expect(jsonBytes[0]).not.toBe(0x50);

    const parsed = parseProjectBytes(jsonBytes);
    expect(parsed).not.toBeNull();
    expect(parsed!.project).toEqual(project);
    expect(parsed!.assets.fonts.size).toBe(0);
  });

  it("round-trips uploaded font bytes embedded in the ZIP", () => {
    const fontId = "uploaded:MyFont.ttf";
    const project = projectWithUploadedFont(fontId);
    const fontBytes = new Uint8Array([1, 2, 3, 4, 5]);
    const assets = emptyProjectAssets();
    assets.fonts.set(fontId, { bytes: fontBytes, ext: "ttf" });

    const zip = serializeProjectToZip(project, assets);
    const parsed = parseProjectBytes(zip);
    expect(parsed).not.toBeNull();
    expect(parsed!.project).toEqual(project);

    const back = parsed!.assets.fonts.get(fontId);
    expect(back).toBeDefined();
    expect(back!.ext).toBe("ttf");
    expect(Array.from(back!.bytes)).toEqual([1, 2, 3, 4, 5]);
  });

  it("returns null for malformed JSON bytes", () => {
    expect(parseProjectBytes(new TextEncoder().encode("not json"))).toBeNull();
  });
});
