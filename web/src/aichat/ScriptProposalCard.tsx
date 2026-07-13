/**
 * The gate the user actually sees (ticket #347).
 *
 * The assistant may WRITE FieldSkript, but it cannot RUN it. A proposed script
 * surfaces here — intent, then the full source, unedited — and nothing happens
 * until the user presses "Ausführen". Discarding is the safe path and the one a
 * closed dialog takes.
 *
 * This is not decoration: a script runs inside the feature build with everything
 * FieldSkript can do. The guard restricts the script's vocabulary, not the power
 * of the runtime. The user is the part that judges intent.
 */

import { useSyncExternalStore, useState } from "react";
import type { KernelStore } from "../store";
import {
  acceptScriptProposal,
  discardScriptProposal,
  getScriptProposal,
  subscribeScriptProposal,
} from "./scriptProposals";

export function ScriptProposalCard({ store }: { store: KernelStore }) {
  const proposal = useSyncExternalStore(
    subscribeScriptProposal,
    getScriptProposal,
  );
  const [error, setError] = useState<string | null>(null);

  if (!proposal) return null;

  const run = () => {
    const result = acceptScriptProposal(store);
    if (!result.ok) setError(result.error);
    else setError(null);
  };

  return (
    <div className="fd-script-proposal" data-testid="script-proposal">
      <div className="head">
        <strong>Der Assistent möchte ein Skript ausführen</strong>
        <span className="name">{proposal.name}</span>
      </div>
      {proposal.intent && <p className="intent">{proposal.intent}</p>}
      <pre className="source" data-testid="script-proposal-source">
        <code>{proposal.source}</code>
      </pre>
      {error && (
        <p className="error" role="alert" data-testid="script-proposal-error">
          {error}
        </p>
      )}
      <p className="note">
        Es läuft im Modell-Kernel und kann Geometrie erzeugen und verändern —
        rückgängig machen geht mit Strg+Z.
      </p>
      <div className="actions">
        <button
          type="button"
          className="secondary"
          data-testid="script-proposal-discard"
          onClick={() => {
            setError(null);
            discardScriptProposal();
          }}
        >
          Verwerfen
        </button>
        <button
          type="button"
          className="primary"
          data-testid="script-proposal-run"
          onClick={run}
        >
          Ausführen
        </button>
      </div>
    </div>
  );
}
