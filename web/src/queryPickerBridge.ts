/**
 * Query-picker bridge.
 *
 * When a FeatureDialog's `QueryWidget` enters "click-focus" mode
 * (onshape-ux `Selection-Input` with `status: "Active/Highlight"`),
 * it registers itself here. The Viewport's click handler checks
 * the bridge *before* doing its default selection behaviour:
 * if a picker is active, the viewport converts the pick result
 * into a query expression and hands it to the registered callback
 * instead of changing the feature selection.
 *
 * The bridge is intentionally a plain module-scoped singleton —
 * there is only ever one focused picker at a time, and both the
 * widget and the viewport need cheap synchronous access without
 * threading React context through R3F's Canvas boundary.
 *
 * Subscribe pattern:
 *   - `register(handler)` activates a picker. Returns an `unregister`
 *     function that the widget calls on unmount or when the user
 *     commits/cancels.
 *   - `getActive()` returns the current handler or null; the
 *     viewport calls this on every click.
 *   - `subscribe(listener)` lets UI parts (e.g. the widget itself)
 *     react to active-picker changes — we use it so that activating
 *     widget A implicitly deactivates widget B.
 */

export interface ActivePicker {
  /** Stable id of the owning widget — used to guard against a
   *  second activation from the same widget cancelling its own
   *  registration. */
  ownerId: string;
  /** Called by the Viewport click handler. Returning `true`
   *  indicates the pick was consumed and the Viewport should
   *  suppress its default selection behaviour for this click. */
  onPick: (query: string) => boolean;
}

let active: ActivePicker | null = null;
const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of listeners) listener();
}

/**
 * Activate a picker. Any previously active picker is deactivated
 * silently (its owner widget is expected to notice via
 * `subscribe()` and drop its local focus state).
 */
export function register(picker: ActivePicker): () => void {
  active = picker;
  notify();
  return () => {
    if (active && active.ownerId === picker.ownerId) {
      active = null;
      notify();
    }
  };
}

/** Current active picker, or `null`. */
export function getActive(): ActivePicker | null {
  return active;
}

/** Is the widget with this `ownerId` currently active? */
export function isActive(ownerId: string): boolean {
  return active !== null && active.ownerId === ownerId;
}

/** Subscribe to active-picker changes. Returns unsubscribe. */
export function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
