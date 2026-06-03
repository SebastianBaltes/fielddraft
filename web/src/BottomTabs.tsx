/**
 * MT-B: bottom workspace tab strip (Sheets/Onshape style).
 *
 * Renders the [+] add-tab button at the left, then one button per project
 * tab. The active tab visually merges into the workspace above (the
 * "tab illusion" — see bottom-tabs.css). Each tab carries a ▾ affordance
 * opening a context menu (Umbenennen / Löschen); the [+] button opens a
 * create-tab menu. Both menus open ABOVE the strip so they aren't clipped
 * at the bottom of the viewport, and close on outside-click / Escape /
 * selection.
 */

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type RefObject,
} from "react";
import type { KernelStore, StoreState } from "./store";
import type { TabKind } from "./project";

const KIND_GLYPH: Record<TabKind, string> = {
  part: "◳",
  script: "ƒ",
  assembly: "⚙",
};

/** Shared click-outside + Escape closer for a popup. Calls `onClose` when
 *  a pointerdown lands outside `ref` or Escape is pressed. */
function useDismiss(
  open: boolean,
  ref: React.RefObject<HTMLElement | null>,
  onClose: () => void,
) {
  useEffect(() => {
    if (!open) return;
    const onPointer = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("pointerdown", onPointer);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onPointer);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, ref, onClose]);
}

/** Position an upward-opening popover as a fixed-position layer anchored to
 *  `anchorRef`. `position: fixed` escapes the `.tabs` overflow scroller (a
 *  plain overflow ancestor never clips fixed children), so the menu always
 *  shows ABOVE the strip where there's room — never tucked under the tab.
 *  Left edge is clamped into the viewport. */
function useUpwardPopup(
  open: boolean,
  anchorRef: RefObject<HTMLElement | null>,
): CSSProperties {
  const [style, setStyle] = useState<CSSProperties>({});
  useLayoutEffect(() => {
    if (!open) return;
    const el = anchorRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const MENU_MIN_WIDTH = 208; // keep in sync with .fd-tab-popup min-width
    const left = Math.max(8, Math.min(r.left, window.innerWidth - MENU_MIN_WIDTH - 8));
    setStyle({
      left: `${left}px`,
      bottom: `${window.innerHeight - r.top + 4}px`,
    });
  }, [open, anchorRef]);
  return style;
}

export function BottomTabs({
  store,
  state,
}: {
  store: KernelStore;
  state: StoreState;
}) {
  const { tabs, activeTabId } = state.project;

  return (
    <div className="fd-bottom-tabs">
      <AddTabButton store={store} />
      <div className="tabs" role="tablist">
        {tabs.map((tab) => (
          <TabButton
            key={tab.id}
            store={store}
            tabId={tab.id}
            kind={tab.kind}
            name={tab.name}
            isActive={tab.id === activeTabId}
          />
        ))}
      </div>
    </div>
  );
}

// ── [+] add-tab button ──────────────────────────────────────────

function AddTabButton({ store }: { store: KernelStore }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement | null>(null);
  const menuStyle = useUpwardPopup(open, ref);
  useDismiss(open, ref, () => setOpen(false));

  const create = (kind: TabKind) => {
    store.createTab(kind);
    setOpen(false);
  };

  return (
    <div className="add" ref={ref}>
      <button
        type="button"
        className="add-button"
        data-testid="tab-add"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label="Tab hinzufügen"
        onClick={() => setOpen((v) => !v)}
      >
        +
      </button>
      {open ? (
        <div
          className="fd-tab-popup"
          role="menu"
          data-testid="tab-add-menu"
          style={menuStyle}
        >
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="tab-add-part"
            onClick={() => create("part")}
          >
            Create Part Editor
          </button>
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="tab-add-script"
            onClick={() => create("script")}
          >
            Create FieldScript Editor
          </button>
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid="tab-add-assembly"
            onClick={() => create("assembly")}
          >
            Create Assembly Editor
          </button>
        </div>
      ) : null}
    </div>
  );
}

// ── per-tab button + ▾ context menu + inline rename ─────────────

function TabButton({
  store,
  tabId,
  kind,
  name,
  isActive,
}: {
  store: KernelStore;
  tabId: string;
  kind: TabKind;
  name: string;
  isActive: boolean;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState(name);

  const ref = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const menuStyle = useUpwardPopup(menuOpen, ref);
  useDismiss(menuOpen, ref, () => setMenuOpen(false));

  useEffect(() => {
    if (renaming) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [renaming]);

  const startRename = () => {
    setDraft(name);
    setRenaming(true);
    setMenuOpen(false);
  };

  const commitRename = () => {
    store.renameTab(tabId, draft);
    setRenaming(false);
  };

  const cancelRename = () => {
    setRenaming(false);
  };

  return (
    <div
      className={"tab" + (isActive ? " is-active" : "")}
      ref={ref}
      role="tab"
      aria-selected={isActive}
    >
      {renaming ? (
        <input
          ref={inputRef}
          className="rename-input"
          data-testid={`tab-rename-input-${tabId}`}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") commitRename();
            else if (e.key === "Escape") cancelRename();
          }}
          onBlur={commitRename}
        />
      ) : (
        <button
          type="button"
          className="label"
          data-testid={`tab-${tabId}`}
          onClick={() => store.setActiveTab(tabId)}
          onDoubleClick={startRename}
        >
          <span className="kind-glyph" aria-hidden="true">
            {KIND_GLYPH[kind]}
          </span>{" "}
          {name}
        </button>
      )}
      <button
        type="button"
        className="caret"
        data-testid={`tab-menu-${tabId}`}
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        aria-label="Tab-Menü"
        onClick={(e) => {
          e.stopPropagation();
          setMenuOpen((v) => !v);
        }}
      >
        ▾
      </button>
      {menuOpen ? (
        <div
          className="fd-tab-popup"
          role="menu"
          data-testid={`tab-menu-popup-${tabId}`}
          style={menuStyle}
        >
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid={`tab-rename-${tabId}`}
            onClick={(e) => {
              e.stopPropagation();
              startRename();
            }}
          >
            Umbenennen
          </button>
          <button
            type="button"
            role="menuitem"
            className="menu-item"
            data-testid={`tab-delete-${tabId}`}
            onClick={(e) => {
              e.stopPropagation();
              setMenuOpen(false);
              store.deleteTab(tabId);
            }}
          >
            Löschen
          </button>
        </div>
      ) : null}
    </div>
  );
}
