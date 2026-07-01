import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ProjectsBrowser } from "./ProjectsBrowser";
import * as platform from "../api/platform";
import type { Project } from "../api/platform";

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

function project(over: Partial<Project> = {}): Project {
  return {
    id: "p1",
    owner_id: "me",
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

function mount(props: Partial<Parameters<typeof ProjectsBrowser>[0]> = {}): {
  onOpen: ReturnType<typeof vi.fn>;
  onOpenCopy: ReturnType<typeof vi.fn>;
} {
  const onOpen = vi.fn();
  const onOpenCopy = vi.fn();
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(
      <ProjectsBrowser
        authed
        onOpen={onOpen}
        onOpenCopy={onOpenCopy}
        selfId="me"
        boundProjectId={null}
        {...props}
      />,
    );
  });
  return { onOpen, onOpenCopy };
}

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function byTestId<T extends HTMLElement = HTMLElement>(id: string): T | null {
  return container.querySelector<T>(`[data-testid="${id}"]`);
}

beforeEach(() => {
  vi.restoreAllMocks();
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("ProjectsBrowser", () => {
  it("lists my projects by default (authed) and offers New project", async () => {
    const listProjects = vi
      .spyOn(platform.platformApi, "listProjects")
      .mockResolvedValue([project({ id: "p1", name: "Gear" })]);
    const listGallery = vi
      .spyOn(platform.platformApi, "listGallery")
      .mockResolvedValue([]);
    mount();
    await flush();
    expect(listProjects).toHaveBeenCalled();
    expect(listGallery).not.toHaveBeenCalled();
    expect(byTestId("project-card-p1")).toBeTruthy();
    expect(byTestId("projects-new")).toBeTruthy();
  });

  it("switches to the Public filter and lists gallery projects", async () => {
    vi.spyOn(platform.platformApi, "listProjects").mockResolvedValue([]);
    const listGallery = vi
      .spyOn(platform.platformApi, "listGallery")
      .mockResolvedValue([project({ id: "pub", owner_id: "someone" })]);
    mount();
    await flush();
    act(() => byTestId<HTMLButtonElement>("projects-filter-public")!.click());
    await flush();
    expect(listGallery).toHaveBeenCalled();
    expect(byTestId("project-card-pub")).toBeTruthy();
    // A non-owned public project offers "open a copy", not owner actions.
    expect(byTestId("project-open-copy-pub")).toBeTruthy();
    expect(byTestId("project-delete-pub")).toBeNull();
  });

  it("opens an owned project via onOpen", async () => {
    vi.spyOn(platform.platformApi, "listProjects").mockResolvedValue([
      project({ id: "p1", owner_id: "me" }),
    ]);
    vi.spyOn(platform.platformApi, "listGallery").mockResolvedValue([]);
    const full = project({ id: "p1", owner_id: "me" });
    vi.spyOn(platform.platformApi, "getProject").mockResolvedValue(full);
    const { onOpen } = mount();
    await flush();
    act(() => byTestId<HTMLButtonElement>("project-open-p1")!.click());
    await flush();
    expect(onOpen).toHaveBeenCalledWith(full);
  });

  it("opens a non-owned public project as a copy via onOpenCopy", async () => {
    vi.spyOn(platform.platformApi, "listProjects").mockResolvedValue([]);
    const pub = project({ id: "pub", owner_id: "other" });
    vi.spyOn(platform.platformApi, "listGallery").mockResolvedValue([pub]);
    vi.spyOn(platform.platformApi, "getProject").mockResolvedValue(pub);
    const { onOpenCopy } = mount();
    await flush();
    act(() => byTestId<HTMLButtonElement>("projects-filter-public")!.click());
    await flush();
    act(() => byTestId<HTMLButtonElement>("project-open-pub")!.click());
    await flush();
    expect(onOpenCopy).toHaveBeenCalledWith(pub);
  });

  it("New project creates an empty project and opens it", async () => {
    vi.spyOn(platform.platformApi, "listProjects").mockResolvedValue([]);
    vi.spyOn(platform.platformApi, "listGallery").mockResolvedValue([]);
    const created = project({ id: "fresh" });
    const create = vi
      .spyOn(platform.platformApi, "createProject")
      .mockResolvedValue(created);
    const { onOpen } = mount();
    await flush();
    act(() => byTestId<HTMLButtonElement>("projects-new")!.click());
    await flush();
    expect(create).toHaveBeenCalledWith("Untitled", {});
    expect(onOpen).toHaveBeenCalledWith(created);
  });

  it("has no × close button and no Save Current Workspace action", async () => {
    vi.spyOn(platform.platformApi, "listProjects").mockResolvedValue([]);
    vi.spyOn(platform.platformApi, "listGallery").mockResolvedValue([]);
    mount();
    await flush();
    expect(byTestId("dashboard-close")).toBeNull();
    expect(byTestId("dashboard-save-current")).toBeNull();
  });

  it("guests only see the public filter", async () => {
    vi.spyOn(platform.platformApi, "listGallery").mockResolvedValue([]);
    const listProjects = vi
      .spyOn(platform.platformApi, "listProjects")
      .mockResolvedValue([]);
    mount({ authed: false, selfId: null });
    await flush();
    expect(byTestId("projects-filter-public")).toBeTruthy();
    expect(byTestId("projects-filter-mine")).toBeNull();
    expect(byTestId("projects-new")).toBeNull();
    expect(listProjects).not.toHaveBeenCalled();
  });
});
