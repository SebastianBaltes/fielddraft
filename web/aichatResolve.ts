/**
 * Module resolution for the AI-chat library (ticket #347).
 *
 * BaseAIChat is consumed as SOURCE from the git submodule under
 * `third_party/baseaichat` — i.e. from OUTSIDE this project (`web/`). Node's
 * resolution walks UP from the importing file, so its `import "react"` looks in
 * `third_party/**` and the repo root, never in `web/node_modules` where the
 * package actually is: without this map the library cannot resolve a single one
 * of its imports, in the dev server, the build AND vitest alike.
 *
 * So every bare specifier the library uses is pinned to OUR node_modules. That
 * also guarantees ONE React instance (two would break its hooks) — the same job
 * `resolve.dedupe` does for packages that resolve at all.
 *
 * Exact matches (regex), never prefixes: a plain `"react"` alias would also
 * rewrite `"react-dom"`, which starts with the same letters.
 */
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const WEB_ROOT = fileURLToPath(new URL(".", import.meta.url));
const REPO_ROOT = resolve(WEB_ROOT, "..");

/** The library's entry — what `import … from "baseaichat"` resolves to. */
export const AICHAT_ENTRY = resolve(
  REPO_ROOT,
  "third_party/baseaichat/web/src/index.ts",
);

/** Every bare specifier the library imports (grep of its src/). */
const AICHAT_DEPS = [
  "react",
  "react-dom",
  "ai",
  "@ai-sdk/anthropic",
  "@ai-sdk/google",
  "@ai-sdk/openai",
  "@openrouter/ai-sdk-provider",
  "acorn",
  "acorn-typescript",
  "marked",
  "dompurify",
];

/** Alias entries for Vite AND Vitest — keep both configs on this one map.
 *
 *  The pattern matches the package AND its subpaths (`react/jsx-runtime`,
 *  `@ai-sdk/google/internal`), but anchors on the full name — a plain `"react"`
 *  prefix alias would also swallow `"react-dom"`. */
export const aichatAliases = [
  { find: "baseaichat", replacement: AICHAT_ENTRY },
  ...AICHAT_DEPS.map((pkg) => ({
    find: new RegExp(`^${pkg.replace(/[/\\^$*+?.()|[\]{}]/g, "\\$&")}(/.*)?$`),
    replacement: resolve(WEB_ROOT, "node_modules", pkg) + "$1",
  })),
];
