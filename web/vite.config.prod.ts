// Production Vite config — used by the prod image build (web/Dockerfile.prod).
//
// Identical to the dev/base config (vite.config.ts) except it makes the two
// production guarantees from PLATFORM-ARCHITECTURE.md §10.2 explicit and
// verifiable, instead of relying on Vite defaults:
//
//   * build.sourcemap = false  → no TS source maps in the shipped bundle
//     (minimises TS-source leak; the Rust kernel is compiled to WASM anyway).
//   * build.minify   = "esbuild" → minified JS.
//
// The base config is a function of `{ mode }`; we resolve it for the build mode
// and layer the prod build options on top so all the base plugins (live
// version, kernel stamp, asset handling) stay intact.
import { defineConfig, mergeConfig, type UserConfig } from "vite";
import baseFactory from "./vite.config";

export default defineConfig(async (env) => {
  const base = (await (
    baseFactory as unknown as (
      e: typeof env,
    ) => UserConfig | Promise<UserConfig>
  )(env)) as UserConfig;

  return mergeConfig(base, {
    // WP11: enforce the login gate in the deployed image. The base config
    // (vite.config.ts) leaves VITE_REQUIRE_LOGIN unset → local `just dev`, the
    // Playwright e2e and offline modeling all stay ungated. Only this prod path
    // bakes "1" into the bundle, so the shipped FieldDraft is unusable without a
    // session. `import.meta.env.VITE_REQUIRE_LOGIN` reads this constant.
    define: {
      "import.meta.env.VITE_REQUIRE_LOGIN": JSON.stringify("1"),
    },
    build: {
      sourcemap: false,
      minify: "esbuild",
      // Static asset hashing is on by default; keep it for cache-busting.
      emptyOutDir: true,
    },
  } satisfies UserConfig);
});
