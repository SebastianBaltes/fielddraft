import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { Dashboard } from "./Dashboard";
import * as platform from "../api/platform";
import type { Project } from "../api/platform";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

function project(over: Partial<Project> = {}): Project {
  return {
    id: "p1",
    owner_id: "u1",
    name: "Gear",
    visibility: "private",
    document: { tabs: [] },
    updated_at: "2026-06-01T00:00:00Z",
    created_at: "2026-05-01T00:00:00Z",
    ...over,
  };
}

let container: HTMLDivElement;
let root: Root;

function mountDashboard(onOpen = vi.fn()) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(
      <Dashboard
        onOpen={onOpen}
        getCurrentDocument={() => ({ tabs: [] })}
        boundProjectId={null}
        onClose={() => {}}
        onOpenGallery={() => {}}
      />,
    );
  });
  return onOpen;
}

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

beforeEach(() => {
  vi.restoreAllMocks();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("Dashboard", () => {
  it("lists the user's projects sorted newest-first", async () => {
    vi.spyOn(platform.platformApi, "listProjects").mockResolvedValue([
      project({ id: "old", name: "Old", updated_at: "2026-01-01T00:00:00Z" }),
      project({ id: "new", name: "New", updated_at: "2026-06-01T00:00:00Z" }),
    ]);
    mountDashboard();
    await flush();
    const cards = container.querySelectorAll('[data-testid^="project-card-"]');
    expect(cards).toHaveLength(2);
    // Newest first.
    expect(cards[0].getAttribute("data-testid")).toBe("project-card-new");
  });

  it("shows an empty state when there are no projects", async () => {
    vi.spyOn(platform.platformApi, "listProjects").mockResolvedValue([]);
    mountDashboard();
    await flush();
    expect(
      container.querySelector('[data-testid="dashboard-empty"]'),
    ).toBeTruthy();
  });

  it("opens a project via getProject when a card is clicked", async () => {
    vi.spyOn(platform.platformApi, "listProjects").mockResolvedValue([
      project({ id: "p1" }),
    ]);
    const full = project({ id: "p1", document: { tabs: ["x"] } });
    const getProject = vi
      .spyOn(platform.platformApi, "getProject")
      .mockResolvedValue(full);
    const onOpen = mountDashboard();
    await flush();
    const openBtn = container.querySelector<HTMLButtonElement>(
      '[data-testid="project-open-p1"]',
    )!;
    act(() => openBtn.click());
    await flush();
    expect(getProject).toHaveBeenCalledWith("p1");
    expect(onOpen).toHaveBeenCalledWith(full);
  });

  it("toggles visibility private → public", async () => {
    vi.spyOn(platform.platformApi, "listProjects").mockResolvedValue([
      project({ id: "p1", visibility: "private" }),
    ]);
    const setVisibility = vi
      .spyOn(platform.platformApi, "setVisibility")
      .mockResolvedValue(project({ id: "p1", visibility: "public" }));
    mountDashboard();
    await flush();
    const badge = container.querySelector<HTMLButtonElement>(
      '[data-testid="project-visibility-p1"]',
    )!;
    expect(badge.textContent).toBe("Private");
    act(() => badge.click());
    await flush();
    expect(setVisibility).toHaveBeenCalledWith("p1", "public");
  });

  it("deletes a project after confirmation", async () => {
    vi.spyOn(platform.platformApi, "listProjects").mockResolvedValue([
      project({ id: "p1" }),
    ]);
    const deleteProject = vi
      .spyOn(platform.platformApi, "deleteProject")
      .mockResolvedValue(undefined);
    vi.spyOn(window, "confirm").mockReturnValue(true);
    mountDashboard();
    await flush();
    const del = container.querySelector<HTMLButtonElement>(
      '[data-testid="project-delete-p1"]',
    )!;
    act(() => del.click());
    await flush();
    expect(deleteProject).toHaveBeenCalledWith("p1");
  });

  it("surfaces an error when the list fails", async () => {
    vi.spyOn(platform.platformApi, "listProjects").mockRejectedValue(
      new platform.ApiError(500, "boom"),
    );
    mountDashboard();
    await flush();
    expect(
      container.querySelector('[data-testid="dashboard-error"]')?.textContent,
    ).toContain("boom");
  });
});
