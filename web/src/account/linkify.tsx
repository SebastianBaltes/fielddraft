/**
 * Minimal autolinker for user-entered plain text (ticket bodies, comments).
 * Only http(s) URLs become links — no markdown, no other schemes — and they
 * open in a new tab with rel="noreferrer" (untrusted content). Trailing
 * sentence punctuation stays outside the link so "…/commit/abc123." links
 * the hash, not the period.
 *
 * Ticket #364 (teil A): the same pass also turns bare "#N" ticket references
 * into buttons that open that ticket, when a handler is supplied — one
 * matchAll over a combined regex, so URL and ref matches interleave in
 * source order instead of needing a second re-scan of the leftover text.
 */

import type { ReactNode } from "react";

const URL_RE = "https?:\\/\\/[^\\s<>\"']+";
const TICKET_REF_RE = "#\\d+";
const COMBINED_RE = new RegExp(`${URL_RE}|${TICKET_REF_RE}`, "g");
const TRAILING_PUNCT = /[.,;:!?)\]]+$/;

/** Autolink URLs, and — when `onOpenTicket` is given — "#N" ticket refs too
 *  (ticket #364). Without a handler, refs render as plain text (e.g. guests,
 *  who can't open the Issues view). */
export function linkify(
  text: string,
  onOpenTicket?: (ref: string) => void,
): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  let key = 0;
  for (const m of text.matchAll(COMBINED_RE)) {
    const start = m.index;
    let match = m[0];

    if (match[0] === "#") {
      if (!onOpenTicket) continue; // no handler → leave the "#N" as plain text
      const num = match.slice(1);
      if (start > last) out.push(text.slice(last, start));
      out.push(
        <button
          type="button"
          key={`ref-${key++}`}
          className="ticket-ref-link"
          title={`Ticket #${num} öffnen`}
          onClick={() => onOpenTicket(num)}
        >
          {match}
        </button>,
      );
      last = start + match.length;
      continue;
    }

    const punct = TRAILING_PUNCT.exec(match);
    if (punct) match = match.slice(0, -punct[0].length);
    if (match.length === 0) continue;
    if (start > last) out.push(text.slice(last, start));
    out.push(
      <a
        key={`lnk-${key++}`}
        href={match}
        target="_blank"
        rel="noreferrer"
        className="autolink"
      >
        {match}
      </a>,
    );
    last = start + match.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}
