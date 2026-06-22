// ClaudeDraft "open ticket" deep-link loader.
//
// When the app is opened at `/?project=<repo-rel-path>`, these helpers pull the
// `project.fdraft` (and its sibling `settings.json`) out of the dev server's
// OWN filesystem via the same-origin `/api/file?path=…` middleware
// (web/vite.config.ts) — no CORS, no ClaudeDraft round-trip. The bytes feed
// `store.loadProjectFromBytes`; the settings feed a one-shot in-memory override
// (SettingsMenu.setBootSettings) so the repro tab opens in the reported render
// mode without touching the user's localStorage.
//
// Orchestrated by main.tsx: applyBootSettings() runs BEFORE React mounts (so
// the first loadSettings() sees the override); loadBootProject() runs after.

import { getStore } from "./store";
import { setBootSettings } from "./SettingsMenu";
import { ticketDir, fileApiUrl } from "./bootParams";

/**
 * Fetch and stage the ticket's `settings.json` (optional). Must run before the
 * first `loadSettings()` (React mount). Silently no-ops when the file is absent
 * (older tickets carry no settings.json) or the server is unreachable.
 */
export async function applyBootSettings(projectPath: string): Promise<void> {
  const dir = ticketDir(projectPath);
  if (!dir) return;
  try {
    const res = await fetch(fileApiUrl(`${dir}/settings.json`));
    if (!res.ok) return; // 404 → app defaults
    setBootSettings((await res.json()) as Record<string, unknown>);
  } catch {
    // No dev server / network error → app defaults.
  }
}

/**
 * Fetch the ticket's `project.fdraft` and load it into the (singleton) store,
 * replacing the default doc the repro tab started with. Errors are logged, not
 * thrown — a bad deep-link must not wedge the app.
 */
export async function loadBootProject(projectPath: string): Promise<void> {
  try {
    const res = await fetch(fileApiUrl(projectPath));
    if (!res.ok) {
      // eslint-disable-next-line no-console
      console.error(
        `[FieldDraft] deep-link project load failed (${res.status}): ${projectPath}`,
      );
      return;
    }
    const bytes = await res.arrayBuffer();
    const store = await getStore();
    if (!store.loadProjectFromBytes(bytes)) {
      // eslint-disable-next-line no-console
      console.error(`[FieldDraft] deep-link project parse failed: ${projectPath}`);
    }
  } catch (err) {
    // eslint-disable-next-line no-console
    console.error(`[FieldDraft] deep-link project load error: ${String(err)}`);
  }
}
