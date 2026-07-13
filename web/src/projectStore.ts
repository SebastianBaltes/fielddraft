/**
 * ProjectStore — the project-domain controller (MT-A / M12 / WP3).
 *
 * Owns the multi-tab project model: which part tab is loaded into the
 * live kernel, the single persistence funnel (`serializeProject`),
 * `.fdraft` save/load (ZIP + legacy JSON), server-project binding and
 * sync, tab management (create/rename/delete/switch), and the
 * FieldSkript records (save script tabs, insert script instances).
 * The KernelStore facade delegates its public project API 1:1 here.
 */

import type { Kernel } from "./kernel";
import {
  INACTIVE_ASSEMBLY,
  type AssemblyState,
} from "./assembly";
import { INACTIVE_SKETCH_SESSION } from "./sketchSession";
import { getCameraPose, requestCameraRestore } from "./cameraPoseBridge";
import { applyProjectDebug, collectProjectDebug } from "./projectDebug";
import { logUserAction } from "./userActionLog";
import {
  createRecord,
  reviseRecord,
  type FieldSkriptRecord,
} from "./fieldskript/record";
import {
  DEFAULT_SCRIPT_SOURCE,
  allocateTabId,
  createDefaultProject,
  createPartTab,
  createAssemblyTab,
  injectScripts,
  parseProject,
  projectScripts,
  reseedTabIds,
  serializeProject as serializeProjectJson,
  stripScripts,
  type PartTab,
  type Project,
  type ProjectTab,
  type ScriptTab,
  type TabKind,
} from "./project";
import { exportFdraftBytes, readFdraftProject } from "./fdraftFile";
import {
  isLocalFileBound,
  setLocalClean,
  writeBoundFile,
} from "./localFileBridge";
import { parseFeatureSchema, buildDefaultDefinition } from "./widgets/schema";
import { externalizeProject, inlineProject } from "./projectPersistence";
import { serverBlobStore } from "./serverBlobStore";
import {
  StoreCore,
  formatKernelError,
  type StoreState,
} from "./storeCore";

/** Ticket 6398…: the server-project binding survives a page reload. The
 *  localStorage autosave restores the DOCUMENT, this key restores the sync
 *  binding (`projectId`) — without it a reload silently degrades a
 *  server-saved project to guest mode ("save to server first" on export).
 *  Written/cleared in lockstep with the binding transitions below; the boot
 *  rebind (App.tsx) verifies the id against the server before adopting it. */
const SERVER_BINDING_KEY = "fielddraft.serverProjectId";

export function loadStoredServerBinding(): string | null {
  try {
    return localStorage.getItem(SERVER_BINDING_KEY);
  } catch {
    return null;
  }
}

export function clearStoredServerBinding(): void {
  persistServerBinding(null);
}

function persistServerBinding(id: string | null): void {
  try {
    if (id) localStorage.setItem(SERVER_BINDING_KEY, id);
    else localStorage.removeItem(SERVER_BINDING_KEY);
  } catch {
    // storage unavailable (private mode) — binding just won't survive reload
  }
}

export class ProjectStore {
  constructor(private readonly core: StoreCore) {}

  private get kernel(): Kernel {
    return this.core.kernel;
  }
  private get state(): StoreState {
    return this.core.state;
  }
  private set state(next: StoreState) {
    this.core.state = next;
  }
  private patch(partial: Partial<StoreState>): void {
    this.core.patch(partial);
  }
  private notify(): void {
    this.core.notify();
  }

  /** MT-A: which part tab's document is currently loaded into the live
   *  kernel. `null` only transiently during construction. */
  private loadedPartTabId: string | null = null;

  /**
   * MT-A boot path: parse the persisted Project blob (or create the
   * default), mirror its scripts into the kernel, load the active/first
   * part tab and remember it as the loaded tab. Returns the project plus
   * the initial assembly read-view for the facade's very first state
   * snapshot. Runs BEFORE the first StoreState exists, so it must not
   * touch `core.state`.
   */
  bootstrap(persistedProjectJson: string | null): {
    project: Project;
    initialAssembly: AssemblyState;
  } {
    const project =
      parseProject(persistedProjectJson ?? "") ?? createDefaultProject();
    reseedTabIds(project.tabs);
    // Ticket pxff: seed the pose bridge so the first CameraPoseSync mount
    // restores the persisted viewpoint (page reload), not the default view.
    requestCameraRestore(project.view ?? null);
    // Restore the project's debug-tool state (dialogs, overlays).
    applyProjectDebug(project.debug);

    // Mirror every project script into the live kernel so the loaded
    // part regenerates against them.
    for (const record of projectScripts(project)) {
      this.kernel.upsertScript(record);
    }

    // Determine the part tab to load: the active tab if it's a part,
    // else the first part tab. A default project always has ≥1 part
    // tab; if a loaded project somehow has none, create + append one.
    const activeTab = project.tabs.find((t) => t.id === project.activeTabId);
    let partTab =
      activeTab && activeTab.kind === "part"
        ? activeTab
        : project.tabs.find((t): t is PartTab => t.kind === "part");
    if (!partTab) {
      partTab = createPartTab("Part 1");
      project.tabs.push(partTab);
    }
    this.kernel.loadJson(
      injectScripts(partTab.documentJson, projectScripts(project)),
    );
    this.loadedPartTabId = partTab.id;

    const initialAssembly: AssemblyState =
      activeTab && activeTab.kind === "assembly"
        ? { active: true, document: activeTab.document, selectedPartId: null }
        : INACTIVE_ASSEMBLY;
    return { project, initialAssembly };
  }

  /**
   * FT-W: derive the Part toolbar's script-button meta from the
   * kernel's FieldSkript records. Lightweight {id,name,icon} only —
   * recomputed on every commit so saves/deletes surface immediately.
   */
  snapshotScriptTypes(): { id: string; name: string; icon: string }[] {
    return this.kernel
      .listScripts()
      .map((r) => ({ id: r.id, name: r.name, icon: r.icon }));
  }

  /**
   * MT-A: the single persistence funnel. Builds a Project mirroring
   * `state.project` but with the loaded part tab's `documentJson`
   * replaced by the live kernel doc (scripts stripped — they live in
   * script tabs). The active assembly tab is always current (assembly
   * edits write through immediately), so it needs no special-casing.
   * Everything that persists routes here.
   */
  serializeProject(): string {
    return serializeProjectJson(this.buildPersistableProject());
  }

  /** MT-A: build the persistable {@link Project} (live kernel doc folded
   *  back into the loaded part tab, scripts stripped). The JSON funnel and
   *  the wu2v ZIP save both consume this. */
  private buildPersistableProject(): Project {
    const tabs = this.state.project.tabs.map((t) => {
      if (t.id === this.loadedPartTabId && t.kind === "part") {
        // PERSISTENT-NAMING N4: normalise stored element references (fresh
        // hash + fingerprint) right before the document is persisted, so a
        // healed reference never decays across successive edits. Best-effort:
        // a save must never fail on healing.
        try {
          const healed = this.kernel.healElementRefs();
          if (healed.length > 0)
            console.info("[fielddraft] healed element refs on save:", healed);
        } catch {
          // ignore — healing is a normalisation, not a save precondition
        }
        return { ...t, documentJson: stripScripts(this.kernel.toJson()) };
      }
      return t;
    });
    // Ticket pxff: persist the current viewpoint so re-opening restores it.
    // Debug-tool state travels with the project too (repro during dev).
    return {
      ...this.state.project,
      tabs,
      view: getCameraPose() ?? undefined,
      debug: collectProjectDebug(),
    };
  }

  /** Public accessor for the live project as a content-addressed `.fdraft`
   *  ZIP (lean `project.json` envelope + `assets/<hash>` mesh/font blobs),
   *  byte-identical to what Save writes. Used by the in-app bug-report flow
   *  (on3k) so a ticket carries the exact, openable project file — assets
   *  included — not a stripped JSON blob. */
  async exportProjectFdraft(): Promise<Uint8Array> {
    return exportFdraftBytes(this.serializeProject());
  }

  /** Explicit Save (Ctrl+S / menu). Phase 4: when a local `.fdraft` file is
   *  bound as the save target, write the current project straight into that
   *  file (silent overwrite via the FSA handle) and mark the local state
   *  clean. Otherwise — no bound file, or the handle write failed (permission
   *  revoked) — fall back to a browser download.
   *
   *  The file is a content-addressed ZIP (`project.json` envelope +
   *  `assets/<hash>` mesh/font blobs) — self-contained and deduplicated.
   *  Legacy `.fdraft` files (plain JSON or the old `document.json` layout)
   *  still open (the loader sniffs the format). The bug-report path uses the
   *  same ZIP — see exportProjectFdraft. */
  async saveProjectToFile(filename = "project.fdraft"): Promise<void> {
    const zip = await exportFdraftBytes(this.serializeProject());
    // Bound local file → overwrite it in place, no download dialog.
    if (isLocalFileBound() && (await writeBoundFile(zip))) {
      setLocalClean();
      return;
    }
    if (typeof document === "undefined") return;
    // Copy into a fresh ArrayBuffer so Blob doesn't retain the WASM/heap view.
    const blob = new Blob([zip.slice()], { type: "application/zip" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  }

  /** MT-A: load a project from a .fdraft JSON string. Kept for the
   *  report/tests/legacy callers; routes through {@link loadProjectFromBytes}
   *  so a JSON string and a ZIP file share one load path (the byte loader
   *  sniffs the magic bytes and treats non-ZIP bytes as JSON text).
   *  Returns `false` on malformed input. */
  async loadProjectFromJson(json: string): Promise<boolean> {
    return this.loadProjectFromBytes(new TextEncoder().encode(json));
  }

  /** Phase 3d: load a project from raw `.fdraft` bytes — a content-addressed
   *  ZIP, a legacy embedded ZIP, or legacy JSON (auto-detected). Re-inlines
   *  externalized mesh blobs and restores uploaded fonts into the cache, then
   *  applies the project. Returns `false` on malformed input. */
  async loadProjectFromBytes(bytes: Uint8Array | ArrayBuffer): Promise<boolean> {
    const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    const project = await readFdraftProject(u8);
    if (!project) return false;
    logUserAction("doc", "load-project", { bytes: u8.length });
    return this.applyLoadedProject(project);
  }

  /** Common tail of the load paths: reseed ids, rebuild scripts, load the
   *  active/first part doc, replace project state. Returns `false` only if
   *  the project shape is somehow unusable. */
  private applyLoadedProject(project: Project): boolean {
    reseedTabIds(project.tabs);

    // Rebuild the kernel's scripts from scratch.
    for (const r of this.kernel.listScripts()) this.kernel.removeScript(r.id);
    for (const r of projectScripts(project)) this.kernel.upsertScript(r);

    const activeTab = project.tabs.find((t) => t.id === project.activeTabId);
    let partTab =
      activeTab && activeTab.kind === "part"
        ? activeTab
        : project.tabs.find(
            (t): t is PartTab => t.kind === "part",
          );
    if (!partTab) {
      partTab = createPartTab("Part 1");
      project.tabs.push(partTab);
    }
    this.kernel.loadJson(
      injectScripts(partTab.documentJson, projectScripts(project)),
    );
    this.loadedPartTabId = partTab.id;

    this.state = {
      ...this.state,
      project,
      assembly:
        activeTab && activeTab.kind === "assembly"
          ? { active: true, document: activeTab.document, selectedPartId: null }
          : INACTIVE_ASSEMBLY,
      // Ticket 5zd3: loading a project replaces the scene wholesale — drop
      // any active sketch session so a stale sketch view can't survive.
      sketchSession: INACTIVE_SKETCH_SESSION,
      // …and for the same reason drop the meshes: they are the PREVIOUS
      // document's geometry. The commit below republishes them (worker mode:
      // asynchronously; fallback: synchronously in refreshMeshCache). Leaving
      // them meant every consumer of `meshedBodies` saw the old scene for a
      // moment — the project thumbnail captured in that window showed the
      // project you came FROM (ticket #343: an Onshape import wearing the
      // previous project's cube).
      meshedBodies: [],
    };
    this.core.commit(new Set(), null);
    this.core.autosave.flush();
    // Ticket pxff: restore the saved viewpoint (if any) onto the live camera.
    requestCameraRestore(project.view ?? null);
    // Restore the project's debug-tool state (dialogs, overlays).
    applyProjectDebug(project.debug);
    return true;
  }

  /** M12: set the server-side project ID (activates sync). */
  setProjectId(id: string | null): void {
    persistServerBinding(id);
    this.state = {
      ...this.state,
      projectId: id,
      syncState: id ? "dirty" : "offline",
    };
    this.core.emit();
  }

  /** WP3: the live workspace serialized as the full multi-tab Project blob —
   *  the same payload localStorage and `.fdraft` carry. This is what a
   *  server-side project's `document` field stores, so opening a server
   *  project restores the WHOLE workspace (all tabs), not just one part. */
  serializeProjectJson(): string {
    return this.serializeProject();
  }

  /** P2: re-inline a server document's externalized asset blobs (meshes/fonts)
   *  back into a full project JSON, ready for {@link loadProjectFromJson}. The
   *  server stores a lean ENVELOPE (`{ v, doc, fonts }`) plus content-addressed
   *  blobs fetched via {@link serverBlobStore}; a LEGACY full document (no
   *  envelope, no `__asset` refs) round-trips unchanged (no-op). */
  async inlineServerDocument(documentJson: string): Promise<string> {
    const trimmed = documentJson.trim();
    // Empty/new server project — nothing to inline.
    if (trimmed === "" || trimmed === "{}" || trimmed === "null") return documentJson;
    return inlineProject(documentJson, serverBlobStore);
  }

  /** WP3: load a server project into the live workspace and bind it for sync.
   *  `documentJson` is the lean ENVELOPE (or legacy full Project blob) previously
   *  saved via {@link syncToServer}; its externalized mesh/font blobs are
   *  re-inlined from the server first (P2). A freshly-created server project
   *  carries an empty document (`{}` / `""`) — that's not a valid Project blob,
   *  so we start a fresh default workspace and bind the id instead of failing.
   *  Marks the session clean (just-loaded), so the next edit flips it to
   *  "dirty". Returns `false` only on a genuinely malformed (non-empty) blob. */
  async loadServerProject(id: string, documentJson: string): Promise<boolean> {
    const inlined = await this.inlineServerDocument(documentJson);
    const trimmed = inlined.trim();
    const isEmpty = trimmed === "" || trimmed === "{}" || trimmed === "null";
    if (isEmpty) {
      this.newProject();
    } else if (!this.loadProjectFromJson(inlined)) {
      return false;
    }
    persistServerBinding(id);
    this.state = {
      ...this.state,
      projectId: id,
      syncState: "clean",
    };
    this.core.emit();
    return true;
  }

  /** WP3: detach from any server project, returning to guest/offline mode
   *  (e.g. after the bound project is deleted, or on logout). The live
   *  workspace document is left untouched. */
  detachServerProject(): void {
    persistServerBinding(null);
    if (this.state.projectId === null && this.state.syncState === "offline") {
      return;
    }
    this.state = {
      ...this.state,
      projectId: null,
      syncState: "offline",
    };
    this.core.emit();
  }

  /** M12 + P2: sync the current workspace to the bound server project. No-op in
   *  offline mode. Externalizes the workspace to a LEAN document — embedded
   *  meshes and uploaded-font bytes are uploaded as content-addressed blobs
   *  ({@link serverBlobStore}, upload-if-missing) and the PATCH body carries only
   *  features + asset hashes. An ASSET-FREE workspace is sent as the plain
   *  Project blob (no envelope) so publish/gallery-copy keep the exact document
   *  shape they load today; only asset-bearing projects get the `{ v, doc, fonts }`
   *  envelope.
   *
   *  P3b: `opts.keepalive` sets `keepalive: true` on the document PATCH so it can
   *  outlive a `beforeunload` (browsers keep keepalive requests in flight as the
   *  page unloads). Only the small doc PATCH gets it — the preceding blob uploads
   *  are ordinary fetches; on a real unload they have usually already run via the
   *  periodic/exit triggers (IndexedDB is the safety net if not). */
  async syncToServer(opts: { keepalive?: boolean } = {}): Promise<boolean> {
    if (!this.state.projectId || this.state.syncState === "offline") {
      return false;
    }
    this.patch({ syncState: "syncing" });
    this.core.emit();
    try {
      // Uploads every missing mesh/font blob, returns the lean envelope string.
      const envelopeJson = await externalizeProject(
        this.serializeProject(),
        serverBlobStore,
      );
      const envelope = JSON.parse(envelopeJson) as {
        doc: string;
        fonts: Record<string, string>;
      };
      // Asset-free ⇒ store the inner project blob verbatim (viewer/gallery-copy
      // load it directly, unwrapped-envelope not required); asset-bearing ⇒ the
      // full envelope, which the cloud-load path re-inlines. The `__asset` marker
      // lives inside each part tab's nested documentJson string, so it is
      // double-escaped in `doc` — match the bare marker (never the quoted form).
      const hasAssets =
        Object.keys(envelope.fonts).length > 0 ||
        envelope.doc.includes("__asset");
      const document = JSON.parse(hasAssets ? envelopeJson : envelope.doc);
      const res = await fetch(`/api/projects/${this.state.projectId}`, {
        method: "PATCH",
        credentials: "include",
        keepalive: opts.keepalive ?? false,
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ document }),
      });
      if (!res.ok) throw new Error(`sync failed: ${res.status}`);
      this.patch({ syncState: "clean" });
      this.core.emit();
      return true;
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: sync failed", err);
      this.patch({ syncState: "error" });
      this.core.emit();
      return false;
    }
  }

  /** MT-A: reset to a fresh default project (one empty part tab). */
  newProject(): void {
    const project = createDefaultProject();
    reseedTabIds(project.tabs);
    for (const r of this.kernel.listScripts()) this.kernel.removeScript(r.id);
    const partTab = project.tabs[0] as PartTab;
    this.kernel.loadJson(injectScripts(partTab.documentJson, []));
    this.loadedPartTabId = partTab.id;
    this.state = {
      ...this.state,
      project,
      assembly: INACTIVE_ASSEMBLY,
      // Ticket 5zd3: a fresh project must show no sketch view — drop any
      // lingering session (e.g. after deleting the open sketch the session
      // could otherwise survive into the new empty project).
      sketchSession: INACTIVE_SKETCH_SESSION,
    };
    this.core.commit(new Set(), null);
    this.core.autosave.flush();
  }

  async publishProject(): Promise<string | null> {
    if (!this.state.projectId) return null;
    try {
      // docs/PROJECT-ASSET-STORAGE-PLAN §6: publish a SELF-CONTAINED snapshot.
      // The server-stored `document` is LEAN (assets externalized to
      // auth-gated blobs), but the public viewer is unauthenticated and cannot
      // fetch those blobs. `serializeProject()` still yields the FULL document
      // (meshes/fonts inline — the kernel holds them), so we send it as the
      // snapshot source; the viewer then loads it directly, no blob fetch.
      const document = JSON.parse(this.serializeProject());
      const resp = await fetch(
        `/api/projects/${this.state.projectId}/publish`,
        {
          method: "POST",
          credentials: "include",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ document }),
        },
      );
      if (!resp.ok) return null;
      const data = await resp.json();
      const viewUrl = data.view_url ?? `/api/v/${data.snapshot_id}`;
      const embed = `<iframe src="${window.location.origin}${viewUrl}" width="640" height="480" frameborder="0" allow="webgpu"></iframe>`;
      try {
        await navigator.clipboard.writeText(embed);
      } catch {
        prompt("Share embed code:", embed);
      }
      return data.snapshot_id;
    } catch {
      return null;
    }
  }

  // ── MT-A: project / multi-tab management ──────────────────────

  /** Flush the live kernel document back into the loaded part tab. The
   *  active assembly tab is always current (assembly edits write through
   *  immediately), so there is nothing to flush for it.
   *  Pure state shuffle — does NOT notify. */
  private flushLoadedTab(): void {
    const tabs = this.state.project.tabs.map((t) => {
      if (t.id === this.loadedPartTabId && t.kind === "part") {
        // PERSISTENT-NAMING N4: normalise stored element references (fresh
        // hash + fingerprint) right before the document is persisted, so a
        // healed reference never decays across successive edits. Best-effort:
        // a save must never fail on healing.
        try {
          const healed = this.kernel.healElementRefs();
          if (healed.length > 0)
            console.info("[fielddraft] healed element refs on save:", healed);
        } catch {
          // ignore — healing is a normalisation, not a save precondition
        }
        return { ...t, documentJson: stripScripts(this.kernel.toJson()) };
      }
      return t;
    });
    this.state = {
      ...this.state,
      project: { ...this.state.project, tabs },
    };
  }

  /** MT-A: switch the active tab by id. Flushes the currently-loaded
   *  part (and active assembly) first, then loads / activates the
   *  target. Switching to a part tab resets undo (intended). */
  setActiveTab(id: string): void {
    const project = this.state.project;
    if (id === project.activeTabId) return;
    const target = project.tabs.find((t) => t.id === id);
    if (!target) return;

    this.flushLoadedTab();

    if (target.kind === "part") {
      this.kernel.loadJson(
        injectScripts(target.documentJson, projectScripts(this.state.project)),
      );
      this.loadedPartTabId = target.id;
      this.state = {
        ...this.state,
        assembly: INACTIVE_ASSEMBLY,
        project: { ...this.state.project, activeTabId: id },
      };
      // Standard commit path refreshes features / meshes / selection.
      this.core.commit(new Set(), null);
      return;
    }

    if (target.kind === "script") {
      this.state = {
        ...this.state,
        assembly: INACTIVE_ASSEMBLY,
        project: { ...this.state.project, activeTabId: id },
      };
      this.notify();
      this.core.scheduleAutosave();
      return;
    }

    // assembly
    this.state = {
      ...this.state,
      assembly: {
        active: true,
        document: target.document,
        selectedPartId: null,
      },
      project: { ...this.state.project, activeTabId: id },
    };
    this.notify();
    this.core.scheduleAutosave();
  }

  /** MT-A: create a new tab of the given kind, activate it, return its
   *  id. Flushes the currently-loaded part first. */
  createTab(kind: TabKind): string {
    this.flushLoadedTab();
    const project = this.state.project;
    let newTab: ProjectTab;

    if (kind === "part") {
      const maxN = project.tabs.reduce((max, t) => {
        const m = /^Part (\d+)$/.exec(t.name);
        return m ? Math.max(max, Number(m[1])) : max;
      }, 0);
      newTab = createPartTab(`Part ${maxN + 1}`);
    } else if (kind === "assembly") {
      const maxN = project.tabs.reduce((max, t) => {
        const m = /^Assembly (\d+)$/.exec(t.name);
        return m ? Math.max(max, Number(m[1])) : max;
      }, 0);
      newTab = createAssemblyTab(`Assembly ${maxN + 1}`);
    } else {
      const id = this.deriveScriptId(undefined, projectScripts(project));
      const record = createRecord({ id, source: DEFAULT_SCRIPT_SOURCE });
      newTab = {
        id: allocateTabId(),
        kind: "script",
        name: record.name,
        record,
      };
      this.kernel.upsertScript(record);
    }

    this.state = {
      ...this.state,
      project: { ...project, tabs: [...project.tabs, newTab] },
      // A script tab mirrors a new record into the kernel; refresh the
      // toolbar meta now since the script-tab activation path below does
      // not run commit().
      scriptTypes: this.snapshotScriptTypes(),
    };
    this.setActiveTab(newTab.id);
    return newTab.id;
  }

  /** The current project's display name (shown next to the logo). */
  projectName(): string {
    return this.state.project.name;
  }

  /** Rename the whole project (Ticket C: editable title next to the logo).
   *  Trims; ignores an empty or unchanged name. Always updates local store
   *  state (and schedules the autosave). When the project is server-bound
   *  (`projectId` set) it ALSO persists via `PATCH /api/projects/{id}` with
   *  `{name}`; an unbound guest project renames locally only (the next sync
   *  carries the new name). A failed PATCH is swallowed — the local rename
   *  stands and the pending change syncs later. */
  renameProject(name: string): void {
    const trimmed = name.trim();
    if (trimmed.length === 0 || trimmed === this.state.project.name) return;
    this.state = {
      ...this.state,
      project: { ...this.state.project, name: trimmed },
      // A bound project now has an unsynced local change.
      syncState: this.state.projectId ? "dirty" : this.state.syncState,
    };
    this.notify();
    this.core.scheduleAutosave();

    const id = this.state.projectId;
    if (id) {
      void fetch(`/api/projects/${id}`, {
        method: "PATCH",
        credentials: "include",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name: trimmed }),
      }).catch((err) => {
        // eslint-disable-next-line no-console
        console.error("fielddraft: project rename sync failed", err);
      });
    }
  }

  /** MT-A: rename a tab (trim; ignore empty). For a script tab the
   *  record id is unchanged — only the tab label moves. */
  renameTab(id: string, name: string): void {
    const trimmed = name.trim();
    if (trimmed.length === 0) return;
    const tabs = this.state.project.tabs.map((t) =>
      t.id === id ? ({ ...t, name: trimmed } as ProjectTab) : t,
    );
    this.state = {
      ...this.state,
      project: { ...this.state.project, tabs },
    };
    this.notify();
    this.core.scheduleAutosave();
  }

  /** MT-A: delete a tab. Mirrors the kernel for script tabs, picks a
   *  fresh part to load when the loaded part is removed, and never
   *  leaves the tab list empty. */
  deleteTab(id: string): void {
    const project = this.state.project;
    const target = project.tabs.find((t) => t.id === id);
    if (!target) return;

    if (target.kind === "script") {
      this.kernel.removeScript(target.record.id);
    }

    let tabs = project.tabs.filter((t) => t.id !== id);
    const deletedIndex = project.tabs.findIndex((t) => t.id === id);

    // Ensure a part tab still exists to load against.
    const removedLoadedPart =
      target.kind === "part" && target.id === this.loadedPartTabId;
    if (removedLoadedPart) {
      let fallback = tabs.find(
        (t): t is PartTab => t.kind === "part",
      );
      if (!fallback) {
        fallback = createPartTab("Part 1");
        tabs = [...tabs, fallback];
      }
      this.kernel.loadJson(
        injectScripts(
          fallback.documentJson,
          projectScripts({ ...project, tabs }),
        ),
      );
      this.loadedPartTabId = fallback.id;
    }

    // Pick the next active tab if we deleted the active one.
    let activeTabId = project.activeTabId;
    if (id === project.activeTabId) {
      const neighbor = tabs[deletedIndex - 1] ?? tabs[deletedIndex] ?? tabs[0];
      activeTabId = neighbor.id;
    }

    this.state = {
      ...this.state,
      project: { ...this.state.project, tabs, activeTabId },
    };

    // If the active tab changed, route through setActiveTab semantics by
    // re-loading. Simplest: reset activeTabId then reuse setActiveTab when
    // the active tab actually moved. Here we instead resolve inline.
    const newActive = tabs.find((t) => t.id === activeTabId);
    if (newActive?.kind === "assembly") {
      this.state = {
        ...this.state,
        assembly: {
          active: true,
          document: newActive.document,
          selectedPartId: null,
        },
      };
    } else if (newActive?.kind === "part") {
      // Load the active part if it isn't already loaded.
      if (this.loadedPartTabId !== newActive.id) {
        this.kernel.loadJson(
          injectScripts(
            newActive.documentJson,
            projectScripts(this.state.project),
          ),
        );
        this.loadedPartTabId = newActive.id;
      }
      this.state = { ...this.state, assembly: INACTIVE_ASSEMBLY };
    } else {
      this.state = { ...this.state, assembly: INACTIVE_ASSEMBLY };
    }

    // A loaded-part regenerate happened; surface it through commit so
    // features/meshes/scriptTypes refresh and autosave persists.
    this.core.commit();
  }

  /** S4: the document's FieldSkript records (ground truth in the
   *  kernel's `Document.scripts`). */
  listScripts(): FieldSkriptRecord[] {
    return this.kernel.listScripts();
  }

  /**
   * MT-A: save the source of a script tab. Revises the tab's record
   * (keeps id, bumps version), updates tab name + label, mirrors into
   * the kernel, and commits so the live part regenerates + autosave
   * persists. Returns the new record or an error on malformed source.
   */
  saveScriptTab(
    tabId: string,
    source: string,
  ): { ok: true; record: FieldSkriptRecord } | { ok: false; error: string } {
    const tab = this.state.project.tabs.find(
      (t): t is ScriptTab => t.id === tabId && t.kind === "script",
    );
    if (!tab) return { ok: false, error: `Unknown script tab "${tabId}".` };

    let record: FieldSkriptRecord;
    try {
      record = reviseRecord(tab.record, { source });
    } catch (e) {
      return { ok: false, error: String(e instanceof Error ? e.message : e) };
    }

    const tabs = this.state.project.tabs.map((t) =>
      t.id === tabId && t.kind === "script"
        ? ({ ...t, record, name: record.name } as ProjectTab)
        : t,
    );
    this.state = {
      ...this.state,
      project: { ...this.state.project, tabs },
    };
    this.kernel.upsertScript(record);
    this.core.commit();
    return { ok: true, record };
  }

  /**
   * SI-W: instantiate a saved FieldSkript as a feature in the document.
   * Builds the script's default parameter definition from its schema and
   * adds a feature call keyed by the script id (its FeatureCall `type`).
   * On success commits + selects the new feature, mirroring `commitSketch`.
   * Returns the new feature id, or `null` on a missing record / kernel error.
   */
  insertScriptInstance(scriptId: string): number | null {
    const record = this.kernel.listScripts().find((r) => r.id === scriptId);
    if (!record) {
      this.core.setTransientError(`Unknown script "${scriptId}".`);
      return null;
    }
    const paramsJson = JSON.stringify(
      buildDefaultDefinition(parseFeatureSchema(record.schema)),
    );
    let id: number;
    try {
      id = this.kernel.addFeatureCall(scriptId, paramsJson);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.error("fielddraft: addFeatureCall failed", err);
      this.core.setTransientError(formatKernelError(err, "Script insert failed"));
      return null;
    }
    this.core.commit(new Set([id]), id);
    return id;
  }

  /** Derive a stable, deterministic script id from a name (or a
   *  positional fallback), slugged and de-duped against the existing
   *  records. No Date.now/Math.random — repeated saves are stable. */
  private deriveScriptId(
    name: string | undefined,
    existing: readonly FieldSkriptRecord[],
  ): string {
    const taken = new Set(existing.map((r) => r.id));
    const slug = (name ?? "")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "_")
      .replace(/^_+|_+$/g, "");
    const base = slug.length > 0 ? slug : `script_${existing.length + 1}`;
    if (!taken.has(base)) return base;
    let n = 2;
    while (taken.has(`${base}_${n}`)) n += 1;
    return `${base}_${n}`;
  }
}
