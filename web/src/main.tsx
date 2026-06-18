import { createRoot } from "react-dom/client";
import { App } from "./App";
import { installRawInputCapture } from "./userActionLog";
import "./styles/index.css";

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

const container = document.getElementById("root");
if (!container) throw new Error("#root not found");

// NOTE: StrictMode is applied per-region INSIDE App (around the UI chrome,
// outliner, toolbar, script editor) — but NOT around the 3D <Viewport>. Its
// dev double-mount (mount→unmount→mount) breaks the WebGPU/WebGL canvas: R3F's
// react-use-measure ResizeObserver misses the re-observed initial size, so the
// canvas stays blank until a manual window resize (a known R3F+StrictMode
// issue), and the renderer/device init runs twice. See App.tsx.
createRoot(container).render(<App />);
