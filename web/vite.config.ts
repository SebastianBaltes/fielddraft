import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";

const pkg = JSON.parse(readFileSync("package.json", "utf-8"));

const certDir = join(homedir(), ".local/share/tailscale-certs");
const certFile = join(certDir, "sorokan-xps-8960.tailnet.example.ts.net.crt");
const keyFile = join(certDir, "sorokan-xps-8960.tailnet.example.ts.net.key");
const hasTailscaleCert = existsSync(certFile) && existsSync(keyFile);

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  server: {
    port: 5173,
    strictPort: true,
    allowedHosts: true,
    https: hasTailscaleCert
      ? { cert: certFile, key: keyFile }
      : undefined,
  },
  optimizeDeps: {
    exclude: ["./src/wasm/fielddraft_kernel.js"],
  },
  assetsInclude: ["**/*.wasm"],
});
