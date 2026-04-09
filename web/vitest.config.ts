import { defineConfig } from "vitest/config";

// Vitest 3 still pulls Vite 6 internally, which clashes types with the top-
// level Vite 8 install if we also load the React plugin here. Our unit tests
// are pure TS (no JSX), so we keep this config plugin-free and let the tsc
// pre-check run via `tsc -b` in the build step.
export default defineConfig({
  test: {
    environment: "jsdom",
    globals: true,
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
});
