/**
 * Type surface of the BaseAIChat library we actually use (ticket #347).
 *
 * Why a hand-written declaration instead of pointing TypeScript at the library's
 * source: the library is consumed as SOURCE through a Vite alias (it lives in the
 * git submodule under third_party/, outside this project's root). Letting `tsc`
 * follow that alias would drag ITS files through OUR compiler settings — a
 * different `strict` profile, a different module resolution root, its own React
 * types — and report errors in code that is not ours to fix. Vite still bundles
 * the real source; this file only describes the boundary we lean on.
 *
 * Pinned against baseaichat v0.3.1 (submodule tag). If a signature here stops
 * matching, that is the pin telling you the library moved — update both together.
 */
declare module "baseaichat" {
  import type { ReactElement } from "react";

  export type Evaluator = (code: string) => Promise<string>;

  export interface EvaluatorOptions {
    api: Record<string, unknown>;
    onBeforeRun?: () => void;
    onAfterRun?: () => void;
    transformResult?: (result: unknown) => unknown;
    maxCodeLength?: number;
    maxResultLength?: number;
    timeoutMs?: number;
    maxCallsPerWindow?: number;
    rateWindowMs?: number;
    /** Additional globals the guard must reject (v0.2.0: the guard is a
     *  blocklist, so app-specific globals have to be named here). */
    extraBlocked?: string[];
    /** Globals to un-block from the default list. */
    allowBlocked?: string[];
  }
  export function createEvaluator(options: EvaluatorOptions): Evaluator;

  export interface GuardOptions {
    extraBlocked?: string[];
    allowBlocked?: string[];
    maxLength?: number;
    maxNodes?: number;
  }
  export interface GuardResult {
    ok: boolean;
    reason?: string;
    at?: { line: number; column: number };
  }
  export function guardCode(code: string, options?: GuardOptions): GuardResult;

  export interface ScriptGuardOptions extends GuardOptions {
    /** Module specifiers a script may import. Empty = no imports at all. */
    allowImportsFrom?: string[];
  }
  /** Guards TYPESCRIPT (classes and `this` allowed) with the same blocklists. */
  export function guardScript(
    source: string,
    options?: ScriptGuardOptions,
  ): GuardResult;

  export interface UIStateOptions {
    root?: ParentNode;
    withPositions?: boolean;
    precision?: number;
    context?: unknown;
  }
  export function readUIState(
    options?: UIStateOptions,
  ): Record<string, unknown>;

  export interface UIActionOptions {
    root?: ParentNode;
    highlightClass?: string;
    highlightMs?: number;
    maxActionsPerWindow?: number;
    rateWindowMs?: number;
    onAction?: (action: string, testId: string) => void;
  }
  /**
   * The FOUR functions the library actually returns (browser/uiActions.ts) — named
   * one by one, on purpose. This used to be `Record<string, (...args: never[]) =>
   * unknown>`, which types every member as "some function": our agent API could
   * then bind `click: ui.clickElement` even if the library had no such function,
   * and the compiler shrugged. These four are the ones whose names the model-facing
   * description got wrong — they are exactly the ones that must be checked.
   *
   * The keys are the LIBRARY's names. What the model gets to call is decided in
   * setup.ts (`click: ui.clickElement`), not here.
   */
  export interface UIActions {
    highlightElement(key: string): void;
    clickElement(key: string): void;
    selectOption(key: string, option: string): void;
    fillInput(key: string, value: string | number): void;
  }
  export function createUIActions(options?: UIActionOptions): UIActions;

  export function expandRuntimeTypes(
    source: string,
    context?: Record<string, unknown>,
  ): string;
  export function roundValues(value: unknown, precision: number): unknown;

  export type ProviderName = "google" | "openai" | "anthropic" | "openrouter";

  export interface ChatAgentOptions {
    provider?: ProviderName;
    model: string;
    visionModel?: string;
    proxyUrl?: string;
    systemPrompt: string | string[];
    toolDescription: string;
    evaluate: Evaluator;
    appContext?: () => string;
    maxSteps?: number;
  }

  export interface Suggestion {
    label: string;
    prompt?: string;
  }

  export interface ChatPanelProps {
    agent: ChatAgentOptions;
    title?: string;
    launcherLabel?: string;
    greeting?: string;
    placeholder?: string;
    suggestions?: Suggestion[];
    defaultOpen?: boolean;
    allowImages?: boolean;
    showToolActivity?: boolean;
  }
  export function ChatPanel(props: ChatPanelProps): ReactElement;
}
