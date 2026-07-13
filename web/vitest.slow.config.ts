import { defineConfig } from "vitest/config";

// The SLOW suite: acceptance tests that drive the real wasm kernel
// end-to-end (Onshape import, …). They weave the body after every feature,
// so a single file can cost tens of seconds — far too much for the edit
// loop, which is why `vitest.config.ts` excludes them. Run on demand:
//   npm run test:slow   /   just test-slow
export default defineConfig({
  define: {
    __APP_VERSION__: JSON.stringify("test"),
    __KERNEL_EXPECTED__: JSON.stringify("test"),
  },
  test: {
    environment: "jsdom",
    globals: true,
    include: ["src/**/*.slow.test.ts"],
    // A kernel weave is the unit of work here, not a millisecond assertion.
    testTimeout: 180_000,
    hookTimeout: 180_000,
  },
});
