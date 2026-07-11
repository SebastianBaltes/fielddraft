/**
 * In-app bug / feature-request dialog.
 *
 * Opened from the application menu. On mount it snapshots the whole app
 * window (toolbar, panels and the 3D viewport); the user adds a free-form
 * description — plus optional file/image attachments via the file picker or
 * Ctrl+V (ticket 8onw) — and submits. The ticket bundles the screenshot, the
 * attachments, the live project JSON, the app version and a timestamp, and
 * is POSTed to the backend ticket store (see reportTicket.ts).
 */

import { useEffect, useRef, useState } from "react";
import type { KernelStore } from "./store";
import { loadSettings } from "./settings";
import {
  bytesToBase64,
  captureWindow,
  fileToAttachment,
  submitTicket,
  type TicketAttachment,
  type TicketPayload,
} from "./reportTicket";
import { serializeActionLog } from "./userActionLog";

interface ReportDialogProps {
  store: KernelStore | null;
  onClose: () => void;
}

type Phase = "edit" | "sending" | "error" | "sent";

export function ReportDialog({ store, onClose }: ReportDialogProps) {
  const [description, setDescription] = useState("");
  const [includeShot, setIncludeShot] = useState(true);
  const [includeActions, setIncludeActions] = useState(true);
  const [screenshot, setScreenshot] = useState<string | null>(null);
  const [attachments, setAttachments] = useState<TicketAttachment[]>([]);
  const [attachError, setAttachError] = useState("");
  const [phase, setPhase] = useState<Phase>("edit");
  const [errorMsg, setErrorMsg] = useState("");
  // The id of the created ticket (set on success) so the "sent" view can link
  // to it via the account layer.
  const [ticketId, setTicketId] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const pasteCounter = useRef(0);
  // ticket ttzl: the dialog is a non-modal, draggable, minimizable panel so
  // the user can keep using the app (copy text, grab a screenshot, double-check
  // something) while writing the report. `pos` is null until first dragged —
  // then the CSS default top-left anchor (below the toolbar, ticket nwgi)
  // gives way to explicit coordinates.
  const dialogRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{ dx: number; dy: number } | null>(null);
  const [minimized, setMinimized] = useState(false);
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);

  const onHeaderPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    // Let the header buttons (minimize/close) handle their own clicks.
    if ((e.target as HTMLElement).closest("button")) return;
    const rect = dialogRef.current?.getBoundingClientRect();
    if (!rect) return;
    dragRef.current = { dx: e.clientX - rect.left, dy: e.clientY - rect.top };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onHeaderPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!dragRef.current) return;
    // Keep at least a sliver on-screen so the panel can't be lost off-edge.
    setPos({
      x: Math.max(0, Math.min(window.innerWidth - 48, e.clientX - dragRef.current.dx)),
      y: Math.max(0, Math.min(window.innerHeight - 48, e.clientY - dragRef.current.dy)),
    });
  };
  const onHeaderPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    dragRef.current = null;
    e.currentTarget.releasePointerCapture?.(e.pointerId);
  };

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

  // Esc closes while editing — but only when focus is inside the dialog. The
  // panel is non-modal (ticket ttzl), so Escape must still cancel tools and
  // selections in the app behind it when the user is interacting there.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || phase === "sending") return;
      if (!dialogRef.current?.contains(document.activeElement)) return;
      onClose();
      e.stopPropagation();
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
      // on3k: ship the real .fdraft ZIP (document + font assets), base64-encoded
      // for the JSON POST, so the ticket carries an openable project file.
      projectFdraft: store ? bytesToBase64(await store.exportProjectFdraft()) : "",
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
      // Capture the live UI state (camera, selection, active tool/mode) so the
      // report reproduces the exact view, not just the document (ticket uz5q).
      uiState: store ? store.uiStateSnapshot() : undefined,
      // Replayable user-action trace (semantic + raw input) so a report carries
      // the steps that led to the bug — opt-out via the checkbox.
      actionLog: includeActions ? serializeActionLog() : undefined,
    };
    const result = await submitTicket(payload);
    if (result.ok) {
      // Surface a confirmation with a link to the freshly-created issue (WP4):
      // the DB-backed endpoint returns the ticket id, so the user can jump
      // straight to the thread instead of the report vanishing silently.
      setTicketId(result.ticketId ?? null);
      setPhase("sent");
    } else {
      setErrorMsg(result.error ?? "unknown error");
      setPhase("error");
    }
  };

  // Open the created issue in the account layer's tickets overlay (App.tsx
  // listens for this event and deep-links via account.openTicket).
  const viewTicket = () => {
    if (ticketId) {
      window.dispatchEvent(
        new CustomEvent("fd:open-ticket", { detail: ticketId }),
      );
    }
    onClose();
  };

  return (
    <div className="fd-report-overlay">
      <div
        ref={dialogRef}
        className={`fd-report-dialog${minimized ? " minimized" : ""}`}
        role="dialog"
        aria-label="Report an issue"
        data-testid="report-dialog"
        style={pos ? { left: pos.x, top: pos.y, right: "auto" } : undefined}
      >
        <div
          className="header"
          onPointerDown={onHeaderPointerDown}
          onPointerMove={onHeaderPointerMove}
          onPointerUp={onHeaderPointerUp}
        >
          <span className="title">Report an issue</span>
          <span className="version-tag">v{__APP_VERSION__}</span>
          <button
            type="button"
            className="icon-btn"
            data-testid="report-minimize"
            aria-label={minimized ? "Maximize" : "Minimize"}
            title={minimized ? "Maximize" : "Minimize"}
            onClick={() => setMinimized((m) => !m)}
          >
            {minimized ? "▢" : "–"}
          </button>
          <button
            type="button"
            className="icon-btn"
            aria-label="Close"
            title="Close"
            disabled={phase === "sending"}
            onClick={onClose}
          >
            {"×"}
          </button>
        </div>

        <div className="body" hidden={minimized}>
          {phase === "sent" ? (
            <div className="sent" data-testid="report-sent">
              <p className="sent-msg">Thanks — your issue has been filed.</p>
              <div className="actions">
                <button
                  type="button"
                  className="secondary"
                  data-testid="report-sent-close"
                  onClick={onClose}
                >
                  Close
                </button>
                {ticketId ? (
                  <button
                    type="button"
                    className="primary"
                    data-testid="report-view-ticket"
                    onClick={viewTicket}
                  >
                    View issue
                  </button>
                ) : null}
              </div>
            </div>
          ) : (
          <>
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

          <label className="field-inline">
            <input
              type="checkbox"
              data-testid="report-include-actions"
              checked={includeActions}
              disabled={phase === "sending"}
              onChange={(e) => setIncludeActions(e.target.checked)}
            />
            <span className="label">
              Attach action log (recent clicks, tools, picks, typed values)
            </span>
          </label>

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
            The current project file and app version are attached automatically.
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
          </>
          )}
        </div>
      </div>
    </div>
  );
}
