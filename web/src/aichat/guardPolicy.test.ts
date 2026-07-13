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
import { guardCode } from "baseaichat";
import { GUARDED_APP_GLOBALS } from "./setup";

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
