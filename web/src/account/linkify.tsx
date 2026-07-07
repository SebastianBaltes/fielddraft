/**
 * Minimal autolinker for user-entered plain text (ticket bodies, comments).
 * Only http(s) URLs become links — no markdown, no other schemes — and they
 * open in a new tab with rel="noreferrer" (untrusted content). Trailing
 * sentence punctuation stays outside the link so "…/commit/abc123." links
 * the hash, not the period.
 */

import type { ReactNode } from "react";

const URL_RE = /https?:\/\/[^\s<>"']+/g;
const TRAILING_PUNCT = /[.,;:!?)\]]+$/;

export function linkify(text: string): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  let key = 0;
  for (const m of text.matchAll(URL_RE)) {
    let url = m[0];
    const punct = TRAILING_PUNCT.exec(url);
    if (punct) url = url.slice(0, -punct[0].length);
    if (url.length === 0) continue;
    const start = m.index;
    if (start > last) out.push(text.slice(last, start));
    out.push(
      <a
        key={`lnk-${key++}`}
        href={url}
        target="_blank"
        rel="noreferrer"
        className="autolink"
      >
        {url}
      </a>,
    );
    last = start + url.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}
