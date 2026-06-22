import { describe, it, expect } from "vitest";
import {
  parseProjectParam,
  ticketDir,
  fileApiUrl,
} from "./bootParams";

describe("parseProjectParam", () => {
  it("extracts the project param", () => {
    expect(
      parseProjectParam("?project=tickets/open/abc/project.fdraft"),
    ).toBe("tickets/open/abc/project.fdraft");
  });

  it("decodes percent-encoded slashes (ClaudeDraft URL-encodes the value)", () => {
    expect(
      parseProjectParam("?project=tickets%2Fopen%2Fabc%2Fproject.fdraft"),
    ).toBe("tickets/open/abc/project.fdraft");
  });

  it("returns null when absent or empty", () => {
    expect(parseProjectParam("?foo=bar")).toBeNull();
    expect(parseProjectParam("")).toBeNull();
  });
});

describe("ticketDir", () => {
  it("returns the folder of a path", () => {
    expect(ticketDir("tickets/open/abc/project.fdraft")).toBe(
      "tickets/open/abc",
    );
  });

  it("returns empty for a bare filename", () => {
    expect(ticketDir("project.fdraft")).toBe("");
  });
});

describe("fileApiUrl", () => {
  it("builds an encoded same-origin read url (slashes → %2F)", () => {
    expect(fileApiUrl("tickets/open/abc/project.fdraft")).toBe(
      "/api/file?path=tickets%2Fopen%2Fabc%2Fproject.fdraft",
    );
  });

  it("round-trips through parseProjectParam-style decoding", () => {
    const path = "tickets/done/x y/proj.fdraft";
    const url = fileApiUrl(path);
    const decoded = new URLSearchParams(url.split("?")[1]).get("path");
    expect(decoded).toBe(path);
  });
});
