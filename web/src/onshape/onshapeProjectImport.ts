/**
 * Build a FieldDraft project document from an Onshape document (ticket #343).
 *
 * The one place that turns an Onshape URL into a project envelope — shared by
 * the import dialog (which creates a new project from it) and the project card's
 * "Reimport" (which replaces an existing project's document with a fresh pull).
 * Keeping it here is what makes the two paths provably identical: a reimport
 * lands on exactly the Part Studio the original import used, because the
 * resolved element id travels WITH the project (`Project.onshape`).
 *
 * The project title is the Onshape DOCUMENT name ("Lötkolbenhalter"), not the
 * element name ("Part Studio 1") and not the old hard-coded "Onshape Import".
 */

import { platformApi } from "../api/platform";
import { Kernel } from "../kernel";
import { importOnshapeFeatures } from "./onshapeImport";
import {
  onshapeSourceUrl,
  parseOnshapeUrl,
  type OnshapeDocRef,
  type OnshapeSource,
} from "./onshapeUrl";
import {
  createPartTab,
  PROJECT_SCHEMA_VERSION,
  type Project as ProjectDoc,
} from "../project";

/** The outcome of one import run: the document to store, plus what the
 *  translator could and could not do (the dialog shows both). */
export interface OnshapeImportResult {
  project: ProjectDoc;
  source: OnshapeSource;
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

/** Resolve the Part Studio: the URL's `/e/…` when it has one, else the
 *  document's first Part Studio. */
async function resolveElementId(ref: OnshapeDocRef): Promise<string> {
  if (ref.elementId) return ref.elementId;
  const elements = elementList(
    await platformApi.getOnshapeElements(ref.documentId, ref.workspaceId),
  );
  const studio = elements.find((e) => e.elementType === "PARTSTUDIO");
  if (!studio?.id) throw new Error("Kein Part Studio im Onshape-Dokument gefunden");
  return studio.id;
}

/** The Onshape document's own name — the project title. Falls back to the
 *  previous name (reimport) or "Onshape Import" when Onshape gives us none. */
async function documentName(documentId: string, fallback: string): Promise<string> {
  try {
    const doc = (await platformApi.getOnshapeDocument(documentId)) as {
      name?: unknown;
    };
    return typeof doc?.name === "string" && doc.name.trim()
      ? doc.name.trim()
      : fallback;
  } catch {
    // A missing/renamed document must not sink an otherwise-fine import.
    return fallback;
  }
}

/**
 * Pull an Onshape Part Studio and translate it into a project document.
 * `now` is injected so callers (and tests) control the `importedAt` stamp.
 */
export async function importOnshapeProject(
  src: OnshapeSource | OnshapeDocRef | string,
  opts: { now: string; fallbackName?: string },
): Promise<OnshapeImportResult> {
  const ref =
    typeof src === "string"
      ? (() => {
          const parsed = parseOnshapeUrl(src);
          if (!parsed) {
            throw new Error(
              "Keine Onshape-URL: erwartet …/documents/<id>/w/<id>/e/<id> (oder /d/…)",
            );
          }
          return parsed;
        })()
      : src;
  // The URL to remember on the source (ticket #376). Only trust the pasted
  // string when it already named the Part Studio (`/e/…`) — one that stopped
  // after the workspace resolves to a DIFFERENT (guessed) element below, so
  // remembering it verbatim would point "Open in Onshape" at the wrong studio.
  // Otherwise fall back to a prior source's own URL (reimport — keeps a
  // share-link `/d/…` form instead of silently normalizing it away), and only
  // reconstruct from the ids as a last resort.
  const pastedUrl =
    typeof src === "string" && ref.elementId ? src : undefined;
  const priorUrl =
    typeof src === "object" && "url" in src ? (src as OnshapeSource).url : undefined;

  const elementId = await resolveElementId({
    documentId: ref.documentId,
    workspaceId: ref.workspaceId,
    elementId: "elementId" in ref ? (ref.elementId ?? null) : null,
  });
  const name = await documentName(
    ref.documentId,
    opts.fallbackName ?? "Onshape Import",
  );

  const featuresJson = (await platformApi.getOnshapeFeatures(
    ref.documentId,
    ref.workspaceId,
    elementId,
  )) as { features?: unknown[] };

  // Onshapes eigene 3D-Antwort für dieselben Sketches. Sie ist die einzige
  // exakte Quelle für den Sketch-FRAME auf einer Körperfläche (#415) — ohne sie
  // landen solche Sketches um 90° verdreht. Ein Fehlschlag darf den Import
  // trotzdem nicht versenken: der Rest des Dokuments ist davon unberührt, also
  // wird er importiert und die Lücke benannt.
  let sketches3d: unknown = null;
  let sketches3dError: string | null = null;
  try {
    sketches3d = await platformApi.getOnshapeSketches(
      ref.documentId,
      ref.workspaceId,
      elementId,
    );
  } catch (err) {
    sketches3dError =
      `Onshapes 3D-Sketch-Daten sind nicht abrufbar (${String(err)}) — ` +
      `Sketches auf Körperflächen können um 90° verdreht sein`;
  }

  // A TEMPORARY kernel (its own wasm document — the live one is never touched);
  // the importer drives it feature by feature, its JSON becomes the part tab.
  const kernel = await Kernel.create();
  const result = await importOnshapeFeatures(featuresJson as any, kernel, {
    sketches3d: sketches3d as any,
  });
  if (sketches3dError) result.warnings.unshift(sketches3dError);

  const part = createPartTab("Part 1");
  part.documentJson = kernel.toJson();

  const source: OnshapeSource = {
    documentId: ref.documentId,
    workspaceId: ref.workspaceId,
    elementId,
    documentName: name,
    importedAt: opts.now,
  };
  source.url =
    pastedUrl ??
    priorUrl ??
    onshapeSourceUrl(source);

  return {
    project: {
      schemaVersion: PROJECT_SCHEMA_VERSION,
      name,
      tabs: [part],
      activeTabId: part.id,
      onshape: source,
    },
    source,
    warnings: result.warnings,
    errors: result.errors,
    featureCount: result.created.length,
  };
}
