/**
 * Gallery — the public, read-only project gallery (`GET /api/gallery`).
 *
 * No auth required, so it works in guest mode too. Each card shows a thumbnail,
 * name and last-updated date; there are no owner actions. A logged-in user can
 * open a public project into their workspace (a fresh, unbound copy).
 */

import { useCallback, useEffect, useState } from "react";
import { platformApi, thumbnailUrl, type Project } from "../api/platform";
import { accountErrorMessage } from "./useAccount";

interface GalleryProps {
  /** Open a public project as an unbound copy (only offered when logged in /
   *  the modeling app is available). */
  onOpen?: (project: Project) => void;
  onClose(): void;
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

export function Gallery({ onOpen, onClose }: GalleryProps) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await platformApi.listGallery();
      list.sort((a, b) => b.updated_at.localeCompare(a.updated_at));
      setProjects(list);
    } catch (err) {
      setError(accountErrorMessage(err, "Could not load the gallery"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  return (
    <div className="fd-gallery" data-testid="gallery">
      <div className="gallery-bar">
        <h2 className="gallery-title">Public gallery</h2>
        <button
          type="button"
          className="close"
          data-testid="gallery-close"
          aria-label="Close"
          onClick={onClose}
        >
          &times;
        </button>
      </div>

      {error && (
        <p className="error" role="alert" data-testid="gallery-error">
          {error}
        </p>
      )}

      {loading ? (
        <p className="empty" data-testid="gallery-loading">
          Loading…
        </p>
      ) : projects.length === 0 ? (
        <p className="empty" data-testid="gallery-empty">
          Nothing published yet.
        </p>
      ) : (
        <ul className="project-grid" data-testid="gallery-list">
          {projects.map((p) => (
            <li
              key={p.id}
              className="project-card"
              data-testid={`gallery-card-${p.id}`}
            >
              <div className="thumb">
                <img
                  src={thumbnailUrl(p.id)}
                  alt=""
                  onError={(e) => {
                    (e.currentTarget as HTMLImageElement).style.visibility =
                      "hidden";
                  }}
                />
              </div>
              <div className="card-body">
                <span className="name" title={p.name}>
                  {p.name}
                </span>
                <span className="meta">Updated {formatDate(p.updated_at)}</span>
                {onOpen && (
                  <button
                    type="button"
                    className="link"
                    data-testid={`gallery-open-${p.id}`}
                    onClick={() => onOpen(p)}
                  >
                    Open a copy
                  </button>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
