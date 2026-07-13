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
 *    a hardcoded client-side model id would just earn a 403.
 *
 * Loaded lazily by App.tsx: the AI SDK is heavy and most sessions never open the
 * chat.
 */

import { useEffect, useMemo, useState } from "react";
import { ChatPanel } from "baseaichat";
import type { KernelStore } from "../store";
import { agentOptions, type AIChatConfig } from "./setup";

export function ChatPopup({ store }: { store: KernelStore }) {
  const [config, setConfig] = useState<AIChatConfig | null>(null);

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
    () => (config?.enabled ? agentOptions(store, config.models) : null),
    [store, config],
  );

  if (!agent) return null;

  return (
    <ChatPanel
      agent={agent}
      title="FieldDraft-Assistent"
      launcherLabel="Assistent"
      greeting={
        "Ich kann Fragen zu FieldDraft beantworten und das Modell für dich bauen. " +
        "Zum Beispiel: „Was ist in diesem Dokument?“ oder „Bau mir eine Grundplatte " +
        "80 × 60 × 5 mm.“"
      }
      placeholder="Frag mich etwas — oder sag mir, was ich bauen soll…"
      suggestions={[
        { label: "Was ist in diesem Dokument?", prompt: "Was ist in diesem Dokument?" },
        {
          label: "Grundplatte bauen",
          prompt: "Bau mir eine Grundplatte 80 × 60 × 5 mm.",
        },
        {
          label: "Wie skizziere ich?",
          prompt: "Wie erstelle ich in FieldDraft eine Skizze und extrudiere sie?",
        },
      ]}
    />
  );
}
