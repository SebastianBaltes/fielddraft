import { describe, expect, it, beforeEach, afterEach } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ReleaseNotesDialog } from "./ReleaseNotesDialog";
import { RELEASE_NOTES } from "./releaseNotes";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

function semverTuple(v: string): [number, number, number] {
  const p = v.split(".").map(Number);
  return [p[0], p[1], p[2]];
}

describe("RELEASE_NOTES data", () => {
  it("is non-empty and every entry is well-formed", () => {
    expect(RELEASE_NOTES.length).toBeGreaterThan(0);
    for (const rel of RELEASE_NOTES) {
      expect(rel.version).toMatch(/^\d+\.\d+\.\d+$/);
      expect(rel.entries.length).toBeGreaterThan(0);
      for (const e of rel.entries) {
        expect(["feat", "fix", "perf"]).toContain(e.kind);
        expect(e.text.trim().length).toBeGreaterThan(0);
        // No leftover version/ticket clause bled into the text.
        expect(e.text).not.toMatch(/v\d+\.\d+\.\d+/);
      }
    }
  });

  it("is ordered newest version first", () => {
    for (let i = 1; i < RELEASE_NOTES.length; i++) {
      const a = semverTuple(RELEASE_NOTES[i - 1].version);
      const b = semverTuple(RELEASE_NOTES[i].version);
      // a should be strictly greater than b (distinct, descending).
      expect(a[0] * 1e6 + a[1] * 1e3 + a[2]).toBeGreaterThan(
        b[0] * 1e6 + b[1] * 1e3 + b[2],
      );
    }
  });

  it("retains historical releases (e.g. the 20fc extrude fix in 0.6.342)", () => {
    const rel = RELEASE_NOTES.find((r) => r.version === "0.6.342");
    expect(rel).toBeDefined();
    expect(rel!.entries.some((e) => e.tickets?.includes("20fc"))).toBe(true);
  });
});

describe("ReleaseNotesDialog", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  function setFilter(value: string) {
    const input = container.querySelector<HTMLInputElement>(
      '[data-testid="release-notes-filter"]',
    )!;
    act(() => {
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLInputElement.prototype,
        "value",
      )!.set!;
      setter.call(input, value);
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
  }

  it("renders all releases unfiltered", () => {
    act(() => root.render(<ReleaseNotesDialog onClose={() => {}} />));
    const sections = container.querySelectorAll(".release");
    expect(sections.length).toBe(RELEASE_NOTES.length);
  });

  it("filters by ticket id down to the matching release", () => {
    act(() => root.render(<ReleaseNotesDialog onClose={() => {}} />));
    setFilter("20fc");
    const sections = container.querySelectorAll(".release");
    expect(sections.length).toBe(1);
    expect(sections[0].querySelector(".release-version")?.textContent).toBe(
      "v0.6.342",
    );
    expect(container.textContent).toContain("#20fc");
  });

  it("shows the empty state for a query with no hits", () => {
    act(() => root.render(<ReleaseNotesDialog onClose={() => {}} />));
    setFilter("zzz-no-such-thing-zzz");
    expect(
      container.querySelector('[data-testid="release-notes-empty"]'),
    ).not.toBeNull();
    expect(container.querySelectorAll(".release").length).toBe(0);
  });
});
