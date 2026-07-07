import { createRoot } from "react-dom/client";
import { App } from "./App";
import { installRawInputCapture } from "./userActionLog";
import { LS_AUTO_RELOAD } from "./settings";
import { bootProjectPath } from "./bootParams";
import { applyBootSettings, loadBootProject } from "./bootProject";
import "./styles/index.css";

// Ticket cztw / a020ac: let the user keep a running session across edits.
// The dev server never sends Vite's raw `full-reload` — the reloadGate plugin
// (vite.config.ts) rewrites every one to this custom event, because Vite 8
// runs vite:beforeFullReload listeners via Promise.allSettled and the old
// "throw to cancel" trick no longer aborts the reload. The client decides:
// with "Auto-reload on code change" off the app keeps running and the user
// reloads manually (F5) when ready. Dev-only — `import.meta.hot` is
// undefined in a production build.
if (import.meta.hot) {
  import.meta.hot.on("fd:full-reload", () => {
    if (localStorage.getItem(LS_AUTO_RELOAD) === "false") {
      // eslint-disable-next-line no-console
      console.info(
        "[FieldDraft] Full reload skipped (Auto-reload off in Settings). Press F5 to apply the latest code.",
      );
      return;
    }
    location.reload();
  });

  // Ticket dc02f591: with Auto-reload OFF the app must be COMPLETELY
  // untouched by code changes — hot module updates re-execute modules and
  // reset UI state even without a reload. Announce the preference so the
  // reloadGate plugin (vite.config.ts) drops hot payloads at our socket.
  // Re-announced on every (re)connect and live on a settings toggle.
  const announceHotMute = () => {
    import.meta.hot?.send("fd:hot-mute", {
      muted: localStorage.getItem(LS_AUTO_RELOAD) === "false",
    });
  };
  announceHotMute();
  import.meta.hot.on("vite:ws:connect", announceHotMute);
  window.addEventListener("fd:autoreload-changed", announceHotMute);
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
