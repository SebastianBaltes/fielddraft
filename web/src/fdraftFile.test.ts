/**
 * Phase 3d: the self-contained, content-addressed `.fdraft` boundary.
 *
 * Verifies that a project with an embedded mesh exports to a ZIP whose
 * `project.json` is LEAN (mesh externalized to a hash) and whose bytes live
 * in `assets/<hash>` — and that re-importing yields the identical project
 * (mesh re-inlined). Also covers backward-compat: an OLD fully-embedded
 * `.fdraft` (mesh inline in `document.json`) still loads.
 */
import { describe, expect, it } from "vitest";
import { zipSync, unzipSync } from "fflate";
import { exportFdraftBytes, readFdraftProject, ZipBlobBackend } from "./fdraftFile";
import { serializeProject } from "./project";

const enc = (s: string) => new TextEncoder().encode(s);
const dec = (b: Uint8Array) => new TextDecoder().decode(b);

/** A full (mesh-inline) serialized Project whose single part tab embeds an
 *  `imported_mesh`. All values are f32-exact so the round-trip is lossless. */
function projectWithMeshJson(): string {
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

/** Parse a Project and its part tabs' inner docs so a deep comparison is
 *  key-order-independent (the externalize→inline round-trip re-appends payload
 *  keys, so the inner string is equal only after re-parsing). */
function normalized(projectJson: string): unknown {
  const p = JSON.parse(projectJson) as { tabs: { kind: string; documentJson?: string }[] };
  for (const tab of p.tabs) {
    if (tab.kind === "part" && typeof tab.documentJson === "string") {
      (tab as { documentJson: unknown }).documentJson = JSON.parse(tab.documentJson);
    }
  }
  return p;
}

describe("ZipBlobBackend", () => {
  it("is an idempotent, content-addressed in-memory blob store", async () => {
    const b = new ZipBlobBackend();
    expect(await b.hasBlob("h")).toBe(false);
    await b.writeBlob("h", new Uint8Array([1, 2]));
    expect(await b.hasBlob("h")).toBe(true);
    // Immutable: a second write for the same hash is a no-op.
    await b.writeBlob("h", new Uint8Array([9, 9, 9]));
    expect(Array.from((await b.readBlob("h"))!)).toEqual([1, 2]);
    expect(await b.readBlob("missing")).toBeNull();
  });
});

describe("exportFdraftBytes / readFdraftProject (round-trip)", () => {
  it("externalizes the mesh to `assets/<hash>` and keeps `project.json` lean", async () => {
    const zip = await exportFdraftBytes(projectWithMeshJson());
    // ZIP magic ("PK").
    expect(zip[0]).toBe(0x50);
    expect(zip[1]).toBe(0x4b);

    const files = unzipSync(zip);
    const paths = Object.keys(files);
    // Exactly one asset blob, named by its sha256 hex, plus the envelope.
    const assetPaths = paths.filter((p) => p.startsWith("assets/"));
    expect(assetPaths).toHaveLength(1);
    expect(assetPaths[0]).toMatch(/^assets\/[0-9a-f]{64}$/);
    expect(files["project.json"]).toBeDefined();
    expect(files["document.json"]).toBeUndefined();

    // The envelope is LEAN: the raw mesh payload is gone, only a hash ref.
    const envelope = dec(files["project.json"]);
    expect(envelope).not.toContain("__positions");
    expect(envelope).not.toContain("__indices");
    expect(envelope).toContain("__asset");
    // The blob's name IS the referenced hash.
    const hash = assetPaths[0].slice("assets/".length);
    expect(envelope).toContain(hash);
  });

  it("re-imports to the identical project (mesh re-inlined)", async () => {
    const original = projectWithMeshJson();
    const zip = await exportFdraftBytes(original);

    const project = await readFdraftProject(zip);
    expect(project).not.toBeNull();
    const back = serializeProject(project!);
    // The mesh is inline again and structurally matches the original.
    expect(back).toContain("__positions");
    expect(back).not.toContain("__asset");
    expect(normalized(back)).toEqual(normalized(original));
  });
});

describe("readFdraftProject — backward compatibility", () => {
  it("loads an OLD fully-embedded `.fdraft` (mesh inline in document.json)", async () => {
    // A legacy ZIP: the whole (mesh-inline) project under `document.json`, no
    // envelope, no `assets/<hash>` — exactly what the previous format wrote.
    const legacyZip = zipSync({
      "document.json": enc(projectWithMeshJson()),
      "manifest.json": enc(JSON.stringify({ fonts: {} })),
    });

    const project = await readFdraftProject(legacyZip);
    expect(project).not.toBeNull();
    const back = serializeProject(project!);
    // The embedded mesh survives untouched (nothing to inline).
    expect(back).toContain("__positions");
    expect(normalized(back)).toEqual(normalized(projectWithMeshJson()));
  });

  it("loads legacy plain-JSON `.fdraft` bytes", async () => {
    const original = projectWithMeshJson();
    const project = await readFdraftProject(enc(original));
    expect(project).not.toBeNull();
    expect(normalized(serializeProject(project!))).toEqual(normalized(original));
  });

  it("returns null for malformed bytes", async () => {
    expect(await readFdraftProject(enc("not a project"))).toBeNull();
  });
});
