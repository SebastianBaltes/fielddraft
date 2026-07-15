import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { VoxelDialog } from "./VoxelDialog";
import { getVoxelDisplay } from "./voxelDebugStore";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

// #379: the solid voxel fill must be scoped to the Voxel dialog's lifetime —
// NOT to a "voxel" render mode (removed from `RenderMode` entirely). Stub
// `useKernelStore` so the dialog mounts without booting the real WASM kernel;
// none of these assertions touch kernel/store behaviour.
vi.mock("./store", () => ({
  useKernelStore: () => ({ store: null, state: null, error: null }),
}));

describe("VoxelDialog solid-fill scoping (#379)", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it("activates the solid fill on mount and deactivates it on unmount", () => {
    expect(getVoxelDisplay().solidActive).toBe(false);

    act(() => {
      root.render(<VoxelDialog onClose={() => {}} />);
    });
    expect(getVoxelDisplay().solidActive).toBe(true);

    act(() => root.unmount());
    expect(getVoxelDisplay().solidActive).toBe(false);
  });

  it("clears any published diff on unmount", () => {
    act(() => {
      root.render(<VoxelDialog onClose={() => {}} />);
    });
    act(() => root.unmount());
    expect(getVoxelDisplay().diff).toBeNull();
  });
});
