/**
 * In-app bug / feature-request ticket plumbing.
 *
 * The "Report Issue" dialog (ReportDialog.tsx) collects a user title +
 * description, grabs a screenshot of the 3D viewport and the live project
 * JSON, and POSTs everything to `/api/tickets`. In local dev that endpoint
 * is served by the `ticketWriter` Vite plugin (vite.config.ts), which drops
 * the ticket into `<repo>/tickets/open/<id>/` so the agent can pick it up;
 * in production the Go backend handles the same route.
 *
 * The WebGPU canvas is captured directly via `toDataURL` — verified to
 * return real pixels on three.js' WebGPURenderer canvas, so no readback or
 * server-side rendering is needed.
 */

export interface TicketPayload {
  title: string;
  description: string;
  /** Serialized .fdraft project JSON (may be empty if unavailable). */
  projectJson: string;
  /** PNG data URL of the viewport, or null if capture failed. */
  screenshot: string | null;
  /** App version, e.g. "0.6.8". */
  version: string;
  /** ISO-8601 client timestamp. */
  timestamp: string;
  /** Browser UA string for environment context. */
  userAgent: string;
  /** Page URL the report was filed from. */
  url: string;
}

/**
 * Capture the current 3D viewport as a PNG data URL. Returns null if no
 * canvas is present or the browser refuses the readback.
 */
export function captureViewport(): string | null {
  if (typeof document === "undefined") return null;
  const renderer = (window as unknown as { __threeRenderer?: { domElement?: HTMLCanvasElement } })
    .__threeRenderer;
  const canvas: HTMLCanvasElement | null =
    renderer?.domElement ?? document.querySelector("canvas");
  if (!canvas) return null;
  try {
    return canvas.toDataURL("image/png");
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
