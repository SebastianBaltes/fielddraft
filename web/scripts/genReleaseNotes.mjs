#!/usr/bin/env node
// One-shot seeder for src/releaseNotes.ts from the git history.
//
// The project's commit discipline is the source of truth: every user-facing
// commit ends in `(… vX.Y.Z)` and carries a conventional-commit type. This
// script parses `git log`, keeps the release-note-worthy types (feat/fix/perf),
// groups them by version (newest first), and emits the typed RELEASE_NOTES
// array.
//
// It is a DEV CONVENIENCE, run by hand to (re)seed history — NOT part of the
// build. Going forward the data file is the hand-maintained SSOT: add one entry
// per version bump (see CLAUDE.md's "bump version on every change" rule). Re-run
// only to rebuild the whole list from scratch.
//
//   node scripts/genReleaseNotes.mjs > src/releaseNotes.ts
//
import { execSync } from "node:child_process";

/** Conventional-commit types we surface to users. Others (docs/test/chore/
 *  refactor/style/build/merge/revert) are internal and excluded. */
const KEEP = new Set(["feat", "fix", "perf"]);

/** Strip the trailing `(… vX.Y.Z)` / ` vX.Y.Z` clause and return
 *  { text, version, clause }. */
function splitVersion(rest) {
  // Last parenthesised group that ends the string and contains a version.
  const paren = rest.match(/\s*\(([^()]*v\d+\.\d+\.\d+[^()]*)\)\s*$/);
  if (paren) {
    const v = paren[1].match(/v(\d+\.\d+\.\d+)/);
    return { text: rest.slice(0, paren.index).trim(), version: v[1], clause: paren[1] };
  }
  const bare = rest.match(/\s*v(\d+\.\d+\.\d+)\s*$/);
  if (bare) {
    return { text: rest.slice(0, bare.index).trim(), version: bare[1], clause: "" };
  }
  return null;
}

/** Pull ticket slugs out of the trailing clause only (so `Ctrl+M / K` in the
 *  body never reads as a ticket). Slugs are the 3–6 char alnum ids the ticket
 *  system mints, after the `Ticket`/`Tickets` keyword. */
function extractTickets(clause) {
  const m = clause.match(/Tickets?\s+([^,)]*)/i);
  if (!m) return [];
  return [...m[1].matchAll(/[0-9a-z]{3,6}/g)].map((x) => x[0]);
}

function semverDesc(a, b) {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let i = 0; i < 3; i++) {
    if (pa[i] !== pb[i]) return pb[i] - pa[i];
  }
  return 0;
}

const raw = execSync("git log --pretty=format:%s", { encoding: "utf-8", maxBuffer: 64 * 1024 * 1024 });
const byVersion = new Map(); // version -> { entries: [], seen: Set }

for (const subject of raw.split("\n")) {
  const head = subject.match(/^(feat|fix|perf)(?:\(([^)]*)\))?:\s*(.*)$/);
  if (!head) continue;
  const [, kind, scope, rest] = head;
  if (!KEEP.has(kind)) continue;
  const sv = splitVersion(rest);
  if (!sv || !sv.text) continue;

  const tickets = extractTickets(sv.clause);
  if (!byVersion.has(sv.version)) byVersion.set(sv.version, { entries: [], seen: new Set() });
  const bucket = byVersion.get(sv.version);
  const dedupeKey = `${kind}|${sv.text}`;
  if (bucket.seen.has(dedupeKey)) continue;
  bucket.seen.add(dedupeKey);

  const entry = { kind, text: sv.text };
  if (scope) entry.scope = scope;
  if (tickets.length) entry.tickets = tickets;
  bucket.entries.push(entry);
}

const releases = [...byVersion.entries()]
  .sort((a, b) => semverDesc(a[0], b[0]))
  .map(([version, b]) => ({ version, entries: b.entries }));

const header = `// AUTO-SEEDED from git history by scripts/genReleaseNotes.mjs, then the
// hand-maintained SSOT for the in-app release notes. Add one entry per version
// bump (newest version first); re-run the seeder only to rebuild from scratch.

export type ReleaseNoteKind = "feat" | "fix" | "perf";

export interface ReleaseNoteEntry {
  /** feat = new capability, fix = bug fix, perf = performance. */
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

/** All releases, newest first. */
export const RELEASE_NOTES: ReleaseNote[] = `;

process.stdout.write(header + JSON.stringify(releases, null, 2) + ";\n");
