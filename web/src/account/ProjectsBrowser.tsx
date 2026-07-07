/**
 * ProjectsBrowser — the unified project browser (WP13 Projects tab).
 *
 * Merges the old Dashboard (my projects) and Gallery (public projects) into a
 * single filterable list:
 *   - a filter toggle: **My projects / Public** (and **All** for logged-in
 *     users),
 *   - a **New project** action (creates an empty project and opens it),
 *   - per-card open, rename / duplicate / delete (owner only), and a
 *     private⇄public visibility toggle (owner only),
 *   - opening a project loads it into the kernel and enters the modeling view.
 *
 * It never reaches into the kernel store directly: App.tsx passes `onOpen`
 * (load an owned server project + bind for sync) and `onOpenCopy` (load a
 * public project as a fresh unbound copy). There is NO close (×) button — the
 * browser is a persistent Overview tab, not an overlay.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  platformApi,
  thumbnailUrl,
  type Project,
  type Visibility,
} from "../api/platform";
import { accountErrorMessage } from "./useAccount";
import { parseProjectBytes, serializeProject } from "../project";
import {
  isLocalProjectsSupported,
  listLocalProjects,
  removeLocalProject,
  type LocalProjectSummary,
} from "../localProjects";

/** Which projects the browser lists. `mine` and `public` map to the owner /
 *  gallery endpoints; `all` is their union (logged-in only). */
export type ProjectFilter = "mine" | "public" | "all";

interface ProjectsBrowserProps {
  /** Whether a session is active (gates owner actions + the "mine"/"all"
   *  filters + New project). */
  authed: boolean;
  /** Open an OWNED server project: load its document + bind for sync. */
  onOpen(project: Project): void;
  /** Open a PUBLIC project as a fresh unbound copy. */
  onOpenCopy(project: Project): void;
  /** Reopen a remembered LOCAL project by registry id (ticket 14661fc4). */
  onOpenLocal(id: string): void;
  /** The current admin's / owner's user id, to tell owned projects apart in the
   *  `all` view (owner actions only on your own). */
  selfId: string | null;
  /** The currently-bound server project id, if any (highlighted). */
  boundProjectId: string | null;
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

export function ProjectsBrowser({
  authed,
  onOpen,
  onOpenCopy,
  onOpenLocal,
  selfId,
  boundProjectId,
}: ProjectsBrowserProps) {
  // Guests can only see the public gallery.
  const [filter, setFilter] = useState<ProjectFilter>(
    authed ? "mine" : "public",
  );
  const [mine, setMine] = useState<Project[]>([]);
  const [publicProjects, setPublicProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const wantMine = authed && (filter === "mine" || filter === "all");
      const wantPublic = filter === "public" || filter === "all";
      const [mineList, pubList] = await Promise.all([
        wantMine ? platformApi.listProjects() : Promise.resolve([]),
        wantPublic ? platformApi.listGallery() : Promise.resolve([]),
      ]);
      mineList.sort((a, b) => b.updated_at.localeCompare(a.updated_at));
      pubList.sort((a, b) => b.updated_at.localeCompare(a.updated_at));
      setMine(mineList);
      setPublicProjects(pubList);
    } catch (err) {
      setError(accountErrorMessage(err, "Could not load projects"));
    } finally {
      setLoading(false);
    }
  }, [authed, filter]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // Ticket 14661fc4: locally opened .fdraft files (File System Access API)
  // are remembered in IndexedDB and listed alongside the cloud projects in
  // the "mine"/"all" filters. Device-scoped, so independent of the session.
  const [localList, setLocalList] = useState<LocalProjectSummary[]>([]);
  useEffect(() => {
    if (filter === "public") return;
    let cancelled = false;
    void listLocalProjects().then((list) => {
      if (!cancelled) setLocalList(list);
    });
    return () => {
      cancelled = true;
    };
  }, [filter]);

  const forgetLocal = (id: string) => {
    void removeLocalProject(id).then(() =>
      listLocalProjects().then(setLocalList),
    );
  };

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

  // The list shown for the current filter. In `all`, owned projects come first,
  // then public ones the user does not own (dedup by id).
  const shown = useMemo(() => {
    if (filter === "mine") return mine;
    if (filter === "public") return publicProjects;
    const ownedIds = new Set(mine.map((p) => p.id));
    return [...mine, ...publicProjects.filter((p) => !ownedIds.has(p.id))];
  }, [filter, mine, publicProjects]);

  const ownsProject = (p: Project) =>
    authed && selfId !== null && p.owner_id === selfId;

  const createEmpty = () =>
    guard(
      "__new__",
      async () => {
        const created = await platformApi.createProject("Untitled", {});
        onOpen(created);
      },
      "Could not create a project",
    );

  // Ticket 421c3610: import a local `.fdraft` as a NEW server project (next
  // to "New project"). The file is parsed client-side (ZIP or legacy JSON)
  // and its Project blob becomes the server document; embedded ZIP assets
  // (uploaded fonts) are not part of a server document and are dropped, as
  // with any server save. Named after the file; opens bound, like New.
  const importInputRef = useRef<HTMLInputElement>(null);
  const importLocal = (file: File) =>
    guard(
      "__import__",
      async () => {
        const bytes = new Uint8Array(await file.arrayBuffer());
        const parsed = parseProjectBytes(bytes);
        if (!parsed) {
          throw new Error(`“${file.name}” is not a valid FieldDraft project`);
        }
        const name =
          file.name.replace(/\.(fdraft|json)$/i, "").trim() ||
          parsed.project.name;
        const created = await platformApi.createProject(
          name,
          JSON.parse(serializeProject(parsed.project)),
        );
        onOpen(created);
      },
      "Could not import the project",
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
        const full = await platformApi.getProject(p.id);
        if (ownsProject(full)) onOpen(full);
        else onOpenCopy(full);
      },
      "Could not open the project",
    );

  const FILTERS: { key: ProjectFilter; label: string }[] = authed
    ? [
        { key: "mine", label: "My projects" },
        { key: "public", label: "Public" },
        { key: "all", label: "All" },
      ]
    : [{ key: "public", label: "Public" }];

  return (
    <div className="fd-projects-browser" data-testid="projects-browser">
      <div className="browser-bar">
        <div
          className="filter-toggle"
          role="tablist"
          aria-label="Project filter"
          data-testid="projects-filter"
        >
          {FILTERS.map((f) => (
            <button
              key={f.key}
              type="button"
              role="tab"
              aria-selected={filter === f.key}
              className={"seg" + (filter === f.key ? " is-active" : "")}
              data-testid={`projects-filter-${f.key}`}
              onClick={() => setFilter(f.key)}
            >
              {f.label}
            </button>
          ))}
        </div>
        {authed && (
          <>
            <button
              type="button"
              className="secondary"
              data-testid="projects-import"
              disabled={busyId !== null}
              onClick={() => importInputRef.current?.click()}
            >
              Import…
            </button>
            <button
              type="button"
              className="primary"
              data-testid="projects-new"
              disabled={busyId !== null}
              onClick={createEmpty}
            >
              New project
            </button>
            <input
              ref={importInputRef}
              type="file"
              accept=".fdraft,.json"
              hidden
              data-testid="projects-import-input"
              onChange={(e) => {
                const file = e.currentTarget.files?.[0];
                if (file) void importLocal(file);
                e.currentTarget.value = "";
              }}
            />
          </>
        )}
      </div>

      {error && (
        <p className="error" role="alert" data-testid="projects-error">
          {error}
        </p>
      )}

      {filter !== "public" &&
        isLocalProjectsSupported() &&
        localList.length > 0 && (
          <div className="local-projects" data-testid="local-projects">
            <h4 className="local-title">On this device</h4>
            <ul className="local-list">
              {localList.map((lp) => (
                <li
                  key={lp.id}
                  className="local-row"
                  data-testid={`local-project-${lp.id}`}
                >
                  <span className="badge">Local</span>
                  <button
                    type="button"
                    className="local-open"
                    data-testid={`local-open-${lp.id}`}
                    disabled={busyId !== null}
                    onClick={() => onOpenLocal(lp.id)}
                    title="Reopen this file (may ask for permission)"
                  >
                    {lp.name}
                  </button>
                  <button
                    type="button"
                    className="local-forget"
                    data-testid={`local-forget-${lp.id}`}
                    onClick={() => forgetLocal(lp.id)}
                    title="Remove from this list (the file stays on disk)"
                  >
                    Forget
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}

      {loading ? (
        <p className="empty" data-testid="projects-loading">
          Loading…
        </p>
      ) : shown.length === 0 ? (
        <p className="empty" data-testid="projects-empty">
          {filter === "public"
            ? "Nothing published yet."
            : "No projects yet. Create one to get started."}
        </p>
      ) : (
        <ul className="project-grid" data-testid="project-list">
          {shown.map((p) => {
            const owned = ownsProject(p);
            return (
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
                    {owned ? (
                      <button
                        type="button"
                        className={
                          "badge " +
                          (p.visibility === "public"
                            ? "is-public"
                            : "is-private")
                        }
                        data-testid={`project-visibility-${p.id}`}
                        disabled={busyId !== null}
                        title="Toggle public / private"
                        onClick={() => toggleVisibility(p)}
                      >
                        {p.visibility === "public" ? "Public" : "Private"}
                      </button>
                    ) : (
                      <span
                        className="badge is-public static"
                        data-testid={`project-badge-${p.id}`}
                      >
                        Public
                      </span>
                    )}
                  </div>
                  <span className="meta">
                    Updated {formatDate(p.updated_at)}
                  </span>
                  <div className="card-actions">
                    {owned ? (
                      <>
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
                      </>
                    ) : (
                      <button
                        type="button"
                        className="link"
                        data-testid={`project-open-copy-${p.id}`}
                        disabled={busyId !== null}
                        onClick={() => open(p)}
                      >
                        Open a copy
                      </button>
                    )}
                  </div>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
