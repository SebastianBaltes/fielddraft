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
  it("starts loading in the modeling view (guest default) with no deep-link", () => {
    const s = initialAccountState("");
    expect(s.status).toBe("loading");
    expect(s.user).toBeNull();
    expect(s.page).toBe("project");
    expect(s.overviewTab).toBe("projects");
    expect(s.auth).toBeNull();
    expect(s.invitePrefill).toBe("");
  });

  it("auto-opens the register auth screen and prefills the invite from ?invite=", () => {
    const s = initialAccountState("?invite=ABC123");
    expect(s.auth).toBe("register");
    expect(s.invitePrefill).toBe("ABC123");
  });

  it("auto-opens reset-confirm and prefills from ?reset=, winning over invite", () => {
    const s = initialAccountState("?invite=ABC&reset=R9");
    expect(s.auth).toBe("reset-confirm");
    expect(s.resetPrefill).toBe("R9");
  });
});

describe("accountReducer", () => {
  const base = initialAccountState("");

  it("session-resolved → guest keeps the modeling view", () => {
    const s = accountReducer(base, { type: "session-resolved", user: null });
    expect(s.status).toBe("guest");
    expect(s.user).toBeNull();
    expect(s.page).toBe("project");
  });

  it("session-resolved → authed lands on the Overview", () => {
    const s = accountReducer(base, {
      type: "session-resolved",
      user: plainUser,
    });
    expect(s.status).toBe("authed");
    expect(s.user).toEqual(plainUser);
    expect(s.page).toBe("overview");
  });

  it("session-resolved dismisses the login auth screen for a logged-in user", () => {
    const onLogin: AccountState = { ...base, auth: "login" };
    const s = accountReducer(onLogin, {
      type: "session-resolved",
      user: plainUser,
    });
    expect(s.auth).toBeNull();
  });

  it("session-resolved keeps a deep-link register auth screen for a guest", () => {
    const withInvite = initialAccountState("?invite=Z");
    const s = accountReducer(withInvite, {
      type: "session-resolved",
      user: null,
    });
    expect(s.auth).toBe("register");
  });

  it("authed lands on the overview/projects tab and burns the prefills", () => {
    const withInvite = initialAccountState("?invite=Z");
    const s = accountReducer(withInvite, { type: "authed", user: plainUser });
    expect(s.status).toBe("authed");
    expect(s.page).toBe("overview");
    expect(s.overviewTab).toBe("projects");
    expect(s.auth).toBeNull();
    expect(s.invitePrefill).toBe("");
  });

  it("logged-out returns to guest on the overview", () => {
    const authed = accountReducer(base, { type: "authed", user: plainUser });
    const s = accountReducer(authed, { type: "logged-out" });
    expect(s.status).toBe("guest");
    expect(s.user).toBeNull();
    expect(s.page).toBe("overview");
    expect(s.overviewTab).toBe("projects");
  });

  it("enter-project switches to the modeling view", () => {
    const s = accountReducer(base, { type: "enter-project" });
    expect(s.page).toBe("project");
  });

  it("show-overview returns to the overview, optionally selecting a tab", () => {
    const inProject = accountReducer(base, { type: "enter-project" });
    const s = accountReducer(inProject, {
      type: "show-overview",
      tab: "issues",
    });
    expect(s.page).toBe("overview");
    expect(s.overviewTab).toBe("issues");
  });

  it("select-tab switches the overview tab (and forces the overview page)", () => {
    const inProject = accountReducer(base, { type: "enter-project" });
    const s = accountReducer(inProject, { type: "select-tab", tab: "invites" });
    expect(s.page).toBe("overview");
    expect(s.overviewTab).toBe("invites");
  });

  it("open-ticket opens the Issues tab deep-linked to an id", () => {
    const s = accountReducer(base, { type: "open-ticket", ticketId: "t9" });
    expect(s.page).toBe("overview");
    expect(s.overviewTab).toBe("issues");
    expect(s.ticketPrefill).toBe("t9");
  });

  it("open-ticket with null opens the Issues list (no deep-link)", () => {
    const s = accountReducer(base, { type: "open-ticket", ticketId: null });
    expect(s.overviewTab).toBe("issues");
    expect(s.ticketPrefill).toBeNull();
  });

  it("selecting a non-issues tab clears a stale ticket deep-link", () => {
    const deep = accountReducer(base, { type: "open-ticket", ticketId: "t9" });
    const s = accountReducer(deep, { type: "select-tab", tab: "projects" });
    expect(s.ticketPrefill).toBeNull();
  });

  it("entering a project clears the ticket deep-link", () => {
    const deep = accountReducer(base, { type: "open-ticket", ticketId: "t9" });
    const s = accountReducer(deep, { type: "enter-project" });
    expect(s.ticketPrefill).toBeNull();
  });

  it("open-auth / close-auth toggle the standalone auth screen", () => {
    const opened = accountReducer(base, { type: "open-auth", mode: "login" });
    expect(opened.auth).toBe("login");
    const closed = accountReducer(opened, { type: "close-auth" });
    expect(closed.auth).toBeNull();
  });

  it("set-bell toggles the admin alerts panel", () => {
    const open = accountReducer(base, { type: "set-bell", open: true });
    expect(open.bellOpen).toBe(true);
    const closed = accountReducer(open, { type: "set-bell", open: false });
    expect(closed.bellOpen).toBe(false);
  });

  it("unseen-alerts sets the badge count (clamped at 0)", () => {
    expect(initialAccountState("").unseenAlertCount).toBe(0);
    const s = accountReducer(base, { type: "unseen-alerts", count: 3 });
    expect(s.unseenAlertCount).toBe(3);
    const cleared = accountReducer(s, { type: "unseen-alerts", count: -5 });
    expect(cleared.unseenAlertCount).toBe(0);
  });

  it("logged-out resets the unseen-alert badge and closes the bell", () => {
    const withBadge = accountReducer(base, {
      type: "unseen-alerts",
      count: 4,
    });
    const bellOpen = accountReducer(withBadge, {
      type: "set-bell",
      open: true,
    });
    const out = accountReducer(bellOpen, { type: "logged-out" });
    expect(out.unseenAlertCount).toBe(0);
    expect(out.bellOpen).toBe(false);
  });
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
