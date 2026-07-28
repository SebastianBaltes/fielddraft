/**
 * TicketMarkdown — renders ticket bodies/comments as sanitized Markdown,
 * including "#N" ticket-reference links and Mermaid diagrams (ticket #441).
 *
 * Agents write comments with headings, tables, code blocks and occasional
 * Mermaid diagrams; the old plain-text rendering (`linkify`) left all of
 * that as a wall of raw markup. This is the single place both the ticket
 * body and every comment go through.
 *
 * Security: ticket text is untrusted user content. `marked` only turns it
 * into HTML — the actual defense is `DOMPurify.sanitize` right before it
 * hits `dangerouslySetInnerHTML` (same pairing baseaichat's ChatPanel uses
 * for AI-chat output). Never render `marked.parse()` output directly.
 *
 * "#N" references (ticket #364) render via a dedicated inline marked
 * extension rather than a text-level regex pass, so it naturally leaves
 * code spans/fences alone (marked's built-in tokenizers claim those before
 * this one ever runs) instead of needing a hand-rolled "don't touch code"
 * carve-out. The extension always emits a `<span class="ticket-ref-link"
 * data-ticket-ref="N">` — a `<span>`, not an `<a>`, because there is
 * nothing to navigate to without a handler. The click is delegated from
 * the container to `onOpenTicketByNumber`, the same idea as the old
 * `linkify`'s conditional button.
 *
 * Mermaid is heavy (several hundred KB) and most tickets never use it, so
 * it is dynamic-`import()`ed lazily — only once an actual ```mermaid fence
 * shows up in the text — and must never end up in the main bundle.
 */

import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
} from "react";
import DOMPurify from "dompurify";
import { Marked, type TokenizerAndRendererExtension } from "marked";

/* ── markdown pipeline ──────────────────────────────────────────── */

/** Turns a bare "#42" into a `<span class="ticket-ref-link" data-ticket-ref="42">`.
 *  An inline marked extension, not a text-level regex — marked's own
 *  tokenizers (code spans, fenced code blocks) already claim that content
 *  before this extension gets a look, so it skips code for free. */
const ticketRefExtension: TokenizerAndRendererExtension = {
  name: "ticketRef",
  level: "inline",
  start(src) {
    return src.match(/#\d+\b/)?.index;
  },
  tokenizer(src) {
    const match = /^#(\d+)\b/.exec(src);
    if (!match) return undefined;
    return { type: "ticketRef", raw: match[0], number: match[1] };
  },
  renderer(token) {
    return `<span class="ticket-ref-link" data-ticket-ref="${token.number}" role="button" tabindex="0">#${token.number}</span>`;
  },
};

// A dedicated `Marked` instance — NOT the shared default `marked` export.
// baseaichat's ChatPanel.tsx configures that same package singleton for
// AI-chat rendering, and registering the ticketRef extension on it would
// leak "#N" styling into unrelated chat responses.
const markdown = new Marked({ breaks: true, gfm: true });
markdown.use({ extensions: [ticketRefExtension] });

// Open every genuine markdown link (never the ticket-ref spans above, which
// aren't anchors) in a new tab — matches the old `linkify`'s autolinks.
DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node.tagName === "A" && node.hasAttribute("href")) {
    node.setAttribute("target", "_blank");
    node.setAttribute("rel", "noreferrer");
  }
});

/** Markdown text → sanitized HTML, ready for `dangerouslySetInnerHTML`. Pure
 *  and exported so tests can check sanitization without mounting React. */
export function ticketMarkdownToHtml(text: string): string {
  const rendered = markdown.parse(text, { async: false }) as string;
  return DOMPurify.sanitize(rendered, { USE_PROFILES: { html: true } });
}

/* ── mermaid fences ─────────────────────────────────────────────── */

// `String.split` with a capturing regex keeps the captured group
// interleaved with the surrounding text: even indices are markdown, odd
// indices are raw mermaid source (ticket #441).
const MERMAID_FENCE_RE = /```mermaid[ \t]*\r?\n([\s\S]*?)\r?\n```/g;

function splitMermaidFences(text: string): string[] {
  return text.split(MERMAID_FENCE_RE);
}

/* ── component ──────────────────────────────────────────────────── */

export interface TicketMarkdownProps {
  text: string;
  /** Opens the referenced ticket for a "#N" ref (ticket #364). Without it,
   *  refs still render (styled the same) but do nothing when clicked. */
  onOpenTicketByNumber?(ref: string): void;
  className?: string;
  testId?: string;
}

export function TicketMarkdown({
  text,
  onOpenTicketByNumber,
  className,
  testId,
}: TicketMarkdownProps) {
  const parts = useMemo(() => splitMermaidFences(text), [text]);

  const openRefFrom = (target: EventTarget | null) => {
    if (!onOpenTicketByNumber) return;
    const el = (target as HTMLElement | null)?.closest?.(
      ".ticket-ref-link",
    ) as HTMLElement | null;
    const ref = el?.dataset.ticketRef;
    if (ref) onOpenTicketByNumber(ref);
  };

  const onClick = (e: MouseEvent<HTMLDivElement>) => openRefFrom(e.target);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    if (!(e.target as HTMLElement).closest?.(".ticket-ref-link")) return;
    e.preventDefault();
    openRefFrom(e.target);
  };

  return (
    <div
      className={`fd-ticket-markdown${className ? ` ${className}` : ""}`}
      data-testid={testId}
      onClick={onClick}
      onKeyDown={onKeyDown}
    >
      {parts.map((part, i) =>
        i % 2 === 1 ? (
          <MermaidBlock key={i} source={part} />
        ) : (
          <MarkdownHtml key={i} text={part} />
        ),
      )}
    </div>
  );
}

function MarkdownHtml({ text }: { text: string }) {
  const html = useMemo(() => ticketMarkdownToHtml(text), [text]);
  if (!text.trim()) return null;
  // Sanitized just above via DOMPurify — never render marked's raw output.
  return <div className="md" dangerouslySetInnerHTML={{ __html: html }} />;
}

/* ── mermaid ────────────────────────────────────────────────────── */

function MermaidBlock({ source }: { source: string }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [failed, setFailed] = useState(false);
  const domId = `fd-mermaid-${useId().replace(/[^a-zA-Z0-9]/g, "")}`;

  useEffect(() => {
    let cancelled = false;
    setFailed(false);
    import("mermaid")
      .then(async ({ default: mermaid }) => {
        mermaid.initialize({ startOnLoad: false, securityLevel: "strict" });
        const { svg } = await mermaid.render(domId, source);
        if (cancelled || !containerRef.current) return;
        containerRef.current.innerHTML = DOMPurify.sanitize(svg, {
          USE_PROFILES: { svg: true, svgFilters: true },
        });
      })
      .catch(() => {
        // Broken diagram syntax (or the render failed for any other reason)
        // — fall back to a plain code block instead of an empty gap or a
        // crashed view.
        if (!cancelled) setFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [domId, source]);

  if (failed) {
    return (
      <pre className="mermaid-fallback" data-testid="mermaid-fallback">
        <code>{source}</code>
      </pre>
    );
  }

  return (
    <div
      className="mermaid-diagram"
      data-testid="mermaid-diagram"
      ref={containerRef}
    />
  );
}
