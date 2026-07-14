/**
 * The release notes must never be able to break the build (v0.6.594).
 *
 * They used to be TS string literals, so prose was code: one ASCII `"` inside a
 * German sentence closed the string and `tsc -b` died — four times, the last one
 * on `main`. The text now lives in `releaseNotes.md` and is parsed at runtime.
 * These tests pin down both halves of that fix:
 *
 *  1. ANY character in a note is harmless — quotes, backslashes, braces, colons.
 *  2. A malformed line is caught HERE, not by a user: the parser collects it
 *     instead of throwing, and this suite fails on it.
 */

import { describe, expect, it } from "vitest";
import {
  MALFORMED_LINES,
  RELEASE_NOTES,
  parseReleaseNotes,
} from "./releaseNotes";

describe("the real release-notes file", () => {
  it("parses, and every line in it is readable", () => {
    expect(MALFORMED_LINES).toEqual([]);
    expect(RELEASE_NOTES.length).toBeGreaterThan(600);
  });

  it("lists the newest release first and never repeats a version", () => {
    const versions = RELEASE_NOTES.map((r) => r.version);
    expect(new Set(versions).size).toBe(versions.length);

    const asNumbers = versions.map((v) => v.split(".").map(Number));
    for (let i = 1; i < asNumbers.length; i++) {
      const [aMaj, aMin, aPatch] = asNumbers[i - 1];
      const [bMaj, bMin, bPatch] = asNumbers[i];
      const prev = aMaj * 1e6 + aMin * 1e3 + aPatch;
      const next = bMaj * 1e6 + bMin * 1e3 + bPatch;
      expect(prev, `${versions[i - 1]} must be newer than ${versions[i]}`)
        .toBeGreaterThan(next);
    }
  });

  it("gives every entry a kind and a non-empty text", () => {
    for (const rel of RELEASE_NOTES) {
      expect(rel.entries.length, `${rel.version} has entries`).toBeGreaterThan(0);
      for (const e of rel.entries) {
        expect(["feat", "fix", "perf"]).toContain(e.kind);
        expect(e.text.trim()).not.toBe("");
      }
    }
  });
});

describe("a note can contain anything a sentence can", () => {
  it("survives the exact character that used to kill the build", () => {
    const { releases: notes, malformed } = parseReleaseNotes(
      '## 1.2.3\n- fix(build): Er schrieb "voraus" statt „voraus“ — und der Build starb.\n',
    );
    expect(malformed).toEqual([]);
    expect(notes[0].entries[0].text).toBe(
      'Er schrieb "voraus" statt „voraus“ — und der Build starb.',
    );
  });

  it("survives backslashes, braces, semicolons and further colons", () => {
    const { releases: notes, malformed } = parseReleaseNotes(
      "## 1.2.3\n- perf(sdf): C:\\Pfad\\{a;b}: 50% schneller `${x}` \\n\n",
    );
    expect(malformed).toEqual([]);
    expect(notes[0].entries[0].text).toBe("C:\\Pfad\\{a;b}: 50% schneller `${x}` \\n");
  });
});

describe("the format", () => {
  it("reads scope and tickets when present, and omits them when not", () => {
    const { releases: notes } = parseReleaseNotes(
      "## 1.0.0\n- fix(brep) #348 #350: mit allem\n- feat: nur der Text\n",
    );
    expect(notes[0].entries[0]).toEqual({
      kind: "fix",
      scope: "brep",
      tickets: ["348", "350"],
      text: "mit allem",
    });
    expect(notes[0].entries[1]).toEqual({ kind: "feat", text: "nur der Text" });
  });

  it("joins an indented continuation line onto the entry above it", () => {
    const { releases: notes } = parseReleaseNotes(
      "## 1.0.0\n- fix: erste Zeile\n  zweite Zeile\n",
    );
    expect(notes[0].entries[0].text).toBe("erste Zeile zweite Zeile");
  });

  it("collects a malformed entry instead of throwing", () => {
    // `fx` is not a kind — the app must still render every OTHER note.
    const { releases: notes, malformed } = parseReleaseNotes("## 1.0.0\n- fx: Tippfehler\n- fix: gut\n");
    expect(notes[0].entries).toHaveLength(1);
    expect(malformed).toEqual(["- fx: Tippfehler"]);
  });
});
