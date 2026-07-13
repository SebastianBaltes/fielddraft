/**
 * Throttled project-thumbnail capture (ticket 14cff61; Phase 3c cadence rework).
 *
 * A thumbnail is a nice-to-have gallery preview, never worth stealing a frame on
 * every autosave. Since P3b's cloud auto-sync fires the `syncing → clean` edge
 * far more often, the capture is now DECOUPLED from the server save: App.tsx
 * triggers it only on EDITOR EXIT (leaving the modeling view — the moment the
 * gallery preview matters) and once on NEW-PROJECT creation. Both paths run
 * through here, sharing one leading-edge throttle window.
 *
 * The capture is destination-agnostic: the caller passes a {@link ThumbnailSink}
 * (cloud PUT for a server project, IndexedDB write for a local one). Everything
 * is best-effort — a missing thumbnail is harmless, so any failure is swallowed.
 */
import { platformApi } from "./api/platform";
import { putLocalThumbnail } from "./localProjects";
import { renderThumbnailBlob, type ThumbnailBody } from "./thumbnailCapture";

/** At most one capture this often, across all projects (leading-edge). */
export const THUMBNAIL_MIN_INTERVAL_MS = 30_000;

let lastCaptureMs = -Infinity;

/** Where a freshly captured thumbnail PNG is persisted. */
export type ThumbnailSink = (png: Blob) => Promise<void>;

/** Injectable dependencies (for tests). */
export interface ThumbnailDeps {
  render?: (bodies: readonly ThumbnailBody[]) => Promise<Blob | null>;
  nowMs?: number;
  /** Ignore the throttle window — the ONE-SHOT capture of a project that was
   *  just opened or created (ticket #343). The throttle exists to stop frequent
   *  editor-exit captures from stealing frames; a single capture per opened
   *  project is exactly what the gallery needs, and dropping it left the project
   *  with no preview at all (it never gets a second chance). */
  force?: boolean;
}

/**
 * Render `bodies` to a PNG and hand it to `sink`, throttled to at most one
 * every {@link THUMBNAIL_MIN_INTERVAL_MS} (unless `force`). Resolves `true` when
 * a thumbnail was captured + persisted. No-op (`false`) when throttled, when
 * there is no geometry, or on any capture/persist failure. The throttle window
 * is only claimed once we know there IS geometry to draw, so an empty attempt
 * does not block a real capture that follows immediately.
 */
export async function maybeCaptureThumbnail(
  bodies: readonly ThumbnailBody[],
  sink: ThumbnailSink,
  deps: ThumbnailDeps = {},
): Promise<boolean> {
  const render = deps.render ?? renderThumbnailBlob;
  const nowMs = deps.nowMs ?? performance.now();

  if (!deps.force && nowMs - lastCaptureMs < THUMBNAIL_MIN_INTERVAL_MS) {
    return false;
  }
  if (!bodies || bodies.length === 0) return false;
  // Claim the throttle window up-front so overlapping triggers don't double-capture.
  lastCaptureMs = nowMs;

  try {
    const blob = await render(bodies);
    if (!blob) return false;
    await sink(blob);
    return true;
  } catch {
    return false;
  }
}

/** Sink that uploads to a CLOUD project's thumbnail endpoint. */
export function cloudThumbnailSink(projectId: string): ThumbnailSink {
  return (png) => platformApi.putThumbnail(projectId, png);
}

/** Sink that stores a LOCAL project's thumbnail in IndexedDB (per-project key). */
export function localThumbnailSink(localId: string): ThumbnailSink {
  return (png) => putLocalThumbnail(localId, png);
}

// ── One-shot arming ─────────────────────────────────────────────────
// Creating a project (empty, imported, "save current workspace") AND opening an
// existing one arm a one-shot capture: as soon as the geometry OF THAT PROJECT
// is meshed, App.tsx captures a preview. So a project gets its gallery image the
// moment it is opened, instead of only on the first editor exit (ticket #343).
//
// The arm carries the project KEY it was armed for (server project id, or local
// project id). Without it the pending arm was consumed by whatever geometry
// happened to arrive next — and right after a project switch that is the OLD
// project's mesh, which is how a freshly imported project ended up wearing the
// previous project's picture.

let armedKey: string | null = null;

/** Arm a one-shot capture for `projectKey` (server project id or local id). */
export function armProjectThumbnailCapture(projectKey: string): void {
  armedKey = projectKey;
}

/** Consume the arm — only for the project it was armed for. Returns `true`
 *  (and disarms) exactly once; a different project leaves the arm standing. */
export function takeArmedProjectCapture(projectKey: string | null): boolean {
  if (!projectKey || armedKey !== projectKey) return false;
  armedKey = null;
  return true;
}

/** Put back an arm whose capture did not succeed (no geometry yet, render
 *  failed): the next mesh of that project tries again, rather than the project
 *  silently never getting a preview. */
export function rearmProjectThumbnailCapture(projectKey: string): void {
  armedKey = projectKey;
}

/** Test hook: clear the throttle + arm so a fresh capture is allowed at once. */
export function resetThumbnailThrottle(): void {
  lastCaptureMs = -Infinity;
  armedKey = null;
}
