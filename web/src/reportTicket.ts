/**
 * In-app bug / feature-request ticket plumbing.
 *
 * The "Report Issue" dialog (ReportDialog.tsx) collects a user title +
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

export interface TicketPayload {
  title: string;
  description: string;
  /** Serialized .fdraft project JSON (may be empty if unavailable). */
  projectJson: string;
  /** PNG data URL of the window, or null if capture failed. */
  screenshot: string | null;
  /** App version, e.g. "0.6.9". */
  version: string;
  /** ISO-8601 client timestamp. */
  timestamp: string;
  /** Browser UA string for environment context. */
  userAgent: string;
  /** Page URL the report was filed from. */
  url: string;
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
    return await toPng(document.body, {
      // Drop the report modal (overlay + backdrop) from the snapshot.
      filter: (node) =>
        !(
          node instanceof HTMLElement &&
          node.classList?.contains("fd-report-overlay")
        ),
      pixelRatio: window.devicePixelRatio || 1,
      cacheBust: true,
    });
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
export async function submitTicket(payload: TicketPayload): Promise<SubmitResult> {
  try {
    const res = await fetch("/api/tickets", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });
    if (!res.ok) {
      return { ok: false, error: `server returned ${res.status}` };
    }
    const data = (await res.json().catch(() => ({}))) as { ticket_id?: string };
    return { ok: true, ticketId: data.ticket_id };
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
}
