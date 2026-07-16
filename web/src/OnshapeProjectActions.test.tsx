import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { OnshapeProjectActions } from "./OnshapeProjectActions";
import type { KernelStore } from "./store";
import type { OnshapeSource } from "./onshape/onshapeUrl";

/**
 * Ticket #388 — topbar "open in Onshape" link + "reimport" (with in-app
 * confirmation, never native `confirm()`) for a project that carries an
 * Onshape origin. Only the wiring matters here (the pull+translate itself is
 * covered by onshapeProjectImport's own tests), so `importOnshapeProject` is
 * mocked and the store is a stub exposing the three methods the component
 * calls.
 */

const importOnshapeProject = vi.fn();
vi.mock("./onshape/onshapeProjectImport", () => ({
  importOnshapeProject: (...args: unknown[]) => importOnshapeProject(...args),
}));

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

const SRC: OnshapeSource = {
  documentId: "a".repeat(24),
  workspaceId: "b".repeat(24),
  elementId: "c".repeat(24),
  documentName: "Lötkolbenhalter",
  url: "https://cad.onshape.com/documents/aaa/w/bbb/e/ccc",
};

function makeStore(): {
  store: KernelStore;
  loadProjectFromJson: ReturnType<typeof vi.fn>;
  renameProject: ReturnType<typeof vi.fn>;
  showTransientError: ReturnType<typeof vi.fn>;
} {
  const loadProjectFromJson = vi.fn().mockResolvedValue(true);
  const renameProject = vi.fn();
  const showTransientError = vi.fn();
  const store = {
    loadProjectFromJson,
    renameProject,
    showTransientError,
  } as unknown as KernelStore;
  return { store, loadProjectFromJson, renameProject, showTransientError };
}

function render(
  store: KernelStore,
  onshape: OnshapeSource | undefined,
  projectName = "Lötkolbenhalter",
) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(
      <OnshapeProjectActions
        store={store}
        onshape={onshape}
        projectName={projectName}
      />,
    );
  });
}

function click(testid: string) {
  const el = container.querySelector<HTMLElement>(`[data-testid="${testid}"]`)!;
  act(() => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

beforeEach(() => {
  vi.restoreAllMocks();
  importOnshapeProject.mockReset();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("OnshapeProjectActions", () => {
  it("renders nothing for a project without an Onshape origin", () => {
    const { store } = makeStore();
    render(store, undefined);
    expect(container.querySelector(".fd-onshape-project-actions")).toBeNull();
  });

  it("shows the Onshape link + reimport button for an Onshape-sourced project", () => {
    const { store } = makeStore();
    render(store, SRC);
    const link = container.querySelector<HTMLAnchorElement>(
      '[data-testid="project-onshape-open"]',
    );
    expect(link).not.toBeNull();
    expect(link!.href).toBe(SRC.url);
    expect(link!.target).toBe("_blank");
    expect(
      container.querySelector('[data-testid="project-onshape-reimport"]'),
    ).not.toBeNull();
  });

  it("clicking reimport opens an in-app confirm dialog, not a native confirm()", () => {
    const confirmSpy = vi.spyOn(window, "confirm");
    const { store } = makeStore();
    render(store, SRC);
    expect(
      container.querySelector('[data-testid="onshape-reimport-confirm-dialog"]'),
    ).toBeNull();
    click("project-onshape-reimport");
    expect(
      container.querySelector('[data-testid="onshape-reimport-confirm-dialog"]'),
    ).not.toBeNull();
    expect(confirmSpy).not.toHaveBeenCalled();
    expect(container.textContent).toContain("Lötkolbenhalter");
  });

  it("cancel closes the dialog without importing", () => {
    const { store } = makeStore();
    render(store, SRC);
    click("project-onshape-reimport");
    click("onshape-reimport-cancel");
    expect(
      container.querySelector('[data-testid="onshape-reimport-confirm-dialog"]'),
    ).toBeNull();
    expect(importOnshapeProject).not.toHaveBeenCalled();
  });

  it("confirming reimports, loads the result and renames the project", async () => {
    const { store, loadProjectFromJson, renameProject } = makeStore();
    const project = {
      schemaVersion: 1,
      name: "Lötkolbenhalter v2",
      tabs: [],
      activeTabId: "t1",
      onshape: SRC,
    };
    importOnshapeProject.mockResolvedValue({
      project,
      source: SRC,
      warnings: [],
      errors: [],
      featureCount: 3,
    });
    render(store, SRC);
    click("project-onshape-reimport");
    await act(async () => {
      click("onshape-reimport-confirm");
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(importOnshapeProject).toHaveBeenCalledWith(
      SRC,
      expect.objectContaining({ fallbackName: "Lötkolbenhalter" }),
    );
    expect(loadProjectFromJson).toHaveBeenCalledTimes(1);
    expect(JSON.parse(loadProjectFromJson.mock.calls[0][0]).name).toBe(
      "Lötkolbenhalter v2",
    );
    expect(renameProject).toHaveBeenCalledWith("Lötkolbenhalter v2");
    expect(
      container.querySelector('[data-testid="onshape-reimport-confirm-dialog"]'),
    ).toBeNull();
  });

  it("surfaces a partial-translation failure via the store's transient error", async () => {
    const { store, showTransientError } = makeStore();
    const project = {
      schemaVersion: 1,
      name: "Lötkolbenhalter",
      tabs: [],
      activeTabId: "t1",
      onshape: SRC,
    };
    importOnshapeProject.mockResolvedValue({
      project,
      source: SRC,
      warnings: [],
      errors: ["Unsupported feature: Loft"],
      featureCount: 2,
    });
    render(store, SRC);
    click("project-onshape-reimport");
    await act(async () => {
      click("onshape-reimport-confirm");
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(showTransientError).toHaveBeenCalledWith(
      expect.stringContaining("Unsupported feature: Loft"),
    );
  });

  it("shows an inline error and keeps the dialog open on failure", async () => {
    const { store } = makeStore();
    importOnshapeProject.mockRejectedValue(new Error("network down"));
    render(store, SRC);
    click("project-onshape-reimport");
    await act(async () => {
      click("onshape-reimport-confirm");
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(
      container.querySelector('[data-testid="onshape-reimport-error"]')
        ?.textContent,
    ).toBe("network down");
    expect(
      container.querySelector('[data-testid="onshape-reimport-confirm-dialog"]'),
    ).not.toBeNull();
  });
});
