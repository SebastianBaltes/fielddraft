/**
 * Ticket D1: the active tool's instruction, rendered as a centered pill at the
 * top of the viewport (Onshape style). Subscribes to {@link toolHintBridge};
 * renders nothing when no tool is guiding the user. Purely informational —
 * `pointer-events: none` so it never eats a click meant for the scene.
 */
import { useSyncExternalStore } from "react";
import { getToolHint, subscribeToolHint } from "./toolHintBridge";

export function ToolHint() {
  const hint = useSyncExternalStore(subscribeToolHint, getToolHint, getToolHint);
  if (!hint) return null;
  return (
    <div className="fd-tool-hint" role="status" data-testid="tool-hint">
      {hint}
    </div>
  );
}
