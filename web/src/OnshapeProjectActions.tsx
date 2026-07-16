/**
 * OnshapeProjectActions — topbar actions for a project imported from Onshape
 * (ticket #388).
 *
 * Shown next to {@link ProjectTitle} ONLY when the live project carries an
 * Onshape origin (`Project.onshape`, ticket #343/#376). Two icon buttons:
 *
 *   - a link that opens the original Onshape document in a new tab, and
 *   - a refresh button that — after an in-app confirmation (never the native
 *     `confirm()`, which would also block the E2E/verification tooling) —
 *     re-runs the very same pull+translate the project card's "Reimport" uses
 *     ({@link importOnshapeProject}) and replaces the LIVE workspace with the
 *     result via `store.loadProjectFromJson`. Unlike the (unopened) project
 *     card, the workspace here is already loaded and possibly server-bound;
 *     `commit()` inside the load path marks a bound session dirty, so the
 *     existing cloud-sync triggers (periodic/exit/unload) push it — no direct
 *     `PATCH` needed here. The project's *name* is not part of that document
 *     PATCH, so `renameProject` is called explicitly to sync a changed
 *     Onshape document name to the server record too.
 *
 * The reimport REPLACES the whole document — any local edits since the last
 * import are lost. The confirm dialog says so explicitly.
 */

import { useState } from "react";
import type { KernelStore } from "./store";
import { serializeProject } from "./project";
import { importOnshapeProject } from "./onshape/onshapeProjectImport";
import { onshapeSourceUrl, type OnshapeSource } from "./onshape/onshapeUrl";
import { accountErrorMessage } from "./account/useAccount";

export function OnshapeProjectActions({
  store,
  onshape,
  projectName,
}: {
  store: KernelStore;
  onshape: OnshapeSource | undefined;
  projectName: string;
}) {
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!onshape) return null;

  const closeConfirm = () => {
    if (busy) return;
    setConfirmOpen(false);
    setError(null);
  };

  const runReimport = async () => {
    setBusy(true);
    setError(null);
    try {
      const result = await importOnshapeProject(onshape, {
        now: new Date().toISOString(),
        fallbackName: projectName,
      });
      await store.loadProjectFromJson(serializeProject(result.project));
      // The document PATCH the load path's autosave/sync triggers send does
      // not carry the project name (see renameProject's own doc comment) —
      // sync it explicitly so a renamed Onshape document also renames the
      // server record. A no-op when the name is unchanged.
      store.renameProject(result.project.name);
      setConfirmOpen(false);
      if (result.errors.length > 0) {
        store.showTransientError(
          `Reimport mit ${result.errors.length} Fehler(n): ${result.errors[0]}`,
        );
      }
    } catch (err) {
      setError(accountErrorMessage(err, "Reimport aus Onshape fehlgeschlagen"));
    } finally {
      setBusy(false);
    }
  };

  return (
    <span className="fd-onshape-project-actions">
      <a
        className="icon-btn"
        data-testid="project-onshape-open"
        href={onshapeSourceUrl(onshape)}
        target="_blank"
        rel="noreferrer noopener"
        title={
          onshape.documentName
            ? `${onshape.documentName} in Onshape öffnen`
            : "In Onshape öffnen"
        }
        aria-label="In Onshape öffnen"
      >
        ↗
      </a>
      <button
        type="button"
        className="icon-btn"
        data-testid="project-onshape-reimport"
        title="Aus Onshape neu importieren…"
        aria-label="Aus Onshape neu importieren…"
        onClick={() => setConfirmOpen(true)}
      >
        ⟳
      </button>

      {confirmOpen && (
        <div
          className="confirm-overlay"
          data-testid="onshape-reimport-overlay"
          onClick={(e) => {
            if (e.target === e.currentTarget) closeConfirm();
          }}
        >
          <div
            className="dialog"
            role="alertdialog"
            aria-label="Aus Onshape neu importieren"
            data-testid="onshape-reimport-confirm-dialog"
          >
            <div className="dialog-header">
              <h3 className="dialog-title">Aus Onshape neu importieren?</h3>
              <button
                type="button"
                className="dialog-close"
                data-testid="onshape-reimport-cancel-x"
                disabled={busy}
                onClick={closeConfirm}
              >
                &times;
              </button>
            </div>
            <div className="dialog-body">
              <p className="warning">
                „{projectName}“ wird durch den aktuellen Stand aus Onshape{" "}
                <strong>ersetzt</strong> — lokale Änderungen an diesem Projekt
                seit dem letzten Import gehen verloren.
              </p>
              {error && (
                <p
                  className="error"
                  role="alert"
                  data-testid="onshape-reimport-error"
                >
                  {error}
                </p>
              )}
              <div className="actions">
                <button
                  type="button"
                  className="secondary"
                  data-testid="onshape-reimport-cancel"
                  disabled={busy}
                  onClick={closeConfirm}
                >
                  Abbrechen
                </button>
                <button
                  type="button"
                  className="primary"
                  data-testid="onshape-reimport-confirm"
                  disabled={busy}
                  onClick={() => void runReimport()}
                >
                  {busy ? "Importiere…" : "Neu importieren"}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </span>
  );
}
