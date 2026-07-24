/**
 * Ticket #422 — opening/switching a project must apply the TARGET project's
 * camera environment, never inherit the one it was opened from.
 *
 * Bug 1: opening a project WITHOUT a saved view left the previous project's
 * pose live in `cameraPoseBridge`, and the autosave then persisted it into the
 * new project. Now such an open clears the pose (so nothing stale is saved) and
 * requests a default view that auto-fits the model.
 *
 * Bug 2 (consequence): the auto-fit rescues the camera from any absurd standoff
 * the previous project's view might have carried into the new one.
 *
 * Reuses the FakeKernel seam from store.openProject.test.ts under jsdom.
 */

import { describe, expect, it, beforeEach } from "vitest";
import { KernelStore } from "./store";
import type { Kernel } from "./kernel";
import { createNoopAutoSave } from "./persistence";
import {
  type CameraPose,
  clearCameraPose,
  getCameraPose,
  requestCameraRestore,
} from "./cameraPoseBridge";
import { consumeViewCommand, peekViewCommand } from "./viewCommands";

class FakeKernel {
  loadedJson =
    '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';
  /** Number of bodies the loaded doc reports — drives empty-vs-deferred fit. */
  bodyCountValue = 0;
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
    return { size: [0, 0, 0], min: [0, 0, 0], max: [0, 0, 0] };
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
    return this.bodyCountValue;
  }
  healElementRefs() {
    return [];
  }
  // Body accessors used by the sync-meshing snapshot when bodyCount > 0.
  bodyHasTransformDelta() {
    return false;
  }
  bodyTransformDelta() {
    return new Float32Array([0, 0, 0]);
  }
  bodyId() {
    return 1;
  }
  bodySeedFeatureId() {
    return 0;
  }
  bodyVersion() {
    return 1;
  }
  bodyPositionsView() {
    return new Float32Array(0);
  }
  bodyNormalsView() {
    return new Float32Array(0);
  }
  bodyIndicesView() {
    return new Uint32Array(0);
  }
  bodyClassificationsView() {
    return new Uint32Array(0);
  }
  bodyContributorsView() {
    return new Uint32Array(0);
  }
}

const POSE_A: CameraPose = {
  position: [12000, 9600, 12000],
  quaternion: [0, 0, 0, 1],
  up: [0, 1, 0],
  zoom: 0.1,
  target: [250, 11.3, 1150],
};

const POSE_B: CameraPose = {
  position: [1, 2, 3],
  quaternion: [0, 0, 0, 1],
  up: [0, 1, 0],
  zoom: 4,
  target: [4, 5, 6],
};

function projectJson(name: string, view?: CameraPose): string {
  const doc =
    '{"schema_version":1,"name":"Part","feature_calls":[],"next_feature_id":1}';
  const project: Record<string, unknown> = {
    schemaVersion: 1,
    name,
    tabs: [{ id: "tab_1", kind: "part", name: "Part 1", documentJson: doc }],
    activeTabId: "tab_1",
  };
  if (view) project.view = view;
  return JSON.stringify(project);
}

function makeStore(bodyCount = 0): { store: KernelStore; kernel: FakeKernel } {
  const kernel = new FakeKernel();
  kernel.bodyCountValue = bodyCount;
  const store = new KernelStore(
    kernel as unknown as Kernel,
    createNoopAutoSave(),
    null,
    false,
  );
  return { store, kernel };
}

describe("open project applies target environment (ticket #422)", () => {
  beforeEach(() => {
    localStorage.clear();
    clearCameraPose();
    while (consumeViewCommand()) {
      /* drain any pending view command from a previous test */
    }
  });

  it("opening an EMPTY project WITHOUT a view forgets the pose and fits", async () => {
    const { store } = makeStore(0);
    while (consumeViewCommand()) {
      /* drain the boot fit for a fresh no-view default project */
    }

    // Simulate the previous project A leaving its pose live in the bridge.
    requestCameraRestore(POSE_A);
    expect(getCameraPose()).toEqual(POSE_A);

    const ok = await store.loadProjectFromJson(projectJson("Cablebox"));
    expect(ok).toBe(true);

    // Bug 1: the stale pose is gone, so the autosave can't persist it into B.
    expect(getCameraPose()).toBeNull();
    // …and the serialized project carries NO view (it will get its own once the
    // fit runs and saveCameraPose repopulates the bridge on the next frame).
    expect(JSON.parse(store.serializeProjectJson()).view).toBeUndefined();

    // Bug 2: a default view + auto-fit is requested (rescues the camera).
    expect(peekViewCommand()).toEqual({ kind: "preset", preset: "iso" });
  });

  it("opening a project WITH geometry fits immediately (fresh CSG bounds)", async () => {
    const { store } = makeStore(2); // bodyCount > 0
    while (consumeViewCommand()) {
      /* drain the boot fit */
    }
    requestCameraRestore(POSE_A);

    const ok = await store.loadProjectFromJson(projectJson("Wanne"));
    expect(ok).toBe(true);

    // loadJson regenerated the spine synchronously, so sceneAabb is fresh and
    // the fit is issued straight away — no dependence on the async mesh path.
    expect(getCameraPose()).toBeNull();
    expect(peekViewCommand()).toEqual({ kind: "preset", preset: "iso" });
  });

  it("fits view-less even when DC meshing is OFF (sdf-deferred default)", async () => {
    // Live-system regression (#422): production default is renderMode
    // sdf-deferred → dcMeshingEnabled=false, mesherAvailable=true. The fit must
    // NOT route through the mesh path (no worker job is ever posted there), so
    // it must fire from applyProjectView regardless of meshing config.
    const { store } = makeStore(2);
    store.setDcMeshingEnabled(false); // mimic sdf-deferred
    while (consumeViewCommand()) {
      /* drain */
    }
    requestCameraRestore(POSE_A);

    const ok = await store.loadProjectFromJson(projectJson("TomtomAbdeckung"));
    expect(ok).toBe(true);

    expect(getCameraPose()).toBeNull();
    expect(peekViewCommand()).toEqual({ kind: "preset", preset: "iso" });
  });

  it("opening a project WITH a saved view restores it, no fit override", async () => {
    const { store } = makeStore(2);
    while (consumeViewCommand()) {
      /* drain the boot fit */
    }
    requestCameraRestore(POSE_A);

    const ok = await store.loadProjectFromJson(projectJson("Saved", POSE_B));
    expect(ok).toBe(true);

    // The target project's saved view is now live — not A's, not a fit.
    expect(getCameraPose()).toEqual(POSE_B);
    expect(peekViewCommand()).toBeNull();
  });

  it("newProject forgets the previous pose and resets the view", () => {
    const { store } = makeStore(0);
    while (consumeViewCommand()) {
      /* drain the boot fit */
    }
    requestCameraRestore(POSE_A);

    store.newProject();

    expect(getCameraPose()).toBeNull();
    expect(peekViewCommand()).toEqual({ kind: "preset", preset: "iso" });
  });
});
