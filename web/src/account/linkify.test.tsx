/** Autolinker for ticket bodies/comments — URLs clickable, rest plain text. */

import { describe, expect, it } from "vitest";
import { isValidElement } from "react";
import type { ReactElement } from "react";
import { linkify } from "./linkify";

type AnchorProps = {
  href: string;
  target?: string;
  rel?: string;
  children?: string;
};
const anchors = (nodes: ReturnType<typeof linkify>) =>
  nodes.filter((n): n is ReactElement<AnchorProps> => isValidElement(n));

describe("linkify", () => {
  it("passes plain text through untouched", () => {
    expect(linkify("kein Link hier")).toEqual(["kein Link hier"]);
  });

  it("turns an http(s) URL into a new-tab anchor", () => {
    const url = "https://github.com/SebastianBaltes/fielddraft/commit/b38fe65";
    const nodes = linkify(`Commit: ${url} (v0.6.448)`);
    const [a] = anchors(nodes);
    expect(a.props.href).toBe(url);
    expect(a.props.target).toBe("_blank");
    expect(a.props.rel).toBe("noreferrer");
    expect(nodes[0]).toBe("Commit: ");
    expect(nodes[2]).toBe(" (v0.6.448)");
  });

  it("keeps trailing sentence punctuation outside the link", () => {
    const nodes = linkify("Siehe https://example.com/fix. Danach neu laden.");
    const [a] = anchors(nodes);
    expect(a.props.href).toBe("https://example.com/fix");
    expect(nodes[2]).toBe(". Danach neu laden.");
  });

  it("links multiple URLs and ignores non-http schemes", () => {
    const nodes = linkify(
      "https://a.example und https://b.example sowie javascript:alert(1)",
    );
    const hrefs = anchors(nodes).map((a) => a.props.href);
    expect(hrefs).toEqual(["https://a.example", "https://b.example"]);
    expect(nodes[nodes.length - 1]).toBe(" sowie javascript:alert(1)");
  });
});
