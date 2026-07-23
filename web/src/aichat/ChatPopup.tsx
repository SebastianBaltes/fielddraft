/**
 * The FieldDraft chat popup (ticket #347).
 *
 * A thin shell around BaseAIChat's `ChatPanel` (which brings its own floating
 * launcher + panel). Two things this layer owns:
 *
 *  · **The feature gate.** The server answers /api/aichat/config with
 *    `enabled:false` when no OpenRouter key is configured — then nothing mounts,
 *    rather than a chat that fails on the first message.
 *  · **The model comes from the SERVER.** The proxy's allowlist is the authority;
 *    a hardcoded client-side model id would just earn a 403. Admins may pick
 *    among the allowlisted models via a dropdown of its own row below the
 *    panel header (ticket #361) — the request still goes through the same
 *    proxy allowlist, so an admin can only ever select a model the server
 *    already permits.
 *
 * Loaded lazily by App.tsx: the AI SDK is heavy and most sessions never open the
 * chat.
 */

import { useEffect, useMemo, useState } from "react";
import { ChatPanel } from "baseaichat";
import type { KernelStore } from "../store";
import { agentOptions, type AIChatConfig } from "./setup";
import { ScriptProposalCard } from "./ScriptProposalCard";

/** Shortens `provider/model-id` to just the model id for the dropdown label —
 *  the provider prefix is implied (OpenRouter) and only adds noise. */
function modelLabel(model: string): string {
  const slash = model.indexOf("/");
  return slash === -1 ? model : model.slice(slash + 1);
}

export function ChatPopup({
  store,
  isAdmin = false,
}: {
  store: KernelStore;
  /** Admins get a model picker below the panel header (ticket #361); everyone
   *  else gets the server's default model with no control over it. */
  isAdmin?: boolean;
}) {
  const [config, setConfig] = useState<AIChatConfig | null>(null);
  const [selectedModel, setSelectedModel] = useState<string | undefined>(
    undefined,
  );

  useEffect(() => {
    let cancelled = false;
    void fetch("/api/aichat/config", { credentials: "include" })
      .then((r) => (r.ok ? r.json() : { enabled: false, models: [] }))
      .then((c: AIChatConfig) => {
        if (!cancelled) setConfig(c);
      })
      .catch(() => {
        if (!cancelled) setConfig({ enabled: false, models: [] });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const agent = useMemo(
    () =>
      config?.enabled
        ? agentOptions(store, config.models, selectedModel)
        : null,
    [store, config, selectedModel],
  );

  if (!agent) return null;

  const modelPicker =
    isAdmin && config && config.models.length > 1 ? (
      <select
        className="fd-aichat-model-select"
        aria-label="OpenRouter-Modell"
        value={selectedModel ?? config.models[0]}
        onChange={(e) => setSelectedModel(e.target.value)}
      >
        {config.models.map((m) => (
          <option key={m} value={m}>
            {modelLabel(m)}
          </option>
        ))}
      </select>
    ) : undefined;

  return (
    <>
      {/* The assistant may write FieldSkript but never run it — the user does
        (scriptProposals.ts). Rendered outside the panel so it stays visible
        even if the user collapses the chat. */}
      <ScriptProposalCard store={store} />
      {/* #360: the panel's own styling ships small (11–15 px) low-contrast text.
        This wrapper scopes FieldDraft's readability overrides (aichat.css) to the
        embedded BaseAIChat panel — the panel renders in-tree (no portal), so a
        `.fd-aichat` ancestor reliably out-specifies the submodule's `.bac-*`
        rules regardless of stylesheet load order. */}
      <div className="fd-aichat">
        <ChatPanel
          agent={agent}
          title="FieldDraft-Assistent"
          launcherLabel="Assistent"
          greeting="Ich kann Fragen zu FieldDraft beantworten und das Modell für dich bauen."
          placeholder="Frag mich…"
          subheader={modelPicker}
        />
      </div>
    </>
  );
}
