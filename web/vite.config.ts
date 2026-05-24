import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import basicSsl from "@vitejs/plugin-basic-ssl";
import { readFileSync } from "node:fs";

const pkg = JSON.parse(readFileSync("package.json", "utf-8"));

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react(), basicSsl()],
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
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
