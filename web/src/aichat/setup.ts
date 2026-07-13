/**
 * Wiring of the chat agent (ticket #347): API + guard + prompt + context.
 *
 * Four decisions live here, and each one has a reason:
 *
 *  · **Undo is the safety net, not the guard.** The guard controls WHICH
 *    functions the model can reach; it says nothing about whether the result is
 *    what the user wanted. Every API mutation commits through the store, so each
 *    one is a normal undo step — a bad turn is Ctrl+Z, as often as it made
 *    changes. (A single undo step per TURN would need undo grouping in the
 *    kernel; that does not exist yet and is not worth faking here.)
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
