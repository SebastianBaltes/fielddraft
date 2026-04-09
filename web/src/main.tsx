import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles/index.css";

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

createRoot(container).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
