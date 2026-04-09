/**
 * Minimal in-memory store wrapping the wasm kernel, with autosave + a
 * tiny pub-sub for React re-renders.
 *
 * The kernel is the ground truth. The store's job is to:
 *  - own the singleton kernel handle,
 *  - bump a `version` counter on every mutation so React subscribers
 *    re-render,
 *  - hold ephemeral UI state (current selection),
 *  - schedule debounced `localStorage` autosaves on every mutation.
 */

import { useEffect, useState, useSyncExternalStore } from "react";
import {
  BoolOpTag,
  Kernel,
  type BoolOpTagValue,
  type FeatureSummary,
  type PrimitiveKindName,
} from "./kernel";
import { createAutoSave, loadPersisted, type AutoSaveHandle } from "./persistence";

export interface StoreState {
  features: FeatureSummary[];
  selection: ReadonlySet<number>;
  /** Anchor for shift-click range selection — id of the last single-clicked
   *  feature. `null` while nothing has been clicked yet. */
  selectionAnchor: number | null;
  canUndo: boolean;
  canRedo: boolean;
  /** Default op applied when adding the next primitive. */
  nextOp: BoolOpTagValue;
}

export class KernelStore {
  readonly kernel: Kernel;
  private readonly autosave: AutoSaveHandle;
  private listeners = new Set<() => void>();
  private state: StoreState;

  constructor(kernel: Kernel, autosave: AutoSaveHandle) {
    this.kernel = kernel;
    this.autosave = autosave;
    this.state = {
      features: kernel.listFeatures(),
      selection: new Set(),
      selectionAnchor: null,
      canUndo: kernel.canUndo(),
      canRedo: kernel.canRedo(),
      nextOp: BoolOpTag.Union,
    };
  }

  // ── pub-sub ───────────────────────────────────────────────────────

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getState = (): StoreState => this.state;

  primitivesView(): Float32Array {
    return this.kernel.primitivesView();
  }

  private commit(nextSelection?: ReadonlySet<number>, nextAnchor?: number | null): void {
    this.state = {
      features: this.kernel.listFeatures(),
      selection: nextSelection ?? this.state.selection,
      selectionAnchor: nextAnchor !== undefined ? nextAnchor : this.state.selectionAnchor,
      canUndo: this.kernel.canUndo(),
      canRedo: this.kernel.canRedo(),
      nextOp: this.state.nextOp,
    };
    // Drop selection entries pointing at deleted features.
    if (this.state.selection.size > 0) {
      const live = new Set(this.state.features.map((f) => f.id));
      const filtered = new Set<number>();
      for (const id of this.state.selection) {
        if (live.has(id)) filtered.add(id);
      }
      if (filtered.size !== this.state.selection.size) {
        this.state = { ...this.state, selection: filtered };
      }
    }
    // Anchor must point at a still-living feature, or be cleared.
    if (
      this.state.selectionAnchor !== null &&
      !this.state.features.some((f) => f.id === this.state.selectionAnchor)
    ) {
      this.state = { ...this.state, selectionAnchor: null };
    }
    this.autosave.schedule(this.kernel.toJson());
    for (const listener of this.listeners) listener();
  }

  // ── mutations ─────────────────────────────────────────────────────

  setNextOp(op: BoolOpTagValue): void {
    if (this.state.nextOp === op) return;
    this.state = { ...this.state, nextOp: op };
    for (const listener of this.listeners) listener();
  }

  addPrimitive(kind: PrimitiveKindName, op: BoolOpTagValue = this.state.nextOp): number {
    // Default sizes are tuned to be clearly readable inside the
    // ~600 mm build volume, and the per-add stagger pulls each new
    // feature off the previous one so freshly clicked primitives
    // aren't hidden inside their predecessors.
    const offset: [number, number, number] = [
      this.state.features.length * 80,
      0,
      0,
    ];
    let id: number;
    switch (kind) {
      case "box":
        id = this.kernel.addBox(op, offset, [50, 50, 50]);
        break;
      case "sphere":
        id = this.kernel.addSphere(op, offset, 50);
        break;
      case "cylinder":
        id = this.kernel.addCylinder(op, offset, 40, 60);
        break;
    }
    this.commit(new Set([id]), id);
    return id;
  }

  /** Move every selected feature by `(dx, dy, dz)`. */
  nudgeSelected(dx: number, dy: number, dz: number): boolean {
    if (this.state.selection.size === 0) return false;
    let any = false;
    for (const id of this.state.selection) {
      if (this.kernel.nudgeFeature(id, dx, dy, dz)) any = true;
    }
    if (any) this.commit();
    return any;
  }

  removeSelected(): boolean {
    if (this.state.selection.size === 0) return false;
    let removed = false;
    for (const id of this.state.selection) {
      if (this.kernel.removeFeature(id)) removed = true;
    }
    if (removed) this.commit(new Set(), null);
    return removed;
  }

  removeFeature(id: number): boolean {
    if (!this.kernel.removeFeature(id)) return false;
    const next = new Set(this.state.selection);
    next.delete(id);
    const nextAnchor = this.state.selectionAnchor === id ? null : this.state.selectionAnchor;
    this.commit(next, nextAnchor);
    return true;
  }

  setFeatureOp(id: number, op: BoolOpTagValue): boolean {
    if (!this.kernel.setFeatureOp(id, op)) return false;
    this.commit();
    return true;
  }

  undo(): boolean {
    if (!this.kernel.undo()) return false;
    this.commit();
    return true;
  }

  redo(): boolean {
    if (!this.kernel.redo()) return false;
    this.commit();
    return true;
  }

  // ── selection ─────────────────────────────────────────────────────

  selectOnly(id: number): void {
    this.state = {
      ...this.state,
      selection: new Set([id]),
      selectionAnchor: id,
    };
    for (const listener of this.listeners) listener();
  }

  toggleSelect(id: number): void {
    const next = new Set(this.state.selection);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    this.state = {
      ...this.state,
      selection: next,
      selectionAnchor: id,
    };
    for (const listener of this.listeners) listener();
  }

  /**
   * Shift+Click range selection: select every feature between the
   * current anchor (inclusive) and the clicked id (inclusive). If no
   * anchor exists yet this falls back to a single selection.
   */
  selectRangeTo(id: number): void {
    const features = this.state.features;
    const targetIdx = features.findIndex((f) => f.id === id);
    if (targetIdx === -1) return;

    const anchor = this.state.selectionAnchor;
    if (anchor === null) {
      this.selectOnly(id);
      return;
    }
    const anchorIdx = features.findIndex((f) => f.id === anchor);
    if (anchorIdx === -1) {
      // Anchor stale; reset.
      this.selectOnly(id);
      return;
    }
    const [lo, hi] = anchorIdx <= targetIdx ? [anchorIdx, targetIdx] : [targetIdx, anchorIdx];
    const next = new Set<number>();
    for (let i = lo; i <= hi; i++) next.add(features[i].id);
    this.state = {
      ...this.state,
      selection: next,
      // Anchor stays put — that's the standard finder/Onshape behaviour.
      selectionAnchor: anchor,
    };
    for (const listener of this.listeners) listener();
  }

  clearSelection(): void {
    if (this.state.selection.size === 0 && this.state.selectionAnchor === null) return;
    this.state = { ...this.state, selection: new Set(), selectionAnchor: null };
    for (const listener of this.listeners) listener();
  }
}

let singleton: KernelStore | null = null;
let storePromise: Promise<KernelStore> | null = null;

export async function getStore(): Promise<KernelStore> {
  if (singleton) return singleton;
  if (storePromise) return storePromise;

  storePromise = (async () => {
    const kernel = await Kernel.create();

    const persisted = loadPersisted();
    if (persisted) {
      try {
        kernel.loadJson(persisted);
      } catch (err) {
        console.warn("fielddraft: could not rehydrate persisted document", err);
      }
    }

    const autosave = createAutoSave();
    const store = new KernelStore(kernel, autosave);

    if (typeof window !== "undefined") {
      window.addEventListener("beforeunload", () => autosave.flush());
    }

    singleton = store;
    return store;
  })();

  return storePromise;
}

/** React hook that boots the store and re-renders on every mutation. */
export function useKernelStore(): {
  store: KernelStore | null;
  state: StoreState | null;
  error: Error | null;
} {
  const [store, setStore] = useState<KernelStore | null>(null);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    let cancelled = false;
    getStore()
      .then((s) => {
        if (!cancelled) setStore(s);
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e : new Error(String(e)));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Subscribe to the live state when the store is ready.
  const state = useSyncExternalStore(
    (onChange) => (store ? store.subscribe(onChange) : () => {}),
    () => (store ? store.getState() : null),
    () => (store ? store.getState() : null),
  );

  return { store, state, error };
}
