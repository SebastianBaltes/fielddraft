/**
 * The API the chat agent writes JavaScript against (ticket #347).
 *
 * BaseAIChat runs its agent loop IN THE BROWSER: the model emits JS, an AST
 * guard checks it against an allowlist, and it runs against exactly the object
 * this module builds. So this file — not the model, not the prompt — decides
 * what the assistant can do to a user's CAD document.
 *
 * Three rules it is built on:
 *
 *  1. **Go through the store, never around it.** Every mutation is the same
 *     `KernelStore` call the UI itself makes, so validation, regeneration, undo
 *     and autosave all happen exactly as they do for a human click. Nothing here
 *     reaches into the kernel directly.
 *  2. **Reads hand out COPIES.** A model that mutates a returned object and then
 *     reports success is a classic failure mode; the wire-form params are cloned
 *     on the way out, and `api.ts` says so in the description the model reads.
 *  3. **No script execution.** FieldSkript would let the model run arbitrary
 *     TypeScript in the app — which is the guard's whole point, undone. Writing
 *     scripts is deliberately NOT in this API (see docs/AI-CHAT-PLAN.md).
 *
 * Undo is the real safety net: every turn opens with a checkpoint, so a bad
 * agent turn is one Ctrl+Z away.
 */

import type { KernelStore } from "../store";
import { requestView, requestViewFit, type ViewPreset } from "../viewCommands";

/** The camera presets the assistant may ask for (mirrors ViewPreset). */
const VIEWS: ViewPreset[] = [
  "front",
  "back",
  "left",
  "right",
  "top",
  "bottom",
  "iso",
];

/** A feature as the assistant sees it — no kernel handles, no live references. */
export interface FeatureInfo {
  id: number;
  type: string;
  name: string;
  suppressed: boolean;
}

/** What the assistant is told about the document before each turn. */
export interface DocumentSummary {
  featureCount: number;
  bodyCount: number;
  features: FeatureInfo[];
  selection: number[];
  /** Feature id of the sketch being edited, or null when not in a sketch. */
  activeSketch: number | null;
  variables: { name: string; value: number | string }[];
  /** Scene bounding-box size in mm, [x, y, z]. */
  sceneSize: [number, number, number];
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

export function createFieldDraftApi(store: KernelStore) {
  const state = () => store.getState();

  const api = {
    // ── read ────────────────────────────────────────────────────────

    /** The whole document at a glance. Cheap; call it whenever unsure. */
    getDocument(): DocumentSummary {
      const s = state();
      return {
        featureCount: s.features.length,
        bodyCount: store.kernel.bodyCount(),
        features: s.features.map((f) => ({
          id: f.id,
          type: f.kind,
          name: f.name,
          suppressed: !!f.suppressed,
        })),
        selection: [...s.selection],
        activeSketch: s.sketchSession.active
          ? s.sketchSession.editingFeatureId
          : null,
        variables: s.variables.map((v) => ({ name: v.name, value: v.value })),
        sceneSize: s.sceneSize,
      };
    },

    /** Every feature type this document can create — built-ins AND the user's
     *  own script features. The list is dynamic; never assume a fixed set. */
    listFeatureTypes(): string[] {
      return store.kernel.listFeatureTypes();
    },

    /** The parameter schema of one feature type. Read it BEFORE building the
     *  params for `addFeature` — the field names are not guessable. */
    schemaForType(type: string): unknown {
      return store.kernel.schemaForType(type);
    },

    /** A feature's current parameters (a COPY — mutating it changes nothing;
     *  write back with `updateFeature`). */
    getFeature(id: number): Record<string, unknown> | null {
      const s = state();
      if (!s.features.some((f) => f.id === id)) return null;
      return clone(store.getFeatureParams(id));
    },

    // ── write ───────────────────────────────────────────────────────

    /** Create a feature of `type` with `params` (shape per `schemaForType`).
     *  Returns the new feature's id. This is the generic path — primitives,
     *  booleans, patterns, transforms and script features all go through it. */
    addFeature(type: string, params: Record<string, unknown> = {}): number {
      return store.addFeatureByType(type, params);
    },

    /** Replace a feature's parameters (full wire form — read `getFeature`
     *  first, change what you need, write the whole object back). */
    updateFeature(id: number, params: Record<string, unknown>): boolean {
      return store.updateFeatureParams(id, params);
    },

    /** Rename a feature (display name only). */
    renameFeature(id: number, name: string): boolean {
      return store.renameFeature(id, name);
    },

    /** Suppress / un-suppress a feature (kept in the tree, skipped on rebuild). */
    setFeatureSuppressed(id: number, suppressed: boolean): void {
      const f = state().features.find((x) => x.id === id);
      if (f && f.suppressed !== suppressed) store.toggleFeatureSuppressed(id);
    },

    /** DESTRUCTIVE — delete a feature. Ask the user first (see instructions). */
    removeFeature(id: number): void {
      store.removeFeature(id);
    },

    /** Document variables — usable in any numeric field as a formula. */
    setVariable(name: string, value: number): boolean {
      return store.setVariable(name, value);
    },

    undo(): void {
      store.undo();
    },
    redo(): void {
      store.redo();
    },

    // ── selection & view ────────────────────────────────────────────

    /** Select ONE feature (or clear the selection with `null`). */
    select(id: number | null): void {
      if (id === null) store.clearSelection();
      else store.selectOnly(id, "list");
    },

    /** Point the camera at a standard view: "front" | "back" | "left" |
     *  "right" | "top" | "bottom" | "iso". */
    setView(preset: string): void {
      if (!VIEWS.includes(preset as ViewPreset)) {
        throw new Error(
          `Unknown view "${preset}" — one of: ${VIEWS.join(", ")}`,
        );
      }
      requestView(preset as ViewPreset);
    },

    /** Zoom so the whole model fits the viewport. */
    fitView(): void {
      requestViewFit();
    },
  };

  return api;
}

export type FieldDraftApi = ReturnType<typeof createFieldDraftApi>;
