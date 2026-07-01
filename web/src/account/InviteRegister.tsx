/**
 * InviteRegister — the `?invite=<token>` self-registration screen (Ticket A).
 *
 * Rendered by App.tsx whenever the URL carries an `?invite=` token, taking
 * precedence over both the login gate and the mounted app: an invite link must
 * always land on a registration form, **even when a session is already active**
 * (e.g. admin). Registering issues a fresh session cookie, so the visitor
 * becomes the newly-created user.
 *
 * On mount it fetches the PUBLIC `GET /api/invites/{token}/info` to prefill the
 * email + name (the invitee only sets a password, and may adjust the name). An
 * invalid / used / expired token shows an "invite invalid" message instead of
 * the form. A successful register dispatches `authed` into the account session
 * (via `account.register`), flipping into the app as the new user.
 */

import { useEffect, useState } from "react";
import { platformApi, type InviteInfo } from "../api/platform";
import { AuthScreen } from "./AuthScreen";
import { decideInviteRegister } from "./inviteRegister";
import type { UseAccount } from "./useAccount";

export function InviteRegister({
  account,
  token,
}: {
  account: UseAccount;
  token: string;
}) {
  const { state, register } = account;
  const [info, setInfo] = useState<InviteInfo | null>(null);

  // Fetch the public invite info once. A failure (network / server) is treated
  // as an invalid invite so the visitor gets a clear message rather than a
  // silent hang.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const result = await platformApi.getInviteInfo(token);
        if (!cancelled) setInfo(result);
      } catch {
        if (!cancelled) setInfo({ valid: false });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [token]);

  const view = decideInviteRegister({
    status: state.status,
    info,
    authedEmail: state.user?.email ?? null,
  });

  if (view.kind === "loading") {
    return (
      <div className="fd-auth-loading" data-testid="invite-loading">
        <div className="spinner" />
      </div>
    );
  }

  if (view.kind === "invalid") {
    return (
      <div className="fd-auth fd-auth-full" data-testid="invite-invalid">
        <div className="auth-card" role="dialog" aria-label="Invite invalid">
          <div className="auth-header">
            <h2 className="auth-title">Invite invalid</h2>
          </div>
          <p className="auth-note">
            This invite link is invalid, has already been used, or has expired.
            Ask an admin for a fresh invite.
          </p>
        </div>
      </div>
    );
  }

  return (
    <AuthScreen
      fullPage
      mode="register"
      invitePrefill={token}
      resetPrefill=""
      emailPrefill={view.emailPrefill}
      namePrefill={view.namePrefill}
      note={view.note}
      // Login is irrelevant on the invite screen; register is the only path.
      onLogin={async () => {}}
      onRegister={register}
      // The invite screen has no sibling screens to switch to.
      onSwitch={() => {}}
      onClose={() => {}}
    />
  );
}
