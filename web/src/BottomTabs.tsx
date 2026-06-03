/**
 * MT-A: bottom tab strip (Sheets/Onshape style). Renders one button per
 * project tab and switches the active tab. Chunk B (MT-B) adds the [+]
 * button, the per-tab ▾ context menu, and the tab-illusion CSS — here we
 * keep it minimal but functional.
 */

import type { KernelStore, StoreState } from "./store";
import type { TabKind } from "./project";

const KIND_GLYPH: Record<TabKind, string> = {
  part: "◳",
  script: "ƒ",
  assembly: "⚙",
};

export function BottomTabs({
  store,
  state,
}: {
  store: KernelStore;
  state: StoreState;
}) {
  const { tabs, activeTabId } = state.project;
  return (
    <div className="fd-bottom-tabs" role="tablist">
      {tabs.map((tab) => {
        const isActive = tab.id === activeTabId;
        return (
          <button
            key={tab.id}
            type="button"
            role="tab"
            className={"tab" + (isActive ? " is-active" : "")}
            aria-selected={isActive}
            data-testid={`tab-${tab.id}`}
            onClick={() => store.setActiveTab(tab.id)}
          >
            <span className="kind-glyph" aria-hidden="true">
              {KIND_GLYPH[tab.kind]}
            </span>{" "}
            {tab.name}
          </button>
        );
      })}
    </div>
  );
}
