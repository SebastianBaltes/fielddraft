import { describe, expect, it } from "vitest";
import { parseOnshapeUrl, type OnshapeSource } from "./onshapeUrl";
import { createDefaultProject, parseProject, serializeProject } from "../project";

const DID = "a1b2c3d4e5f60718293a4b5c";
const WID = "0123456789abcdef01234567";
const EID = "fedcba98765432100fedcba9";

describe("parseOnshapeUrl", () => {
  it("parses a full /documents/…/w/…/e/… URL", () => {
    expect(
      parseOnshapeUrl(`https://cad.onshape.com/documents/${DID}/w/${WID}/e/${EID}`),
    ).toEqual({ documentId: DID, workspaceId: WID, elementId: EID });
  });

  it("parses the short /d/… share form", () => {
    expect(
      parseOnshapeUrl(`https://cad.onshape.com/d/${DID}/w/${WID}/e/${EID}`),
    ).toEqual({ documentId: DID, workspaceId: WID, elementId: EID });
  });

  it("returns elementId null when /e/… is missing", () => {
    expect(
      parseOnshapeUrl(`https://cad.onshape.com/documents/${DID}/w/${WID}`),
    ).toEqual({ documentId: DID, workspaceId: WID, elementId: null });
  });

  it("tolerates surrounding whitespace and trailing path segments", () => {
    expect(
      parseOnshapeUrl(
        `  https://cad.onshape.com/documents/${DID}/w/${WID}/e/${EID}?renderMode=0  `,
      ),
    ).toEqual({ documentId: DID, workspaceId: WID, elementId: EID });
  });

  it("accepts uppercase hex ids", () => {
    const up = DID.toUpperCase();
    expect(parseOnshapeUrl(`/documents/${up}/w/${WID}/e/${EID}`)).toEqual({
      documentId: up,
      workspaceId: WID,
      elementId: EID,
    });
  });

  it("rejects non-Onshape strings and short ids", () => {
    expect(parseOnshapeUrl("https://example.com/foo")).toBeNull();
    expect(parseOnshapeUrl(`/documents/abc123/w/${WID}/e/${EID}`)).toBeNull();
    expect(parseOnshapeUrl("")).toBeNull();
  });
});

// Ticket #376: the operator opened a project saved from an Onshape import and
// found neither the Onshape URL nor the document name in the stored `.fdraft`
// — `OnshapeSource` used to carry only the three raw ids, so the human-facing
// origin (URL + name) was silently dropped on the very first save/load cycle,
// even though the import step itself had both.
describe("OnshapeSource url/documentName survive the save→load round trip (#376)", () => {
  it("keeps url + documentName through serializeProject → parseProject", () => {
    const project = createDefaultProject();
    const onshape: OnshapeSource = {
      documentId: DID,
      workspaceId: WID,
      elementId: EID,
      documentName: "Lötkolbenhalter",
      url: `https://cad.onshape.com/documents/${DID}/w/${WID}/e/${EID}`,
      importedAt: "2026-07-13T18:44:25.171Z",
    };
    project.onshape = onshape;

    // The real save path: serializeProject() is exactly what `.fdraft`'s
    // `project.json` envelope embeds as its `doc` string (project.ts's own
    // `serializeFdraftZip` comment), and parseProject() is its inverse.
    const json = serializeProject(project);
    const loaded = parseProject(json);

    expect(loaded).not.toBeNull();
    expect(loaded!.onshape).toEqual(onshape);
    expect(loaded!.onshape?.url).toBe(
      `https://cad.onshape.com/documents/${DID}/w/${WID}/e/${EID}`,
    );
    expect(loaded!.onshape?.documentName).toBe("Lötkolbenhalter");
  });

  it("still round-trips a pre-#376 source that has no url/documentName", () => {
    const project = createDefaultProject();
    project.onshape = { documentId: DID, workspaceId: WID, elementId: EID };

    const loaded = parseProject(serializeProject(project));

    expect(loaded!.onshape).toEqual({
      documentId: DID,
      workspaceId: WID,
      elementId: EID,
    });
  });
});
