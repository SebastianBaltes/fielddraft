/**
 * Dashboard — the logged-in user's server projects.
 *
 * Lists the user's projects (thumbnail, name, last-updated, visibility badge)
 * with: open (loads into the modeling app), create (new empty or "from current
 * workspace"), rename, duplicate, delete, and a private⇄public toggle. Wired to
 * `/api/projects*` through {@link platformApi}.
 *
 * It never reaches into the kernel store directly — App.tsx passes `onOpen`
 * (load a server document into the live workspace) and `getCurrentDocument`
 * (serialize the live workspace for "save as new project"), so guest mode stays
 * fully decoupled.
 */

import { useCallback, useEffect, useState } from "react";
import {
  platformApi,
  thumbnailUrl,
  type Project,
  type Visibility,
} from "../api/platform";
import { accountErrorMessage } from "./useAccount";

interface DashboardProps {
  /** Open a server project in the modeling app. Receives the project's stored
   *  document (the Project blob) as a JSON string. */
  onOpen(project: Project): void;
  /** Serialize the current live workspace for "save as new project", or `null`
   *  when the workspace is empty / unavailable. */
  getCurrentDocument(): unknown | null;
  /** The currently-bound server project id, if any (highlighted in the list). */
  boundProjectId: string | null;
  onClose(): void;
  onOpenGallery(): void;
  onOpenInvites?: () => void;
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

export function Dashboard({
  onOpen,
  getCurrentDocument,
  boundProjectId,
  onClose,
  onOpenGallery,
  onOpenInvites,
}: DashboardProps) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await platformApi.listProjects();
      // Most-recently-updated first.
      list.sort((a, b) => b.updated_at.localeCompare(a.updated_at));
      setProjects(list);
    } catch (err) {
      setError(accountErrorMessage(err, "Could not load your projects"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const guard = useCallback(
    async (id: string, fn: () => Promise<void>, fallback: string) => {
      setBusyId(id);
      setError(null);
      try {
        await fn();
      } catch (err) {
        setError(accountErrorMessage(err, fallback));
      } finally {
        setBusyId(null);
      }
    },
    [],
  );

  const createEmpty = () =>
    guard(
      "__new__",
      async () => {
        const created = await platformApi.createProject("Untitled", {});
        await reload();
        onOpen(created);
      },
      "Could not create a project",
    );

  const createFromCurrent = () =>
    guard(
      "__new__",
      async () => {
        const doc = getCurrentDocument();
        const created = await platformApi.createProject(
          "My workspace",
          doc ?? {},
        );
        await reload();
        // Bind the live workspace to the freshly-created project without
        // reloading (it IS the current workspace).
        onOpen(created);
      },
      "Could not save your workspace",
    );

  const rename = (p: Project) =>
    guard(
      p.id,
      async () => {
        const name = window.prompt("Rename project", p.name);
        if (name === null || name.trim() === "" || name === p.name) return;
        await platformApi.updateProject(p.id, { name: name.trim() });
        await reload();
      },
      "Rename failed",
    );

  const duplicate = (p: Project) =>
    guard(
      p.id,
      async () => {
        // Pull the full document, then create a copy.
        const full = await platformApi.getProject(p.id);
        await platformApi.createProject(`${p.name} copy`, full.document ?? {});
        await reload();
      },
      "Duplicate failed",
    );

  const remove = (p: Project) =>
    guard(
      p.id,
      async () => {
        if (!window.confirm(`Delete “${p.name}”? This cannot be undone.`)) {
          return;
        }
        await platformApi.deleteProject(p.id);
        await reload();
      },
      "Delete failed",
    );

  const toggleVisibility = (p: Project) =>
    guard(
      p.id,
      async () => {
        const next: Visibility =
          p.visibility === "public" ? "private" : "public";
        await platformApi.setVisibility(p.id, next);
        await reload();
      },
      "Could not change visibility",
    );

  const open = (p: Project) =>
    guard(
      p.id,
      async () => {
        // Re-fetch to be sure we have the full document (list responses carry
        // it today, but a future trim shouldn't break opening).
        const full = await platformApi.getProject(p.id);
        onOpen(full);
      },
      "Could not open the project",
    );

  return (
    <div className="fd-dashboard" data-testid="dashboard">
      <div className="dashboard-bar">
        <h2 className="dashboard-title">Your projects</h2>
        <div className="dashboard-actions">
          {onOpenInvites && (
            <button
              type="button"
              className="secondary"
              data-testid="dashboard-invites"
              onClick={onOpenInvites}
            >
              Invites
            </button>
          )}
          <button
            type="button"
            className="secondary"
            data-testid="dashboard-gallery"
            onClick={onOpenGallery}
          >
            Public gallery
          </button>
          <button
            type="button"
            className="secondary"
            data-testid="dashboard-save-current"
            disabled={busyId !== null}
            onClick={createFromCurrent}
          >
            Save current workspace
          </button>
          <button
            type="button"
            className="primary"
            data-testid="dashboard-new"
            disabled={busyId !== null}
            onClick={createEmpty}
          >
            New project
          </button>
          <button
            type="button"
            className="close"
            data-testid="dashboard-close"
            aria-label="Back to editor"
            onClick={onClose}
          >
            &times;
          </button>
        </div>
      </div>

      {error && (
        <p className="error" role="alert" data-testid="dashboard-error">
          {error}
        </p>
      )}

      {loading ? (
        <p className="empty" data-testid="dashboard-loading">
          Loading…
        </p>
      ) : projects.length === 0 ? (
        <p className="empty" data-testid="dashboard-empty">
          No projects yet. Create one to get started.
        </p>
      ) : (
        <ul className="project-grid" data-testid="project-list">
          {projects.map((p) => (
            <li
              key={p.id}
              className={
                "project-card" + (p.id === boundProjectId ? " is-bound" : "")
              }
              data-testid={`project-card-${p.id}`}
            >
              <button
                type="button"
                className="thumb"
                data-testid={`project-open-${p.id}`}
                disabled={busyId !== null}
                onClick={() => open(p)}
                title="Open"
              >
                <img
                  src={thumbnailUrl(p.id)}
                  alt=""
                  onError={(e) => {
                    (e.currentTarget as HTMLImageElement).style.visibility =
                      "hidden";
                  }}
                />
              </button>
              <div className="card-body">
                <div className="card-head">
                  <span className="name" title={p.name}>
                    {p.name}
                  </span>
                  <button
                    type="button"
                    className={
                      "badge " +
                      (p.visibility === "public" ? "is-public" : "is-private")
                    }
                    data-testid={`project-visibility-${p.id}`}
                    disabled={busyId !== null}
                    title="Toggle public / private"
                    onClick={() => toggleVisibility(p)}
                  >
                    {p.visibility === "public" ? "Public" : "Private"}
                  </button>
                </div>
                <span className="meta">Updated {formatDate(p.updated_at)}</span>
                <div className="card-actions">
                  <button
                    type="button"
                    className="link"
                    data-testid={`project-rename-${p.id}`}
                    disabled={busyId !== null}
                    onClick={() => rename(p)}
                  >
                    Rename
                  </button>
                  <button
                    type="button"
                    className="link"
                    data-testid={`project-duplicate-${p.id}`}
                    disabled={busyId !== null}
                    onClick={() => duplicate(p)}
                  >
                    Duplicate
                  </button>
                  <button
                    type="button"
                    className="link danger"
                    data-testid={`project-delete-${p.id}`}
                    disabled={busyId !== null}
                    onClick={() => remove(p)}
                  >
                    Delete
                  </button>
                </div>
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
