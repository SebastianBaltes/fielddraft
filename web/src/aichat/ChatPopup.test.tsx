/**
 * The admin-only model picker (ticket #361).
 *
 * `ChatPopup` renders BaseAIChat's `ChatPanel` with `headerExtra` set to a
 * `<select>` of the server's allowlisted models — but only for admins; a
 * non-admin gets `headerExtra={undefined}` so no picker ever reaches a
 * regular user. We stub `ChatPanel` itself (real one is the submodule's
 * floating-launcher UI, irrelevant here) so this test is pinned on exactly
 * that wiring: what FieldDraft passes into the slot.
 */

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { ReactNode } from "react";
import { ChatPopup } from "./ChatPopup";
import type { KernelStore } from "../store";

vi.mock("baseaichat", async (importOriginal) => ({
  ...(await importOriginal<typeof import("baseaichat")>()),
  // The real ChatPanel is a floating launcher/panel with its own open state;
  // all this test needs is to see what lands in `headerExtra`.
  ChatPanel: ({ headerExtra }: { headerExtra?: ReactNode }) => (
    <div data-testid="chat-panel">{headerExtra}</div>
  ),
}));

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

const MODELS = ["google/gemini-3-flash-preview", "anthropic/claude-sonnet-4.5"];

// Minimal stub — ChatPopup/agentOptions only touch `kernel.listFeatureTypes()`
// and `getState().variables` synchronously; everything else is wired up as
// unexecuted closures (see setup.test.ts).
const stubStore = {
  kernel: { listFeatureTypes: () => [] },
  getState: () => ({ variables: [] }),
} as unknown as KernelStore;

let container: HTMLDivElement;
let root: Root;

function render(isAdmin: boolean) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(<ChatPopup store={stubStore} isAdmin={isAdmin} />);
  });
}

async function flushConfigFetch() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

beforeEach(() => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => ({
      ok: true,
      json: async () => ({ enabled: true, models: MODELS }),
    })),
  );
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

describe("ChatPopup model picker", () => {
  it("shows the model dropdown for an admin", async () => {
    render(true);
    await flushConfigFetch();

    const select = container.querySelector("select.fd-aichat-model-select");
    expect(select).not.toBeNull();
    const options = Array.from(select!.querySelectorAll("option")).map(
      (o) => (o as HTMLOptionElement).value,
    );
    expect(options).toEqual(MODELS);
  });

  it("hides the model dropdown for a non-admin", async () => {
    render(false);
    await flushConfigFetch();

    expect(
      container.querySelector("select.fd-aichat-model-select"),
    ).toBeNull();
  });
});
