import { describe, expect, it } from "vitest";
import {
  accountReducer,
  initialAccountState,
  isAdmin,
  type AccountState,
} from "./accountState";
import type { User } from "../api/platform";

const adminUser: User = {
  id: "u1",
  email: "admin@x.io",
  display_name: "Admin",
  role: "admin",
  created_at: "2026-01-01T00:00:00Z",
};

const plainUser: User = { ...adminUser, id: "u2", role: "user" };

describe("initialAccountState", () => {
  it("starts loading, guest, no overlay with no deep-link", () => {
    const s = initialAccountState("");
    expect(s.status).toBe("loading");
    expect(s.user).toBeNull();
    expect(s.overlay).toBeNull();
    expect(s.invitePrefill).toBe("");
  });

  it("auto-opens register and prefills the invite from ?invite=", () => {
    const s = initialAccountState("?invite=ABC123");
    expect(s.overlay).toBe("register");
    expect(s.invitePrefill).toBe("ABC123");
  });

  it("auto-opens reset-confirm and prefills from ?reset=, winning over invite", () => {
    const s = initialAccountState("?invite=ABC&reset=R9");
    expect(s.overlay).toBe("reset-confirm");
    expect(s.resetPrefill).toBe("R9");
  });
});

describe("accountReducer", () => {
  const base = initialAccountState("");

  it("session-resolved → guest when no user", () => {
    const s = accountReducer(base, { type: "session-resolved", user: null });
    expect(s.status).toBe("guest");
    expect(s.user).toBeNull();
  });

  it("session-resolved → authed with the user", () => {
    const s = accountReducer(base, {
      type: "session-resolved",
      user: plainUser,
    });
    expect(s.status).toBe("authed");
    expect(s.user).toEqual(plainUser);
  });

  it("session-resolved bounces a logged-in user off the login screen to the dashboard", () => {
    const onLogin: AccountState = { ...base, overlay: "login" };
    const s = accountReducer(onLogin, {
      type: "session-resolved",
      user: plainUser,
    });
    expect(s.overlay).toBe("dashboard");
  });

  it("session-resolved keeps a deep-link register overlay for a guest", () => {
    const withInvite = initialAccountState("?invite=Z");
    const s = accountReducer(withInvite, {
      type: "session-resolved",
      user: null,
    });
    expect(s.overlay).toBe("register");
  });

  it("authed opens the dashboard and burns the prefills", () => {
    const withInvite = initialAccountState("?invite=Z");
    const s = accountReducer(withInvite, { type: "authed", user: plainUser });
    expect(s.status).toBe("authed");
    expect(s.overlay).toBe("dashboard");
    expect(s.invitePrefill).toBe("");
  });

  it("logged-out returns to guest with no overlay", () => {
    const authed = accountReducer(base, { type: "authed", user: plainUser });
    const s = accountReducer(authed, { type: "logged-out" });
    expect(s.status).toBe("guest");
    expect(s.user).toBeNull();
    expect(s.overlay).toBeNull();
  });

  it("open / close toggle the overlay", () => {
    const opened = accountReducer(base, { type: "open", overlay: "gallery" });
    expect(opened.overlay).toBe("gallery");
    const closed = accountReducer(opened, { type: "close" });
    expect(closed.overlay).toBeNull();
  });

  it("open-ticket opens the tickets overlay deep-linked to an id", () => {
    const s = accountReducer(base, { type: "open-ticket", ticketId: "t9" });
    expect(s.overlay).toBe("tickets");
    expect(s.ticketPrefill).toBe("t9");
  });

  it("open-ticket with null opens the list (no deep-link)", () => {
    const s = accountReducer(base, { type: "open-ticket", ticketId: null });
    expect(s.overlay).toBe("tickets");
    expect(s.ticketPrefill).toBeNull();
  });

  it("opening a non-tickets overlay clears a stale ticket deep-link", () => {
    const deep = accountReducer(base, { type: "open-ticket", ticketId: "t9" });
    const s = accountReducer(deep, { type: "open", overlay: "dashboard" });
    expect(s.ticketPrefill).toBeNull();
  });

  it("close clears the ticket deep-link", () => {
    const deep = accountReducer(base, { type: "open-ticket", ticketId: "t9" });
    const s = accountReducer(deep, { type: "close" });
    expect(s.overlay).toBeNull();
    expect(s.ticketPrefill).toBeNull();
  });

  it("opens the admin overlays", () => {
    const alerts = accountReducer(base, {
      type: "open",
      overlay: "admin-alerts",
    });
    expect(alerts.overlay).toBe("admin-alerts");
    const users = accountReducer(base, {
      type: "open",
      overlay: "admin-users",
    });
    expect(users.overlay).toBe("admin-users");
  });

  it("unseen-alerts sets the badge count (clamped at 0)", () => {
    expect(s0().unseenAlertCount).toBe(0);
    const s = accountReducer(base, { type: "unseen-alerts", count: 3 });
    expect(s.unseenAlertCount).toBe(3);
    const cleared = accountReducer(s, { type: "unseen-alerts", count: -5 });
    expect(cleared.unseenAlertCount).toBe(0);
  });

  it("logged-out resets the unseen-alert badge", () => {
    const withBadge = accountReducer(base, {
      type: "unseen-alerts",
      count: 4,
    });
    const out = accountReducer(withBadge, { type: "logged-out" });
    expect(out.unseenAlertCount).toBe(0);
  });

  function s0() {
    return initialAccountState("");
  }
});

describe("isAdmin", () => {
  it("is true only for the admin role", () => {
    const authedAdmin = accountReducer(initialAccountState(""), {
      type: "authed",
      user: adminUser,
    });
    const authedUser = accountReducer(initialAccountState(""), {
      type: "authed",
      user: plainUser,
    });
    expect(isAdmin(authedAdmin)).toBe(true);
    expect(isAdmin(authedUser)).toBe(false);
    expect(isAdmin(initialAccountState(""))).toBe(false);
  });
});
