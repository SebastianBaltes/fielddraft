/**
 * Chunk 3a: minimal ElementInfo render test — kind label + clear button.
 * The kernel resolves no bodies (bodyCount → 0), so the panel falls back to
 * "not resolvable" but still renders its header, kind label, clear button and
 * the canonical query string.
 */

import { describe, expect, it, beforeEach } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ElementInfo } from "./ElementInfo";
import type { KernelStore, StoreState } from "./store";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function makeStore(cleared: { called: boolean }): KernelStore {
  return {
    kernel: { bodyCount: () => 0 },
    clearElementSelection: () => {
      cleared.called = true;
    },
  } as unknown as KernelStore;
}

function makeState(): StoreState {
  return {
    selectedElement: { kind: "edge", idHash: "98765" },
    features: [],
  } as unknown as StoreState;
}

describe("ElementInfo", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  it("renders the German kind label and the query string", () => {
    const cleared = { called: false };
    act(() => {
      root.render(<ElementInfo store={makeStore(cleared)} state={makeState()} />);
    });
    expect(
      container.querySelector('[data-testid="element-info-kind"]')?.textContent,
    ).toBe("Kante");
    expect(
      container.querySelector('[data-testid="element-info-query"]')?.textContent,
    ).toBe("qEdge(98765)");
    act(() => root.unmount());
  });

  it("clear button calls store.clearElementSelection", () => {
    const cleared = { called: false };
    act(() => {
      root.render(<ElementInfo store={makeStore(cleared)} state={makeState()} />);
    });
    const btn = container.querySelector<HTMLButtonElement>(
      '[data-testid="element-info-clear"]',
    );
    act(() => {
      btn?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(cleared.called).toBe(true);
    act(() => root.unmount());
  });
});
