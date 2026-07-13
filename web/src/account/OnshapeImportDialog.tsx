/**
 * OnshapeImportDialog — "Aus Onshape…" (Onshape-Import Schritt 4, UI).
 *
 * Modal (SettingsMenu dialog pattern): the user pastes an Onshape document
 * URL, the dialog resolves the Part Studio via the server's signed
 * read-only proxy (`/api/onshape/elements|features`), translates the
 * feature list with `importOnshapeFeatures` and creates a NEW server
 * project from the result.
 *
 * Kernel strategy: a fresh TEMPORARY `Kernel.create()` (same wasm module
 * as the app's live kernel, but its own `WasmKernel` document — the live
 * document is never touched). The importer drives it feature by feature,
 * then `kernel.toJson()` becomes the part tab's document of a new
 * {@link Project} envelope which feeds `platformApi.createProject`,
 * exactly like ProjectsBrowser's `importLocal`. The temp kernel is simply
 * dropped afterwards (no explicit free on the wasm surface; one-shot).
 *
 * Failure policy: translation warnings (yellow) and errors (red) are
 * listed IN the dialog. With errors the project is still created, but the
 * dialog stays open until the user explicitly opens it — visible partial
 * failure, never a silent swallow.
 */

import { useEffect, useRef, useState } from "react";
import { platformApi, type Project } from "../api/platform";
import { accountErrorMessage } from "./useAccount";
import { Kernel } from "../kernel";
import { importOnshapeFeatures } from "../onshape/onshapeImport";
import { parseOnshapeUrl } from "../onshape/onshapeUrl";
import {
  createPartTab,
  serializeProject,
  PROJECT_SCHEMA_VERSION,
  type Project as ProjectDoc,
} from "../project";
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

/** Minimal element shape of the Onshape elements endpoint. */
interface OnshapeElement {
  id?: string;
  name?: string;
  elementType?: string;
}

function elementList(raw: unknown): OnshapeElement[] {
  if (Array.isArray(raw)) return raw as OnshapeElement[];
  if (raw && typeof raw === "object" && Array.isArray((raw as any).elements)) {
    return (raw as any).elements as OnshapeElement[];
  }
  return [];
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
    armProjectThumbnailCapture();
    onOpen(created);
    onClose();
  };

  const runImport = async () => {
    setBusy(true);
    setError(null);
    setOutcome(null);
    try {
      const ref = parseOnshapeUrl(url);
      if (!ref) {
        throw new Error(
          "Keine Onshape-URL: erwartet …/documents/<id>/w/<id>/e/<id> (oder /d/…)",
        );
      }

      // Element auflösen: fehlt /e/… in der URL, nimm das erste Part
      // Studio des Workspaces. Der Element-Name wird der Projektname.
      let elementId = ref.elementId;
      let projectName = "Onshape Import";
      if (!elementId) {
        const elements = elementList(
          await platformApi.getOnshapeElements(ref.documentId, ref.workspaceId),
        );
        const studio = elements.find((e) => e.elementType === "PARTSTUDIO");
        if (!studio?.id) {
          throw new Error("Kein Part Studio im Onshape-Dokument gefunden");
        }
        elementId = studio.id;
        if (studio.name) projectName = studio.name;
      }

      const featuresJson = (await platformApi.getOnshapeFeatures(
        ref.documentId,
        ref.workspaceId,
        elementId,
      )) as { features?: unknown[] };

      // Temporärer Kernel (frisches WASM-Dokument, s. Modul-Doku oben) —
      // sein Klassen-API deckt das ImportKernel-Interface strukturell ab.
      const kernel = await Kernel.create();
      const result = await importOnshapeFeatures(featuresJson as any, kernel);

      // Neues Projekt-Envelope um das importierte Kernel-Dokument, dann
      // als Server-Projekt anlegen (wie importLocal in ProjectsBrowser).
      const part = createPartTab("Part 1");
      part.documentJson = kernel.toJson();
      const project: ProjectDoc = {
        schemaVersion: PROJECT_SCHEMA_VERSION,
        name: projectName,
        tabs: [part],
        activeTabId: part.id,
      };
      const created = await platformApi.createProject(
        projectName,
        JSON.parse(serializeProject(project)),
      );

      const next: ImportOutcome = {
        created,
        warnings: result.warnings,
        errors: result.errors,
        featureCount: result.created.length,
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
