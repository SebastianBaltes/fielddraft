/**
 * UserMenu — the 👤 user menu in the topbar (WP13).
 *
 * Replaces the old "A Admin" name button. Renders:
 *   - guest:  a "Log in" button that opens the login screen;
 *   - authed: a black person glyph (👤, no name) whose dropdown holds a small
 *             identity line (email + role) and **Log out**.
 *
 * The Projects / Issues / Invites / Users entries are NO LONGER here — they are
 * the Overview's main-level tabs. Report Issue lives in the settings burger
 * (with the F2 shortcut shown, ticket b5a60122), not here.
 *
 * A thin surface over the shared {@link useAccount} session; it never resolves
 * `/api/me` itself.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import type { UseAccount } from "./useAccount";

interface UserMenuProps {
  account: UseAccount;
  /** Detach the live workspace from its server project on logout. */
  onLoggedOut?: () => void;
}

export function UserMenu({ account, onLoggedOut }: UserMenuProps) {
  const { state, isAdmin, openAuth, logout } = account;
  const [menuOpen, setMenuOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  const close = useCallback(() => setMenuOpen(false), []);

  useEffect(() => {
    if (!menuOpen) return;
    const onClick = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        close();
      }
    };
    const t = window.setTimeout(
      () => window.addEventListener("click", onClick),
      0,
    );
    return () => {
      window.clearTimeout(t);
      window.removeEventListener("click", onClick);
    };
  }, [menuOpen, close]);

  // While the session is still resolving, render a neutral placeholder so the
  // button doesn't flash "Log in" → glyph.
  if (state.status === "loading") {
    return (
      <div className="fd-user-menu" data-testid="user-menu">
        <button type="button" className="trigger" disabled aria-label="Account">
          …
        </button>
      </div>
    );
  }

  if (state.status === "guest") {
    return (
      <div className="fd-user-menu" data-testid="user-menu">
        <button
          type="button"
          className="trigger login"
          data-testid="user-login"
          onClick={() => openAuth("login")}
        >
          Log in
        </button>
      </div>
    );
  }

  return (
    <div className="fd-user-menu" ref={rootRef} data-testid="user-menu">
      <button
        type="button"
        className="trigger person"
        data-testid="user-menu-trigger"
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        aria-label="Account menu"
        title={state.user?.email ?? "Account"}
        onClick={() => setMenuOpen((v) => !v)}
      >
        <span className="glyph" aria-hidden="true">
          👤
        </span>
      </button>
      {menuOpen && (
        <div className="user-popover" role="menu" data-testid="user-popover">
          <div className="identity" data-testid="user-identity">
            <span className="email">{state.user?.email}</span>
            <span className="role">{isAdmin ? "admin" : "user"}</span>
          </div>
          <div className="menu-separator" role="separator" />
          {/* Ticket b5a60122: "Report Issue…" lives in the settings burger
              (with F2), not here — the user menu is identity + logout. */}
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="user-logout"
            onClick={() => {
              void logout().then(() => onLoggedOut?.());
              close();
            }}
          >
            Log out
          </button>
        </div>
      )}
    </div>
  );
}
