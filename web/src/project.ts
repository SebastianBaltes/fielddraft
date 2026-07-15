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

import { unzipSync, zipSync, type Zippable } from "fflate";
import type { FieldSkriptRecord } from "./fieldskript/record";
import { type AssemblyDocument, createEmptyAssembly } from "./assembly";
import { type CameraPose, parseCameraPose } from "./cameraPoseBridge";
import { type ProjectDebug, parseProjectDebug } from "./projectDebug";
import type { OnshapeSource } from "./onshape/onshapeUrl";

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
  /** Ticket pxff: saved camera pose (position / orientation / target / zoom)
   *  so re-opening the project restores the exact viewpoint. Optional — legacy
   *  files and freshly-created projects have none. */
  view?: CameraPose;
  /** Debug-tool state (open dialogs + their parameters, overlays) so a saved
   *  project — especially a ticket repro — re-opens with the exact debug
   *  setup it was captured with. Optional; absent when nothing is active. */
  debug?: ProjectDebug;
  /** Where this project was imported from, when it came from Onshape (ticket
   *  #343). Drives the project card's "Open in Onshape" + "Reimport". Absent on
   *  everything not imported from Onshape. */
  onshape?: OnshapeSource;
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

// ── Phase 3d: content-addressed `.fdraft` ZIP ────────────────────
//
// A saved `.fdraft` is a ZIP archive (via fflate, exactly like
// `threemf.ts`), self-contained and content-addressed. Layout:
//
//   project.json     the lean document ENVELOPE (`{ v, doc, fonts }` from
//                    projectPersistence.externalizeProject) — features +
//                    `__asset` mesh hashes + a `fontId → hash` manifest.
//   assets/<hash>    every referenced binary blob (meshes AND fonts),
//                    sha256hex-named and deduplicated. No raw mesh bytes
//                    live in the document any more.
//
// The heavy externalize/inline + font-cache round-trip lives in
// `fdraftFile.ts` (async, over a `ZipBlobBackend`); THIS module owns only
// the pure byte layer: {@link serializeFdraftZip} writes the ZIP,
// {@link readFdraftBytes} reads it back into its raw pieces.
//
// Backward compatibility (LOAD ONLY): an OLD `.fdraft` — plain JSON, or a
// ZIP whose document lives in `document.json` with fonts under
// `assets/fonts/<id>.<ext>` + a `manifest.json` and meshes embedded inline —
// keeps opening. {@link readFdraftBytes} sniffs the format and returns a
// `legacy` variant for those; no new `.fdraft` is ever written that way.
//
// NOTE: the in-app bug-report / server / vite ticket path keeps writing
// plain JSON (see store.exportProjectJson) so tickets stay agent-readable;
// only the user-facing Save/Open uses the ZIP.

/** New format: the lean envelope entry (`{ v, doc, fonts }` JSON). */
const ENVELOPE_PATH = "project.json";
/** New format: content-addressed blob directory (`assets/<sha256hex>`). */
const BLOB_DIR = "assets/";
/** Legacy format: the embedded (full) project document. */
const DOCUMENT_PATH = "document.json";
/** Legacy format: the font-path index. */
const MANIFEST_PATH = "manifest.json";

/** Binary assets carried in a LEGACY `.fdraft` ZIP alongside the document.
 *  Fonts are keyed by their `sketchTextFontId` (e.g. `uploaded:Foo.ttf`).
 *  New `.fdraft` files route every binary through the content-addressed
 *  `assets/<hash>` path instead; this type only backs the legacy reader. */
export interface ProjectAssets {
  fonts: Map<string, { bytes: Uint8Array; ext: string }>;
}

/** An empty asset set (legacy JSON loads / projects with no uploads). */
export function emptyProjectAssets(): ProjectAssets {
  return { fonts: new Map() };
}

interface ProjectManifest {
  /** font id → archive path of its bytes (legacy format only). */
  fonts: Record<string, string>;
}

/**
 * Phase 3d: serialize a lean document envelope + its content-addressed blob
 * map into a `.fdraft` ZIP (`project.json` + `assets/<hash>`). The envelope
 * and blobs come from {@link externalizeProject} (see `fdraftFile.ts`).
 */
export function serializeFdraftZip(
  envelopeJson: string,
  blobs: Map<string, Uint8Array>,
): Uint8Array {
  const files: Zippable = {
    [ENVELOPE_PATH]: new TextEncoder().encode(envelopeJson),
  };
  for (const [hash, bytes] of blobs) {
    files[`${BLOB_DIR}${hash}`] = bytes;
  }
  return zipSync(files);
}

/** The raw pieces {@link readFdraftBytes} extracts from a `.fdraft`.
 *  `v2` is the content-addressed format (envelope + hash blobs); `legacy`
 *  is an old fully-embedded file (plain JSON or `document.json`+manifest). */
export type FdraftContents =
  | { kind: "v2"; envelopeJson: string; blobs: Map<string, Uint8Array> }
  | { kind: "legacy"; project: Project; assets: ProjectAssets };

/**
 * Phase 3d: read `.fdraft` bytes into their raw pieces, auto-detecting the
 * format. Returns `null` on malformed input. The async inlining (mesh blobs
 * back into the document, font blobs back into the cache) happens in
 * `fdraftFile.ts`; here we only unzip and classify.
 *
 *  - Not a ZIP → legacy plain-JSON Project (report/legacy path).
 *  - ZIP with `project.json` → `v2` (envelope + every `assets/<hash>` blob).
 *  - ZIP with `document.json` → legacy embedded (fonts via manifest).
 */
export function readFdraftBytes(bytes: Uint8Array): FdraftContents | null {
  // ZIP local-file-header magic: 0x50 0x4b ("PK").
  const isZip = bytes.length >= 2 && bytes[0] === 0x50 && bytes[1] === 0x4b;
  if (!isZip) {
    // Legacy / report path: the whole file is the serialized Project JSON.
    const project = parseProject(new TextDecoder().decode(bytes));
    if (!project) return null;
    return { kind: "legacy", project, assets: emptyProjectAssets() };
  }

  let files: Record<string, Uint8Array>;
  try {
    files = unzipSync(bytes);
  } catch {
    return null;
  }

  // New content-addressed format: `project.json` + `assets/<hash>` blobs.
  const envelopeBytes = files[ENVELOPE_PATH];
  if (envelopeBytes) {
    const blobs = new Map<string, Uint8Array>();
    for (const [path, data] of Object.entries(files)) {
      if (!path.startsWith(BLOB_DIR)) continue;
      const hash = path.slice(BLOB_DIR.length);
      // Only direct `assets/<hash>` children (never a nested legacy subdir).
      if (hash.length === 0 || hash.includes("/")) continue;
      blobs.set(hash, data);
    }
    return {
      kind: "v2",
      envelopeJson: new TextDecoder().decode(envelopeBytes),
      blobs,
    };
  }

  // Legacy ZIP: `document.json` (embedded meshes) + `assets/fonts/…` + manifest.
  const docBytes = files[DOCUMENT_PATH];
  if (!docBytes) return null;
  const project = parseProject(new TextDecoder().decode(docBytes));
  if (!project) return null;

  const assets = emptyProjectAssets();
  const manifestBytes = files[MANIFEST_PATH];
  if (manifestBytes) {
    let manifest: ProjectManifest | null = null;
    try {
      manifest = JSON.parse(new TextDecoder().decode(manifestBytes)) as ProjectManifest;
    } catch {
      manifest = null;
    }
    if (manifest && manifest.fonts && typeof manifest.fonts === "object") {
      for (const [id, path] of Object.entries(manifest.fonts)) {
        const fontBytes = files[path];
        if (!fontBytes) continue;
        const ext = path.slice(path.lastIndexOf(".") + 1);
        assets.fonts.set(id, { bytes: fontBytes, ext });
      }
    }
  }

  return { kind: "legacy", project, assets };
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
    // Optional saved viewpoint (ticket pxff); a malformed view is dropped, not
    // fatal, so an otherwise-valid project still loads.
    ...(parseCameraPose(p.view) ? { view: parseCameraPose(p.view)! } : {}),
    // Optional debug-tool state; malformed blobs are dropped the same way.
    ...(parseProjectDebug(p.debug) ? { debug: parseProjectDebug(p.debug) } : {}),
    // Optional Onshape origin (ticket #343), same drop-if-malformed rule.
    ...(parseOnshapeSource(p.onshape)
      ? { onshape: parseOnshapeSource(p.onshape)! }
      : {}),
  };
}

/** Validate a stored Onshape origin. All three ids must be present and look like
 *  Onshape ids; anything else is dropped (the project still loads, it just has
 *  no reimport target). */
function parseOnshapeSource(raw: unknown): OnshapeSource | null {
  if (!raw || typeof raw !== "object") return null;
  const o = raw as Record<string, unknown>;
  const id = (v: unknown) =>
    typeof v === "string" && /^[0-9a-f]{24}$/i.test(v) ? v : null;
  const documentId = id(o.documentId);
  const workspaceId = id(o.workspaceId);
  const elementId = id(o.elementId);
  if (!documentId || !workspaceId || !elementId) return null;
  return {
    documentId,
    workspaceId,
    elementId,
    ...(typeof o.documentName === "string"
      ? { documentName: o.documentName }
      : {}),
    ...(typeof o.url === "string" ? { url: o.url } : {}),
    ...(typeof o.importedAt === "string" ? { importedAt: o.importedAt } : {}),
  };
}
