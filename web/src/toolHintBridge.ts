/**
 * Ticket D1 (jq6i / 3rz2 / "generally all tools"): a single module-level signal
 * for the active tool's current instruction. Any tool — sketch or not —
 * publishes a short hint ("Click the mirror axis…") via {@link setToolHint};
 * the {@link ToolHint} component renders it as a centered pill at the top of
 * the viewport (Onshape style). Mirrors the lightweight bridge pattern used by
 * the sketch HUD / camera bridges so tools can drive UI outside their subtree
 * without threading props or store state through every layer.
 */

let hint: string | null = null;
const listeners = new Set<() => void>();

/** Set (or clear, with `null`) the active tool hint. No-op when unchanged so a
 *  per-frame caller doesn't spuriously re-render the pill. */
export function setToolHint(next: string | null): void {
  if (next === hint) return;
  hint = next;
  for (const l of listeners) l();
}

/** The current hint, or `null` when no tool is guiding the user. */
export function getToolHint(): string | null {
  return hint;
}

/** Subscribe to hint changes; returns an unsubscribe fn. */
export function subscribeToolHint(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}
