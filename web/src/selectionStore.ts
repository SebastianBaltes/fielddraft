/**
 * SelectionStore — the selection-domain controller.
 *
 * Owns the three mutually exclusive selection notions and their
 * reconciliation: the feature-level `selection` set (with shift-range
 * anchor), the tool-free B-Rep element selection (single + multi +
 * box-select), and the default-datum selection — plus body highlight
 * and the active-body LOD sync. Selecting a sketch feature IS editing
 * it (ticket pqoe): `syncSketchSessionToSelection` reconciles the
 * active sketch session on every list-origin selection change. The
 * KernelStore facade delegates its public selection API 1:1 here.
 */

import type { Kernel } from "./kernel";
import type { PickedElement } from "./elementQuery";
import { logUserAction } from "./userActionLog";
import { bodyFeatureIds, resolveSelectionLinks } from "./selectionLinks";
import {
  StoreCore,
  type SelectionOrigin,
  type StoreState,
} from "./storeCore";

/** Cross-domain pieces the selection controller needs from the facade. */
export interface SelectionStoreDeps {
  /** Pending-feature lifecycle (feature domain): deselecting the pending
   *  extrude confirms it; selecting the pending feature itself must not. */
  isPendingFeature(id: number): boolean;
  confirmPendingFeature(): void;
  /** Worker orchestration: an anchor change can flip the kernel's active
   *  body, which re-meshes at context resolution. */
  refreshMeshCache(): void;
  /** Sketch domain: a selection moving away from the edited sketch
   *  commits the session; selecting a sketch enters its edit session. */
  commitSketch(): number | null;
  editSketch(featureId: number): void;
}

export class SelectionStore {
  constructor(
    private readonly core: StoreCore,
    private readonly deps: SelectionStoreDeps,
  ) {}

  private get kernel(): Kernel {
    return this.core.kernel;
  }
  private get state(): StoreState {
    return this.core.state;
  }
  private set state(next: StoreState) {
    this.core.state = next;
  }
  private patch(partial: Partial<StoreState>): void {
    this.core.patch(partial);
  }
  private notify(): void {
    this.core.notify();
  }

  /**
   * K3 part 4 LOD hook: push the selection anchor's body to the
   * kernel as the active body, or clear the active body when the
   * anchor is gone. Called from `commit()` after every mutation and
   * from every selection helper. The kernel's
   * `setActiveBodyByFeature` takes care of looking up which body
   * owns the feature and triggers a re-mesh only when the active
   * body actually changes.
   */
  syncActiveBody(): void {
    const anchor = this.state.selectionAnchor;
    if (anchor === null) {
      if (this.kernel.activeBodyId() !== 0) {
        this.kernel.clearActiveBodyId();
      }
      return;
    }
    this.kernel.setActiveBodyByFeature(anchor);
  }

  selectOnly(id: number, origin: SelectionOrigin = "list"): void {
    logUserAction("feature", "select", { id, origin });
    if (!this.deps.isPendingFeature(id)) this.deps.confirmPendingFeature();
    this.state = {
      ...this.state,
      selection: new Set([id]),
      selectionAnchor: id,
      // Feature selection and element selection are mutually exclusive.
      selectedElement: null,
      selectedElements: [],
      selectedDatum: null,
    };
    this.syncActiveBody();
    this.deps.refreshMeshCache();
    this.syncSketchSessionToSelection(origin);
    this.core.emit();
  }

  toggleSelect(id: number, origin: SelectionOrigin = "list"): void {
    if (!this.deps.isPendingFeature(id)) this.deps.confirmPendingFeature();
    const next = new Set(this.state.selection);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    this.state = {
      ...this.state,
      selection: next,
      selectionAnchor: id,
      selectedElement: null,
      selectedElements: [],
      selectedDatum: null,
    };
    this.syncActiveBody();
    this.deps.refreshMeshCache();
    this.syncSketchSessionToSelection(origin);
    this.core.emit();
  }

  /**
   * Shift+Click range selection: select every feature between the
   * current anchor (inclusive) and the clicked id (inclusive). If no
   * anchor exists yet this falls back to a single selection.
   */
  selectRangeTo(id: number, origin: SelectionOrigin = "list"): void {
    if (!this.deps.isPendingFeature(id)) this.deps.confirmPendingFeature();
    const features = this.state.features;
    const targetIdx = features.findIndex((f) => f.id === id);
    if (targetIdx === -1) return;

    const anchor = this.state.selectionAnchor;
    if (anchor === null) {
      this.selectOnly(id, origin);
      return;
    }
    const anchorIdx = features.findIndex((f) => f.id === anchor);
    if (anchorIdx === -1) {
      // Anchor stale; reset.
      this.selectOnly(id, origin);
      return;
    }
    const [lo, hi] =
      anchorIdx <= targetIdx ? [anchorIdx, targetIdx] : [targetIdx, anchorIdx];
    const next = new Set<number>();
    for (let i = lo; i <= hi; i++) next.add(features[i].id);
    this.state = {
      ...this.state,
      selection: next,
      // Anchor stays put — that's the standard finder/Onshape behaviour.
      selectionAnchor: anchor,
      selectedElement: null,
      selectedElements: [],
      selectedDatum: null,
    };
    this.syncActiveBody();
    this.deps.refreshMeshCache();
    this.syncSketchSessionToSelection(origin);
    this.core.emit();
  }

  highlightBody(index: number | null): void {
    this.patch({ highlightedBodyIndex: index });
    this.core.emit();
  }

  selectAll(): void {
    const all = new Set(this.state.features.map((f) => f.id));
    this.patch({ selection: all, selectionAnchor: null });
    this.core.emit();
  }

  clearSelection(origin: SelectionOrigin = "list"): void {
    // Deselecting the pending ghost confirms it (the dialog's OK path).
    this.deps.confirmPendingFeature();
    if (
      this.state.selection.size === 0 &&
      this.state.selectionAnchor === null &&
      this.state.selectedElement === null &&
      this.state.selectedDatum === null
    )
      return;
    this.patch({
      selection: new Set(),
      selectionAnchor: null,
      selectedElement: null,
      selectedElements: [],
      selectedDatum: null,
    });
    this.syncActiveBody();
    this.deps.refreshMeshCache();
    this.syncSketchSessionToSelection(origin);
    this.core.emit();
  }

  /**
   * Ticket pqoe: a sketch behaves like any other feature — selecting it
   * IS editing it. Reconcile the active sketch session with the current
   * feature selection:
   *  - exactly one sketch feature selected → enter its edit session
   *    (unless we are already editing that same id);
   *  - selection moved away → commit the active session so live edits
   *    persist (the ✗ Cancel button is the explicit discard path).
   * Idempotent and called only from the explicit selection mutators, so
   * the selection change that `commitSketch` performs internally cannot
   * re-trigger it (no loop).
   *
   * Ticket 7hgr (a): the sketch session is STICKY against the 3D scene. A
   * scene-origin selection change (clicking a body / datum / sketch entity /
   * empty space in the viewport) must NEVER commit or exit the active
   * session — only the ✓/✗ buttons, the Extrude/Revolve sketch-toolbar
   * buttons, delete (vev1) and New Project (5zd3) end it. So while a session
   * is active we ignore scene-origin changes entirely. List-origin changes
   * (clicking a feature row in the outliner — the default) keep the pqoe
   * behaviour: selecting another sketch enters its edit session, selecting
   * a non-sketch feature commits the current one.
   */
  private syncSketchSessionToSelection(origin: SelectionOrigin = "list"): void {
    const session = this.state.sketchSession;
    // Scene clicks never touch an active session (ticket 7hgr a).
    if (origin === "scene" && session.active) return;
    const editingId = session.active ? session.editingFeatureId : null;
    const sole =
      this.state.selection.size === 1
        ? (this.state.features.find((f) => this.state.selection.has(f.id)) ??
          null)
        : null;
    const targetSketchId = sole && sole.kind === "sketch" ? sole.id : null;

    // Already editing exactly the selected sketch → nothing to do. (A fresh
    // draft session has editingFeatureId === null but is NOT "no session",
    // so it must still be reconciled below when the selection moves.)
    if (session.active && targetSketchId === editingId && editingId !== null)
      return;
    if (!session.active && targetSketchId === null) return;

    // Selection no longer matches the active session → commit it so live
    // edits persist (the ✗ button is the explicit discard). commitSketch()
    // routes through `commit()`, which resets the selection to the committed
    // sketch id; restore the caller's intended selection afterwards so a
    // switch A→B (or sketch→other feature) doesn't leave the old id selected.
    if (session.active) {
      const intended = this.state.selection;
      const intendedAnchor = this.state.selectionAnchor;
      this.deps.commitSketch();
      this.state = {
        ...this.state,
        selection: intended,
        selectionAnchor: intendedAnchor,
      };
      this.syncActiveBody();
    }
    // A sketch is now the sole selection → enter its edit session.
    if (targetSketchId !== null) this.deps.editSketch(targetSketchId);
  }

  // ── Chunk 3a: persistent B-Rep element selection ──────────────────

  /** Select a single B-Rep element (face / edge / vertex) picked tool-free
   *  in the viewport. Clears the feature-level selection so the two notions
   *  never both show. Ephemeral UI state — not persisted. */
  selectElement(el: PickedElement): void {
    this.deps.confirmPendingFeature();
    this.state = {
      ...this.state,
      selection: new Set(),
      selectionAnchor: null,
      selectedElement: el,
      selectedElements: [el],
      selectedDatum: null,
    };
    this.syncActiveBody();
    this.notify();
  }

  /** Add or remove `el` from the multi-select set (Shift/Ctrl-click). The
   *  primary `selectedElement` becomes the toggled element when added, or the
   *  new last member when removed. Used to gather coplanar faces for a single
   *  face-extrude. Clears the feature-level selection like {@link selectElement}. */
  toggleElement(el: PickedElement): void {
    this.deps.confirmPendingFeature();
    const same = (a: PickedElement, b: PickedElement) =>
      a.kind === b.kind && a.idHash === b.idHash;
    const present = this.state.selectedElements.some((e) => same(e, el));
    const next = present
      ? this.state.selectedElements.filter((e) => !same(e, el))
      : [...this.state.selectedElements, el];
    this.state = {
      ...this.state,
      selection: new Set(),
      selectionAnchor: null,
      selectedElement: next.length > 0 ? next[next.length - 1] : null,
      selectedElements: next,
      selectedDatum: null,
    };
    this.syncActiveBody();
    this.notify();
  }

  /** Replace (or, additive, extend) the element multi-select set with a
   *  whole batch at once — the box-select path (rectangle drag). Dedupes by
   *  kind+idHash; the last element becomes the primary. An empty batch with
   *  additive=false clears the element selection. */
  applyBoxSelection(els: PickedElement[], additive: boolean): void {
    const base = additive ? [...this.state.selectedElements] : [];
    const seen = new Set(base.map((e) => `${e.kind}:${e.idHash}`));
    for (const el of els) {
      const key = `${el.kind}:${el.idHash}`;
      if (seen.has(key)) continue;
      seen.add(key);
      base.push(el);
    }
    this.state = {
      ...this.state,
      selection: new Set(),
      selectionAnchor: null,
      selectedElement: base.length > 0 ? base[base.length - 1] : null,
      selectedElements: base,
      selectedDatum: null,
    };
    this.syncActiveBody();
    this.notify();
  }

  /** Replace (or extend) the feature-level selection with a set of ids —
   *  the object-mode box-select path (whole bodies → their seed features). */
  selectFeatureSet(ids: number[], additive: boolean): void {
    logUserAction("feature", "select-set", { ids, additive });
    const next = additive ? new Set(this.state.selection) : new Set<number>();
    for (const id of ids) next.add(id);
    this.state = {
      ...this.state,
      selection: next,
      selectionAnchor: null,
      selectedElement: null,
      selectedElements: [],
      selectedDatum: null,
    };
    this.syncActiveBody();
    this.deps.refreshMeshCache();
    this.notify();
  }

  /** Clear the persistent element selection. No-op when nothing is selected. */
  clearElementSelection(): void {
    if (
      this.state.selectedElement === null &&
      this.state.selectedElements.length === 0 &&
      this.state.selectedDatum === null
    )
      return;
    this.patch({
      selectedElement: null,
      selectedElements: [],
      selectedDatum: null,
    });
    this.notify();
  }

  /** Select a default-geometry datum ("origin", "top", "front", "right"),
   *  or clear with null (ticket r36f). Clears the feature- and element-
   *  level selections — only one selection notion shows at a time. */
  selectDatum(datumId: string | null): void {
    if (this.state.selectedDatum === datumId) {
      if (datumId === null) return;
    }
    this.state = {
      ...this.state,
      selection: new Set(),
      selectionAnchor: null,
      selectedElement: null,
      selectedElements: [],
      selectedDatum: datumId,
      highlightedBodyIndex: null,
    };
    this.notify();
  }

  // ── selectors (Review-Befund 8): components read these instead of
  //    touching store.kernel directly ─────────────────────────────────

  /** Features owning the selected B-Rep elements (or contributing to the
   *  highlighted body) — drives the outliner's related-row highlight
   *  (ticket r36f). */
  relatedFeatureIds(): ReadonlySet<number> {
    if (this.state.selectedElements.length > 0) {
      return resolveSelectionLinks(this.kernel, this.state.selectedElements)
        .featureIds;
    }
    if (this.state.highlightedBodyIndex !== null) {
      return bodyFeatureIds(this.kernel, this.state.highlightedBodyIndex);
    }
    return new Set<number>();
  }

  /** Body index owning the primary selected element, or `null` when no
   *  element is selected / the hash is unknown (mid-regen). */
  relatedBodyIndex(): number | null {
    if (this.state.selectedElements.length === 0) return null;
    return resolveSelectionLinks(this.kernel, this.state.selectedElements)
      .bodyIndex;
  }
}
