import { describe, expect, it } from "vitest";
import { parseOnshapeUrl } from "./onshapeUrl";

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
