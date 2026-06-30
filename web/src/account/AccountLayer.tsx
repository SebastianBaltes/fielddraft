/**
 * AccountLayer — the additive account UI mounted on top of the modeling app.
 *
 * Owns the {@link useAccount} session and renders, as a full-screen overlay,
 * whichever account screen is open (login / register / reset / dashboard /
 * gallery / invites). When no overlay is open it renders nothing, so guest mode
 * and the modeling workflow are completely undisturbed.
 *
 * The entry point (an account button) is a separate, lightweight
 * {@link AccountButton} placed in the topbar; it shares the session via the
 * `account` prop so there is exactly one `/api/me` resolution per app load.
 *
 * Bridges to the kernel store are passed in by App.tsx:
 *   - `onOpenProject(project)` loads a server document into the live workspace
 *     and binds it for sync,
 *   - `getCurrentDocument()` serializes the live workspace for "save current".
 */

import { AuthScreen } from "./AuthScreen";
import { Dashboard } from "./Dashboard";
import { Gallery } from "./Gallery";
import { InvitePanel } from "./InvitePanel";
import type { UseAccount } from "./useAccount";
import type { Project } from "../api/platform";

interface AccountLayerProps {
  account: UseAccount;
  /** Bind + load a server project into the live workspace. */
  onOpenProject(project: Project): void;
  /** Open a public/gallery project as a fresh unbound copy. */
  onOpenCopy(project: Project): void;
  /** Serialize the live workspace, or null when unavailable. */
  getCurrentDocument(): unknown | null;
  /** The bound server project id (highlights it in the dashboard). */
  boundProjectId: string | null;
}

export function AccountLayer({
  account,
  onOpenProject,
  onOpenCopy,
  getCurrentDocument,
  boundProjectId,
}: AccountLayerProps) {
  const { state, isAdmin, open, close, login, register } = account;
  const overlay = state.overlay;
  if (overlay === null) return null;

  // While the boot `/api/me` is still in flight, only render deep-link auth
  // screens (invite/reset) — never the dashboard, which needs a session.
  if (
    state.status === "loading" &&
    (overlay === "dashboard" || overlay === "invites" || overlay === "gallery")
  ) {
    return null;
  }

  if (
    overlay === "login" ||
    overlay === "register" ||
    overlay === "reset-request" ||
    overlay === "reset-confirm"
  ) {
    return (
      <AuthScreen
        mode={overlay}
        invitePrefill={state.invitePrefill}
        resetPrefill={state.resetPrefill}
        onLogin={login}
        onRegister={register}
        onSwitch={open}
        onClose={close}
      />
    );
  }

  if (overlay === "dashboard") {
    return (
      <div className="fd-account-overlay" data-testid="account-overlay">
        <Dashboard
          onOpen={(p) => {
            onOpenProject(p);
            close();
          }}
          getCurrentDocument={getCurrentDocument}
          boundProjectId={boundProjectId}
          onClose={close}
          onOpenGallery={() => open("gallery")}
          onOpenInvites={isAdmin ? () => open("invites") : undefined}
        />
      </div>
    );
  }

  if (overlay === "gallery") {
    return (
      <div className="fd-account-overlay" data-testid="account-overlay">
        <Gallery
          onOpen={(p) => {
            onOpenCopy(p);
            close();
          }}
          onClose={() => open(state.status === "authed" ? "dashboard" : null)}
        />
      </div>
    );
  }

  if (overlay === "invites") {
    // Defensive: the panel is admin-only. A non-admin landing here (shouldn't
    // happen — the entry points gate on role) is bounced to the dashboard.
    if (!isAdmin) {
      return (
        <div className="fd-account-overlay" data-testid="account-overlay">
          <Dashboard
            onOpen={(p) => {
              onOpenProject(p);
              close();
            }}
            getCurrentDocument={getCurrentDocument}
            boundProjectId={boundProjectId}
            onClose={close}
            onOpenGallery={() => open("gallery")}
          />
        </div>
      );
    }
    return (
      <div className="fd-account-overlay" data-testid="account-overlay">
        <InvitePanel onClose={() => open("dashboard")} />
      </div>
    );
  }

  return null;
}
