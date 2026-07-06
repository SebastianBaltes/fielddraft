/**
 * Ticket 4z37 — explicit "Open project" must survive a reload, even in a
 * deep-link repro tab.
 *
 * A `?project=` deep-link tab boots with an inert autosave (so it can't
 * clobber another tab's persisted project) and re-fetches the ticket on
 * reload. Opening a project from disk there is a deliberate "work on THIS"
 * action: `openUserProjectFromBytes` must persist it AND graduate the tab
 * (real autosave, `?project=` stripped) so a reload restores it.
 *
 * Reuses the FakeKernel seam from the other store tests to drive a real
 * KernelStore under jsdom.
 */

import { describe, expect, it, beforeEach } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import { createNoopAutoSave, loadPersisted, STORAGE_KEY } from "./persistence";

class FakeKernel {
  loadedJson =
    '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';
  listScripts() {
    return [];
  }
  upsertScript() {
    return false;
  }
  removeScript() {
    return false;
  }
  loadJson(json: string) {
    this.loadedJson = json;
  }
  listFeatures() {
    return [];
  }
  listVariables() {
    return [];
  }
  canUndo() {
    return false;
  }
  undoDepth() {
    return 0;
  }
  canRedo() {
    return false;
  }
  redoDepth() {
    return 0;
  }
  getRollbackIndex() {
    return -1;
  }
  toJson() {
    return this.loadedJson;
  }
  sceneAabb() {
    return { size: [0, 0, 0] };
  }
  setSyncMeshing() {
    return false;
  }
  activeBodyId() {
    return 0;
  }
  clearActiveBodyId() {}
  setActiveBodyByFeature() {}
  bodyCount() {
    return 0;
  }
}

function projectBytes(name: string): Uint8Array {
  const doc =
    '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';
  const project = {
    schemaVersion: 1,
    name,
    tabs: [{ id: "tab_1", kind: "part", name: "Part 1", documentJson: doc }],
    activeTabId: "tab_1",
  };
  return new TextEncoder().encode(JSON.stringify(project));
}

describe("KernelStore.openUserProjectFromBytes (ticket 4z37)", () => {
  beforeEach(() => {
    localStorage.clear();
    window.history.replaceState(null, "", "/?project=tickets/open/x/project.fdraft");
  });

  it("persists an opened project and graduates the deep-link tab", () => {
    // Deep-link repro tab: inert autosave, `?project=` in the URL.
    const store = new KernelStore(
      new FakeKernel() as unknown as Kernel,
      createNoopAutoSave(),
      null,
      true, // deepLink
    );

    // Before opening, nothing is persisted (inert autosave).
    expect(loadPersisted()).toBeNull();

    const ok = store.openUserProjectFromBytes(projectBytes("Opened4z37"));
    expect(ok).toBe(true);

    // The opened project is now in localStorage — a reload would restore it.
    // (The only way this lands despite the inert BOOT handle is the graduate
    // step swapping in a real autosave, so this proves graduation happened.)
    const persisted = loadPersisted();
    expect(persisted).not.toBeNull();
    expect(JSON.parse(persisted!).name).toBe("Opened4z37");

    // The `?project=` deep-link is stripped so reload uses localStorage.
    expect(new URLSearchParams(window.location.search).has("project")).toBe(false);

    // Graduated for good: a subsequent open still persists through the now-real
    // autosave (not the inert boot handle).
    localStorage.removeItem(STORAGE_KEY);
    store.openUserProjectFromBytes(projectBytes("Edited4z37"));
    expect(JSON.parse(loadPersisted()!).name).toBe("Edited4z37");
  });
});
