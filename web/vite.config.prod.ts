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
    build: {
      sourcemap: false,
      minify: "esbuild",
      // Static asset hashing is on by default; keep it for cache-busting.
      emptyOutDir: true,
    },
  } satisfies UserConfig);
});
