/**
 * InvitePanel — admin-only invite management (`/api/invites`).
 *
 * Rendered only when `me.role === "admin"`. Lets an admin:
 *   - create an invite (optional note) → shows the copyable
 *     `/register?invite=<token>` link,
 *   - list invites with their status (open / used-by / expired),
 *   - revoke (delete) an invite.
 *
 * The backend is the gate too (every endpoint re-checks admin), so this panel
 * is a convenience surface, not the security boundary.
 */

import { useCallback, useEffect, useState } from "react";
import { platformApi, type Invite } from "../api/platform";
import { accountErrorMessage } from "./useAccount";

interface InvitePanelProps {
  onClose(): void;
}

function formatDate(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

/** Absolute register link for an invite token, anchored at the current origin. */
function registerLink(invite: Invite): string {
  const origin = typeof window !== "undefined" ? window.location.origin : "";
  // The backend already supplies a relative register_url; make it absolute so
  // it is copy-paste shareable.
  return invite.register_url.startsWith("http")
    ? invite.register_url
    : `${origin}${invite.register_url}`;
}

export function InvitePanel({ onClose }: InvitePanelProps) {
  const [invites, setInvites] = useState<Invite[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await platformApi.listInvites();
      list.sort((a, b) => b.created_at.localeCompare(a.created_at));
      setInvites(list);
    } catch (err) {
      setError(accountErrorMessage(err, "Could not load invites"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      await platformApi.createInvite(note.trim() || undefined);
      setNote("");
      await reload();
    } catch (err) {
      setError(accountErrorMessage(err, "Could not create an invite"));
    } finally {
      setBusy(false);
    }
  };

  const revoke = async (token: string) => {
    if (!window.confirm("Revoke this invite?")) return;
    setBusy(true);
    setError(null);
    try {
      await platformApi.deleteInvite(token);
      await reload();
    } catch (err) {
      setError(accountErrorMessage(err, "Could not revoke the invite"));
    } finally {
      setBusy(false);
    }
  };

  const copy = async (invite: Invite) => {
    const link = registerLink(invite);
    try {
      await navigator.clipboard.writeText(link);
      setCopied(invite.token);
      window.setTimeout(() => setCopied(null), 2000);
    } catch {
      window.prompt("Copy this register link:", link);
    }
  };

  return (
    <div className="fd-invite-panel" data-testid="invite-panel">
      <div className="invite-bar">
        <h2 className="invite-title">Invites</h2>
        <button
          type="button"
          className="close"
          data-testid="invite-close"
          aria-label="Close"
          onClick={onClose}
        >
          &times;
        </button>
      </div>

      <div className="invite-create">
        <input
          className="input"
          type="text"
          placeholder="Note (who is this for?)"
          data-testid="invite-note"
          value={note}
          onChange={(e) => setNote(e.target.value)}
        />
        <button
          type="button"
          className="primary"
          data-testid="invite-create"
          disabled={busy}
          onClick={create}
        >
          Create invite
        </button>
      </div>

      {error && (
        <p className="error" role="alert" data-testid="invite-error">
          {error}
        </p>
      )}

      {loading ? (
        <p className="empty" data-testid="invite-loading">
          Loading…
        </p>
      ) : invites.length === 0 ? (
        <p className="empty" data-testid="invite-empty">
          No invites yet. Create one above.
        </p>
      ) : (
        <table className="invite-table" data-testid="invite-list">
          <thead>
            <tr>
              <th>Note</th>
              <th>Status</th>
              <th>Expires</th>
              <th>Link</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {invites.map((inv) => (
              <tr key={inv.token} data-testid={`invite-row-${inv.token}`}>
                <td>{inv.note || <span className="muted">—</span>}</td>
                <td>
                  <span className={`status status-${inv.status}`}>
                    {inv.status === "used" && inv.used_by ? `used` : inv.status}
                  </span>
                </td>
                <td>{formatDate(inv.expires_at)}</td>
                <td>
                  {inv.status === "open" ? (
                    <button
                      type="button"
                      className="link"
                      data-testid={`invite-copy-${inv.token}`}
                      onClick={() => copy(inv)}
                    >
                      {copied === inv.token ? "Copied!" : "Copy link"}
                    </button>
                  ) : (
                    <span className="muted">—</span>
                  )}
                </td>
                <td>
                  <button
                    type="button"
                    className="link danger"
                    data-testid={`invite-revoke-${inv.token}`}
                    disabled={busy}
                    onClick={() => revoke(inv.token)}
                  >
                    Revoke
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
