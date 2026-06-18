/**
 * User-action log (ticket: Report-Issue action history).
 *
 * A lightweight, privacy-conscious ring buffer of what the user did, so an
 * "Report an issue" can ship a replayable trace alongside the screenshot +
 * project. Two layers, one buffer:
 *
 *  - **Semantic actions** — the high-signal intent: entered a sketch, picked
 *    the Line tool, applied a constraint, selected a feature, picked an entity
 *    in the viewport. Emitted explicitly via `logUserAction()` from the store
 *    and a few UI boundaries (things that are NOT plain DOM clicks).
 *  - **Raw input** — pointer/keyboard/wheel/resize with coordinates and the
 *    nearest `data-testid`, so interaction bugs (drag distance, which element,
 *    timing) are reconstructable. Installed once via `installRawInputCapture`.
 *
 * Privacy: never logs typed text or character keys — only control/modifier
 * keys by name, otherwise just "printable". Coordinates, window size and
 * tool/feature names are safe.
 *
 * Plain module singleton — no React state, no store coupling, so any layer can
 * log without threading context. The Report dialog serialises it on demand.
 */

export interface ActionEntry {
  /** ms since page load (performance.now, rounded). */
  t: number;
  kind: "action";
  /** Coarse bucket: "sketch" | "feature" | "doc" | "pick" | "view" | … */
  cat: string;
  /** Specific action, e.g. "tool:sketch_line", "constraint:parallel". */
  name: string;
  /** Optional structured payload (ids, values — never raw typed text). */
  detail?: Record<string, unknown>;
}

export interface InputEntry {
  t: number;
  kind: "input";
  /** "pointerdown" | "pointerup" | "drag" | "wheel" | "key" | "resize" */
  type: string;
  x?: number;
  y?: number;
  button?: number;
  /** Wheel direction sign. */
  dy?: number;
  /** Control/modifier key name, or "printable" for ordinary characters. */
  key?: string;
  /** Active modifiers, e.g. "ctrl+shift". */
  mods?: string;
  /** Nearest `data-testid` (or tagName) under the event. */
  target?: string;
  /** Window size for resize entries. */
  w?: number;
  h?: number;
}

export type LogEntry = ActionEntry | InputEntry;

// Keep the buffer bounded: last MAX_ENTRIES, and drop anything older than
// MAX_AGE_MS relative to the newest entry (a multi-hour idle session shouldn't
// ship stale history). Both are generous — a few minutes of dense interaction.
const MAX_ENTRIES = 2000;
const MAX_AGE_MS = 5 * 60 * 1000;

const buffer: LogEntry[] = [];

function now(): number {
  // performance.now is monotonic and unaffected by clock changes. Falls back to
  // 0 in the (test) environments that lack it.
  return typeof performance !== "undefined" ? Math.round(performance.now()) : 0;
}

function push(entry: LogEntry): void {
  buffer.push(entry);
  const cutoff = entry.t - MAX_AGE_MS;
  // Trim by age from the front (cheap — entries are time-ordered)…
  while (buffer.length > 0 && buffer[0].t < cutoff) buffer.shift();
  // …then by count.
  while (buffer.length > MAX_ENTRIES) buffer.shift();
}

/**
 * Record a semantic action. `detail` must not contain raw typed text — pass
 * ids, type names, numeric values, query strings, but not field contents.
 */
export function logUserAction(
  cat: string,
  name: string,
  detail?: Record<string, unknown>,
): void {
  push({ t: now(), kind: "action", cat, name, ...(detail ? { detail } : {}) });
}

/** Current log, oldest-first. Returned as a copy so callers can't mutate it. */
export function getActionLog(): LogEntry[] {
  return buffer.slice();
}

/** Drop the whole log (e.g. on a fresh document / after a successful report). */
export function clearActionLog(): void {
  buffer.length = 0;
}

/** Serialise the log to a compact JSON document for the ticket payload. */
export function serializeActionLog(): {
  capturedAt: string;
  entryCount: number;
  spanMs: number;
  entries: LogEntry[];
} {
  const entries = getActionLog();
  const spanMs =
    entries.length > 0 ? entries[entries.length - 1].t - entries[0].t : 0;
  return {
    capturedAt: new Date().toISOString(),
    entryCount: entries.length,
    spanMs,
    entries,
  };
}

// ─── raw input capture ──────────────────────────────────────────────

/** Control keys we log by name (everything else is "printable" — never the
 *  actual character, to keep typed text out of the log). */
const NAMED_KEYS = new Set([
  "Escape",
  "Enter",
  "Tab",
  "Backspace",
  "Delete",
  "Insert",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  " ",
  "F1",
  "F2",
  "F3",
  "F4",
  "F5",
  "F6",
  "F7",
  "F8",
  "F9",
  "F10",
  "F11",
  "F12",
]);

function modString(e: {
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}): string | undefined {
  const m: string[] = [];
  if (e.ctrlKey) m.push("ctrl");
  if (e.shiftKey) m.push("shift");
  if (e.altKey) m.push("alt");
  if (e.metaKey) m.push("meta");
  return m.length ? m.join("+") : undefined;
}

/** Nearest meaningful identifier for the event target: the closest
 *  `data-testid` walking up the tree, else the tagName. */
function targetLabel(target: EventTarget | null): string | undefined {
  let el = target instanceof Element ? target : null;
  for (let i = 0; el && i < 6; i++) {
    const tid = el.getAttribute("data-testid");
    if (tid) return tid;
    el = el.parentElement;
  }
  return target instanceof Element ? target.tagName.toLowerCase() : undefined;
}

let installed = false;

/**
 * Install global raw-input capture (idempotent). Listens in the CAPTURE phase
 * so it sees events even when a handler stops propagation. pointermove is
 * sampled only while a button is held (drags), throttled, so static mouse
 * motion doesn't flood the buffer.
 */
export function installRawInputCapture(): void {
  if (installed || typeof window === "undefined") return;
  installed = true;

  let dragging = false;
  let lastMoveT = 0;

  window.addEventListener(
    "pointerdown",
    (e) => {
      dragging = true;
      push({
        t: now(),
        kind: "input",
        type: "pointerdown",
        x: Math.round(e.clientX),
        y: Math.round(e.clientY),
        button: e.button,
        mods: modString(e),
        target: targetLabel(e.target),
      });
    },
    true,
  );

  window.addEventListener(
    "pointermove",
    (e) => {
      if (!dragging) return; // only sample drags
      const t = now();
      if (t - lastMoveT < 120) return; // throttle
      lastMoveT = t;
      push({
        t,
        kind: "input",
        type: "drag",
        x: Math.round(e.clientX),
        y: Math.round(e.clientY),
        button: e.buttons,
        target: targetLabel(e.target),
      });
    },
    true,
  );

  window.addEventListener(
    "pointerup",
    (e) => {
      dragging = false;
      push({
        t: now(),
        kind: "input",
        type: "pointerup",
        x: Math.round(e.clientX),
        y: Math.round(e.clientY),
        button: e.button,
        target: targetLabel(e.target),
      });
    },
    true,
  );

  window.addEventListener(
    "wheel",
    (e) => {
      push({
        t: now(),
        kind: "input",
        type: "wheel",
        x: Math.round(e.clientX),
        y: Math.round(e.clientY),
        dy: Math.sign(e.deltaY),
        target: targetLabel(e.target),
      });
    },
    { capture: true, passive: true },
  );

  window.addEventListener(
    "keydown",
    (e) => {
      // Privacy: only name control keys; ordinary characters log as "printable"
      // (with modifiers, so shortcuts like ctrl+z are still legible).
      const key = NAMED_KEYS.has(e.key)
        ? e.key === " "
          ? "Space"
          : e.key
        : e.key.length === 1 && (e.ctrlKey || e.metaKey || e.altKey)
          ? e.key
          : e.key.length === 1
            ? "printable"
            : e.key;
      push({
        t: now(),
        kind: "input",
        type: "key",
        key,
        mods: modString(e),
        target: targetLabel(e.target),
      });
    },
    true,
  );

  let resizeT = 0;
  let lastW = -1;
  let lastH = -1;
  const recordResize = () => {
    const t = now();
    if (t - resizeT < 200) return; // throttle bursts
    if (window.innerWidth === lastW && window.innerHeight === lastH) return; // dedup boot churn
    resizeT = t;
    lastW = window.innerWidth;
    lastH = window.innerHeight;
    push({ t, kind: "input", type: "resize", w: lastW, h: lastH });
  };
  window.addEventListener("resize", recordResize);

  // Seed the log with the initial window size so a report always has the
  // viewport dimensions, even with no resize during the session.
  recordResize();
}
