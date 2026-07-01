/**
 * AdminUsers — admin-only user management (`/api/admin/users`), WP13 Users tab.
 *
 * Rendered only when `me.role === "admin"`. Lists every account (email, name,
 * role, active/banned status) and lets an admin ban (disable) or unban a user.
 * Banning is confirmed first and disabled on the admin's own row (the server
 * also refuses self-bans / the last admin, so this is convenience, not the
 * gate). A banned user shows a "Banned" badge and an "Unban" action. It is a
 * persistent Overview tab — there is NO close (×) button.
 */

import { useCallback, useEffect, useState } from "react";
import { platformApi, type AdminUser } from "../api/platform";
import { accountErrorMessage } from "./useAccount";

interface AdminUsersProps {
  /** The current admin's own id — its ban button is disabled. */
  selfId: string | null;
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

export function AdminUsers({ selfId }: AdminUsersProps) {
  const [users, setUsers] = useState<AdminUser[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await platformApi.listAdminUsers();
      list.sort((a, b) => a.email.localeCompare(b.email));
      setUsers(list);
    } catch (err) {
      setError(accountErrorMessage(err, "Could not load users"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  /** Optimistically swap a single user's `disabled` flag from the response. */
  const applyUpdate = useCallback((updated: AdminUser) => {
    setUsers((prev) => prev.map((u) => (u.id === updated.id ? updated : u)));
  }, []);

  const ban = (u: AdminUser) =>
    void (async () => {
      if (
        !window.confirm(
          `Ban ${u.display_name || u.email}? They will be logged out and ` +
            `cannot sign in until unbanned.`,
        )
      ) {
        return;
      }
      setBusyId(u.id);
      setError(null);
      try {
        applyUpdate(await platformApi.banUser(u.id));
      } catch (err) {
        setError(accountErrorMessage(err, "Could not ban the user"));
      } finally {
        setBusyId(null);
      }
    })();

  const unban = (u: AdminUser) =>
    void (async () => {
      setBusyId(u.id);
      setError(null);
      try {
        applyUpdate(await platformApi.unbanUser(u.id));
      } catch (err) {
        setError(accountErrorMessage(err, "Could not unban the user"));
      } finally {
        setBusyId(null);
      }
    })();

  return (
    <div className="fd-admin-users" data-testid="admin-users">
      {error && (
        <p className="error" role="alert" data-testid="admin-users-error">
          {error}
        </p>
      )}

      {loading ? (
        <p className="empty" data-testid="admin-users-loading">
          Loading…
        </p>
      ) : users.length === 0 ? (
        <p className="empty" data-testid="admin-users-empty">
          No users.
        </p>
      ) : (
        <table className="users-table" data-testid="admin-users-list">
          <thead>
            <tr>
              <th>Email</th>
              <th>Name</th>
              <th>Role</th>
              <th>Status</th>
              <th>Joined</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {users.map((u) => (
              <tr key={u.id} data-testid={`admin-user-${u.id}`}>
                <td>{u.email}</td>
                <td>{u.display_name || <span className="muted">—</span>}</td>
                <td>
                  <span className={`role role-${u.role}`}>{u.role}</span>
                </td>
                <td>
                  <span
                    className={`status ${u.disabled ? "is-banned" : "is-active"}`}
                    data-testid={`admin-user-status-${u.id}`}
                  >
                    {u.disabled ? "Banned" : "Active"}
                  </span>
                </td>
                <td>{formatDate(u.created_at)}</td>
                <td>
                  {u.disabled ? (
                    <button
                      type="button"
                      className="link"
                      data-testid={`admin-user-unban-${u.id}`}
                      disabled={busyId !== null}
                      onClick={() => unban(u)}
                    >
                      Unban
                    </button>
                  ) : (
                    <button
                      type="button"
                      className="link danger"
                      data-testid={`admin-user-ban-${u.id}`}
                      disabled={busyId !== null || u.id === selfId}
                      title={
                        u.id === selfId ? "You cannot ban yourself" : undefined
                      }
                      onClick={() => ban(u)}
                    >
                      Ban
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
