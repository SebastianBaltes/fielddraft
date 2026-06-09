/**
 * In-app bug / feature-request dialog.
 *
 * Opened from the application menu. On mount it snapshots the whole app
 * window (toolbar, panels and the 3D viewport); the user adds a title +
 * description and submits. The ticket bundles the screenshot, the live
 * project JSON, the app version and a timestamp, and is written to the
 * repo's `tickets/open/` directory (see reportTicket.ts).
 */

import { useEffect, useState } from "react";
import type { KernelStore } from "./store";
import {
  captureWindow,
  submitTicket,
  type TicketPayload,
} from "./reportTicket";

interface ReportDialogProps {
  store: KernelStore | null;
  onClose: () => void;
}

type Phase = "edit" | "sending" | "error";

export function ReportDialog({ store, onClose }: ReportDialogProps) {
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [includeShot, setIncludeShot] = useState(true);
  const [screenshot, setScreenshot] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>("edit");
  const [errorMsg, setErrorMsg] = useState("");

  // Snapshot the window once, when the dialog opens. Capturing here (rather
  // than on submit) freezes the view the user is reporting about. The WebGPU
  // canvas only yields pixels via toDataURL right after a paint, so we wait
  // two animation frames before reading it — that lets R3F flush the current
  // frame and avoids grabbing a stale/blank buffer. captureWindow is async
  // (it serializes the DOM), so guard against setState after unmount.
  useEffect(() => {
    let cancelled = false;
    let raf2 = 0;
    const raf1 = requestAnimationFrame(() => {
      raf2 = requestAnimationFrame(() => {
        captureWindow().then((shot) => {
          if (!cancelled) setScreenshot(shot);
        });
      });
    });
    return () => {
      cancelled = true;
      cancelAnimationFrame(raf1);
      cancelAnimationFrame(raf2);
    };
  }, []);

  // Esc closes while editing.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && phase !== "sending") {
        onClose();
        e.stopPropagation();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose, phase]);

  const canSubmit =
    title.trim().length > 0 && (phase === "edit" || phase === "error");

  const handleSubmit = async () => {
    if (!canSubmit) return;
    setPhase("sending");
    const payload: TicketPayload = {
      title: title.trim(),
      description: description.trim(),
      projectJson: store?.exportProjectJson() ?? "",
      screenshot: includeShot ? screenshot : null,
      version: __APP_VERSION__,
      timestamp: new Date().toISOString(),
      userAgent: navigator.userAgent,
      url: window.location.href,
    };
    const result = await submitTicket(payload);
    if (result.ok) {
      // No confirmation step — the report is filed, just close the dialog.
      onClose();
    } else {
      setErrorMsg(result.error ?? "unknown error");
      setPhase("error");
    }
  };

  return (
    <div
      className="fd-report-overlay"
      onClick={() => phase !== "sending" && onClose()}
    >
      <div
        className="fd-report-dialog"
        role="dialog"
        aria-label="Report an issue"
        data-testid="report-dialog"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="header">
          <span className="title">Report an issue</span>
          <span className="version-tag">v{__APP_VERSION__}</span>
        </div>

        <div className="body">
            <label className="field">
              <span className="label">
                Title<span className="required">*</span>
              </span>
              <input
                className="input"
                data-testid="report-title"
                value={title}
                autoFocus
                placeholder="Short summary"
                disabled={phase === "sending"}
                onChange={(e) => setTitle(e.target.value)}
              />
            </label>

            <label className="field">
              <span className="label">Description</span>
              <textarea
                className="textarea"
                data-testid="report-description"
                value={description}
                rows={5}
                placeholder="What happened? What did you expect? Steps to reproduce…"
                disabled={phase === "sending"}
                onChange={(e) => setDescription(e.target.value)}
              />
            </label>

            <label className="field-inline">
              <input
                type="checkbox"
                checked={includeShot}
                disabled={phase === "sending" || !screenshot}
                onChange={(e) => setIncludeShot(e.target.checked)}
              />
              <span className="label">
                Attach screenshot
                {screenshot ? "" : " (capture unavailable)"}
              </span>
            </label>

            {includeShot && screenshot ? (
              <img
                className="preview"
                src={screenshot}
                alt="Window screenshot preview"
              />
            ) : null}

            <p className="hint">
              The current project JSON and app version are attached
              automatically.
            </p>

            {phase === "error" ? (
              <p className="error" role="alert">
                Could not save ticket: {errorMsg}
              </p>
            ) : null}

            <div className="actions">
              <button
                type="button"
                className="secondary"
                disabled={phase === "sending"}
                onClick={onClose}
              >
                Cancel
              </button>
              <button
                type="button"
                className="primary"
                data-testid="report-submit"
                disabled={!canSubmit}
                onClick={handleSubmit}
              >
                {phase === "sending" ? "Sending…" : "Send report"}
              </button>
            </div>
        </div>
      </div>
    </div>
  );
}
