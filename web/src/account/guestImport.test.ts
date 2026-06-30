import { describe, expect, it } from "vitest";
import { guestDocumentHasContent, shouldOfferGuestImport } from "./guestImport";

/** A serialized project document with the given tabs. */
function doc(tabs: unknown[]): unknown {
  return { schemaVersion: 1, name: "Untitled", tabs, activeTabId: "tab_1" };
}

const EMPTY_PART = {
  id: "tab_1",
  kind: "part",
  name: "Part 1",
  documentJson:
    '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}',
};

const PART_WITH_FEATURE = {
  id: "tab_1",
  kind: "part",
  name: "Part 1",
  documentJson:
    '{"schema_version":1,"name":"Part","feature_calls":[{"id":1}],"next_feature_id":2}',
};

describe("guestDocumentHasContent", () => {
  it("is false for a pristine single empty part tab", () => {
    expect(guestDocumentHasContent(doc([EMPTY_PART]))).toBe(false);
  });

  it("is true when the single part tab has features", () => {
    expect(guestDocumentHasContent(doc([PART_WITH_FEATURE]))).toBe(true);
  });

  it("is true with more than one tab", () => {
    expect(
      guestDocumentHasContent(
        doc([EMPTY_PART, { ...EMPTY_PART, id: "tab_2", name: "Part 2" }]),
      ),
    ).toBe(true);
  });

  it("is true for a script tab", () => {
    expect(
      guestDocumentHasContent(
        doc([{ id: "tab_1", kind: "script", name: "S", record: {} }]),
      ),
    ).toBe(true);
  });

  it("is true for an assembly with part instances", () => {
    expect(
      guestDocumentHasContent(
        doc([
          {
            id: "tab_1",
            kind: "assembly",
            name: "A",
            document: { parts: [{ id: "x" }], mates: [] },
          },
        ]),
      ),
    ).toBe(true);
  });

  it("is false for an empty assembly", () => {
    expect(
      guestDocumentHasContent(
        doc([
          {
            id: "tab_1",
            kind: "assembly",
            name: "A",
            document: { parts: [], mates: [] },
          },
        ]),
      ),
    ).toBe(false);
  });

  it("accepts a JSON string as well as a parsed object", () => {
    expect(
      guestDocumentHasContent(JSON.stringify(doc([PART_WITH_FEATURE]))),
    ).toBe(true);
  });

  it("is false for malformed / empty input", () => {
    expect(guestDocumentHasContent("not json")).toBe(false);
    expect(guestDocumentHasContent(null)).toBe(false);
    expect(guestDocumentHasContent({})).toBe(false);
    expect(guestDocumentHasContent(doc([]))).toBe(false);
  });
});

describe("shouldOfferGuestImport", () => {
  const content = doc([PART_WITH_FEATURE]);

  it("offers when authed, unbound, with real content", () => {
    expect(
      shouldOfferGuestImport({
        authed: true,
        boundProjectId: null,
        guestDocument: content,
      }),
    ).toBe(true);
  });

  it("does not offer to a guest (not authed)", () => {
    expect(
      shouldOfferGuestImport({
        authed: false,
        boundProjectId: null,
        guestDocument: content,
      }),
    ).toBe(false);
  });

  it("does not offer when already bound to a server project", () => {
    expect(
      shouldOfferGuestImport({
        authed: true,
        boundProjectId: "p1",
        guestDocument: content,
      }),
    ).toBe(false);
  });

  it("does not offer for a pristine / empty workspace", () => {
    expect(
      shouldOfferGuestImport({
        authed: true,
        boundProjectId: null,
        guestDocument: doc([EMPTY_PART]),
      }),
    ).toBe(false);
    expect(
      shouldOfferGuestImport({
        authed: true,
        boundProjectId: null,
        guestDocument: null,
      }),
    ).toBe(false);
  });
});
