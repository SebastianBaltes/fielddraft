/**
 * True when a keyboard event originates from a text-entry surface that
 * must keep the keystroke for itself — so the global shortcut handler
 * (App.tsx) bails instead of stealing bare-key shortcuts mid-typing in
 * a form field (feature rename, parameter values, …).
 *
 * The script editor (Monaco) is handled one level up by gating bare-key
 * shortcuts on the active tab kind, not here: Monaco is only focusable
 * on the script tab, where the modelling shortcuts don't run at all.
 */
export function isTextEntryTarget(target: EventTarget | null): boolean {
  // Duck-typed rather than `instanceof HTMLElement`: that check is
  // unreliable across realms (and under jsdom in tests).
  const el = target as HTMLElement | null;
  if (!el || typeof el.tagName !== "string") return false;
  const tag = el.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA") return true;
  // `isContentEditable` is the browser signal; the attribute is a
  // fallback (jsdom doesn't implement the property).
  return (
    el.isContentEditable || el.getAttribute?.("contenteditable") === "true"
  );
}
