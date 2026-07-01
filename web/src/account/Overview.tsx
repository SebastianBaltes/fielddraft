/**
 * Overview — the main-level tabbed browser (WP13).
 *
 * The default landing after login. A persistent page below the shared topbar
 * with tabs **Projects | Issues | Invites | Users** (Invites & Users render
 * only for admins). Clicking a project (Projects tab) enters the modeling view;
 * the topbar `◀` returns here.
 *
 * It owns none of the session — App.tsx passes the shared {@link UseAccount} and
 * the kernel bridges (open a project / open a copy / load a `.fdraft`).
 */

import { ProjectsBrowser } from "./ProjectsBrowser";
import { TicketsView } from "./TicketsView";
import { InvitePanel } from "./InvitePanel";
import { AdminUsers } from "./AdminUsers";
import type { OverviewTab } from "./accountState";
import type { UseAccount } from "./useAccount";
import type { Project } from "../api/platform";

interface OverviewProps {
  account: UseAccount;
  /** Open an OWNED server project (load + bind for sync) → modeling view. */
  onOpenProject(project: Project): void;
  /** Open a PUBLIC project as a fresh unbound copy → modeling view. */
  onOpenCopy(project: Project): void;
  /** Load a ticket's `.fdraft` artifact into the live workspace → modeling. */
  onLoadFdraft(bytes: ArrayBuffer): void;
  /** The currently-bound server project id, if any (highlighted). */
  boundProjectId: string | null;
}

const BASE_TABS: { key: OverviewTab; label: string }[] = [
  { key: "projects", label: "Projects" },
  { key: "issues", label: "Issues" },
];

const ADMIN_TABS: { key: OverviewTab; label: string }[] = [
  { key: "invites", label: "Invites" },
  { key: "users", label: "Users" },
];

export function Overview({
  account,
  onOpenProject,
  onOpenCopy,
  onLoadFdraft,
  boundProjectId,
}: OverviewProps) {
  const { state, isAdmin, selectTab } = account;
  const tabs = isAdmin ? [...BASE_TABS, ...ADMIN_TABS] : BASE_TABS;
  // A non-admin never has invites/users selected; guard so a stale tab (e.g.
  // after logout from admin) falls back to Projects.
  const active = tabs.some((t) => t.key === state.overviewTab)
    ? state.overviewTab
    : "projects";

  return (
    <div className="fd-overview" data-testid="overview">
      <nav className="overview-tabs" role="tablist" aria-label="Overview">
        {tabs.map((t) => (
          <button
            key={t.key}
            type="button"
            role="tab"
            aria-selected={active === t.key}
            className={"tab" + (active === t.key ? " is-active" : "")}
            data-testid={`overview-tab-${t.key}`}
            onClick={() => selectTab(t.key)}
          >
            {t.label}
          </button>
        ))}
      </nav>

      <div
        className="overview-body"
        role="tabpanel"
        data-testid="overview-body"
      >
        {active === "projects" && (
          <ProjectsBrowser
            authed={state.status === "authed"}
            onOpen={onOpenProject}
            onOpenCopy={onOpenCopy}
            selfId={state.user?.id ?? null}
            boundProjectId={boundProjectId}
          />
        )}
        {active === "issues" && (
          <TicketsView
            user={state.user}
            isAdmin={isAdmin}
            onLoadFdraft={onLoadFdraft}
            initialTicketId={state.ticketPrefill}
          />
        )}
        {active === "invites" && isAdmin && <InvitePanel />}
        {active === "users" && isAdmin && (
          <AdminUsers selfId={state.user?.id ?? null} />
        )}
      </div>
    </div>
  );
}
