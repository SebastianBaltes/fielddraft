import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: true,
    allowedHosts: true,
  },
  optimizeDeps: {
    exclude: ["./src/wasm/fielddraft_kernel.js"],
  },
  assetsInclude: ["**/*.wasm"],
});
