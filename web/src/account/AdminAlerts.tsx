/**
 * AdminAlerts — admin-only moderation-awareness feed (`/api/admin/alerts`).
 *
 * Rendered only when `me.role === "admin"`. Lists durable admin alerts
 * (§4.4) newest-first: a kind badge, the detail line, a link into the
 * referenced ticket (when `ticket_id` is set), and a timestamp. An "unseen
 * only" filter narrows the list, and each unseen alert can be marked seen
 * (which decrements the topbar badge via `onSeenChange`).
 *
 * The backend re-checks admin on every endpoint, so this panel is a
 * convenience surface, not the security boundary.
 */

import { useCallback, useEffect, useState } from "react";
import { platformApi, type AdminAlert, type AlertKind } from "../api/platform";
import { accountErrorMessage } from "./useAccount";

interface AdminAlertsProps {
  onClose(): void;
  /** Deep-link into a ticket (opens the tickets overlay on that ticket). */
  onOpenTicket(ticketId: string): void;
  /** Report the current unseen count so the topbar badge stays in sync. */
  onSeenChange(unseenCount: number): void;
}

/** Human label per alert kind. */
const KIND_LABEL: Record<AlertKind, string> = {
  malicious_ticket: "Malicious ticket",
  malicious_comment: "Malicious comment",
  withheld_comment: "Withheld comment",
  user_banned: "User banned",
};

function formatDateTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function AdminAlerts({
  onClose,
  onOpenTicket,
  onSeenChange,
}: AdminAlertsProps) {
  const [alerts, setAlerts] = useState<AdminAlert[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [unseenOnly, setUnseenOnly] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await platformApi.listAdminAlerts(unseenOnly);
      // The backend already returns newest-first; sort defensively anyway.
      list.sort((a, b) => b.created_at.localeCompare(a.created_at));
      setAlerts(list);
    } catch (err) {
      setError(accountErrorMessage(err, "Could not load alerts"));
    } finally {
      setLoading(false);
    }
  }, [unseenOnly]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const markSeen = async (alert: AdminAlert) => {
    setBusyId(alert.id);
    setError(null);
    try {
      await platformApi.markAdminAlertSeen(alert.id);
      // Refresh the authoritative unseen count for the topbar badge.
      const unseen = await platformApi.listAdminAlerts(true);
      onSeenChange(unseen.length);
      await reload();
    } catch (err) {
      setError(accountErrorMessage(err, "Could not mark the alert seen"));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="fd-admin-alerts" data-testid="admin-alerts">
      <div className="admin-bar">
        <h2 className="admin-title">Alerts</h2>
        <label className="filter" data-testid="admin-alerts-unseen-toggle">
          <input
            type="checkbox"
            checked={unseenOnly}
            onChange={(e) => setUnseenOnly(e.target.checked)}
          />
          Unseen only
        </label>
        <button
          type="button"
          className="close"
          data-testid="admin-alerts-close"
          aria-label="Close"
          onClick={onClose}
        >
          &times;
        </button>
      </div>

      {error && (
        <p className="error" role="alert" data-testid="admin-alerts-error">
          {error}
        </p>
      )}

      {loading ? (
        <p className="empty" data-testid="admin-alerts-loading">
          Loading…
        </p>
      ) : alerts.length === 0 ? (
        <p className="empty" data-testid="admin-alerts-empty">
          {unseenOnly ? "No unseen alerts." : "No alerts yet."}
        </p>
      ) : (
        <ul className="alert-list" data-testid="admin-alerts-list">
          {alerts.map((a) => (
            <li
              key={a.id}
              className={"alert-row" + (a.seen ? " is-seen" : " is-unseen")}
              data-testid={`admin-alert-${a.id}`}
            >
              <span className={`kind kind-${a.kind}`}>
                {KIND_LABEL[a.kind]}
              </span>
              <div className="alert-body">
                <span className="detail">{a.detail}</span>
                <span className="meta">
                  {formatDateTime(a.created_at)}
                  {a.ticket_id && (
                    <>
                      {" · "}
                      <button
                        type="button"
                        className="link"
                        data-testid={`admin-alert-ticket-${a.id}`}
                        onClick={() => onOpenTicket(a.ticket_id!)}
                      >
                        View ticket
                      </button>
                    </>
                  )}
                </span>
              </div>
              {a.seen ? (
                <span
                  className="seen-tag"
                  data-testid={`admin-alert-seen-${a.id}`}
                >
                  Seen
                </span>
              ) : (
                <button
                  type="button"
                  className="link"
                  data-testid={`admin-alert-mark-${a.id}`}
                  disabled={busyId !== null}
                  onClick={() => markSeen(a)}
                >
                  Mark seen
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
