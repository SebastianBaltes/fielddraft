/**
 * In-app bug / feature-request ticket plumbing.
 *
 * The "Report Issue" dialog (ReportDialog.tsx) collects a free-form
 * description, grabs a screenshot of the whole app window and the live
 * project JSON, and POSTs everything to `/api/tickets`. In local dev that
 * endpoint is served by the `ticketWriter` Vite plugin (vite.config.ts),
 * which drops the ticket into `<repo>/tickets/open/<id>/` so the agent can
 * pick it up; in production the Go backend handles the same route.
 *
 * The screenshot covers the full window — toolbar, panels and the 3D
 * viewport — via html-to-image, which serializes the live DOM and inlines
 * each <canvas> through `toDataURL` (verified to return real pixels on
 * three.js' WebGPURenderer canvas), so the rendered scene is composited in
 * place with the surrounding UI. No server-side rendering needed.
 */

import { toPng } from "html-to-image";

/** A user-supplied file riding along with the ticket (ticket 8onw): picked
 *  via the file chooser or pasted with Ctrl+V. Transported as a data URL so
 *  the whole ticket stays one JSON POST. */
export interface TicketAttachment {
  /** Original filename ("pasted-1.png" for clipboard images). */
  name: string;
  /** MIME type, e.g. "image/png"; used to pick thumbnail vs. file chip. */
  mime: string;
  /** Base64 data URL with the file contents. */
  dataUrl: string;
}

/** Per-file cap. Base64 inflates ~4/3 and the ticket endpoints cap the whole
 *  JSON body at 25 MB (alongside the ~1 MB window screenshot), so 8 MB per
 *  file keeps even a couple of attachments comfortably under the limit. */
export const MAX_ATTACHMENT_BYTES = 8 * 1024 * 1024;

/** Read a File into a {@link TicketAttachment}. Rejects (returns an error
 *  string) when the file exceeds {@link MAX_ATTACHMENT_BYTES}. */
export function fileToAttachment(
  file: File,
  fallbackName: string,
): Promise<TicketAttachment | { error: string }> {
  if (file.size > MAX_ATTACHMENT_BYTES) {
    const mb = Math.round(MAX_ATTACHMENT_BYTES / 1024 / 1024);
    return Promise.resolve({
      error: `"${file.name || fallbackName}" is larger than ${mb} MB`,
    });
  }
  return new Promise((resolve) => {
    const reader = new FileReader();
    reader.onload = () =>
      resolve({
        name: file.name || fallbackName,
        mime: file.type || "application/octet-stream",
        dataUrl: String(reader.result),
      });
    reader.onerror = () =>
      resolve({ error: `could not read "${file.name || fallbackName}"` });
    reader.readAsDataURL(file);
  });
}

/** Base64-encode raw bytes for JSON transport (the ticket POST is one JSON
 *  body, so the .fdraft ZIP rides along as base64). Chunked to avoid blowing
 *  the argument-count limit of `String.fromCharCode(...spread)` on large
 *  buffers. */
export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

export interface TicketPayload {
  // ticket xqll: no separate title — all info lives in the description.
  description: string;
  /** The live project as a `.fdraft` ZIP, base64-encoded (on3k). Empty when
   *  no project is available. Written to disk verbatim as project.fdraft. */
  projectFdraft: string;
  /** PNG data URL of the window, or null if capture failed. */
  screenshot: string | null;
  /** Extra user-attached files/images (ticket 8onw). */
  attachments: TicketAttachment[];
  /** App version, e.g. "0.6.9". */
  version: string;
  /** ISO-8601 client timestamp. */
  timestamp: string;
  /** Browser UA string for environment context. */
  userAgent: string;
  /** Page URL the report was filed from. */
  url: string;
  /** Per-device app/UI settings (renderMode, controlsPreset, meshAlgorithm,
   *  theme, …) from localStorage — NOT part of the project. Captured so a
   *  report carries the environment that produced it (e.g. which renderer or
   *  controls preset was active). Written as a separate settings.json next to
   *  ticket.md (ticket 478k). */
  settings?: Record<string, unknown>;
  /** Ephemeral live UI state — viewport camera, selection, active tool/mode,
   *  sketch-session context, view toggles — that the project file does NOT
   *  carry (`store.uiStateSnapshot()`). Lets a report reproduce the exact
   *  view, not just the document. Written as a separate ui_state.json next to
   *  ticket.md (ticket uz5q). */
  uiState?: Record<string, unknown>;
  /** Replayable user-action trace — semantic actions (entered sketch, picked
   *  tool, applied constraint, picked entity) interleaved with raw input
   *  (pointer/key/wheel/resize + field values, with `data-testid` targets) —
   *  from `serializeActionLog()`. Reconstructs the steps that led to the bug.
   *  Dev phase: logs everything, including typed text. actions.json. */
  actionLog?: Record<string, unknown>;
}

/**
 * Capture the entire app window — toolbar, side panels and the 3D viewport —
 * as a PNG data URL. The 3D canvas is composited in place (html-to-image
 * inlines it via toDataURL). The report dialog's own overlay is filtered out
 * so the shot shows the window the user is reporting about, not the modal.
 * Returns null if capture is unavailable (e.g. no DOM) or fails.
 */
export async function captureWindow(): Promise<string | null> {
  if (typeof document === "undefined" || !document.body) return null;
  try {
    const shot = toPng(document.body, {
      // Drop the report modal (overlay + backdrop) from the snapshot.
      filter: (node) => {
        if (
          node instanceof HTMLElement &&
          node.classList?.contains("fd-report-overlay")
        ) {
          return false;
        }
        // Ticket ddd6d8: a broken <img> (e.g. the dashboard's 404 project
        // thumbnails) hangs html-to-image's clone step forever — its load
        // event never fires. Skip anything not successfully decoded.
        if (
          node instanceof HTMLImageElement &&
          (!node.complete || node.naturalWidth === 0)
        ) {
          return false;
        }
        // Ticket ddd6d8: `hidden` subtrees (the modeling workspace stays
        // mounted behind the dashboard) are invisible anyway, and cloning
        // their WebGPU canvas stalls the capture. Skip them wholesale.
        if (node instanceof HTMLElement && node.hidden) {
          return false;
        }
        return true;
      },
      pixelRatio: window.devicePixelRatio || 1,
      // Ticket ddd6d8: font embedding walks every stylesheet and re-fetches
      // font resources — html-to-image hangs indefinitely on some views
      // (Projects dashboard), leaving "Attach screenshot" permanently
      // unavailable. The UI uses system fonts, so nothing is lost.
      skipFonts: true,
      cacheBust: true,
    });
    // Hard cap as a safety net: a hung capture must degrade to "capture
    // unavailable", not block forever.
    const timeout = new Promise<null>((resolve) =>
      setTimeout(() => resolve(null), 8000),
    );
    return await Promise.race([shot, timeout]);
  } catch {
    return null;
  }
}

export interface SubmitResult {
  ok: boolean;
  ticketId?: string;
  error?: string;
}

/** POST a ticket to the backend. Never throws — failures come back as
 *  `{ ok: false, error }` so the dialog can surface them inline. */
export async function submitTicket(
  payload: TicketPayload,
): Promise<SubmitResult> {
  try {
    const res = await fetch("/api/tickets", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      // The DB-backed ticket endpoint authenticates via the session cookie.
      credentials: "include",
      body: JSON.stringify(payload),
    });
    if (!res.ok) {
      // The DB-backed endpoint requires a session; surface that distinctly so
      // the dialog can prompt the user to log in.
      if (res.status === 401) {
        return { ok: false, error: "Please log in to file a report." };
      }
      return { ok: false, error: `server returned ${res.status}` };
    }
    // The create endpoint returns the full ticket (server `ticketDetailResponse`
    // embeds the Ticket → `id`, not `ticket_id`), so the dialog can link to it.
    const data = (await res.json().catch(() => ({}))) as { id?: string };
    return { ok: true, ticketId: data.id };
  } catch (err) {
    return {
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}
