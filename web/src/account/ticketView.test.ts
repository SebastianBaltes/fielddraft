import { describe, expect, it } from "vitest";
import type { TicketComment, TicketState } from "../api/platform";
import {
  categoryLabel,
  commentAuthorLabel,
  commentAuthorTag,
  commentVisible,
  personaLabel,
  personaTag,
  settableStates,
  stateBucket,
  stateLabel,
  ticketAuthorLabel,
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
      "in_progress",
      "parked",
      "closed",
      "rejected",
    ];
    for (const s of states) {
      expect(stateLabel(s)).toBeTruthy();
    }
  });

  it("buckets states into open / progress / backlog / done / rejected", () => {
    expect(stateBucket("triage")).toBe("open");
    expect(stateBucket("needs_info")).toBe("open");
    expect(stateBucket("in_progress")).toBe("progress");
    expect(stateBucket("parked")).toBe("backlog");
    expect(stateBucket("closed")).toBe("done");
    expect(stateBucket("rejected")).toBe("rejected");
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

  it("shows the human name over the persona when author_name is set (#359)", () => {
    // A state-change audit a user triggered: author_type=system, but named.
    const audit = comment({ author_type: "system", author_name: "Sebastian Baltes" });
    expect(commentAuthorLabel(audit)).toBe("Sebastian Baltes");
    expect(commentAuthorTag(audit)).toBe("SB");

    // A user comment with the poster's name.
    const mine = comment({ author_type: "user", author_name: "sorokan" });
    expect(commentAuthorLabel(mine)).toBe("sorokan");
    expect(commentAuthorTag(mine)).toBe("SO");
  });

  it("falls back to the persona for automated events with no name (#359)", () => {
    const sys = comment({ author_type: "system", author_name: undefined });
    expect(commentAuthorLabel(sys)).toBe("System");
    expect(commentAuthorTag(sys)).toBe("SYS");
  });

  it("labels the list's Author column with the resolved name, falling back to Unknown (#365)", () => {
    expect(ticketAuthorLabel({ author_name: "Sebastian Baltes" })).toBe(
      "Sebastian Baltes",
    );
    expect(ticketAuthorLabel({ author_name: "  " })).toBe("Unknown");
    expect(ticketAuthorLabel({ author_name: undefined })).toBe("Unknown");
    expect(ticketAuthorLabel({})).toBe("Unknown");
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
      expect(settableStates("triage", admin)).toContain("closed");
      expect(settableStates("rejected", admin)).toContain("triage");
    });

    it("gives the author close + park from triage/needs_info", () => {
      expect(settableStates("triage", author)).toEqual(["parked", "closed"]);
      expect(settableStates("needs_info", author)).toEqual(["parked", "closed"]);
    });

    it("gives the author un-park, reopen and close where they apply", () => {
      // parked → un-park (triage) or close.
      expect(settableStates("parked", author)).toEqual(["triage", "closed"]);
      // closed → reopen to triage, or straight back to work ("didn't stick").
      expect(settableStates("closed", author)).toEqual(["triage", "in_progress"]);
      // in_progress → the author may still close their own ticket.
      expect(settableStates("in_progress", author)).toEqual(["closed"]);
    });

    it("gives the author nothing from a terminal reject", () => {
      expect(settableStates("rejected", author)).toEqual([]);
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
