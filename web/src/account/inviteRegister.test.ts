import { describe, expect, it } from "vitest";
import { authedInviteNote, decideInviteRegister } from "./inviteRegister";

/**
 * Ticket A — the pure invite-prefill decision. Given the session status and the
 * fetched invite info, `decideInviteRegister` picks loading / invalid / register
 * (with prefills + an optional authed note). No fetches, no dispatch.
 */
describe("decideInviteRegister", () => {
  it("waits while the session is still resolving", () => {
    const view = decideInviteRegister({
      status: "loading",
      info: { valid: true, invitee_email: "a@b.c", invitee_name: "A" },
      authedEmail: null,
    });
    expect(view.kind).toBe("loading");
  });

  it("waits while the invite info has not arrived", () => {
    const view = decideInviteRegister({
      status: "guest",
      info: null,
      authedEmail: null,
    });
    expect(view.kind).toBe("loading");
  });

  it("shows the invalid screen for a missing / used / expired token", () => {
    const view = decideInviteRegister({
      status: "guest",
      info: { valid: false },
      authedEmail: null,
    });
    expect(view.kind).toBe("invalid");
  });

  it("prefills email + name for a guest with a valid invite (no note)", () => {
    const view = decideInviteRegister({
      status: "guest",
      info: {
        valid: true,
        invitee_email: "invitee@x.io",
        invitee_name: "Invitee",
      },
      authedEmail: null,
    });
    expect(view).toEqual({
      kind: "register",
      emailPrefill: "invitee@x.io",
      namePrefill: "Invitee",
      note: null,
    });
  });

  it("adds the new-account note when the visitor is already authed", () => {
    const view = decideInviteRegister({
      status: "authed",
      info: { valid: true, invitee_email: "new@x.io", invitee_name: "New" },
      authedEmail: "admin@local",
    });
    expect(view.kind).toBe("register");
    if (view.kind === "register") {
      expect(view.emailPrefill).toBe("new@x.io");
      expect(view.note).toBe(authedInviteNote("admin@local"));
      expect(view.note).toContain("admin@local");
      expect(view.note).toContain("neues Konto");
    }
  });

  it("tolerates a valid invite missing the optional name/email fields", () => {
    const view = decideInviteRegister({
      status: "guest",
      info: { valid: true },
      authedEmail: null,
    });
    expect(view).toEqual({
      kind: "register",
      emailPrefill: "",
      namePrefill: "",
      note: null,
    });
  });
});
