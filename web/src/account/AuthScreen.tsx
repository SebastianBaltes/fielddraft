/**
 * AuthScreen — login, register and password-reset forms.
 *
 * A single component switching on the account overlay so the four auth flows
 * share one card chrome and one error/pending surface:
 *   - "login"          → email + password
 *   - "register"       → email + display name + password + invite (prefilled
 *                        from the `?invite=` deep-link)
 *   - "reset-request"  → email (always reports success — no account enumeration)
 *   - "reset-confirm"  → reset token (prefilled from `?reset=`) + new password
 *
 * Register is invite-gated by the backend; the invite field is required here so
 * a missing token is caught before the round-trip.
 */

import { useState } from "react";
import { platformApi } from "../api/platform";
import { useAsyncSubmit } from "./useAccount";
import type { AccountOverlay } from "./accountState";

interface AuthScreenProps {
  mode: Extract<
    AccountOverlay,
    "login" | "register" | "reset-request" | "reset-confirm"
  >;
  invitePrefill: string;
  resetPrefill: string;
  onLogin(email: string, password: string): Promise<void>;
  onRegister(input: {
    email: string;
    displayName: string;
    password: string;
    invite: string;
  }): Promise<void>;
  /** Switch to another auth screen (e.g. "no account? register"). */
  onSwitch(mode: AccountOverlay): void;
  onClose(): void;
}

const TITLES: Record<AuthScreenProps["mode"], string> = {
  login: "Log in",
  register: "Create your account",
  "reset-request": "Reset your password",
  "reset-confirm": "Set a new password",
};

export function AuthScreen({
  mode,
  invitePrefill,
  resetPrefill,
  onLogin,
  onRegister,
  onSwitch,
  onClose,
}: AuthScreenProps) {
  const [email, setEmail] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [password, setPassword] = useState("");
  const [invite, setInvite] = useState(invitePrefill);
  const [resetToken, setResetToken] = useState(resetPrefill);
  const [done, setDone] = useState<string | null>(null);
  const { pending, error, run, setError } = useAsyncSubmit();

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    setDone(null);
    if (mode === "login") {
      void run(() => onLogin(email.trim(), password), "Login failed");
    } else if (mode === "register") {
      if (!invite.trim()) {
        setError("An invite is required to register.");
        return;
      }
      void run(
        () =>
          onRegister({
            email: email.trim(),
            displayName: displayName.trim(),
            password,
            invite: invite.trim(),
          }),
        "Registration failed",
      );
    } else if (mode === "reset-request") {
      void run(async () => {
        await platformApi.requestPasswordReset(email.trim());
        setDone(
          "If that email has an account, a reset link is on its way. Check your inbox.",
        );
      }, "Could not request a reset");
    } else {
      void run(async () => {
        await platformApi.resetPassword(resetToken.trim(), password);
        setDone("Password updated. You can log in with your new password now.");
      }, "Could not reset the password");
    }
  };

  return (
    <div className="fd-auth" data-testid="auth-screen" data-mode={mode}>
      <div className="auth-card" role="dialog" aria-label={TITLES[mode]}>
        <div className="auth-header">
          <h2 className="auth-title">{TITLES[mode]}</h2>
          <button
            type="button"
            className="auth-close"
            data-testid="auth-close"
            aria-label="Close"
            onClick={onClose}
          >
            &times;
          </button>
        </div>

        <form className="auth-form" onSubmit={submit}>
          {(mode === "login" ||
            mode === "register" ||
            mode === "reset-request") && (
            <label className="field">
              <span className="label">Email</span>
              <input
                className="input"
                type="email"
                autoComplete="email"
                required
                data-testid="auth-email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
              />
            </label>
          )}

          {mode === "register" && (
            <label className="field">
              <span className="label">Display name</span>
              <input
                className="input"
                type="text"
                autoComplete="name"
                data-testid="auth-display-name"
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
              />
            </label>
          )}

          {mode === "reset-confirm" && (
            <label className="field">
              <span className="label">Reset token</span>
              <input
                className="input"
                type="text"
                required
                data-testid="auth-reset-token"
                value={resetToken}
                onChange={(e) => setResetToken(e.target.value)}
              />
            </label>
          )}

          {(mode === "login" ||
            mode === "register" ||
            mode === "reset-confirm") && (
            <label className="field">
              <span className="label">
                {mode === "reset-confirm" ? "New password" : "Password"}
              </span>
              <input
                className="input"
                type="password"
                autoComplete={
                  mode === "login" ? "current-password" : "new-password"
                }
                required
                minLength={mode === "login" ? undefined : 8}
                data-testid="auth-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </label>
          )}

          {mode === "register" && (
            <label className="field">
              <span className="label">Invite token</span>
              <input
                className="input"
                type="text"
                required
                data-testid="auth-invite"
                placeholder="Paste your invite token"
                value={invite}
                onChange={(e) => setInvite(e.target.value)}
              />
              <span className="hint">
                Registration is invite-only. Use the link an admin shared with
                you.
              </span>
            </label>
          )}

          {error && (
            <p className="error" role="alert" data-testid="auth-error">
              {error}
            </p>
          )}
          {done && (
            <p className="success" data-testid="auth-done">
              {done}
            </p>
          )}

          <button
            type="submit"
            className="primary"
            data-testid="auth-submit"
            disabled={pending}
          >
            {pending ? "Working…" : TITLES[mode]}
          </button>
        </form>

        <div className="auth-switch">
          {mode === "login" && (
            <>
              <button
                type="button"
                className="link"
                data-testid="auth-go-register"
                onClick={() => onSwitch("register")}
              >
                Have an invite? Create an account
              </button>
              <button
                type="button"
                className="link"
                data-testid="auth-go-reset"
                onClick={() => onSwitch("reset-request")}
              >
                Forgot your password?
              </button>
            </>
          )}
          {mode === "register" && (
            <button
              type="button"
              className="link"
              data-testid="auth-go-login"
              onClick={() => onSwitch("login")}
            >
              Already have an account? Log in
            </button>
          )}
          {(mode === "reset-request" || mode === "reset-confirm") && (
            <button
              type="button"
              className="link"
              data-testid="auth-back-login"
              onClick={() => onSwitch("login")}
            >
              Back to log in
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
