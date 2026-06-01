import { defineConfig, loadEnv, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";

const pkg = JSON.parse(readFileSync("package.json", "utf-8"));

function liveVersion(): Plugin {
  const versionModules = new Set<string>();
  return {
    name: "live-version",
    config(_, { command }) {
      if (command === "build") {
        return { define: { __APP_VERSION__: JSON.stringify(pkg.version) } };
      }
    },
    configureServer(server) {
      server.watcher.on("change", (file) => {
        if (!file.endsWith("package.json")) return;
        for (const id of versionModules) {
          const mod = server.moduleGraph.getModuleById(id);
          if (mod) server.moduleGraph.invalidateModule(mod);
        }
        server.hot.send({ type: "full-reload" });
      });
    },
    transform(code, id) {
      if (!id.includes("node_modules") && code.includes("__APP_VERSION__")) {
        versionModules.add(id);
        const v = JSON.parse(readFileSync("package.json", "utf-8")).version;
        return code.replaceAll("__APP_VERSION__", JSON.stringify(v));
      }
    },
  };
}

const certDir = join(homedir(), ".local/share/tailscale-certs");
const certFile = join(certDir, "sorokan-xps-8960.tailnet.example.ts.net.crt");
const keyFile = join(certDir, "sorokan-xps-8960.tailnet.example.ts.net.key");
const hasTailscaleCert = existsSync(certFile) && existsSync(keyFile);

// Per-instance dev-server port: lets `fielddraft` and `fielddraft-a` run side
// by side. Sourced (in order) from the recipe env (e.g. `just dev`, which
// dotenv-loads the repo-root .env), then the repo-root .env directly (so a bare
// `npm run dev` works too), then the 5173 default. See ../.env.example.
function webPort(mode: string): number {
  const repoRoot = join(process.cwd(), "..");
  const env = loadEnv(mode, repoRoot, "");
  const raw = process.env.FIELDDRAFT_WEB_PORT ?? env.FIELDDRAFT_WEB_PORT;
  const port = Number(raw);
  return Number.isInteger(port) && port > 0 ? port : 5173;
}

// https://vitejs.dev/config/
export default defineConfig(({ mode }) => ({
  plugins: [react(), liveVersion()],
  server: {
    port: webPort(mode),
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
}));
