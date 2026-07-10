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
}

/**
 * Render `bodies` to a PNG and hand it to `sink`, throttled to at most one
 * every {@link THUMBNAIL_MIN_INTERVAL_MS}. Resolves `true` when a thumbnail was
 * captured + persisted. No-op (`false`) when throttled, when there is no
 * geometry, or on any capture/persist failure. The throttle window is only
 * claimed once we know there IS geometry to draw, so an empty attempt does not
 * block a real capture that follows immediately.
 */
export async function maybeCaptureThumbnail(
  bodies: readonly ThumbnailBody[],
  sink: ThumbnailSink,
  deps: ThumbnailDeps = {},
): Promise<boolean> {
  const render = deps.render ?? renderThumbnailBlob;
  const nowMs = deps.nowMs ?? performance.now();

  if (nowMs - lastCaptureMs < THUMBNAIL_MIN_INTERVAL_MS) return false;
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

// ── New-project arming ──────────────────────────────────────────────
// Creating a project (empty, imported, or "save current workspace") arms a
// one-shot capture: the next time geometry is ready for the bound project,
// App.tsx captures a preview so a fresh project gets a gallery thumbnail
// without waiting for the first editor exit. Plain "open an existing project"
// never arms, so opening does NOT trigger a redundant re-capture.

let armed = false;

/** Arm a one-shot capture for the next project whose geometry becomes ready. */
export function armProjectThumbnailCapture(): void {
  armed = true;
}

/** Consume the armed flag: returns `true` (and disarms) exactly once. */
export function takeArmedProjectCapture(): boolean {
  if (!armed) return false;
  armed = false;
  return true;
}

/** Test hook: clear the throttle + arm so a fresh capture is allowed at once. */
export function resetThumbnailThrottle(): void {
  lastCaptureMs = -Infinity;
  armed = false;
}
