/**
 * K7.21c — Compact-Mode-Tooltips.
 *
 * Guarantees that every `<button>` rendered by Toolbar.tsx carries a
 * non-empty `title` attribute. Compact Mode hides the text label so
 * the browser-native tooltip on hover is the only remaining way for a
 * user to discover what a given icon does — losing it silently would
 * break the feature.
 *
 * The test parses Toolbar.tsx statically rather than rendering the
 * component, because mounting the real Toolbar requires the full
 * KernelStore + WASM bridge and the guarantee we want is about the
 * source, not the runtime state.
 */

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const toolbarSource = readFileSync(resolve(here, "Toolbar.tsx"), "utf8");

/** Find every `<button ...>` opening tag in the source, including
 *  attributes that may span multiple lines. We stop at the first `>`
 *  that isn't inside a JSX expression (`{...}`) or a string. */
function extractButtonTags(src: string): string[] {
  const tags: string[] = [];
  const re = /<button\b/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(src)) !== null) {
    const start = m.index;
    let depth = 0;
    let quote: string | null = null;
    let i = start + "<button".length;
    for (; i < src.length; i++) {
      const c = src[i];
      if (quote) {
        if (c === quote && src[i - 1] !== "\\") quote = null;
        continue;
      }
      if (c === '"' || c === "'" || c === "`") {
        quote = c;
        continue;
      }
      if (c === "{") depth++;
      else if (c === "}") depth--;
      else if (c === ">" && depth === 0) {
        break;
      }
    }
    tags.push(src.slice(start, i + 1));
  }
  return tags;
}

describe("Toolbar tooltips (K7.21c)", () => {
  const buttons = extractButtonTags(toolbarSource);

  it("finds at least one button tag in Toolbar.tsx", () => {
    // Sanity: if the parser breaks silently we'd get an empty list
    // and every assertion below would vacuously pass.
    expect(buttons.length).toBeGreaterThan(5);
  });

  it("every <button> has a title attribute", () => {
    const missing = buttons.filter((tag) => !/\btitle\s*=/.test(tag));
    expect(
      missing,
      `buttons without a title attribute:\n${missing.join("\n---\n")}`,
    ).toEqual([]);
  });

  it("no button has an empty title", () => {
    // Matches title="" or title={""} or title={``} — any of those
    // would render an empty tooltip and defeat the point.
    const empties = buttons.filter((tag) =>
      /\btitle\s*=\s*(?:""|''|\{\s*(?:""|''|``)\s*\})/.test(tag),
    );
    expect(
      empties,
      `buttons with an empty title:\n${empties.join("\n---\n")}`,
    ).toEqual([]);
  });
});
