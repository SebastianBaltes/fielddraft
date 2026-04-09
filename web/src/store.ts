/**
 * Minimal in-memory store wrapping the wasm kernel, with an autosave hook.
 *
 * This is deliberately tiny — no Redux, no Zustand — because the whole point
 * of FieldDraft's architecture is that the Rust kernel is the ground truth.
 * The store just holds the `Kernel` handle, tracks a version counter so
 * React re-renders on mutation, and triggers the debounced `localStorage`
 * persistence.
 */

import { useEffect, useState } from "react";
import { Kernel, PRIMITIVE_STRIDE } from "./kernel";
import { createAutoSave, loadPersisted, type AutoSaveHandle } from "./persistence";

export interface KernelStore {
  kernel: Kernel;
  /** Monotonic counter bumped on every mutation. Used as a React render key. */
  version: number;
  autosave: AutoSaveHandle;
  /** Current snapshot of the packed primitive buffer. */
  primitives(): Float32Array;
}

let singleton: KernelStore | null = null;

/**
 * Lazily create the store. Subsequent calls return the same instance.
 */
export async function getStore(): Promise<KernelStore> {
  if (singleton) return singleton;

  const kernel = await Kernel.create();

  // Rehydrate from localStorage if a previous session exists.
  const persisted = loadPersisted();
  if (persisted) {
    try {
      kernel.loadJson(persisted);
    } catch (err) {
      console.warn("fielddraft: could not rehydrate persisted document", err);
    }
  }

  const autosave = createAutoSave();

  const store: KernelStore = {
    kernel,
    version: 0,
    autosave,
    primitives: () => kernel.primitivesView(),
  };

  // Flush on unload so in-flight edits aren't lost.
  if (typeof window !== "undefined") {
    window.addEventListener("beforeunload", () => autosave.flush());
  }

  singleton = store;
  return store;
}

/** React hook that boots the store and re-renders on version changes. */
export function useKernelStore(): {
  store: KernelStore | null;
  error: Error | null;
  primitiveCount: number;
  stride: number;
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

  return {
    store,
    error,
    primitiveCount: store?.kernel.primitiveCount() ?? 0,
    stride: PRIMITIVE_STRIDE,
  };
}
