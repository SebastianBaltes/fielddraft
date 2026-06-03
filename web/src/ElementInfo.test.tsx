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

/** Records every store call chunk 3b's actions might make. */
interface StoreCalls {
  cleared: number;
  enterSketch: string[];
  addMateConnectorAtFace: string[];
  addFilletAtEdge: string[];
}

function makeCalls(): StoreCalls {
  return { cleared: 0, enterSketch: [], addMateConnectorAtFace: [], addFilletAtEdge: [] };
}

function makeStore(calls: StoreCalls): KernelStore {
  return {
    kernel: { bodyCount: () => 0 },
    clearElementSelection: () => {
      calls.cleared += 1;
    },
    enterSketch: (q: string) => calls.enterSketch.push(q),
    addMateConnectorAtFace: (q: string) => calls.addMateConnectorAtFace.push(q),
    addFilletAtEdge: (q: string) => calls.addFilletAtEdge.push(q),
  } as unknown as KernelStore;
}

function makeState(
  el: { kind: "face" | "edge" | "vertex"; idHash: string } = { kind: "edge", idHash: "98765" },
): StoreState {
  return {
    selectedElement: el,
    features: [],
  } as unknown as StoreState;
}

function click(container: HTMLElement, testid: string) {
  const btn = container.querySelector<HTMLButtonElement>(`[data-testid="${testid}"]`);
  act(() => {
    btn?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
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
    const calls = makeCalls();
    act(() => {
      root.render(<ElementInfo store={makeStore(calls)} state={makeState()} />);
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
    const calls = makeCalls();
    act(() => {
      root.render(<ElementInfo store={makeStore(calls)} state={makeState()} />);
    });
    click(container, "element-info-clear");
    expect(calls.cleared).toBe(1);
    act(() => root.unmount());
  });

  // ── Chunk 3b: context-actions ──────────────────────────────────────

  it("FACE: renders sketch + mate actions; clicking each fires the right store method with qFace and clears", () => {
    const calls = makeCalls();
    act(() => {
      root.render(
        <ElementInfo store={makeStore(calls)} state={makeState({ kind: "face", idHash: "4242" })} />,
      );
    });

    const sketch = container.querySelector('[data-testid="element-action-sketch"]');
    const mate = container.querySelector('[data-testid="element-action-mate"]');
    expect(sketch?.textContent).toBe("Skizze auf Fläche");
    expect(mate?.textContent).toBe("Mate-Connector");

    click(container, "element-action-sketch");
    expect(calls.enterSketch).toEqual(["qFace(4242)"]);
    expect(calls.cleared).toBe(1);

    click(container, "element-action-mate");
    expect(calls.addMateConnectorAtFace).toEqual(["qFace(4242)"]);
    expect(calls.cleared).toBe(2);
    act(() => root.unmount());
  });

  it("EDGE: renders only the fillet action; clicking it fires addFilletAtEdge with qEdge and clears", () => {
    const calls = makeCalls();
    act(() => {
      root.render(
        <ElementInfo store={makeStore(calls)} state={makeState({ kind: "edge", idHash: "777" })} />,
      );
    });

    const fillet = container.querySelector('[data-testid="element-action-fillet"]');
    expect(fillet?.textContent).toBe("Verrundung");
    expect(container.querySelector('[data-testid="element-action-sketch"]')).toBeNull();
    expect(container.querySelector('[data-testid="element-action-mate"]')).toBeNull();

    click(container, "element-action-fillet");
    expect(calls.addFilletAtEdge).toEqual(["qEdge(777)"]);
    expect(calls.cleared).toBe(1);
    act(() => root.unmount());
  });

  it("VERTEX: renders no Aktionen section at all", () => {
    const calls = makeCalls();
    act(() => {
      root.render(
        <ElementInfo store={makeStore(calls)} state={makeState({ kind: "vertex", idHash: "1" })} />,
      );
    });
    expect(container.querySelector('[data-testid="element-info-actions"]')).toBeNull();
    act(() => root.unmount());
  });
});
