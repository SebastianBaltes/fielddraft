/**
 * True when a keyboard event originates from a text-entry surface that
 * must keep the keystroke for itself — so the global shortcut handler
 * (App.tsx) bails instead of stealing bare-key shortcuts mid-typing.
 *
 * Plain form controls are caught by tag/contentEditable, but Monaco's
 * input element is not always a <textarea>: with the EditContext API it
 * is a focusable <div> that is neither TEXTAREA nor contentEditable.
 * Matching on the editor root's `.monaco-editor` class covers every
 * Monaco input surface (textarea, contenteditable, EditContext div).
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
  if (el.isContentEditable || el.getAttribute?.("contenteditable") === "true") {
    return true;
  }
  return el.closest?.(".monaco-editor") != null;
}
