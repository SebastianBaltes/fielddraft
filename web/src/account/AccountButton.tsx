/**
 * AccountButton — the account entry point in the topbar.
 *
 * Guest:  a "Log in" button that opens the login screen.
 * Authed: the user's display name with a small dropdown — Dashboard, Public
 *         gallery, Invites (admin only) and Log out.
 *
 * It is a thin surface over the shared {@link useAccount} session; it never
 * resolves `/api/me` itself. Placed alongside the version + settings menu so it
 * does not redesign the shell.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import type { UseAccount } from "./useAccount";

interface AccountButtonProps {
  account: UseAccount;
  /** Detach the live workspace from its server project on logout (so the next
   *  edit doesn't try to sync into a session that no longer exists). */
  onLoggedOut?: () => void;
}

export function AccountButton({ account, onLoggedOut }: AccountButtonProps) {
  const { state, isAdmin, open, logout } = account;
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
  // button doesn't flash "Log in" → name.
  if (state.status === "loading") {
    return (
      <div className="fd-account-button" data-testid="account-button">
        <button type="button" className="trigger" disabled>
          …
        </button>
      </div>
    );
  }

  if (state.status === "guest") {
    return (
      <div className="fd-account-button" data-testid="account-button">
        <button
          type="button"
          className="trigger login"
          data-testid="account-login"
          onClick={() => open("login")}
        >
          Log in
        </button>
      </div>
    );
  }

  const name = state.user?.display_name || state.user?.email || "Account";

  return (
    <div
      className="fd-account-button"
      ref={rootRef}
      data-testid="account-button"
    >
      <button
        type="button"
        className="trigger account"
        data-testid="account-menu-trigger"
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        onClick={() => setMenuOpen((v) => !v)}
      >
        <span className="avatar" aria-hidden="true">
          {name.slice(0, 1).toUpperCase()}
        </span>
        <span className="who">{name}</span>
      </button>
      {menuOpen && (
        <div
          className="account-popover"
          role="menu"
          data-testid="account-popover"
        >
          {isAdmin && <span className="role-tag">admin</span>}
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="account-dashboard"
            onClick={() => {
              open("dashboard");
              close();
            }}
          >
            My projects
          </button>
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="account-gallery"
            onClick={() => {
              open("gallery");
              close();
            }}
          >
            Public gallery
          </button>
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="account-tickets"
            onClick={() => {
              open("tickets");
              close();
            }}
          >
            Issues
          </button>
          {isAdmin && (
            <>
              <div className="menu-separator" role="separator" />
              <button
                type="button"
                role="menuitem"
                className="menu-item"
                data-testid="account-invites"
                onClick={() => {
                  open("invites");
                  close();
                }}
              >
                Invites
              </button>
              <button
                type="button"
                role="menuitem"
                className="menu-item"
                data-testid="account-alerts"
                onClick={() => {
                  open("admin-alerts");
                  close();
                }}
              >
                Alerts
                {state.unseenAlertCount > 0 && (
                  <span className="badge" data-testid="account-alerts-badge">
                    {state.unseenAlertCount}
                  </span>
                )}
              </button>
              <button
                type="button"
                role="menuitem"
                className="menu-item"
                data-testid="account-users"
                onClick={() => {
                  open("admin-users");
                  close();
                }}
              >
                Users
              </button>
            </>
          )}
          <div className="menu-separator" role="separator" />
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="account-logout"
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
