/** Hash routing (ticket 3032c29b): route ↔ hash round trip. */

import { describe, expect, it } from "vitest";
import { routeToHash, parseHash, type Route } from "./hashRoute";

describe("hashRoute", () => {
  it("round-trips every route shape", () => {
    const routes: Route[] = [
      { page: "project", projectId: null },
      { page: "project", projectId: "abc123" },
      { page: "overview", tab: "projects", ticketId: null },
      { page: "overview", tab: "issues", ticketId: null },
      { page: "overview", tab: "issues", ticketId: "63989a9e" },
      { page: "overview", tab: "invites", ticketId: null },
      { page: "overview", tab: "users", ticketId: null },
    ];
    for (const r of routes) {
      expect(parseHash(routeToHash(r))).toEqual(r);
    }
  });

  it("maps the documented hashes", () => {
    expect(routeToHash({ page: "project", projectId: null })).toBe("#/edit");
    expect(routeToHash({ page: "project", projectId: "x" })).toBe("#/p/x");
    expect(
      routeToHash({ page: "overview", tab: "issues", ticketId: "t1" }),
    ).toBe("#/issues/t1");
  });

  it("unknown or empty hashes parse to null", () => {
    expect(parseHash("")).toBeNull();
    expect(parseHash("#/")).toBeNull();
    expect(parseHash("#/nope")).toBeNull();
    expect(parseHash("#/p")).toBeNull();
  });

  it("tolerates trailing slashes and missing #", () => {
    expect(parseHash("#/issues/")).toEqual({
      page: "overview",
      tab: "issues",
      ticketId: null,
    });
    expect(parseHash("/projects")).toEqual({
      page: "overview",
      tab: "projects",
      ticketId: null,
    });
  });
});
