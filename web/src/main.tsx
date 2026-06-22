import { createRoot } from "react-dom/client";
import { App } from "./App";
import { installRawInputCapture } from "./userActionLog";
import { LS_AUTO_RELOAD } from "./SettingsMenu";
import { bootProjectPath } from "./bootParams";
import { applyBootSettings, loadBootProject } from "./bootProject";
import "./styles/index.css";

// Ticket cztw: let the user keep a running session across edits. When Vite has
// a change it can't hot-swap it normally reloads the whole page — wiping the
// in-progress (unsaved) project. With the "Auto-reload on code change" setting
// off, throwing from this handler aborts Vite's `location.reload()`, so the app
// keeps running; the user reloads manually (F5) when ready. Dev-only —
// `import.meta.hot` is undefined in a production build.
if (import.meta.hot) {
  import.meta.hot.on("vite:beforeFullReload", () => {
    if (localStorage.getItem(LS_AUTO_RELOAD) === "false") {
      // The throw is the documented way to cancel Vite's full reload.
      // eslint-disable-next-line no-console
      console.info(
        "[FieldDraft] Full reload skipped (Auto-reload off in Settings). Press F5 to apply the latest code.",
      );
      throw new Error("vite full reload cancelled by FieldDraft setting");
    }
  });
}

// Capture raw pointer/keyboard/wheel/resize input into the user-action log so a
// "Report an issue" can ship a replayable interaction trace. Installed once,
// before React mounts, so it sees the whole session. Privacy-conscious (no
// typed text) — see userActionLog.ts.
installRawInputCapture();

// Silence the upstream three.js deprecation noise we can't fix here.
// `THREE.Clock` is still used internally by R3F 9; the warning fires
// every time R3F mounts a Canvas. Filter it narrowly so other warnings
// continue to surface.
const _origWarn = console.warn.bind(console);
console.warn = (...args: unknown[]) => {
  const first = typeof args[0] === "string" ? args[0] : "";
  if (first.includes("THREE.Clock") || first.includes("THREE.THREE.Clock")) {
    return;
  }
  _origWarn(...args);
};

// ClaudeDraft "open ticket" deep-link (`/?project=<repo-rel>`): when present,
// stage the ticket's settings.json BEFORE mounting (so the first loadSettings()
// opens the tab in the reported render mode), then load the project.fdraft once
// the store exists. `getStore()` already routes this tab to an inert autosave +
// clean start (see store.ts) so a repro tab can't clobber the user's session.
async function boot(): Promise<void> {
  const container = document.getElementById("root");
  if (!container) throw new Error("#root not found");

  const projectPath = bootProjectPath();
  if (projectPath) await applyBootSettings(projectPath);

  // NOTE: StrictMode is applied per-region INSIDE App (around the UI chrome,
  // outliner, toolbar, script editor) — but NOT around the 3D <Viewport>. Its
  // dev double-mount (mount→unmount→mount) breaks the WebGPU/WebGL canvas: R3F's
  // react-use-measure ResizeObserver misses the re-observed initial size, so the
  // canvas stays blank until a manual window resize (a known R3F+StrictMode
  // issue), and the renderer/device init runs twice. See App.tsx.
  createRoot(container).render(<App />);

  if (projectPath) void loadBootProject(projectPath);
}

void boot();
