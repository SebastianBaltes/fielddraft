import { describe, expect, it } from "vitest";
import type { TicketComment, TicketState } from "../api/platform";
import {
  categoryLabel,
  commentVisible,
  personaLabel,
  personaTag,
  settableStates,
  stateBucket,
  stateLabel,
  ticketRef,
  TICKET_STATES,
} from "./ticketView";

function comment(over: Partial<TicketComment> = {}): TicketComment {
  return {
    id: "c1",
    ticket_id: "t1",
    author_type: "user",
    author_id: "u1",
    internal: false,
    body: "hi",
    created_at: "2026-06-01T00:00:00Z",
    withheld: false,
    ...over,
  };
}

describe("ticketView helpers", () => {
  it("labels every state (no fallthrough)", () => {
    const states: TicketState[] = [
      "triage",
      "needs_info",
      "accepted",
      "in_progress",
      "deploying",
      "resolved",
      "closed",
      "declined",
      "backlog",
      "rejected_spam",
      "rejected_malicious",
    ];
    for (const s of states) {
      expect(stateLabel(s)).toBeTruthy();
    }
  });

  it("buckets states into open / progress / backlog / done / rejected", () => {
    expect(stateBucket("triage")).toBe("open");
    expect(stateBucket("needs_info")).toBe("open");
    expect(stateBucket("in_progress")).toBe("progress");
    expect(stateBucket("deploying")).toBe("progress");
    expect(stateBucket("backlog")).toBe("backlog");
    expect(stateBucket("resolved")).toBe("done");
    expect(stateBucket("closed")).toBe("done");
    expect(stateBucket("rejected_spam")).toBe("rejected");
    expect(stateBucket("rejected_malicious")).toBe("rejected");
  });

  it("formats the short #N reference", () => {
    expect(ticketRef(1)).toBe("#1");
    expect(ticketRef(230)).toBe("#230");
  });

  it("labels + tags every persona", () => {
    expect(personaLabel("po")).toBe("Product Owner");
    expect(personaLabel("system")).toBe("System");
    expect(personaTag("dev")).toBe("DEV");
    expect(personaTag("user")).toBe("U");
  });

  it("labels categories with an Uncategorized fallback", () => {
    expect(categoryLabel("bug")).toBe("Bug");
    expect(categoryLabel(null)).toBe("Uncategorized");
    expect(categoryLabel("nonsense")).toBe("Uncategorized");
  });

  // Ticket #345 — mirrors the server: admin sets anything directly, the author
  // gets the safe subset, everyone else nothing.
  describe("settableStates", () => {
    const admin = { isAdmin: true, isAuthor: false };
    const author = { isAdmin: false, isAuthor: true };
    const stranger = { isAdmin: false, isAuthor: false };

    it("gives the admin every state except the one it already has", () => {
      expect(settableStates("triage", admin)).toEqual(
        TICKET_STATES.filter((s) => s !== "triage"),
      );
      // Not a graph edge — the admin may still jump straight there.
      expect(settableStates("triage", admin)).toContain("resolved");
      expect(settableStates("rejected_spam", admin)).toContain("triage");
    });

    it("gives the author close + park from triage", () => {
      expect(settableStates("triage", author)).toEqual(["closed", "backlog"]);
      expect(settableStates("needs_info", author)).toEqual([
        "closed",
        "backlog",
      ]);
    });

    it("gives the author un-park, reopen and close where they apply", () => {
      expect(settableStates("backlog", author)).toEqual(["triage", "closed"]);
      expect(settableStates("closed", author)).toEqual(["triage"]);
      expect(settableStates("resolved", author)).toEqual([
        "in_progress",
        "closed",
      ]);
      expect(settableStates("declined", author)).toEqual(["closed"]);
    });

    it("gives the author nothing mid-flow or from a terminal state", () => {
      expect(settableStates("accepted", author)).toEqual([]);
      expect(settableStates("in_progress", author)).toEqual([]);
      expect(settableStates("deploying", author)).toEqual([]);
      expect(settableStates("rejected_spam", author)).toEqual([]);
    });

    it("gives a stranger nothing at all", () => {
      for (const s of TICKET_STATES) {
        expect(settableStates(s, stranger)).toEqual([]);
      }
    });
  });

  describe("commentVisible (defense-in-depth)", () => {
    it("admins see everything", () => {
      expect(commentVisible(comment({ internal: true }), true)).toBe(true);
      expect(commentVisible(comment({ withheld: true }), true)).toBe(true);
    });

    it("non-admins never see internal dev notes or withheld comments", () => {
      expect(commentVisible(comment({ internal: true }), false)).toBe(false);
      expect(commentVisible(comment({ withheld: true }), false)).toBe(false);
      expect(commentVisible(comment({ author_type: "po" }), false)).toBe(true);
      expect(commentVisible(comment({ author_type: "system" }), false)).toBe(
        true,
      );
    });
  });
});
