/**
 * `agentOptions` model selection (ticket #361).
 *
 * The server's /api/aichat/config `models` allowlist is the authority; an
 * admin's header dropdown (ChatPopup.tsx) only ever narrows the choice to one
 * of those entries via `selectedModel`. This pins the fallback chain: an
 * explicit selection wins, otherwise the first allowlisted model, otherwise
 * the hardcoded default.
 */

import { describe, expect, it } from "vitest";
import { agentOptions } from "./setup";
import type { KernelStore } from "../store";

// `agentOptions` only touches `kernel.listFeatureTypes()` and
// `getState().variables` synchronously (to build the model-facing API
// description) — everything else it wires up lazily as closures, so this
// minimal stand-in is enough to exercise the model-selection logic without
// booting a real kernel.
function fakeStore(): KernelStore {
  return {
    kernel: { listFeatureTypes: () => [] },
    getState: () => ({ variables: [] }),
  } as unknown as KernelStore;
}

const MODELS = [
  "google/gemini-3-flash-preview",
  "anthropic/claude-haiku-4.5",
  "anthropic/claude-sonnet-4.5",
];

describe("agentOptions model selection", () => {
  it("uses the first allowlisted model when none is selected", () => {
    const opts = agentOptions(fakeStore(), MODELS);
    expect(opts.model).toBe(MODELS[0]);
    expect(opts.visionModel).toBe(MODELS[0]);
  });

  it("uses the explicitly selected model when given", () => {
    const opts = agentOptions(fakeStore(), MODELS, "anthropic/claude-sonnet-4.5");
    expect(opts.model).toBe("anthropic/claude-sonnet-4.5");
    expect(opts.visionModel).toBe("anthropic/claude-sonnet-4.5");
  });

  it("falls back to the hardcoded default when the allowlist is empty", () => {
    const opts = agentOptions(fakeStore(), []);
    expect(opts.model).toBe("google/gemini-3-flash-preview");
  });
});
