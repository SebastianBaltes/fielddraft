/**
 * FT-W: bottom tab strip (Sheets/Onshape style) switching the inner
 * workspace between the Part Editor (CAD) and the Feature Script IDE.
 * Both workspaces stay mounted in App; this only flips `activeTab`.
 */

import type { KernelStore, StoreState } from "./store";

export function BottomTabs({
  store,
  state,
}: {
  store: KernelStore;
  state: StoreState;
}) {
  const tab = state.activeTab;
  return (
    <div className="fd-bottom-tabs" role="tablist">
      <button
        type="button"
        role="tab"
        className={"tab" + (tab === "part" ? " is-active" : "")}
        aria-selected={tab === "part"}
        data-testid="tab-part"
        onClick={() => store.setActiveTab("part")}
      >
        Part Editor
      </button>
      <button
        type="button"
        role="tab"
        className={"tab" + (tab === "script" ? " is-active" : "")}
        aria-selected={tab === "script"}
        data-testid="tab-script"
        onClick={() => store.setActiveTab("script")}
      >
        Feature Script
      </button>
    </div>
  );
}
