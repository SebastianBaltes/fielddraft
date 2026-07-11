import { defineConfig, loadEnv, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { existsSync, readFileSync } from "node:fs";
import { join, resolve, isAbsolute, sep } from "node:path";
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

// Ticket a020ac: Vite 8 runs vite:beforeFullReload listeners via
// Promise.allSettled, so the old cztw trick (throwing cancels the reload) is
// silently dead — "Auto-reload on code change: off" reloaded anyway. The dev
// server therefore never sends a raw `full-reload` to the browser anymore:
// every one (Vite core AND our plugins) is rewritten to the custom
// `fd:full-reload` event, and the CLIENT decides in main.tsx whether to call
// location.reload() (the Auto-reload setting). Dev-only by construction.
function reloadGate(): Plugin {
  return {
    name: "fd-reload-gate",
    configureServer(server) {
      const channels = new Set<{ send: (...args: unknown[]) => void }>();
      const clientHot = (
        server as unknown as {
          environments?: { client?: { hot?: { send: (...args: unknown[]) => void } } };
        }
      ).environments?.client?.hot;
      if (clientHot) channels.add(clientHot);
      channels.add(server.hot as unknown as { send: (...args: unknown[]) => void });
      for (const ch of channels) {
        const send = ch.send.bind(ch);
        ch.send = (...args: unknown[]) => {
          const payload = args[0] as { type?: string } | undefined;
          if (payload && payload.type === "full-reload") {
            send({ type: "custom", event: "fd:full-reload" });
            return;
          }
          send(...args);
        };
      }

      // Ticket dc02f591: with "Auto-reload on code change" OFF the app must
      // be COMPLETELY untouched by code changes — module hot-updates
      // re-execute code and reset UI state even without a full reload. The
      // client announces its preference (fd:hot-mute); we then drop every
      // hot payload (update / prune / the fd:full-reload custom event) at
      // that client's socket. F5 still loads the latest code, and toggling
      // the setting back on unmutes live.
      server.ws.on(
        "fd:hot-mute",
        (data: { muted?: boolean }, client: unknown) => {
          const raw = (
            client as { socket?: { send(payload: string): void } }
          ).socket as
            | { send(payload: string): void; __fdSend?: (p: string) => void }
            | undefined;
          if (!raw) return;
          if (data?.muted) {
            if (raw.__fdSend) return; // already muted
            raw.__fdSend = raw.send.bind(raw);
            raw.send = (str: string) => {
              try {
                const p = JSON.parse(str) as { type?: string; event?: string };
                if (
                  p.type === "update" ||
                  p.type === "prune" ||
                  p.type === "full-reload" ||
                  (p.type === "custom" && p.event === "fd:full-reload")
                ) {
                  return;
                }
              } catch {
                // non-JSON frame — pass through
              }
              raw.__fdSend!(str);
            };
          } else if (raw.__fdSend) {
            raw.send = raw.__fdSend;
            delete raw.__fdSend;
          }
        },
      );
    },
  };
}

// Boot-time staleness guard source. `build-wasm` writes the WASM stamp into
// this sidecar; the guard in kernel.ts compares the loaded module's self-
// reported stamp against `__KERNEL_EXPECTED__` (defined from this file). Both
// values come from the same `build-wasm` run, so a cosmetic package.json bump
// never trips the guard — only a genuinely stale loaded module does.
const KERNEL_STAMP = join("src", "wasm", ".kernel-version");

function readKernelStamp(): string {
  try {
    return readFileSync(KERNEL_STAMP, "utf-8").trim() || "dev";
  } catch {
    return "dev";
  }
}

function kernelStamp(): Plugin {
  const stampModules = new Set<string>();
  return {
    name: "kernel-stamp",
    config(_, { command }) {
      if (command === "build") {
        return {
          define: { __KERNEL_EXPECTED__: JSON.stringify(readKernelStamp()) },
        };
      }
    },
    configureServer(server) {
      server.watcher.add(KERNEL_STAMP);
      server.watcher.on("change", (file) => {
        if (!file.endsWith(".kernel-version")) return;
        for (const id of stampModules) {
          const mod = server.moduleGraph.getModuleById(id);
          if (mod) server.moduleGraph.invalidateModule(mod);
        }
        server.hot.send({ type: "full-reload" });
      });
    },
    transform(code, id) {
      if (
        !id.includes("node_modules") &&
        code.includes("__KERNEL_EXPECTED__")
      ) {
        stampModules.add(id);
        return code.replaceAll(
          "__KERNEL_EXPECTED__",
          JSON.stringify(readKernelStamp()),
        );
      }
    },
  };
}

const certDir = join(homedir(), ".local/share/tailscale-certs");
const certFile = join(certDir, "sorokan-xps-8960.tailnet.example.ts.net.crt");
const keyFile = join(certDir, "sorokan-xps-8960.tailnet.example.ts.net.key");
const hasTailscaleCert = existsSync(certFile) && existsSync(keyFile);

// Dev-only same-origin file read for the ClaudeDraft "open ticket" deep-link
// (`/?project=tickets/open/<id>/project.fdraft`). The webapp's boot loader
// (bootProject.ts) fetches `/api/file?path=<repo-root-relative>` to pull the
// project.fdraft + sibling settings.json out of the slave's OWN filesystem —
// same origin, no CORS, no ClaudeDraft round-trip. The tickets dir lives at the
// repo root (sibling of web/), OUTSIDE Vite's static `fs.allow`, so a middleware
// (which bypasses that gate) is the clean way to serve it. Sandboxed to the repo
// root with a path-traversal guard. No-op in a production build (the build has
// no server; the Go backend would mirror this route if ever needed in prod).
function fileReader(): Plugin {
  const repoRoot = resolve(process.cwd(), "..");
  const rootPrefix = repoRoot.endsWith(sep) ? repoRoot : repoRoot + sep;
  return {
    name: "file-reader",
    configureServer(server) {
      server.middlewares.use("/api/file", (req, res) => {
        if (req.method !== "GET") {
          res.statusCode = 405;
          res.end();
          return;
        }
        let rel: string | null = null;
        try {
          rel = new URL(req.url ?? "", "http://localhost").searchParams.get(
            "path",
          );
        } catch {
          rel = null;
        }
        if (!rel || isAbsolute(rel)) {
          res.statusCode = 400;
          res.end("bad path");
          return;
        }
        const resolved = resolve(repoRoot, rel);
        if (resolved !== repoRoot && !resolved.startsWith(rootPrefix)) {
          res.statusCode = 403;
          res.end("outside repo");
          return;
        }
        if (!existsSync(resolved)) {
          res.statusCode = 404;
          res.end("not found");
          return;
        }
        try {
          const data = readFileSync(resolved);
          res.statusCode = 200;
          res.setHeader("Content-Type", "application/octet-stream");
          res.setHeader("Cache-Control", "no-store");
          res.end(data);
        } catch (err) {
          res.statusCode = 500;
          res.end(String(err));
        }
      });
    },
  };
}

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

// Per-instance backend port for the dev proxy — mirrors webPort. The Go server
// (docker compose / `just dev-full`) is published on FIELDDRAFT_SERVER_PORT on
// the host; defaults to 8080. This MUST track that port, or every /api call
// (login, invites, projects, …) proxies to nothing — e.g. the `fielddraft-a`
// instance offsets ports (+1) so its server is on 8081. See ../.env.example.
function serverPort(mode: string): number {
  const repoRoot = join(process.cwd(), "..");
  const env = loadEnv(mode, repoRoot, "");
  const raw = process.env.FIELDDRAFT_SERVER_PORT ?? env.FIELDDRAFT_SERVER_PORT;
  const port = Number(raw);
  return Number.isInteger(port) && port > 0 ? port : 8080;
}

// https://vitejs.dev/config/
export default defineConfig(({ mode }) => ({
  plugins: [react(), reloadGate(), liveVersion(), kernelStamp(), fileReader()],
  server: {
    port: webPort(mode),
    strictPort: true,
    allowedHosts: true,
    https: hasTailscaleCert ? { cert: certFile, key: keyFile } : undefined,
    // Forward API calls to the local Go server (docker compose / `just dev-full`)
    // so a dev running vite on :5173 gets real auth/invites/projects/tickets.
    // Tickets ALWAYS go to the backend (server ticket store: issuevault/db) —
    // the old dev-only FS sink under tickets/open/ is gone. Only the /api/file
    // middleware registers first and keeps handling its own route.
    proxy: {
      "/api": {
        target: `http://localhost:${serverPort(mode)}`,
        changeOrigin: true,
      },
    },
  },
  optimizeDeps: {
    exclude: ["./src/wasm/fielddraft_kernel.js"],
  },
  assetsInclude: ["**/*.wasm"],
}));
