import { describe, expect, it } from "vitest";
import type { TicketComment, TicketState } from "../api/platform";
import {
  categoryLabel,
  commentVisible,
  personaLabel,
  personaTag,
  stateBucket,
  stateLabel,
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
      "rejected_spam",
      "rejected_malicious",
    ];
    for (const s of states) {
      expect(stateLabel(s)).toBeTruthy();
    }
  });

  it("buckets states into open / progress / done / rejected", () => {
    expect(stateBucket("triage")).toBe("open");
    expect(stateBucket("needs_info")).toBe("open");
    expect(stateBucket("in_progress")).toBe("progress");
    expect(stateBucket("deploying")).toBe("progress");
    expect(stateBucket("resolved")).toBe("done");
    expect(stateBucket("closed")).toBe("done");
    expect(stateBucket("rejected_spam")).toBe("rejected");
    expect(stateBucket("rejected_malicious")).toBe("rejected");
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
