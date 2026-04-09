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

  private commit(nextSelection?: ReadonlySet<number>): void {
    this.state = {
      features: this.kernel.listFeatures(),
      selection: nextSelection ?? this.state.selection,
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
    let id: number;
    switch (kind) {
      case "box":
        id = this.kernel.addBox(op, [0, 0, 0], [10, 10, 10]);
        break;
      case "sphere":
        id = this.kernel.addSphere(op, [0, 0, 0], 10);
        break;
      case "cylinder":
        id = this.kernel.addCylinder(op, [0, 0, 0], 8, 10);
        break;
    }
    this.commit(new Set([id]));
    return id;
  }

  removeSelected(): boolean {
    if (this.state.selection.size === 0) return false;
    let removed = false;
    for (const id of this.state.selection) {
      if (this.kernel.removeFeature(id)) removed = true;
    }
    if (removed) this.commit(new Set());
    return removed;
  }

  removeFeature(id: number): boolean {
    if (!this.kernel.removeFeature(id)) return false;
    const next = new Set(this.state.selection);
    next.delete(id);
    this.commit(next);
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
    this.state = { ...this.state, selection: new Set([id]) };
    for (const listener of this.listeners) listener();
  }

  toggleSelect(id: number): void {
    const next = new Set(this.state.selection);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    this.state = { ...this.state, selection: next };
    for (const listener of this.listeners) listener();
  }

  clearSelection(): void {
    if (this.state.selection.size === 0) return;
    this.state = { ...this.state, selection: new Set() };
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
