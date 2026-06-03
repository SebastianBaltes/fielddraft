/**
 * MT-A: project / multi-tab data model.
 *
 * A FieldDraft *project* is a flat list of tabs. Each tab is one of:
 *  - a **part** tab — owns a kernel `Document` JSON (features, variables).
 *    Exactly one part tab's document is loaded into the live WASM kernel
 *    at a time (the store's `loadedPartTabId`).
 *  - a **script** tab — owns a {@link FieldSkriptRecord}. All script tabs'
 *    records are mirrored into the live kernel so the loaded part can
 *    regenerate against them.
 *  - an **assembly** tab — owns a web-side {@link AssemblyDocument}.
 *
 * Scripts are project-level: they live in their own tabs, not inside any
 * part document. When a part document is loaded into the kernel the store
 * injects the project's scripts into it (`injectScripts`); when it is
 * flushed back out the scripts are stripped (`stripScripts`) so they never
 * get duplicated into the persisted part JSON.
 *
 * Everything here is pure — no `Date.now`, no `Math.random`. Tab ids come
 * from a deterministic module-level counter so saves are reproducible.
 */

import type { FieldSkriptRecord } from "./fieldskript/record";
import { type AssemblyDocument, createEmptyAssembly } from "./assembly";

export const PROJECT_SCHEMA_VERSION = 1;

/** Minimal valid empty kernel Document JSON (NO scripts — scripts live in
 *  script tabs). Mirrors the literal `kernel.loadJson` accepts. */
export const EMPTY_PART_DOCUMENT_JSON =
  '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';

/** Starter source for a fresh script tab. Copied from
 *  `FieldSkriptEditor`'s `NEW_SCRIPT_TEMPLATE`. */
export const DEFAULT_SCRIPT_SOURCE = `import { fd } from "fielddraft";

export const definition = fd.feature({
  name: "My Feature",
  icon: "⬡",
  parameters: [
    fd.length("size", { name: "Size", default: 20 }),
  ],
});

export default function build(context, id, def) {
  const s = Number(def.size.value);
  fd.opBox(context, id.sub("body"), {
    half_extents: [s / 2, s / 2, s / 2],
    center: [0, 0, 0],
  });
}
`;

export type TabKind = "part" | "script" | "assembly";

export interface PartTab {
  id: string;
  kind: "part";
  name: string;
  documentJson: string;
}

export interface ScriptTab {
  id: string;
  kind: "script";
  name: string;
  record: FieldSkriptRecord;
}

export interface AssemblyTab {
  id: string;
  kind: "assembly";
  name: string;
  document: AssemblyDocument;
}

export type ProjectTab = PartTab | ScriptTab | AssemblyTab;

export interface Project {
  schemaVersion: number;
  name: string;
  tabs: ProjectTab[];
  activeTabId: string;
}

// ── tab id allocation ───────────────────────────────────────────

let tabCounter = 1;

/** Allocate a fresh, deterministic tab id (`tab_1`, `tab_2`, …). */
export function allocateTabId(): string {
  return `tab_${tabCounter++}`;
}

/** Advance the id counter past the highest numeric `tab_<n>` suffix among
 *  the given tabs so post-load allocations never collide. */
export function reseedTabIds(tabs: ProjectTab[]): void {
  let max = 0;
  for (const tab of tabs) {
    const m = /^tab_(\d+)$/.exec(tab.id);
    if (m) max = Math.max(max, Number(m[1]));
  }
  tabCounter = max + 1;
}

// ── script helpers ──────────────────────────────────────────────

/** Records of every script tab in the project. */
export function projectScripts(project: Project): FieldSkriptRecord[] {
  return project.tabs
    .filter((t): t is ScriptTab => t.kind === "script")
    .map((t) => t.record);
}

/** Parse a part document, set `doc.scripts` to the given records (or
 *  delete the key when empty), re-stringify. */
export function injectScripts(
  documentJson: string,
  records: FieldSkriptRecord[],
): string {
  const doc = JSON.parse(documentJson);
  if (records.length > 0) {
    doc.scripts = records;
  } else {
    delete doc.scripts;
  }
  return JSON.stringify(doc);
}

/** Parse a part document, delete any `doc.scripts`, re-stringify. */
export function stripScripts(documentJson: string): string {
  const doc = JSON.parse(documentJson);
  delete doc.scripts;
  return JSON.stringify(doc);
}

// ── tab constructors ────────────────────────────────────────────

export function createPartTab(name = "Part"): PartTab {
  return {
    id: allocateTabId(),
    kind: "part",
    name,
    documentJson: EMPTY_PART_DOCUMENT_JSON,
  };
}

export function createAssemblyTab(name = "Assembly"): AssemblyTab {
  return {
    id: allocateTabId(),
    kind: "assembly",
    name,
    document: createEmptyAssembly(name),
  };
}

/** A fresh project: one part tab ("Part 1"), active. */
export function createDefaultProject(): Project {
  const part = createPartTab("Part 1");
  return {
    schemaVersion: PROJECT_SCHEMA_VERSION,
    name: "Untitled",
    tabs: [part],
    activeTabId: part.id,
  };
}

// ── (de)serialization ───────────────────────────────────────────

export function serializeProject(project: Project): string {
  return JSON.stringify(project);
}

/** Parse + defensively validate a serialized project. Returns `null` on
 *  any malformed shape. */
export function parseProject(json: string): Project | null {
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    return null;
  }
  if (typeof raw !== "object" || raw === null) return null;
  const p = raw as Record<string, unknown>;
  if (typeof p.schemaVersion !== "number") return null;
  if (typeof p.name !== "string") return null;
  if (!Array.isArray(p.tabs)) return null;
  if (typeof p.activeTabId !== "string") return null;

  const tabs: ProjectTab[] = [];
  for (const t of p.tabs) {
    if (typeof t !== "object" || t === null) return null;
    const tab = t as Record<string, unknown>;
    if (typeof tab.id !== "string" || typeof tab.name !== "string") return null;
    switch (tab.kind) {
      case "part":
        if (typeof tab.documentJson !== "string") return null;
        tabs.push({ id: tab.id, kind: "part", name: tab.name, documentJson: tab.documentJson });
        break;
      case "script":
        if (typeof tab.record !== "object" || tab.record === null) return null;
        tabs.push({
          id: tab.id,
          kind: "script",
          name: tab.name,
          record: tab.record as FieldSkriptRecord,
        });
        break;
      case "assembly":
        if (typeof tab.document !== "object" || tab.document === null) return null;
        tabs.push({
          id: tab.id,
          kind: "assembly",
          name: tab.name,
          document: tab.document as AssemblyDocument,
        });
        break;
      default:
        return null;
    }
  }
  if (tabs.length === 0) return null;
  if (!tabs.some((t) => t.id === p.activeTabId)) return null;

  return {
    schemaVersion: p.schemaVersion,
    name: p.name,
    tabs,
    activeTabId: p.activeTabId,
  };
}
