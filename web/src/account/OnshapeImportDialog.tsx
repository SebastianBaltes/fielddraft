/**
 * OnshapeImportDialog — "Aus Onshape…" (Onshape-Import Schritt 4, UI).
 *
 * Modal (SettingsMenu dialog pattern): the user pastes an Onshape document URL,
 * `importOnshapeProject` (onshape/onshapeProjectImport.ts) pulls and translates
 * it, and the dialog creates a NEW server project from the result. That module —
 * not this dialog — owns the pull: the project card's "Reimport" runs the very
 * same code against the Onshape ids the project remembers (ticket #343).
 *
 * Failure policy: translation warnings (yellow) and errors (red) are listed IN
 * the dialog. With errors the project is still created, but the dialog stays
 * open until the user explicitly opens it — visible partial failure, never a
 * silent swallow.
 */

import { useEffect, useRef, useState } from "react";
import { platformApi, type Project } from "../api/platform";
import { accountErrorMessage } from "./useAccount";
import { importOnshapeProject } from "../onshape/onshapeProjectImport";
import { serializeProject } from "../project";
import { armProjectThumbnailCapture } from "../thumbnailScheduler";

interface OnshapeImportDialogProps {
  /** Open the created server project (ProjectsBrowser's `onOpen`). */
  onOpen(project: Project): void;
  onClose(): void;
}

interface ImportOutcome {
  created: Project;
  warnings: string[];
  errors: string[];
  featureCount: number;
}

export function OnshapeImportDialog({
  onOpen,
  onClose,
}: OnshapeImportDialogProps) {
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  /** Fatal pre-import failure (bad URL, API error) — red banner. */
  const [error, setError] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<ImportOutcome | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Escape closes — capture phase so viewport shortcuts never see it
  // (same pattern as SettingsMenu's dialog).
  useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        e.stopPropagation();
      }
    };
    window.addEventListener("keydown", handleKey, true);
    return () => window.removeEventListener("keydown", handleKey, true);
  }, [onClose]);

  const openCreated = (created: Project) => {
    // Same follow-up as ProjectsBrowser.createEmpty: arm a one-shot
    // thumbnail so the first meshed frame becomes the gallery preview.
    armProjectThumbnailCapture(created.id);
    onOpen(created);
    onClose();
  };

  const runImport = async () => {
    setBusy(true);
    setError(null);
    setOutcome(null);
    try {
      // Pull + translate (shared with the project card's Reimport), then create
      // the server project. The title is the Onshape DOCUMENT name and the
      // resolved Onshape ids travel in the envelope, so the card can offer
      // "Open in Onshape" / "Reimport" later (ticket #343).
      const result = await importOnshapeProject(url, {
        now: new Date().toISOString(),
      });
      const created = await platformApi.createProject(
        result.project.name,
        JSON.parse(serializeProject(result.project)),
      );

      const next: ImportOutcome = {
        created,
        warnings: result.warnings,
        errors: result.errors,
        featureCount: result.featureCount,
      };
      if (next.errors.length === 0 && next.warnings.length === 0) {
        // Sauberer Import → direkt öffnen.
        openCreated(created);
        return;
      }
      // Teilerfolg: Projekt existiert, aber Warnungen/Fehler bleiben
      // sichtbar, bis der Nutzer bewusst „Öffnen" klickt.
      setOutcome(next);
    } catch (err) {
      setError(accountErrorMessage(err, "Onshape-Import fehlgeschlagen"));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      className="fd-onshape-import"
      data-testid="onshape-import-overlay"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className="dialog"
        role="dialog"
        aria-label="Aus Onshape importieren"
        data-testid="onshape-import-dialog"
      >
        <div className="dialog-header">
          <h3 className="dialog-title">Aus Onshape importieren</h3>
          <button
            type="button"
            className="dialog-close"
            data-testid="onshape-import-close"
            onClick={onClose}
          >
            &times;
          </button>
        </div>

        <div className="dialog-body">
          <label className="field">
            <span className="field-label">Onshape-Dokument-URL</span>
            <input
              ref={inputRef}
              type="url"
              className="url-input"
              placeholder="https://cad.onshape.com/documents/…/w/…/e/…"
              value={url}
              disabled={busy}
              data-testid="onshape-import-url"
              onChange={(e) => setUrl(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !busy && url.trim()) void runImport();
              }}
            />
          </label>

          {error && (
            <p className="error" role="alert" data-testid="onshape-import-error">
              {error}
            </p>
          )}

          {outcome && (
            <div className="result" data-testid="onshape-import-result">
              <p className="summary">
                {outcome.featureCount} Feature
                {outcome.featureCount === 1 ? "" : "s"} importiert als „
                {outcome.created.name}“.
              </p>
              {outcome.errors.length > 0 && (
                <ul className="issue-list errors" data-testid="onshape-import-errors">
                  {outcome.errors.map((msg, i) => (
                    <li key={i}>{msg}</li>
                  ))}
                </ul>
              )}
              {outcome.warnings.length > 0 && (
                <ul
                  className="issue-list warnings"
                  data-testid="onshape-import-warnings"
                >
                  {outcome.warnings.map((msg, i) => (
                    <li key={i}>{msg}</li>
                  ))}
                </ul>
              )}
            </div>
          )}

          <div className="actions">
            {outcome ? (
              <button
                type="button"
                className="primary"
                data-testid="onshape-import-open"
                onClick={() => openCreated(outcome.created)}
              >
                Öffnen
              </button>
            ) : (
              <button
                type="button"
                className="primary"
                data-testid="onshape-import-run"
                disabled={busy || !url.trim()}
                onClick={() => void runImport()}
              >
                {busy ? "Importiere…" : "Importieren"}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
