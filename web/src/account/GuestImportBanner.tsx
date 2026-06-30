/**
 * GuestImportBanner — the WP9 B4 first-login guest-import prompt.
 *
 * A slim, dismissable banner shown once, after a guest with real local work
 * logs in or registers, offering to import their localStorage guest project as
 * a new server project. It is deliberately non-intrusive: the modeling app
 * stays fully usable behind it, declining never loses the guest project, and
 * the banner self-dismisses after a successful import.
 *
 * It owns no session state — App.tsx decides *whether* to render it (via
 * {@link shouldOfferGuestImport}) and supplies the import action.
 */

import { useState } from "react";

interface GuestImportBannerProps {
  /** Import the guest project: create a server project from the guest document
   *  and bind it. Resolves on success; rejects (throwing) on failure. */
  onImport(): Promise<void>;
  /** Dismiss without importing (the guest project is kept locally). */
  onDismiss(): void;
}

export function GuestImportBanner({
  onImport,
  onDismiss,
}: GuestImportBannerProps) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const doImport = async () => {
    setBusy(true);
    setError(null);
    try {
      await onImport();
      // onImport dismisses on success (App swaps the banner off); nothing else
      // to do here.
    } catch (err) {
      setError(
        err instanceof Error && err.message
          ? err.message
          : "Could not import your project",
      );
      setBusy(false);
    }
  };

  return (
    <div
      className="fd-guest-import"
      role="region"
      aria-label="Import your local project"
      data-testid="guest-import-banner"
    >
      <div className="message">
        <span className="title">Keep your local work?</span>
        <span className="detail">
          You have an unsaved project on this device. Import it into your
          account as a new project?
        </span>
        {error && (
          <span className="error" role="alert" data-testid="guest-import-error">
            {error}
          </span>
        )}
      </div>
      <div className="actions">
        <button
          type="button"
          className="primary"
          data-testid="guest-import-accept"
          disabled={busy}
          onClick={() => void doImport()}
        >
          {busy ? "Importing…" : "Import project"}
        </button>
        <button
          type="button"
          className="secondary"
          data-testid="guest-import-dismiss"
          disabled={busy}
          onClick={onDismiss}
        >
          Not now
        </button>
      </div>
    </div>
  );
}
