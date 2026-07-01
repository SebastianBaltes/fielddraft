/**
 * AuthGate — the WP11 full-page login gate.
 *
 * Rendered by App.tsx ONLY when the build flag VITE_REQUIRE_LOGIN is on AND the
 * boot `/api/me` resolved to no session. It owns its own auth-screen mode
 * (login / register / reset-request / reset-confirm) and reuses the existing
 * {@link AuthScreen} in its full-page variant, so the four auth flows and their
 * styling are not duplicated.
 *
 * It honours the `?invite=` / `?reset=` deep-links: the initial mode is seeded
 * from the session reducer's URL-derived overlay (a reset link opens
 * reset-confirm, an invite link opens register), with the matching prefills
 * passed straight through.
 *
 * A successful login/register dispatches `authed` into the session, which flips
 * the app out of the gate and into the modeling workspace (App.tsx).
 */

import { useState } from "react";
import { AuthScreen } from "./AuthScreen";
import type { AuthScreenMode } from "./accountState";
import type { UseAccount } from "./useAccount";

type AuthMode = Exclude<AuthScreenMode, null>;

/** Seed the gate's starting screen from the URL-derived auth mode (deep-links),
 *  defaulting to login. */
function initialMode(auth: AuthScreenMode): AuthMode {
  if (
    auth === "register" ||
    auth === "reset-request" ||
    auth === "reset-confirm"
  ) {
    return auth;
  }
  return "login";
}

export function AuthGate({ account }: { account: UseAccount }) {
  const { state, login, register } = account;
  const [mode, setMode] = useState<AuthMode>(() => initialMode(state.auth));

  return (
    <AuthScreen
      fullPage
      mode={mode}
      invitePrefill={state.invitePrefill}
      resetPrefill={state.resetPrefill}
      onLogin={login}
      onRegister={register}
      // Inside the gate the auth screens switch among themselves locally; the
      // overlay reducer is irrelevant here. Ignore any non-auth target.
      onSwitch={(next) => {
        if (
          next === "login" ||
          next === "register" ||
          next === "reset-request" ||
          next === "reset-confirm"
        ) {
          setMode(next);
        }
      }}
      // No-op: the full-page variant renders no close control.
      onClose={() => {}}
    />
  );
}
