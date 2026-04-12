/**
 * M12: Project dashboard — list, create, rename, delete projects.
 *
 * Shown after login when no project is open. Clicking a project
 * row opens it in the editor (App component). The "New" button
 * creates an empty project on the server and opens it.
 */

import { useEffect, useState } from "react";

interface ProjectSummary {
  id: string;
  name: string;
  updated_at: string;
  created_at: string;
}

const API = "/api";

async function apiFetch<T>(
  path: string,
  opts?: RequestInit,
): Promise<T | null> {
  try {
    const res = await fetch(API + path, {
      credentials: "include",
      headers: { "Content-Type": "application/json" },
      ...opts,
    });
    if (!res.ok) return null;
    return (await res.json()) as T;
  } catch {
    return null;
  }
}

interface DashboardProps {
  onOpenProject: (id: string, doc: string) => void;
}

export function Dashboard({ onOpenProject }: DashboardProps) {
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");

  const refresh = async () => {
    const list = await apiFetch<ProjectSummary[]>("/projects");
    setProjects(list ?? []);
    setLoading(false);
  };

  useEffect(() => {
    refresh();
  }, []);

  const handleCreate = async () => {
    const p = await apiFetch<ProjectSummary>("/projects", {
      method: "POST",
      body: JSON.stringify({
        name: "Untitled",
        document: { schema_version: 1, feature_calls: [], next_feature_id: 1 },
      }),
    });
    if (p) {
      onOpenProject(p.id, JSON.stringify({ schema_version: 1, feature_calls: [], next_feature_id: 1 }));
    }
  };

  const handleOpen = async (id: string) => {
    const p = await apiFetch<{ document: unknown }>("/projects/" + id);
    if (p) {
      onOpenProject(id, JSON.stringify(p.document));
    }
  };

  const handleDelete = async (id: string) => {
    if (!confirm("Delete this project?")) return;
    await apiFetch("/projects/" + id, { method: "DELETE" });
    refresh();
  };

  const handleRename = async (id: string, name: string) => {
    await apiFetch("/projects/" + id, {
      method: "PATCH",
      body: JSON.stringify({ name }),
    });
    setRenamingId(null);
    refresh();
  };

  const formatDate = (iso: string) => {
    try {
      return new Date(iso).toLocaleDateString(undefined, {
        year: "numeric",
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
    } catch {
      return iso;
    }
  };

  if (loading) {
    return <div className="fd-dashboard-loading">Loading projects…</div>;
  }

  return (
    <div className="fd-dashboard">
      <div className="fd-dashboard-panel">
        <div className="header">
          <h2>Projects</h2>
          <button type="button" className="create" onClick={handleCreate}>
            + New Project
          </button>
        </div>
        {projects.length === 0 ? (
          <div className="empty">
            No projects yet. Click "New Project" to start.
          </div>
        ) : (
          <ul className="project-list">
            {projects.map((p) => (
              <li key={p.id} className="project-row">
                {renamingId === p.id ? (
                  <input
                    className="rename-input"
                    autoFocus
                    value={renameDraft}
                    onChange={(e) => setRenameDraft(e.currentTarget.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") handleRename(p.id, renameDraft);
                      if (e.key === "Escape") setRenamingId(null);
                    }}
                    onBlur={() => handleRename(p.id, renameDraft)}
                  />
                ) : (
                  <span
                    className="name"
                    onClick={() => handleOpen(p.id)}
                    onDoubleClick={() => {
                      setRenamingId(p.id);
                      setRenameDraft(p.name);
                    }}
                  >
                    {p.name}
                  </span>
                )}
                <span className="date">{formatDate(p.updated_at)}</span>
                <button
                  type="button"
                  className="delete"
                  onClick={() => handleDelete(p.id)}
                >
                  ×
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
