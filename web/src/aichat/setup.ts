/**
 * Wiring of the chat agent (ticket #347): API + guard + prompt + context.
 *
 * Five decisions live here, and each one has a reason:
 *
 *  · **The API object IS the security boundary — not the guard.** Since
 *    baseaichat v0.2.0 the guard is a BLOCKLIST: it restricts language
 *    constructs (`this`, prototype chain, `window`, `fetch`, `import()`, …), not
 *    the vocabulary, so an unknown free identifier now PASSES. That is what makes
 *    a deep, dynamic API like the KernelStore usable at all — and it means the
 *    only thing standing between the model and the raw kernel is what we put in
 *    the API object. Hence `extraBlocked` below: FieldDraft installs debug
 *    globals (`__fdStore`, `__fdKernel`, `__threeRenderer`, …) in dev builds, and
 *    those would hand the model the unguarded kernel straight past this file.
 *  · **Undo is the safety net, not the guard.** The guard says nothing about
 *    whether the RESULT is what the user wanted. Every API mutation commits
 *    through the store, so each one is a normal undo step — a bad turn is Ctrl+Z,
 *    as often as it made changes. (One undo step per TURN would need undo
 *    grouping in the kernel; that does not exist yet and is not worth faking.)
 *  · **The app context is a SUMMARY, never the store.** `JSON.stringify(store)`
 *    per turn would blow up cost and context. The model gets the feature list
 *    (id/type/name), selection, scene size — and pulls schemas and parameters on
 *    demand through the API.
 *  · **Floats are rounded.** CAD arithmetic produces `0.30000000000000004`; that
 *    is noise that costs tokens and confuses the model.
 *  · **The UI it reads is the APP, not the chat.** `readUIState` is scoped to the
 *    app shell, otherwise the agent reads its own panel back.
 */

import {
  createEvaluator,
  createUIActions,
  expandRuntimeTypes,
  readUIState,
  roundValues,
  type ChatAgentOptions,
} from "baseaichat";
import type { KernelStore } from "../store";
import { createFieldDraftApi } from "./fieldDraftApi";
import { API_DESCRIPTION } from "./api";
import instructions from "./instructions.md?raw";

/**
 * FieldDraft's debug globals — blocked for the agent (guardPolicy.test.ts).
 *
 * They exist only in dev builds, but the v0.2.0 guard is a BLOCKLIST: a free
 * `__fdStore` resolves, and the model would be holding the raw kernel with no
 * curation, no validation and no undo semantics — straight past fieldDraftApi.ts,
 * which is the actual security boundary. There is nothing in them the API does
 * not offer safely.
 */
export const GUARDED_APP_GLOBALS = [
  "__fdStore",
  "__fdKernel",
  "__threeRenderer",
  "__fdInvalidate",
  "__fdSketchCamera",
  "__fdSketchToScreen",
  "__fdPerfMonitor",
  "__fdBenchmark",
  "__fdTelemetry",
  "__fdActionLog",
] as const;

/** The app's own DOM — everything outside the chat panel. */
function appRoot(): HTMLElement {
  return (
    (document.querySelector(".fd-shell") as HTMLElement | null) ??
    document.body
  );
}

export interface AIChatConfig {
  enabled: boolean;
  models: string[];
}

/**
 * Build the options for `<ChatPanel agent={…}>`. `models` comes from the server
 * (/api/aichat/config): the proxy's allowlist is the authority, so the client
 * never asks for a model that would come back as a 403.
 */
export function agentOptions(
  store: KernelStore,
  models: string[],
): ChatAgentOptions {
  const api = createFieldDraftApi(store);

  const evaluate = createEvaluator({
    api: {
      ...api,
      ...createUIActions({ root: appRoot() }),
      readUIState: () => readUIState({ root: appRoot() }),
    },
    transformResult: (r) => roundValues(r, 3),
    extraBlocked: [...GUARDED_APP_GLOBALS],
    // CAD sequences are longer than a to-do list's: a plate with four holes and
    // a fillet is already a dozen calls.
    maxCallsPerWindow: 120,
  });

  // Filled with the values that exist RIGHT NOW — including the user's own
  // script features, which no static description could know about.
  const description = expandRuntimeTypes(API_DESCRIPTION, {
    featureTypes: store.kernel.listFeatureTypes(),
    variableNames: store.getState().variables.map((v) => v.name),
  });

  const model = models[0] ?? "google/gemini-3-flash-preview";

  return {
    provider: "openrouter",
    model,
    visionModel: model,
    proxyUrl: "/aichat",
    systemPrompt: [instructions, "## API\n\n```ts\n" + description + "\n```"],
    toolDescription:
      "Führt JavaScript gegen die FieldDraft-App aus (Feature-Baum lesen und " +
      "bauen, Ansicht steuern, Oberfläche bedienen).\n\n```ts\n" +
      description +
      "\n```",
    evaluate,
    appContext: () => {
      const doc = api.getDocument();
      return JSON.stringify({
        document: doc,
        // The UI the agent may operate — data-testid-tagged controls only.
        ui: readUIState({ root: appRoot() }),
      });
    },
  };
}
