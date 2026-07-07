import { StrictMode, useCallback, useEffect, useRef, useState } from "react";
import { Outliner } from "./Outliner";
import { Toolbar } from "./Toolbar";
import { Viewport } from "./Viewport";
import { FieldSkriptEditor } from "./FieldSkriptEditor";
import { BottomTabs } from "./BottomTabs";
import { useKernelStore } from "./store";
import { dispatchKeyboardCommand } from "./keyboardCommands";
import { SettingsMenu } from "./SettingsMenu";
import { loadSettings, WorkspaceUnitsContext } from "./settings";
import type { SettingsState } from "./settings";
import { ProjectTitle } from "./ProjectTitle";
import { CONTROLS_PRESETS, viewShortcutRow } from "./controlsPreset";
import {
  useAccount,
  accountErrorMessage,
  type UseAccountOptions,
} from "./account/useAccount";
import type { UseAccount } from "./account/useAccount";
import { UserMenu } from "./account/UserMenu";
import { Bell } from "./account/Bell";
import { Overview } from "./account/Overview";
import { AuthScreen } from "./account/AuthScreen";
import { AuthGate } from "./account/AuthGate";
import { InviteRegister } from "./account/InviteRegister";
import { GuestImportBanner } from "./account/GuestImportBanner";
import { shouldOfferGuestImport } from "./account/guestImport";
import {
  loadStoredServerBinding,
  clearStoredServerBinding,
} from "./projectStore";
import { platformApi, type Project as ServerProject } from "./api/platform";
import { parseHash, routeToHash } from "./account/hashRoute";
import {
  isLocalProjectsSupported,
  localProjectHandle,
  rememberLocalProject,
  type FsaFileHandle,
} from "./localProjects";
import {
  bindLocalFile,
  clearLocalFileBinding,
} from "./localFileBridge";

// WP11: the deployed build gates the whole app behind login. The gate is a
// build flag (default OFF) so local `just dev`, the Playwright e2e and offline
// modeling stay ungated; the production config (vite.config.prod.ts) bakes it
// ON, making the shipped image unusable without a session.
//
// Session persistence is the existing HttpOnly, 7-day session cookie — sent
// automatically, restored by `getMe()` on boot. We deliberately do NOT mirror
// the session/token into localStorage: an HttpOnly cookie is unreadable by
// page JS, so an XSS payload cannot exfiltrate it; a token in localStorage
// would be trivially stealable. No extra persistence code is needed beyond the
// `/api/me` boot call already in useAccount.
//
// Read at render time (not a module-level const) so the value is the live
// `import.meta.env` — keeps the flag stubbable in tests and dead-code-eliminated
// to a literal in the prod build (vite.config.prod.ts bakes "1").
function requireLogin(): boolean {
  return import.meta.env.VITE_REQUIRE_LOGIN === "1";
}

/**
 * App — the gate boundary. Owns the account session (one `/api/me` per load)
 * and, when the login gate is on, decides whether the modeling app mounts at
 * all:
 *   - session still resolving → a small full-page loading state,
 *   - no session (incl. network error → treated as not authed) → ONLY the
 *     full-page AuthGate; none of the modeling app / toolbar / viewport /
 *     dashboard / account button mounts,
 *   - authed → the full app (AppShell) exactly as before.
 * After login/register the session flips to authed and the app mounts; after
 * logout it returns to the gate.
 *
 * When the gate is off (default) boot is unchanged: AppShell mounts immediately
 * and account features stay optional.
 *
 * `accountOptions` is a test seam — production passes nothing, so useAccount
 * hits the real `/api/me`; tests inject a fake api + search.
 */
export function App({
  accountOptions,
}: {
  accountOptions?: UseAccountOptions;
} = {}) {
  const account = useAccount(accountOptions);

  // Ticket A: an `?invite=<token>` link always lands on the self-registration
  // form, ahead of BOTH the login gate and the mounted app — even for an
  // already-authenticated visitor (registering issues a fresh session cookie,
  // so they become the new user). The prefill is burned on a successful
  // register, which drops us straight into the app as that user.
  if (account.state.invitePrefill) {
    return (
      <InviteRegister account={account} token={account.state.invitePrefill} />
    );
  }

  if (requireLogin()) {
    if (account.state.status === "loading") {
      return (
        <div className="fd-auth-loading" data-testid="auth-gate-loading">
          <div className="spinner" />
        </div>
      );
    }
    if (account.state.status !== "authed") {
      return <AuthGate account={account} />;
    }
  }

  return <AppShell account={account} />;
}

function AppShell({ account }: { account: UseAccount }) {
  const { store, state, error } = useKernelStore();
  const [showHelp, setShowHelp] = useState(false);
  // Mobile-only: the outliner is an off-canvas drawer toggled from the
  // topbar. On desktop the toggle/scrim are hidden by CSS and this stays
  // a no-op.
  const [mobileNavOpen, setMobileNavOpen] = useState(false);
  const [settings, setSettings] = useState<SettingsState>(() => loadSettings());

  // WP3: account session (login/dashboard/gallery/invites). Additive — guest
  // mode stays the default; the account UI surfaces on demand. The session is
  // owned by the outer App (one `/api/me` per load) and passed in, so the WP11
  // login gate and this shell share exactly one resolution.

  // Bridge: load a server project's document into the live workspace, bind it
  // for sync, and enter the modeling view. The server stores the full multi-tab
  // Project blob, so this restores the whole workspace.
  const openServerProject = (project: ServerProject) => {
    if (!store) return;
    const json =
      typeof project.document === "string"
        ? project.document
        : JSON.stringify(project.document ?? {});
    // Ticket 6398…: a malformed server blob must NOT silently enter the
    // modeling view with the previous (guest) workspace unbound — that
    // masquerades as the wrong project and downgrades export to "save to
    // server first". Surface the failure and stay on the dashboard.
    if (!store.loadServerProject(project.id, json)) {
      store.showTransientError(
        `“${project.name}” could not be opened — its stored document is not a valid project.`,
      );
      return;
    }
    clearLocalFileBinding();
    account.enterProject();
  };

  // Bridge: open a public/gallery project as a fresh UNBOUND copy (no server
  // binding — it's not the viewer's project) and enter the modeling view.
  const openServerCopy = (project: ServerProject) => {
    if (!store) return;
    const json =
      typeof project.document === "string"
        ? project.document
        : JSON.stringify(project.document ?? {});
    const trimmed = json.trim();
    if (trimmed === "" || trimmed === "{}" || trimmed === "null") {
      store.newProject();
      store.detachServerProject();
    } else if (store.loadProjectFromJson(json)) {
      store.detachServerProject();
    }
    clearLocalFileBinding();
    account.enterProject();
  };

  // Bridge: load a ticket's `.fdraft` artifact (raw ZIP bytes) into the live
  // workspace as a fresh UNBOUND copy — it's repro material, not the viewer's
  // server project, so we don't bind it for sync — and enter the modeling view.
  const loadFdraftBytes = (bytes: ArrayBuffer) => {
    if (!store) return;
    if (store.loadProjectFromBytes(bytes)) {
      store.detachServerProject();
    }
    clearLocalFileBinding();
    account.enterProject();
  };

  // ── Local projects (ticket 14661fc4) ────────────────────────────
  // Open a local .fdraft through the File System Access API: the handle is
  // remembered in IndexedDB (the Projects overview lists it for reopening)
  // and, with readwrite permission, bound so the autosave mirrors the ZIP
  // back to disk. Falls back to the classic hidden <input type=file> where
  // the API is unavailable (then: no listing, no disk autosave).
  const finishLocalOpen = async (handle: FsaFileHandle, writable: boolean) => {
    if (!store) return;
    const file = await handle.getFile();
    const buf = await file.arrayBuffer();
    clearLocalFileBinding();
    store.detachServerProject();
    if (!store.openUserProjectFromBytes(buf)) {
      store.showTransientError(
        `“${file.name}” is not a valid FieldDraft project.`,
      );
      return;
    }
    void rememberLocalProject(handle).catch(() => {});
    if (writable) {
      bindLocalFile(handle, () => store.exportProjectFdraft());
    }
    account.enterProject();
  };

  const openLocalViaPicker = () => {
    if (!store) return;
    if (!isLocalProjectsSupported()) {
      document
        .querySelector<HTMLInputElement>('[data-testid="load-project-input"]')
        ?.click();
      return;
    }
    void (async () => {
      let handle: FsaFileHandle;
      try {
        const picked = await (
          window as unknown as {
            showOpenFilePicker(opts?: unknown): Promise<FsaFileHandle[]>;
          }
        ).showOpenFilePicker({
          types: [
            {
              description: "FieldDraft project",
              accept: {
                "application/zip": [".fdraft"],
                "application/json": [".json"],
              },
            },
          ],
        });
        handle = picked[0];
      } catch {
        return; // picker cancelled
      }
      // Ask for write access right away (still within the picker's user
      // activation) so the autosave can mirror to disk. Read-only grant
      // still opens the project — it just won't autosave to the file.
      let writable = false;
      try {
        writable =
          typeof handle.requestPermission !== "function" ||
          (await handle.requestPermission({ mode: "readwrite" })) ===
            "granted";
      } catch {
        writable = false;
      }
      await finishLocalOpen(handle, writable);
    })();
  };

  const openLocalProjectById = (id: string) => {
    if (!store) return;
    void (async () => {
      const handle = await localProjectHandle(id);
      if (!handle) {
        store.showTransientError(
          "This local project is no longer accessible — open it once via “Open local project…” again.",
        );
        return;
      }
      await finishLocalOpen(handle, true);
    })();
  };

  const currentDocument = (): unknown | null => {
    if (!store) return null;
    try {
      return JSON.parse(store.serializeProjectJson());
    } catch {
      return null;
    }
  };

  // Ticket 6398…: rebind the server project after a page reload. The
  // localStorage autosave restores the DOCUMENT; this restores the sync
  // binding, which otherwise silently degrades to guest mode ("save to
  // server first" on CGAL export despite an open server project). The stored
  // id is verified against the server first — a deleted/foreign project
  // clears it instead of binding. setProjectId marks the session "dirty", so
  // the next autosave pushes the (autosave-identical) local doc back up.
  useEffect(() => {
    if (!store || account.state.status !== "authed") return;
    if (state?.projectId) return;
    const storedId = loadStoredServerBinding();
    if (!storedId) return;
    let cancelled = false;
    platformApi.getProject(storedId).then(
      (p) => {
        if (!cancelled) store.setProjectId(p.id);
      },
      () => {
        if (!cancelled) clearStoredServerBinding();
      },
    );
    return () => {
      cancelled = true;
    };
  }, [store, account.state.status, state?.projectId]);

  // WP9 B4: first-login guest-project import. When a guest who has done real
  // local work logs in (or registers) for the first time, offer — once — to
  // import their localStorage guest project as a new server project. Declining
  // keeps the guest project intact; guest mode is untouched when logged out.
  const [importPrompt, setImportPrompt] = useState<"hidden" | "offer">(
    "hidden",
  );
  // Guard so the offer is made at most once per app session (not re-shown on
  // every re-render after dismissal).
  const [importDecided, setImportDecided] = useState(false);

  useEffect(() => {
    if (importDecided || importPrompt !== "hidden") return;
    if (!store) return;
    const offer = shouldOfferGuestImport({
      authed: account.state.status === "authed",
      // A stored (not-yet-rehydrated) binding counts as bound — the async
      // rebind above must not race a spurious "import your guest project?".
      boundProjectId: state?.projectId ?? loadStoredServerBinding(),
      guestDocument: currentDocument(),
    });
    if (offer) setImportPrompt("offer");
    // currentDocument reads the live store; depend on the inputs that flip the
    // decision (auth status + bound id), not the function identity.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    account.state.status,
    state?.projectId,
    store,
    importDecided,
    importPrompt,
  ]);

  const importGuestProject = async () => {
    if (!store) return;
    const doc = currentDocument();
    const created = await platformApi.createProject("My project", doc ?? {});
    // Bind the freshly-created project to the live workspace (it IS the current
    // workspace) without reloading — same as the dashboard "save current" path.
    store.setProjectId(created.id);
    setImportPrompt("hidden");
    setImportDecided(true);
  };

  const dismissGuestImport = () => {
    setImportPrompt("hidden");
    setImportDecided(true);
  };

  const compact = settings.compact;

  useEffect(() => {
    document.documentElement.dataset.theme = settings.theme;
  }, [settings.theme]);

  const isMeshMode = settings.renderMode === "mesh";
  // The WebGL fallback (render mode "webgl") draws the kernel's Dual-Contouring
  // meshes so booleans are visible — those only exist while DC meshing is on, so
  // enable it for the WebGL path too (ticket gcvz).
  const webGLFallbackActive = settings.renderMode === "webgl";
  useEffect(() => {
    if (store)
      store.setDcMeshingEnabled(
        isMeshMode || webGLFallbackActive,
        settings.meshAlgorithm,
      );
  }, [store, isMeshMode, webGLFallbackActive, settings.meshAlgorithm]);

  useEffect(() => {
    if (store) store.setParametricWeave(settings.parametricWeave);
  }, [store, settings.parametricWeave]);

  useEffect(() => {
    if (store) store.setDragBrepRecompute(settings.dragBrepRecompute);
  }, [store, settings.dragBrepRecompute]);

  useEffect(() => {
    if (store) store.setLiveExtrudeDrag(settings.liveExtrudeDrag);
  }, [store, settings.liveExtrudeDrag]);

  // WP4: the Report dialog (owned by SettingsMenu) emits `fd:open-ticket` with
  // the created ticket id after a successful submit — open it in the tickets
  // overlay. Decoupled via a window event so the shell isn't restructured.
  useEffect(() => {
    const onOpenTicket = (e: Event) => {
      const id = (e as CustomEvent<string>).detail;
      account.openTicket(typeof id === "string" && id ? id : null);
    };
    window.addEventListener("fd:open-ticket", onOpenTicket);
    return () => window.removeEventListener("fd:open-ticket", onOpenTicket);
  }, [account]);

  // Global keyboard shortcuts. The routing table (key→command + context
  // gates: text-entry guard, script-tab gate, sketch-session scoping,
  // preset view keys) lives in keyboardCommands.ts — the shell only
  // registers the listener and supplies the context.
  useEffect(() => {
    if (!store) return;
    const onKey = (e: KeyboardEvent) =>
      dispatchKeyboardCommand(e, {
        store,
        controlsPreset: settings.controlsPreset,
        toggleHelp: () => setShowHelp((v) => !v),
      });
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [store, settings.controlsPreset]);

  const activeProjectTab = state?.project.tabs.find(
    (t) => t.id === state.project.activeTabId,
  );
  const activeScriptTab =
    activeProjectTab?.kind === "script" ? activeProjectTab : null;

  // WP13: two-level nav. `overview` = the tabbed browser (default landing);
  // `project` = the modeling view. The topbar is a persistent shell element on
  // both; the modeling chrome (toolbar/viewport/outliner/meta) mounts only in
  // the project view.
  const inProject = account.state.page === "project";

  // ── URL-hash routing (ticket 3032c29b) ─────────────────────────────
  // Issues and projects have URLs; browser Back works. The hash is a pure
  // projection of {page, overviewTab, ticketDetail, projectId}; see
  // hashRoute.ts for the shapes. Two directions, both idempotent:

  // hash → state. Applied on hashchange (Back/Forward, pasted URLs) and once
  // on boot after the session resolves. A #/p/<id> deep link to a project
  // that is not currently bound loads it from the server first.
  const applyRoute = useCallback(() => {
    const route = parseHash(window.location.hash);
    if (!route) return;
    if (route.page === "project") {
      const boundId = store?.getState().projectId ?? null;
      if (route.projectId && route.projectId !== boundId) {
        platformApi.getProject(route.projectId).then(
          (p) => openServerProjectRef.current(p),
          () => account.showOverview("projects"),
        );
      } else {
        account.enterProject();
      }
    } else if (route.tab === "issues") {
      account.openTicket(route.ticketId);
    } else {
      account.selectTab(route.tab);
    }
    // account methods are stable useCallbacks; store is a stable singleton.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [account.enterProject, account.openTicket, account.selectTab, account.showOverview, store]);

  // openServerProject is redefined per render (it closes over `account`);
  // the route handler reads the latest through a ref.
  const openServerProjectRef = useRef(openServerProject);
  openServerProjectRef.current = openServerProject;

  useEffect(() => {
    window.addEventListener("hashchange", applyRoute);
    return () => window.removeEventListener("hashchange", applyRoute);
  }, [applyRoute]);

  // Boot deep link: apply the initial hash once the session has resolved
  // (session-resolved may flip the landing page right before this).
  const bootRouteApplied = useRef(false);
  useEffect(() => {
    if (account.state.status === "loading" || bootRouteApplied.current) return;
    bootRouteApplied.current = true;
    applyRoute();
  }, [account.state.status, applyRoute]);

  // state → hash. Every navigation writes a history entry (Back walks the
  // app history); the very first write only normalizes the URL in place.
  const firstHashWrite = useRef(true);
  useEffect(() => {
    if (account.state.status === "loading") return;
    const hash = routeToHash(
      inProject
        ? { page: "project", projectId: state?.projectId ?? null }
        : {
            page: "overview",
            tab: account.state.overviewTab,
            ticketId: account.state.ticketDetail,
          },
    );
    if (window.location.hash === hash) {
      firstHashWrite.current = false;
      return;
    }
    if (firstHashWrite.current) {
      firstHashWrite.current = false;
      // Don't clobber a deep link before the boot apply consumed it.
      if (parseHash(window.location.hash)) return;
      window.history.replaceState(null, "", hash);
    } else {
      window.location.hash = hash;
    }
  }, [
    account.state.status,
    account.state.overviewTab,
    account.state.ticketDetail,
    inProject,
    state?.projectId,
  ]);

  return (
    <WorkspaceUnitsContext.Provider value={settings.units}>
      <div className={"fd-shell" + (compact ? " fd-compact" : "")}>
        {/* StrictMode wraps the UI chrome but NOT the 3D <Viewport> below — its
          dev double-mount breaks the R3F canvas (blank-until-resize) and
          double-inits the renderer. See main.tsx. */}
        <StrictMode>
          <header className="topbar">
            <div className="title">
              {inProject ? (
                <button
                  type="button"
                  className="back-to-overview"
                  data-testid="back-to-overview"
                  aria-label="Back to overview"
                  title="Back to overview"
                  onClick={() => account.showOverview("projects")}
                >
                  ◀
                </button>
              ) : null}
              FieldDraft{" "}
              <button
                type="button"
                className="version"
                title="Release Notes anzeigen"
                data-testid="version-badge"
                onClick={() => window.dispatchEvent(new Event("fd:release-notes"))}
              >
                v{__APP_VERSION__}
              </button>
              <SettingsMenu
                store={store}
                state={state}
                settings={settings}
                onSettingsChange={setSettings}
                inProject={inProject}
                onEnterProject={() => account.enterProject()}
                onOpenLocal={openLocalViaPicker}
                onSaveToCloud={
                  account.state.status === "authed" &&
                  store &&
                  state &&
                  state.projectId === null
                    ? () => {
                        void (async () => {
                          const doc = currentDocument();
                          const created = await platformApi.createProject(
                            state.project.name?.trim() || "Untitled",
                            doc ?? {},
                          );
                          store.setProjectId(created.id);
                        })().catch((err) => {
                          store.showTransientError(
                            accountErrorMessage(
                              err,
                              "Could not save to the cloud",
                            ),
                          );
                        });
                      }
                    : null
                }
              />
              {/* Ticket C: the current project's name next to the logo — click
                  to rename inline (modeling view only). */}
              {inProject && store && state ? (
                <ProjectTitle store={store} name={state.project.name} />
              ) : null}
            </div>
            {inProject ? (
              <div className="meta">
                {error ? (
                  <span className="error">kernel error: {error.message}</span>
                ) : state ? (
                  <>
                    <span data-testid="feature-count">
                      {state.features.length} feature
                      {state.features.length === 1 ? "" : "s"}
                    </span>
                    {isMeshMode && state.meshedBodies.length > 0 ? (
                      <span className="tri-count" data-testid="tri-count">
                        {state.meshedBodies
                          .reduce((sum, b) => sum + b.indices.length / 3, 0)
                          .toLocaleString()}{" "}
                        △
                      </span>
                    ) : null}
                    {state.sceneSize[0] > 0 ||
                    state.sceneSize[1] > 0 ||
                    state.sceneSize[2] > 0 ? (
                      <span className="scene-size" data-testid="scene-size">
                        {state.sceneSize[0].toFixed(1)} ×{" "}
                        {state.sceneSize[1].toFixed(1)} ×{" "}
                        {state.sceneSize[2].toFixed(1)} mm
                      </span>
                    ) : null}
                    {state.transientError ? (
                      <span
                        className="transient-error"
                        data-testid="transient-error"
                        role="alert"
                      >
                        ⚠ {state.transientError}
                      </span>
                    ) : null}
                  </>
                ) : (
                  <span>loading…</span>
                )}
              </div>
            ) : (
              <div className="topbar-spacer">
                {/* Dashboard-side surface for store errors (e.g. a server
                    project whose blob failed to open, ticket 6398…). */}
                {state?.transientError ? (
                  <span
                    className="transient-error"
                    data-testid="transient-error"
                    role="alert"
                  >
                    ⚠ {state.transientError}
                  </span>
                ) : null}
              </div>
            )}
            <Bell account={account} />
            <UserMenu
              account={account}
              onLoggedOut={() => store?.detachServerProject()}
            />
            {/* Ticket qh4l: the feature-panel toggle lives on the RIGHT (where the
            panel docks) and reads as a collapse chevron, not a third ☰ that
            looks identical to the settings menu. ◀ pulls the panel out, ▶
            collapses it. Modeling-view only. */}
            {inProject ? (
              <button
                type="button"
                className="nav-toggle"
                data-testid="mobile-nav-toggle"
                aria-label={mobileNavOpen ? "Collapse panel" : "Expand panel"}
                aria-expanded={mobileNavOpen}
                onClick={() => setMobileNavOpen((v) => !v)}
              >
                {mobileNavOpen ? "▶" : "◀"}
              </button>
            ) : null}
          </header>
        </StrictMode>
        {/* WP13: the Overview (main level). Rendered only when NOT in the
          modeling view. Kept OUTSIDE StrictMode's double-mount to avoid
          duplicate `/api/me`-driven effects in its data-fetching children. */}
        {!inProject ? (
          <Overview
            account={account}
            onOpenProject={openServerProject}
            onOpenCopy={openServerCopy}
            onOpenLocalProject={openLocalProjectById}
            onLoadFdraft={loadFdraftBytes}
            boundProjectId={state?.projectId ?? null}
          />
        ) : null}
        {/* MT-A: both workspaces stay mounted; we toggle visibility with
          the `hidden` attr so the Viewport's GPU context and Monaco are
          never torn down on a tab switch. The active tab is derived from
          the project; part AND assembly both use the part workspace
          (Viewport + Outliner render the assembly when active). The whole
          modeling stack is hidden on the Overview so the viewport GPU
          context survives the round-trip. */}
        <div
          className="fd-workspace"
          hidden={!inProject || activeProjectTab?.kind === "script"}
        >
          {store && state ? (
            <StrictMode>
              <Toolbar store={store} state={state} />
            </StrictMode>
          ) : null}
          <main className="main">
            <Viewport
              renderMode={settings.renderMode}
              brepEdgeMode={settings.brepEdgeMode}
              frameLoop={settings.frameLoop}
              showEdgePoints={settings.showEdgePoints}
              showOctreeCells={settings.showOctreeCells}
              screenSpaceCsg={settings.screenSpaceCsg}
              perspective={settings.perspective}
              controlsPreset={settings.controlsPreset}
            />
            {mobileNavOpen ? (
              <div
                className="fd-nav-scrim"
                data-testid="mobile-nav-scrim"
                onClick={() => setMobileNavOpen(false)}
              />
            ) : null}
            {store && state ? (
              <StrictMode>
                <Outliner
                  store={store}
                  state={state}
                  mobileNavOpen={mobileNavOpen}
                />
              </StrictMode>
            ) : null}
          </main>
        </div>
        <StrictMode>
          <div
            className="fd-workspace fd-script-workspace"
            hidden={!inProject || activeProjectTab?.kind !== "script"}
          >
            {store && state && activeScriptTab ? (
              <FieldSkriptEditor
                store={store}
                tab={activeScriptTab}
                visible={activeProjectTab?.kind === "script"}
              />
            ) : null}
          </div>
          {inProject && store && state ? (
            <BottomTabs store={store} state={state} />
          ) : null}
          {state?.loading ? (
            <div className="fd-loading-overlay">
              <div className="fd-loading-dialog">
                <div className="spinner" />
                <span className="phase">{state.loading.phase}</span>
              </div>
            </div>
          ) : null}
          {showHelp ? (
            <div className="fd-help-overlay" onClick={() => setShowHelp(false)}>
              <div
                className="fd-help-panel"
                onClick={(e) => e.stopPropagation()}
              >
                <h2>Keyboard Shortcuts</h2>
                <table>
                  <tbody>
                    <tr>
                      <td>
                        <kbd>B</kbd>
                      </td>
                      <td>Add Box</td>
                    </tr>
                    <tr>
                      <td>
                        <kbd>S</kbd>
                      </td>
                      <td>Add Sphere</td>
                    </tr>
                    <tr>
                      <td>
                        <kbd>C</kbd>
                      </td>
                      <td>Add Cylinder</td>
                    </tr>
                    <tr>
                      <td>
                        <kbd>T</kbd>
                      </td>
                      <td>Add Torus</td>
                    </tr>
                    <tr>
                      <td>
                        <kbd>H</kbd>
                      </td>
                      <td>Add Hole</td>
                    </tr>
                    <tr>
                      <td>
                        <kbd>Del</kbd>
                      </td>
                      <td>Delete selection</td>
                    </tr>
                    <tr>
                      <td>
                        <kbd>Esc</kbd>
                      </td>
                      <td>Clear selection</td>
                    </tr>
                    <tr>
                      <td>
                        <kbd>Ctrl+Z</kbd>
                      </td>
                      <td>Undo</td>
                    </tr>
                    <tr>
                      <td>
                        <kbd>Ctrl+Shift+Z</kbd>
                      </td>
                      <td>Redo</td>
                    </tr>
                    <tr>
                      <td>
                        <kbd>Ctrl+A</kbd>
                      </td>
                      <td>Select all</td>
                    </tr>
                    <tr>
                      <td>
                        <kbd>Ctrl+D</kbd>
                      </td>
                      <td>Duplicate</td>
                    </tr>
                    <tr>
                      <td>
                        <kbd>Ctrl+S</kbd>
                      </td>
                      <td>Save project</td>
                    </tr>
                    {(() => {
                      const row = viewShortcutRow(
                        CONTROLS_PRESETS[settings.controlsPreset],
                      );
                      return (
                        <tr>
                          <td>
                            {row.keys.split(" / ").map((k, i) => (
                              <span key={k}>
                                {i > 0 && " / "}
                                <kbd>{k}</kbd>
                              </span>
                            ))}
                          </td>
                          <td>{row.action}</td>
                        </tr>
                      );
                    })()}
                    <tr>
                      <td>
                        <kbd>Arrows</kbd>
                      </td>
                      <td>Nudge selection</td>
                    </tr>
                    <tr>
                      <td>
                        <kbd>F1</kbd>
                      </td>
                      <td>Toggle this help</td>
                    </tr>
                    <tr>
                      <td>
                        <kbd>F2</kbd>
                      </td>
                      <td>Report an issue</td>
                    </tr>
                  </tbody>
                </table>
                <button type="button" onClick={() => setShowHelp(false)}>
                  Close
                </button>
              </div>
            </div>
          ) : null}
        </StrictMode>
        {/* WP13: the standalone auth screen (login / register / reset). Renders
          nothing unless a guest opens one; the Overview + modeling views sit
          behind it. Kept OUTSIDE StrictMode's double-mount to avoid duplicate
          `/api/me`-driven effects. */}
        {account.state.auth !== null && (
          <AuthScreen
            mode={account.state.auth}
            invitePrefill={account.state.invitePrefill}
            resetPrefill={account.state.resetPrefill}
            onLogin={account.login}
            onRegister={account.register}
            onSwitch={(next) => account.openAuth(next)}
            onClose={account.closeAuth}
          />
        )}
        {/* WP9 B4: one-shot guest-project import offer (first login with local
          work). Non-intrusive — the modeling app stays usable behind it. */}
        {importPrompt === "offer" && (
          <GuestImportBanner
            onImport={importGuestProject}
            onDismiss={dismissGuestImport}
          />
        )}
      </div>
    </WorkspaceUnitsContext.Provider>
  );
}
