/** Autolinker for ticket bodies/comments — URLs clickable, "#N" ticket refs
 *  clickable when a handler is given (ticket #364), rest plain text. */

import { describe, expect, it, vi } from "vitest";
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
  nodes.filter(
    (n): n is ReactElement<AnchorProps> =>
      isValidElement(n) && n.type === "a",
  );

type ButtonProps = {
  onClick?: () => void;
  children?: string;
};
const refButtons = (nodes: ReturnType<typeof linkify>) =>
  nodes.filter(
    (n): n is ReactElement<ButtonProps> =>
      isValidElement(n) && n.type === "button",
  );

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

  // Ticket #364 (teil A): "#N" ticket references become clickable when a
  // handler is passed — the click fires it with the bare number.
  it("turns a '#N' reference into a button wired to onOpenTicket", () => {
    const onOpenTicket = vi.fn();
    const nodes = linkify("Siehe #42 für Details.", onOpenTicket);
    const [btn] = refButtons(nodes);
    expect(btn.props.children).toBe("#42");
    btn.props.onClick?.();
    expect(onOpenTicket).toHaveBeenCalledWith("42");
    expect(nodes[0]).toBe("Siehe ");
    expect(nodes[2]).toBe(" für Details.");
  });

  it("leaves '#N' as plain text without an onOpenTicket handler", () => {
    const nodes = linkify("Siehe #42 für Details.");
    expect(refButtons(nodes)).toHaveLength(0);
    expect(nodes.join("")).toBe("Siehe #42 für Details.");
  });

  it("links both a URL and a ticket ref in the same text, in source order", () => {
    const onOpenTicket = vi.fn();
    const nodes = linkify(
      "Fix in #364, siehe https://example.com/fix.",
      onOpenTicket,
    );
    const [btn] = refButtons(nodes);
    const [a] = anchors(nodes);
    expect(btn.props.children).toBe("#364");
    expect(a.props.href).toBe("https://example.com/fix");
    // The ref comes before the URL in the source text.
    expect(nodes.indexOf(btn)).toBeLessThan(nodes.indexOf(a));
  });
});
