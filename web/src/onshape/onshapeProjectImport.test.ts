/**
 * Ticket #343 — what an Onshape import must remember.
 *
 * The kernel + translator are stubbed out (they have their own tests); what is
 * under test here is the project envelope the import produces: the title comes
 * from the Onshape DOCUMENT, and the resolved Onshape ids ride along so the
 * project card can offer "Open in Onshape" and "Reimport".
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import * as platform from "../api/platform";

// Plain functions, not vi.fn() — the platformApi spies below are torn down with
// restoreAllMocks, which would strip a vi.fn()'s implementation from these too.
vi.mock("../kernel", () => ({
  Kernel: { create: async () => ({ toJson: () => "{}" }) },
}));
vi.mock("./onshapeImport", () => ({
  importOnshapeFeatures: async () => ({
    created: ["f1", "f2"],
    warnings: [],
    errors: [],
  }),
}));

const { importOnshapeProject } = await import("./onshapeProjectImport");

const DOC = "0123456789abcdef01234567";
const WS = "89abcdef0123456789abcdef";
const EL = "fedcba9876543210fedcba98";
const URL_WITH_ELEMENT = `https://cad.onshape.com/documents/${DOC}/w/${WS}/e/${EL}`;
const NOW = "2026-07-13T20:00:00.000Z";

beforeEach(() => vi.restoreAllMocks());

describe("importOnshapeProject", () => {
  it("titles the project after the Onshape DOCUMENT and stores the source", async () => {
    const doc = vi
      .spyOn(platform.platformApi, "getOnshapeDocument")
      .mockResolvedValue({ name: "Lötkolbenhalter" });
    vi.spyOn(platform.platformApi, "getOnshapeFeatures").mockResolvedValue({
      features: [],
    });

    const result = await importOnshapeProject(URL_WITH_ELEMENT, { now: NOW });

    expect(doc).toHaveBeenCalledWith(DOC);
    expect(result.project.name).toBe("Lötkolbenhalter");
    expect(result.project.onshape).toEqual({
      documentId: DOC,
      workspaceId: WS,
      elementId: EL,
      documentName: "Lötkolbenhalter",
      importedAt: NOW,
    });
    expect(result.featureCount).toBe(2);
  });

  it("resolves the first Part Studio when the URL has no element", async () => {
    const elements = vi
      .spyOn(platform.platformApi, "getOnshapeElements")
      .mockResolvedValue({
        elements: [
          { id: "aaa", elementType: "DRAWING", name: "Drawing 1" },
          { id: EL, elementType: "PARTSTUDIO", name: "Part Studio 1" },
        ],
      });
    vi.spyOn(platform.platformApi, "getOnshapeDocument").mockResolvedValue({
      name: "Lötkolbenhalter",
    });
    const features = vi
      .spyOn(platform.platformApi, "getOnshapeFeatures")
      .mockResolvedValue({ features: [] });

    const result = await importOnshapeProject(
      `https://cad.onshape.com/documents/${DOC}/w/${WS}`,
      { now: NOW },
    );

    expect(elements).toHaveBeenCalledWith(DOC, WS);
    expect(features).toHaveBeenCalledWith(DOC, WS, EL);
    // The RESOLVED element is what the project remembers — so a later reimport
    // hits the same Part Studio instead of re-guessing.
    expect(result.project.onshape?.elementId).toBe(EL);
    // …and the title is the document's, not the element's ("Part Studio 1").
    expect(result.project.name).toBe("Lötkolbenhalter");
  });

  it("reimports from a stored source without touching the elements endpoint", async () => {
    const elements = vi.spyOn(platform.platformApi, "getOnshapeElements");
    vi.spyOn(platform.platformApi, "getOnshapeDocument").mockResolvedValue({
      name: "Lötkolbenhalter v2",
    });
    const features = vi
      .spyOn(platform.platformApi, "getOnshapeFeatures")
      .mockResolvedValue({ features: [] });

    const result = await importOnshapeProject(
      { documentId: DOC, workspaceId: WS, elementId: EL },
      { now: NOW, fallbackName: "Alt" },
    );

    expect(elements).not.toHaveBeenCalled();
    expect(features).toHaveBeenCalledWith(DOC, WS, EL);
    // A rename in Onshape follows through on reimport.
    expect(result.project.name).toBe("Lötkolbenhalter v2");
  });

  it("keeps the old name when Onshape cannot tell us the document name", async () => {
    vi.spyOn(platform.platformApi, "getOnshapeDocument").mockRejectedValue(
      new Error("410 gone"),
    );
    vi.spyOn(platform.platformApi, "getOnshapeFeatures").mockResolvedValue({
      features: [],
    });

    const result = await importOnshapeProject(
      { documentId: DOC, workspaceId: WS, elementId: EL },
      { now: NOW, fallbackName: "Mein Projekt" },
    );

    expect(result.project.name).toBe("Mein Projekt");
  });

  it("rejects a string that is not an Onshape URL", async () => {
    await expect(
      importOnshapeProject("https://example.com/nope", { now: NOW }),
    ).rejects.toThrow(/Onshape-URL/);
  });
});
