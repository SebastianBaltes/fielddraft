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
import { logUserAction } from "./userActionLog";
import {
  registerUploadedFontFromBytes,
  getUploadedFontBytes,
  UPLOADED_FONT_PREFIX,
} from "./sketchTextFont";
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
  serializeProjectToZip,
  parseProjectBytes,
  emptyProjectAssets,
  type ProjectAssets,
  stripScripts,
  type PartTab,
  type Project,
  type ProjectTab,
  type ScriptTab,
  type TabKind,
} from "./project";
import { parseFeatureSchema, buildDefaultDefinition } from "./widgets/schema";
import {
  StoreCore,
  formatKernelError,
  type StoreState,
} from "./storeCore";

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
        return { ...t, documentJson: stripScripts(this.kernel.toJson()) };
      }
      return t;
    });
    return { ...this.state.project, tabs };
  }

  /**
   * wu2v: collect the uploaded fonts actually referenced by a project.
   * Scans every part tab's document JSON for `sketch_path` sub-calls whose
   * `text.fontId` is an `uploaded:` id, then pulls the retained raw bytes.
   * Fonts with no retained bytes (e.g. opened from a legacy JSON that never
   * carried them) are skipped.
   */
  private collectProjectAssets(project: Project): ProjectAssets {
    const assets = emptyProjectAssets();
    const ids = new Set<string>();
    for (const tab of project.tabs) {
      if (tab.kind !== "part") continue;
      let doc: unknown;
      try {
        doc = JSON.parse(tab.documentJson);
      } catch {
        continue;
      }
      // Walk the doc collecting any `fontId` field that names an upload.
      const walk = (value: unknown): void => {
        if (Array.isArray(value)) {
          for (const v of value) walk(v);
          return;
        }
        if (value && typeof value === "object") {
          const obj = value as Record<string, unknown>;
          const fontId = obj.fontId;
          if (
            typeof fontId === "string" &&
            fontId.startsWith(UPLOADED_FONT_PREFIX)
          ) {
            ids.add(fontId);
          }
          for (const v of Object.values(obj)) walk(v);
        }
      };
      walk(doc);
    }
    for (const id of ids) {
      const bytes = getUploadedFontBytes(id);
      if (!bytes) continue;
      // Extension from the id tail (`uploaded:Foo.ttf` → `ttf`), default ttf.
      const dot = id.lastIndexOf(".");
      const ext = dot >= 0 ? id.slice(dot + 1).toLowerCase() : "ttf";
      assets.fonts.set(id, { bytes, ext });
    }
    return assets;
  }

  /** Public accessor for the live project as a `.fdraft` ZIP (document
   *  JSON + uploaded font assets), byte-identical to what Save writes.
   *  Used by the in-app bug-report flow (on3k) so a ticket carries the
   *  exact, openable project file — assets included — not a stripped
   *  JSON blob. */
  exportProjectFdraft(): Uint8Array {
    const project = this.buildPersistableProject();
    const assets = this.collectProjectAssets(project);
    return serializeProjectToZip(project, assets);
  }

  /** Save the current project as a downloadable .fdraft file.
   *
   *  wu2v: the file is a ZIP container (document.json + embedded uploaded
   *  font bytes + manifest) so binary assets survive a reload. Legacy JSON
   *  `.fdraft` files still open (the loader sniffs the ZIP magic bytes).
   *  The bug-report path uses the same ZIP — see exportProjectFdraft. */
  saveProjectToFile(filename = "project.fdraft"): void {
    if (typeof document === "undefined") return;
    const project = this.buildPersistableProject();
    const assets = this.collectProjectAssets(project);
    const zip = serializeProjectToZip(project, assets);
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
  loadProjectFromJson(json: string): boolean {
    return this.loadProjectFromBytes(new TextEncoder().encode(json));
  }

  /** wu2v: load a project from raw `.fdraft` bytes — a ZIP container or
   *  legacy JSON (auto-detected). Re-registers any embedded uploaded fonts
   *  so they render again, then applies the project. Returns `false` on
   *  malformed input. */
  loadProjectFromBytes(bytes: Uint8Array | ArrayBuffer): boolean {
    const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    const parsed = parseProjectBytes(u8);
    if (!parsed) return false;
    logUserAction("doc", "load-project", { bytes: u8.length });
    // Re-register embedded uploaded fonts before applying the project so the
    // font cache is warm when the scene regenerates its text geometry.
    for (const [id, { bytes: fontBytes }] of parsed.assets.fonts) {
      try {
        registerUploadedFontFromBytes(id, fontBytes);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.error("fielddraft: embedded font re-register failed", id, err);
      }
    }
    return this.applyLoadedProject(parsed.project);
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
    };
    this.core.commit(new Set(), null);
    this.core.autosave.flush();
    return true;
  }

  /** M12: set the server-side project ID (activates sync). */
  setProjectId(id: string | null): void {
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

  /** WP3: load a server project into the live workspace and bind it for sync.
   *  `documentJson` is the Project blob previously saved via
   *  {@link serializeProjectJson}. A freshly-created server project carries an
   *  empty document (`{}` / `""`) — that's not a valid Project blob, so we
   *  start a fresh default workspace and bind the id instead of failing. Marks
   *  the session clean (just-loaded), so the next edit flips it to "dirty".
   *  Returns `false` only on a genuinely malformed (non-empty) blob. */
  loadServerProject(id: string, documentJson: string): boolean {
    const trimmed = documentJson.trim();
    const isEmpty = trimmed === "" || trimmed === "{}" || trimmed === "null";
    if (isEmpty) {
      this.newProject();
    } else if (!this.loadProjectFromJson(documentJson)) {
      return false;
    }
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

  /** M12: sync the current workspace to the bound server project. No-op in
   *  offline mode. Sends the full Project blob (see
   *  {@link serializeProjectJson}). */
  async syncToServer(): Promise<boolean> {
    if (!this.state.projectId || this.state.syncState === "offline") {
      return false;
    }
    this.patch({ syncState: "syncing" });
    this.core.emit();
    try {
      const res = await fetch(`/api/projects/${this.state.projectId}`, {
        method: "PATCH",
        credentials: "include",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          document: JSON.parse(this.serializeProject()),
        }),
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
      const resp = await fetch(
        `/api/projects/${this.state.projectId}/publish`,
        {
          method: "POST",
          credentials: "include",
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
      this.core.autosave.schedule(this.serializeProject());
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
    this.core.autosave.schedule(this.serializeProject());
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
    this.core.autosave.schedule(this.serializeProject());

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
    this.core.autosave.schedule(this.serializeProject());
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
