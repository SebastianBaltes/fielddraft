/**
 * Agent-written FieldSkript, and the gate in front of it (ticket #347).
 *
 * The operator accepted the risk: the assistant may write FieldSkript and run it
 * in our script runtime. That is a genuinely different level of access from the
 * `evaluate` API — a script does not call our curated functions, it runs INSIDE
 * the feature build, with everything FieldSkript can do. Two things therefore
 * stand between the model and the document, and they do different jobs:
 *
 *  1. **`guardScript`** (baseaichat v0.3.0) — restricts the script's VOCABULARY:
 *     no `window`, `fetch`, `eval`, `import()`, no imports except `"fielddraft"`,
 *     and none of FieldDraft's debug globals. It parses TypeScript and permits
 *     classes and `this`, which `guardCode` cannot. It does NOT restrict what the
 *     script runtime is allowed to do — that is the accepted risk, not a hole we
 *     forgot.
 *  2. **The user.** A proposed script is never executed on the model's say-so.
 *     It lands here as a PENDING proposal, the app shows the source, and only an
 *     explicit "Ausführen" installs and runs it. Discarding is the default.
 *
 * `guardScript` only holds because our runtime is STRICT (verified: sucrase emits
 * a "use strict" prologue and workerScriptHost/evalDefinition now prepend one
 * themselves). In sloppy mode a plain call binds `this` to `globalThis`, and
 * `this.fetch(…)` would walk past the whole blocklist.
 */

import { guardScript } from "baseaichat";
import type { KernelStore } from "../store";
import { GUARDED_APP_GLOBALS } from "./guardedGlobals";

/** The only module a FieldSkript may import (workerScriptHost's require shim
 *  rejects everything else at runtime; the guard rejects it before that). */
const ALLOWED_IMPORTS = ["fielddraft"];

export interface ScriptProposal {
  id: string;
  /** Feature name the script will show up as. */
  name: string;
  /** The TypeScript source, exactly as the model wrote it. */
  source: string;
  /** What the assistant says it does — shown above the code. */
  intent: string;
}

type Listener = () => void;

let pending: ScriptProposal | null = null;
const listeners = new Set<Listener>();
let counter = 0;

function emit(): void {
  for (const l of listeners) l();
}

export function subscribeScriptProposal(l: Listener): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function getScriptProposal(): ScriptProposal | null {
  return pending;
}

export function discardScriptProposal(): void {
  if (!pending) return;
  pending = null;
  emit();
}

/** The verdict the MODEL gets back — it reads the reason, fixes, retries. */
export interface ProposeResult {
  ok: boolean;
  status: string;
  reason?: string;
  line?: number;
}

/**
 * Guard a script the assistant wrote and, if it passes, park it for the user.
 * Never executes anything — see the module doc.
 */
export function proposeScript(
  name: string,
  source: string,
  intent = "",
): ProposeResult {
  const verdict = guardScript(source, {
    allowImportsFrom: ALLOWED_IMPORTS,
    extraBlocked: [...GUARDED_APP_GLOBALS],
  });
  if (!verdict.ok) {
    return {
      ok: false,
      status: "rejected",
      reason: verdict.reason,
      line: verdict.at?.line,
    };
  }
  pending = { id: `proposal-${++counter}`, name, source, intent };
  emit();
  return {
    ok: true,
    status:
      "awaiting_user_confirmation — the script is shown to the user; it runs " +
      "only if they accept it. Do not assume it ran.",
  };
}

/**
 * The USER accepted the pending proposal: compile it, register it as a script
 * feature type, and instantiate it once. Returns the new feature's id.
 *
 * Compilation errors surface here, not at the guard — a script can be perfectly
 * harmless and still not compile.
 */
export function acceptScriptProposal(
  store: KernelStore,
): { ok: true; featureId: number | null } | { ok: false; error: string } {
  const proposal = pending;
  if (!proposal) return { ok: false, error: "no pending script" };

  const tabId = store.createTab("script");
  const saved = store.saveScriptTab(tabId, proposal.source);
  if (!saved.ok) {
    return { ok: false, error: saved.error };
  }
  const featureId = store.insertScriptInstance(saved.record.id);
  pending = null;
  emit();
  return { ok: true, featureId };
}
