/**
 * The in-app release notes — a PARSER, not the data.
 *
 * The notes themselves live in `releaseNotes.md`, as plain text. That file is the
 * hand-maintained SSOT; this module only reads it.
 *
 * **Why the text is not in this file anymore.** It used to be: 700 German
 * sentences inside double-quoted TS string literals. That made prose into CODE —
 * and a single ASCII `"` in a sentence closed the string, `tsc -b` failed with
 * `TS1005`, and the whole production build was dead. It happened four times; the
 * last one (v0.6.592) shipped a broken build to `main` and blocked the deploy. A
 * release note is documentation. It must never be able to break the program it
 * documents.
 *
 * Now the text passes through a `?raw` import — bytes, never code. A quote, a
 * backslash, a semicolon, an unbalanced brace: all harmless.
 *
 * The parser is deliberately FORGIVING at runtime and STRICT in the tests: a line
 * it cannot read is collected in `MALFORMED_LINES` instead of throwing, so a typo
 * can never blank the dialog for a user — and `releaseNotes.test.ts` asserts that
 * list is empty, so the typo still fails CI.
 */

import source from "./releaseNotes.md?raw";

/** feat = new capability, fix = bug fix, perf = performance. */
export type ReleaseNoteKind = "feat" | "fix" | "perf";

export interface ReleaseNoteEntry {
  kind: ReleaseNoteKind;
  /** Short user-facing description (German, mirrors the commit subject). */
  text: string;
  /** Originating subsystem/scope, e.g. "extrude", "sketch". */
  scope?: string;
  /** Originating ticket id(s), if the change came from a Report Issue ticket. */
  tickets?: string[];
}

export interface ReleaseNote {
  /** Semantic version without the leading "v", e.g. "0.6.342". */
  version: string;
  entries: ReleaseNoteEntry[];
}

/** `## 0.6.593` — starts a release. */
const VERSION_LINE = /^##\s+(\d+\.\d+\.\d+)\s*$/;

/** `- fix(brep) #348 #350: text …` — scope and tickets optional, text is free. */
const ENTRY_LINE =
  /^-\s+(feat|fix|perf)(?:\(([^)]*)\))?((?:\s+#\S+)*)\s*:\s*(\S.*)$/;

export interface ParsedNotes {
  releases: ReleaseNote[];
  /** Lines that look like entries but could not be read. Empty in a healthy file;
   *  `releaseNotes.test.ts` fails when it is not. NEVER thrown — a broken note
   *  must not take the app down with it. */
  malformed: string[];
}

export function parseReleaseNotes(md: string): ParsedNotes {
  const releases: ReleaseNote[] = [];
  const malformed: string[] = [];
  let current: ReleaseNote | null = null;

  for (const raw of md.split("\n")) {
    const version = VERSION_LINE.exec(raw);
    if (version) {
      current = { version: version[1], entries: [] };
      releases.push(current);
      continue;
    }

    // Everything before the first `##` is the file's own preamble — skip it, and
    // with it the format example (whose indented lines look like entries).
    if (!current) continue;

    const line = raw.trimEnd();
    if (line.startsWith("- ")) {
      const m = ENTRY_LINE.exec(line);
      if (!m) {
        malformed.push(line);
        continue;
      }
      const [, kind, scope, tickets, text] = m;
      current.entries.push({
        kind: kind as ReleaseNoteKind,
        text,
        ...(scope ? { scope } : {}),
        // Stored WITHOUT the leading `#`: the dialog renders `#{t}` itself, so
        // keeping it here would print `##348`. The file writes `#348` because
        // that is how a human reads it.
        ...(tickets.trim()
          ? { tickets: tickets.trim().split(/\s+/).map((t) => t.replace(/^#/, "")) }
          : {}),
      });
      continue;
    }

    // An indented continuation line belongs to the entry above it, so a long note
    // may be wrapped in the source without changing what the user reads.
    const last = current.entries[current.entries.length - 1];
    if (last && /^\s+\S/.test(raw)) {
      last.text += " " + line.trim();
    }
  }

  return { releases, malformed };
}

const parsed = parseReleaseNotes(source);

/** All releases, newest first — exactly the order they appear in the file. */
export const RELEASE_NOTES: ReleaseNote[] = parsed.releases;

/** Unreadable lines in the real file — asserted empty by the test suite. */
export const MALFORMED_LINES: string[] = parsed.malformed;
