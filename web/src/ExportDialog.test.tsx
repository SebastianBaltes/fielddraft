/**
 * Export dialog — the Lichen route (ticket faa42489) and the runtime readout
 * (ticket 6a0354e8).
 *
 * jsdom + a stub store: the dialog only talks to the kernel through
 * `exportMeshed` / `exportEstimate`, so the whole two-step flow (pick the
 * method → Mesh → report) is testable without WASM.
 */

import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ExportDialog } from "./ExportDialog";
import type { KernelStore } from "./store";
import type {
  ExportFormat,
  ExportMethodOptions,
  ExportOutcome,
  ExportProgress,
  MeshMethod,
} from "./exportTypes";

// React needs to know it's inside a test renderer for `act` to be silent.
(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

interface MeshCall {
  method: MeshMethod;
  cellSizeMm: number;
  options: ExportMethodOptions;
}

const emptyReport = {
  cell_size_mm: 1,
  total_triangles: 0,
  all_watertight: true,
  max_deviation_mm: 0,
  bodies: [],
};

function stubStore(calls: MeshCall[]): KernelStore {
  return {
    getState: () => ({
      projectId: null,
      syncState: "offline",
      features: [],
      project: { tabs: [], activeTabId: "" },
    }),
    exportEstimate: () => null,
    exportMeshed: (
      _format: ExportFormat,
      cellSizeMm: number,
      method: MeshMethod,
      options: ExportMethodOptions,
      _onProgress?: (p: ExportProgress) => void,
    ): Promise<ExportOutcome> => {
      calls.push({ method, cellSizeMm, options });
      return Promise.resolve({
        bytes: new Uint8Array(),
        report: emptyReport,
        preview: {
          positions: new Float32Array(),
          indices: new Uint32Array(),
        },
      });
    },
    downloadExport: vi.fn(),
  } as unknown as KernelStore;
}

let container: HTMLDivElement;
let root: Root;

function render(calls: MeshCall[]): void {
  act(() => {
    root.render(
      <ExportDialog store={stubStore(calls)} format="stl" onClose={() => {}} />,
    );
  });
}

function select(): HTMLSelectElement {
  return container.querySelector(
    '[data-testid="export-method"]',
  ) as HTMLSelectElement;
}

function pickMethod(value: MeshMethod): void {
  const el = select();
  act(() => {
    el.value = value;
    el.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

beforeEach(() => {
  localStorage.clear();
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  localStorage.clear();
});

describe("ExportDialog — Lichen (faa42489)", () => {
  it("offers Lichen as a meshing method", () => {
    render([]);
    const values = [...select().options].map((o) => o.value);
    expect(values).toContain("lichen");
  });

  it("shows the curvature angle ONLY for Lichen — its one extra knob", () => {
    render([]);
    expect(
      container.querySelector('[data-testid="export-curv-angle"]'),
    ).toBeNull();

    pickMethod("lichen");
    expect(
      container.querySelector('[data-testid="export-curv-angle"]'),
    ).not.toBeNull();
    // …and the cell size is read as the target edge length there.
    const label = container.querySelector('label[for="export-cell-size"]');
    expect(label?.textContent).toContain("Edge length h");

    pickMethod("adaptive");
    expect(
      container.querySelector('[data-testid="export-curv-angle"]'),
    ).toBeNull();
  });

  it("meshes with the Lichen method and the default curvature angle", async () => {
    const calls: MeshCall[] = [];
    render(calls);
    pickMethod("lichen");
    const mesh = container.querySelector(
      '[data-testid="export-mesh"]',
    ) as HTMLButtonElement;
    await act(async () => {
      mesh.click();
    });
    expect(calls).toHaveLength(1);
    expect(calls[0].method).toBe("lichen");
    expect(calls[0].options).toEqual({ curvAngleDeg: 10 });
  });
});

describe("ExportDialog — runtime (6a0354e8)", () => {
  it("reports the runtime after a mesh, for any method", async () => {
    const calls: MeshCall[] = [];
    render(calls);
    expect(
      container.querySelector('[data-testid="export-duration"]'),
    ).toBeNull();

    const mesh = container.querySelector(
      '[data-testid="export-mesh"]',
    ) as HTMLButtonElement;
    await act(async () => {
      mesh.click();
    });

    // Default method (adaptive) — the readout is not Lichen-specific.
    expect(calls[0].method).toBe("adaptive");
    const runtime = container.querySelector('[data-testid="export-duration"]');
    expect(runtime?.textContent).toMatch(/^Runtime \d/);
  });
});
