/**
 * In-app bug / feature-request dialog.
 *
 * Opened from the application menu. On mount it snapshots the whole app
 * window (toolbar, panels and the 3D viewport); the user adds a free-form
 * description — plus optional file/image attachments via the file picker or
 * Ctrl+V (ticket 8onw) — and submits. The ticket bundles the screenshot, the
 * attachments, the live project JSON, the app version and a timestamp, and
 * is written to the repo's `tickets/open/` directory (see reportTicket.ts).
 */

import { useEffect, useRef, useState } from "react";
import type { KernelStore } from "./store";
import { loadSettings } from "./SettingsMenu";
import {
  captureWindow,
  fileToAttachment,
  submitTicket,
  type TicketAttachment,
  type TicketPayload,
} from "./reportTicket";

interface ReportDialogProps {
  store: KernelStore | null;
  onClose: () => void;
}

type Phase = "edit" | "sending" | "error";

export function ReportDialog({ store, onClose }: ReportDialogProps) {
  const [description, setDescription] = useState("");
  const [includeShot, setIncludeShot] = useState(true);
  const [screenshot, setScreenshot] = useState<string | null>(null);
  const [attachments, setAttachments] = useState<TicketAttachment[]>([]);
  const [attachError, setAttachError] = useState("");
  const [phase, setPhase] = useState<Phase>("edit");
  const [errorMsg, setErrorMsg] = useState("");
  const fileInputRef = useRef<HTMLInputElement>(null);
  const pasteCounter = useRef(0);

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

  // Append files (picker or clipboard) to the attachment list. Oversized or
  // unreadable files surface inline via `attachError` instead of vanishing.
  const addFiles = async (files: Iterable<File>) => {
    const errors: string[] = [];
    const added: TicketAttachment[] = [];
    for (const file of files) {
      pasteCounter.current += 1;
      const ext = file.type === "image/png" ? ".png" : "";
      const result = await fileToAttachment(
        file,
        `pasted-${pasteCounter.current}${ext}`,
      );
      if ("error" in result) errors.push(result.error);
      else added.push(result);
    }
    if (added.length > 0) setAttachments((prev) => [...prev, ...added]);
    setAttachError(errors.join("; "));
  };

  // Ctrl+V while the dialog is open attaches clipboard files (ticket 8onw).
  // Text pastes into the focused input keep their default behaviour — only
  // pastes that actually carry files are intercepted.
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      if (phase === "sending") return;
      const files = e.clipboardData?.files;
      if (!files || files.length === 0) return;
      e.preventDefault();
      void addFiles(files);
    };
    window.addEventListener("paste", onPaste, true);
    return () => window.removeEventListener("paste", onPaste, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase]);

  const removeAttachment = (index: number) => {
    setAttachments((prev) => prev.filter((_, i) => i !== index));
  };

  // ticket xqll: a description is the only required input now (no title).
  const canSubmit =
    description.trim().length > 0 && (phase === "edit" || phase === "error");

  const handleSubmit = async () => {
    if (!canSubmit) return;
    setPhase("sending");
    const payload: TicketPayload = {
      description: description.trim(),
      projectJson: store?.exportProjectJson() ?? "",
      screenshot: includeShot ? screenshot : null,
      attachments,
      version: __APP_VERSION__,
      timestamp: new Date().toISOString(),
      userAgent: navigator.userAgent,
      url: window.location.href,
      // Capture the per-device settings (renderMode, controlsPreset, …) so a
      // report carries the environment that produced it (helps reproduce
      // renderer-/controls-dependent bugs).
      settings: loadSettings() as unknown as Record<string, unknown>,
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
                Description<span className="required">*</span>
              </span>
              <textarea
                className="textarea"
                data-testid="report-description"
                value={description}
                autoFocus
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

            <div className="field">
              <div className="attach-row">
                <span className="label">Attachments</span>
                <button
                  type="button"
                  className="secondary attach-button"
                  data-testid="report-attach"
                  disabled={phase === "sending"}
                  onClick={() => fileInputRef.current?.click()}
                >
                  Add files…
                </button>
                <span className="hint">or paste with Ctrl+V</span>
              </div>
              <input
                ref={fileInputRef}
                type="file"
                multiple
                hidden
                data-testid="report-file-input"
                onChange={(e) => {
                  if (e.target.files) void addFiles(e.target.files);
                  e.target.value = "";
                }}
              />
              {attachments.length > 0 ? (
                <div className="attachments" data-testid="report-attachments">
                  {attachments.map((a, i) => (
                    <div className="thumb" key={`${a.name}-${i}`} title={a.name}>
                      {a.mime.startsWith("image/") ? (
                        <img src={a.dataUrl} alt={a.name} />
                      ) : (
                        <span className="file-chip">
                          {a.name.split(".").pop()?.toUpperCase() ?? "FILE"}
                        </span>
                      )}
                      <span className="thumb-name">{a.name}</span>
                      <button
                        type="button"
                        className="thumb-x"
                        aria-label={`Remove ${a.name}`}
                        data-testid={`report-attachment-remove-${i}`}
                        disabled={phase === "sending"}
                        onClick={() => removeAttachment(i)}
                      >
                        ×
                      </button>
                    </div>
                  ))}
                </div>
              ) : null}
              {attachError ? (
                <p className="error" role="alert">
                  {attachError}
                </p>
              ) : null}
            </div>

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
