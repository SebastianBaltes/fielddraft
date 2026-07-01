/**
 * InvitePanel — admin-only invite management (`/api/invites`), WP13 Invites tab.
 *
 * Rendered only when `me.role === "admin"`. Lets an admin:
 *   - create an invite with **Invitee name + Invitee email + Invitation text**
 *     → shows the shareable `/register?invite=<token>` link,
 *   - list invites with their addressee + status (open / used / expired),
 *   - revoke (delete) an invite.
 *
 * The backend is the gate too (every endpoint re-checks admin), so this panel
 * is a convenience surface, not the security boundary. It is a persistent
 * Overview tab — there is NO close (×) button.
 */

import { useCallback, useEffect, useState } from "react";
import { platformApi, type Invite } from "../api/platform";
import { accountErrorMessage } from "./useAccount";

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
  return invite.register_url.startsWith("http")
    ? invite.register_url
    : `${origin}${invite.register_url}`;
}

export function InvitePanel() {
  const [invites, setInvites] = useState<Invite[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const [lastCreated, setLastCreated] = useState<Invite | null>(null);

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

  const canCreate = name.trim().length > 0 && email.trim().length > 0 && !busy;

  const create = async () => {
    if (!canCreate) return;
    setBusy(true);
    setError(null);
    try {
      const created = await platformApi.createInvite({
        invitee_name: name.trim(),
        invitee_email: email.trim(),
        message: message.trim(),
      });
      setLastCreated(created);
      setName("");
      setEmail("");
      setMessage("");
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
      if (lastCreated?.token === token) setLastCreated(null);
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
      <div className="invite-create" data-testid="invite-form">
        <label className="field">
          <span className="label">Invitee name</span>
          <input
            className="input"
            type="text"
            placeholder="Ada Lovelace"
            data-testid="invite-name"
            value={name}
            disabled={busy}
            onChange={(e) => setName(e.target.value)}
          />
        </label>
        <label className="field">
          <span className="label">Invitee email</span>
          <input
            className="input"
            type="email"
            placeholder="ada@example.com"
            data-testid="invite-email"
            value={email}
            disabled={busy}
            onChange={(e) => setEmail(e.target.value)}
          />
        </label>
        <label className="field">
          <span className="label">Invitation text</span>
          <textarea
            className="textarea"
            rows={3}
            placeholder="A personal note the recipient will see…"
            data-testid="invite-message"
            value={message}
            disabled={busy}
            onChange={(e) => setMessage(e.target.value)}
          />
        </label>
        <div className="create-actions">
          <button
            type="button"
            className="primary"
            data-testid="invite-create"
            disabled={!canCreate}
            onClick={create}
          >
            {busy ? "Creating…" : "Create invite"}
          </button>
        </div>
      </div>

      {lastCreated && (
        <div className="invite-created" data-testid="invite-created">
          <span className="created-label">
            Share this link with {lastCreated.invitee_name}:
          </span>
          <code className="created-link" data-testid="invite-created-link">
            {registerLink(lastCreated)}
          </code>
          <button
            type="button"
            className="link"
            data-testid="invite-created-copy"
            onClick={() => copy(lastCreated)}
          >
            {copied === lastCreated.token ? "Copied!" : "Copy link"}
          </button>
        </div>
      )}

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
              <th>Invitee</th>
              <th>Email</th>
              <th>Status</th>
              <th>Expires</th>
              <th>Link</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {invites.map((inv) => (
              <tr key={inv.token} data-testid={`invite-row-${inv.token}`}>
                <td>{inv.invitee_name || <span className="muted">—</span>}</td>
                <td>{inv.invitee_email || <span className="muted">—</span>}</td>
                <td>
                  <span className={`status status-${inv.status}`}>
                    {inv.status === "used" && inv.used_by ? "used" : inv.status}
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
