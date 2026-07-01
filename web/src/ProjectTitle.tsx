/**
 * ProjectTitle — the editable project name shown next to the logo (Ticket C).
 *
 * Displayed in the topbar only inside the modeling view. Clicking the name
 * turns it into an inline text input; committing on Enter or blur renames the
 * project through {@link KernelStore.renameProject}, which updates local store
 * state and — for a server-bound project — persists via
 * `PATCH /api/projects/{id}`. Escape cancels the edit. An empty / unchanged
 * name is a no-op (the store guards it too).
 */

import { useEffect, useRef, useState } from "react";
import type { KernelStore } from "./store";

export function ProjectTitle({
  store,
  name,
}: {
  store: KernelStore;
  name: string;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(name);
  const inputRef = useRef<HTMLInputElement>(null);

  // Seed the draft from the live name whenever an edit starts, and focus +
  // select the input so typing replaces the current name.
  useEffect(() => {
    if (editing) {
      setDraft(name);
      inputRef.current?.focus();
      inputRef.current?.select();
    }
    // `name` intentionally omitted: re-seeding mid-edit on an async rename would
    // clobber the user's in-progress text.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing]);

  const commit = () => {
    store.renameProject(draft);
    setEditing(false);
  };

  const cancel = () => {
    setEditing(false);
  };

  if (editing) {
    return (
      <input
        ref={inputRef}
        className="project-title-input"
        type="text"
        data-testid="project-title-input"
        aria-label="Project name"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          } else if (e.key === "Escape") {
            e.preventDefault();
            cancel();
          }
        }}
      />
    );
  }

  return (
    <button
      type="button"
      className="project-title"
      data-testid="project-title"
      title="Rename project"
      onClick={() => setEditing(true)}
    >
      {name}
    </button>
  );
}
