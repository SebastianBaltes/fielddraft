/**
 * Bell — the admin-only 🔔 alerts button in the topbar (WP13).
 *
 * Renders only for admins. Shows an unseen-count badge and, when clicked,
 * toggles a small popover hosting the existing {@link AdminAlerts} panel.
 * Alerts are no longer a main tab — this is the sole entry point.
 *
 * A thin surface over the shared {@link useAccount} session; it never resolves
 * `/api/me` itself.
 */

import { useCallback, useEffect, useRef } from "react";
import { AdminAlerts } from "./AdminAlerts";
import type { UseAccount } from "./useAccount";

interface BellProps {
  account: UseAccount;
}

export function Bell({ account }: BellProps) {
  const { state, isAdmin, setBell, openTicket, setUnseenAlerts } = account;
  const rootRef = useRef<HTMLDivElement>(null);
  const open = state.bellOpen;

  const close = useCallback(() => setBell(false), [setBell]);

  useEffect(() => {
    if (!open) return;
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
  }, [open, close]);

  if (!isAdmin) return null;

  return (
    <div className="fd-bell" ref={rootRef} data-testid="bell">
      <button
        type="button"
        className="trigger"
        data-testid="bell-trigger"
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label="Alerts"
        title="Alerts"
        onClick={() => setBell(!open)}
      >
        <span className="glyph" aria-hidden="true">
          🔔
        </span>
        {state.unseenAlertCount > 0 && (
          <span className="badge" data-testid="bell-badge">
            {state.unseenAlertCount}
          </span>
        )}
      </button>
      {open && (
        <div className="bell-popover" role="dialog" data-testid="bell-popover">
          <AdminAlerts
            onClose={close}
            onOpenTicket={(ticketId) => {
              close();
              openTicket(ticketId);
            }}
            onSeenChange={(count) => setUnseenAlerts(count)}
          />
        </div>
      )}
    </div>
  );
}
