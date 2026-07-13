import { defineConfig } from "vitest/config";

// Vitest 3 still pulls Vite 6 internally, which clashes types with the top-
// level Vite 8 install if we also load the React plugin here. Our unit tests
// are pure TS (no JSX), so we keep this config plugin-free and let the tsc
// pre-check run via `tsc -b` in the build step.
export default defineConfig({
  // The build-time `define` globals (injected by vite.config.ts in dev/build)
  // are not present in vitest. The App shell reads __APP_VERSION__/
  // __KERNEL_EXPECTED__ at render, so provide inert literals here — without them
  // any test that renders the shell throws "__APP_VERSION__ is not defined".
  define: {
    __APP_VERSION__: JSON.stringify("test"),
    __KERNEL_EXPECTED__: JSON.stringify("test"),
  },
  test: {
    environment: "jsdom",
    globals: true,
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    // `*.slow.test.ts` drives the REAL wasm kernel end-to-end (weaving the
    // body after every feature), which costs tens of seconds — too slow for
    // the edit loop. It runs as its own suite: `npm run test:slow` /
    // `just test-slow` (see vitest.slow.config.ts).
    exclude: ["**/node_modules/**", "**/dist/**", "src/**/*.slow.test.ts"],
  },
});
