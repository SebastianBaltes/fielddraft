/**
 * Release notes — a per-version list of shipped features and bug fixes.
 *
 * Opened from the application menu or by clicking the version badge in the
 * topbar. The content is the typed SSOT in `releaseNotes.ts` (seeded from the
 * commit history, hand-maintained per version bump). A free-text filter spans
 * version / description / scope / ticket id, so "when was ticket X fixed?" is a
 * one-line lookup across the whole history.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import {
  RELEASE_NOTES,
  type ReleaseNote,
  type ReleaseNoteKind,
} from "./releaseNotes";

const KIND_LABEL: Record<ReleaseNoteKind, string> = {
  feat: "Neu",
  fix: "Fix",
  perf: "Schneller",
};

interface ReleaseNotesDialogProps {
  onClose: () => void;
  /** Open the IssueVault ticket behind a numeric release-note reference
   *  (e.g. "347"). Provided only when signed in — without it the refs render
   *  as plain tags, since the Issues view needs auth. Non-numeric references
   *  (internal change-codes like "nwmi") are never linked: they aren't
   *  tickets. */
  onOpenTicket?: (ref: string) => void;
}

/** Keep only the releases (and entries) whose text/version/scope/ticket match
 *  the lowercased query. An empty query passes everything through unchanged. */
function filterReleases(query: string): ReleaseNote[] {
  const q = query.trim().toLowerCase();
  if (!q) return RELEASE_NOTES;
  const out: ReleaseNote[] = [];
  for (const rel of RELEASE_NOTES) {
    if (rel.version.toLowerCase().includes(q)) {
      out.push(rel);
      continue;
    }
    const entries = rel.entries.filter(
      (e) =>
        e.text.toLowerCase().includes(q) ||
        (e.scope?.toLowerCase().includes(q) ?? false) ||
        (e.tickets?.some((t) => t.toLowerCase().includes(q)) ?? false),
    );
    if (entries.length > 0) out.push({ version: rel.version, entries });
  }
  return out;
}

/** A release note references a real IssueVault ticket only when the ref is
 *  purely numeric (the per-project ticket number). Alphabetic refs are internal
 *  change-codes with nothing to link to. */
const isTicketNumber = (ref: string) => /^\d+$/.test(ref);

export function ReleaseNotesDialog({
  onClose,
  onOpenTicket,
}: ReleaseNotesDialogProps) {
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        e.stopPropagation();
      }
    };
    window.addEventListener("keydown", handleKey, true);
    return () => window.removeEventListener("keydown", handleKey, true);
  }, [onClose]);

  // Autofocus the filter so the dialog is keyboard-ready on open.
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const releases = useMemo(() => filterReleases(query), [query]);
  const entryCount = useMemo(
    () => releases.reduce((sum, r) => sum + r.entries.length, 0),
    [releases],
  );

  return (
    <div
      className="fd-release-notes-overlay"
      data-testid="release-notes-overlay"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className="fd-release-notes"
        role="dialog"
        aria-label="Release notes"
        data-testid="release-notes-dialog"
      >
        <div className="dialog-header">
          <h3 className="dialog-title">Release Notes</h3>
          <button
            type="button"
            className="dialog-close"
            data-testid="release-notes-close"
            onClick={onClose}
          >
            &times;
          </button>
        </div>

        <div className="filter-bar">
          <input
            ref={inputRef}
            type="text"
            className="filter-input"
            placeholder="Filter — Version, Stichwort oder Ticket-ID…"
            value={query}
            data-testid="release-notes-filter"
            onChange={(e) => setQuery(e.target.value)}
          />
          {query.trim() !== "" && (
            <span className="filter-count" data-testid="release-notes-count">
              {entryCount} Eintr{entryCount === 1 ? "ag" : "äge"} ·{" "}
              {releases.length} Version{releases.length === 1 ? "" : "en"}
            </span>
          )}
        </div>

        <div className="dialog-body">
          {releases.length === 0 ? (
            <p className="empty" data-testid="release-notes-empty">
              Kein Treffer für „{query.trim()}".
            </p>
          ) : (
            releases.map((rel) => (
              <section className="release" key={rel.version}>
                <h4 className="release-version">v{rel.version}</h4>
                <ul className="release-entries">
                  {rel.entries.map((entry, i) => (
                    <li className="entry" key={i}>
                      <span className={"kind kind-" + entry.kind}>
                        {KIND_LABEL[entry.kind]}
                      </span>
                      <span className="entry-text">
                        {entry.scope && (
                          <span className="entry-scope">{entry.scope}</span>
                        )}
                        {entry.text}
                        {entry.tickets?.map((t) =>
                          onOpenTicket && isTicketNumber(t) ? (
                            <button
                              type="button"
                              className="entry-ticket ticket-link"
                              key={t}
                              data-testid={`release-note-ticket-${t}`}
                              title={`Ticket #${t} öffnen`}
                              onClick={() => onOpenTicket(t)}
                            >
                              #{t}
                            </button>
                          ) : (
                            <span className="entry-ticket" key={t}>
                              #{t}
                            </span>
                          ),
                        )}
                      </span>
                    </li>
                  ))}
                </ul>
              </section>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
