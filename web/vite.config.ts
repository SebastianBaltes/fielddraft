import { defineConfig, loadEnv, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { existsSync, readFileSync, mkdirSync, writeFileSync } from "node:fs";
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

// Dev-only ticket sink. The in-app "Report Issue" dialog POSTs to
// `/api/tickets`; this middleware drops each ticket into
// `<repo>/tickets/open/<id>/` (sibling of web/) so the agent can pick it up
// during local development. In production the Go backend serves the same
// route. No-op outside `vite` dev (the build has no server).
function ticketWriter(): Plugin {
  const ticketsRoot = join(process.cwd(), "..", "tickets", "open");

  function pad(n: number): string {
    return String(n).padStart(2, "0");
  }

  function ticketId(): string {
    const d = new Date();
    const stamp =
      `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}` +
      `-${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`;
    const rand = Math.random().toString(36).slice(2, 6);
    return `${stamp}-${rand}`;
  }

  // User-supplied attachment names end up as filenames on disk — keep only
  // the basename and a safe character set, and namespace with an index so
  // two attachments can never collide or overwrite ticket files.
  function attachmentFilename(index: number, name: string): string {
    const base = name.split(/[\\/]/).pop() ?? "";
    const safe = base.replace(/[^\w.\- ]+/g, "_").slice(-80) || "file";
    return `attachment-${String(index + 1).padStart(2, "0")}-${safe}`;
  }

  // ticket xqll: the ticket.md `# ` heading is derived from the description's
  // first non-empty line (there is no user title anymore), falling back to a
  // generic "Issue <id>" for an empty description.
  function ticketHeading(id: string, description: string): string {
    for (const line of description.split("\n")) {
      const t = line.trim();
      if (t) return t.length > 80 ? `${t.slice(0, 80).trim()}…` : t;
    }
    return `Issue ${id}`;
  }

  return {
    name: "ticket-writer",
    configureServer(server) {
      server.middlewares.use("/api/tickets", (req, res) => {
        if (req.method !== "POST") {
          res.statusCode = 405;
          res.end();
          return;
        }
        let raw = "";
        req.on("data", (chunk) => {
          raw += chunk;
          // Hard cap to avoid an unbounded buffer (screenshots are ~1 MB).
          if (raw.length > 25 * 1024 * 1024) req.destroy();
        });
        req.on("end", () => {
          try {
            const t = JSON.parse(raw) as {
              description?: string;
              projectFdraft?: string;
              screenshot?: string | null;
              attachments?: {
                name?: string;
                mime?: string;
                dataUrl?: string;
              }[];
              version?: string;
              timestamp?: string;
              userAgent?: string;
              url?: string;
              settings?: Record<string, unknown>;
              uiState?: Record<string, unknown>;
              actionLog?: Record<string, unknown>;
            };
            const id = ticketId();
            const dir = join(ticketsRoot, id);
            mkdirSync(dir, { recursive: true });

            // ticket xqll: no ticket.json and no title — ticket.md (below)
            // carries all metadata plus the description and attachments.
            const description = t.description ?? "";
            const version = t.version ?? "";
            const timestamp = t.timestamp ?? new Date().toISOString();
            const userAgent = t.userAgent ?? "";
            const url = t.url ?? "";

            // User attachments (ticket 8onw): each data URL becomes a file
            // next to the screenshot, listed in ticket.md for the agent.
            const attachmentNames: string[] = [];
            (t.attachments ?? []).forEach((a, i) => {
              if (
                typeof a?.dataUrl !== "string" ||
                !a.dataUrl.startsWith("data:")
              )
                return;
              const fname = attachmentFilename(i, a.name ?? "");
              const b64 = a.dataUrl.slice(a.dataUrl.indexOf(",") + 1);
              writeFileSync(join(dir, fname), Buffer.from(b64, "base64"));
              attachmentNames.push(fname);
            });

            // Human-readable ticket — the single source of truth now that
            // ticket.json is gone (ticket xqll). Heading derived from the
            // description's first line.
            const heading = ticketHeading(id, description);
            writeFileSync(
              join(dir, "ticket.md"),
              `# ${heading}\n\n` +
                `- **ID:** ${id}\n` +
                `- **Version:** ${version}\n` +
                `- **Time:** ${timestamp}\n` +
                `- **URL:** ${url}\n` +
                `- **UA:** ${userAgent}\n\n` +
                `## Description\n\n${description || "_(none)_"}\n` +
                (attachmentNames.length > 0
                  ? `\n## Attachments\n\n${attachmentNames.map((n) => `- ${n}`).join("\n")}\n`
                  : ""),
            );

            // ticket 478k: per-device settings (renderMode, controlsPreset, …)
            // live in a separate settings.json, NOT inline in the markdown.
            if (t.settings) {
              writeFileSync(
                join(dir, "settings.json"),
                JSON.stringify(t.settings, null, 2),
              );
            }

            // ticket uz5q: ephemeral live UI state (camera, selection, active
            // tool/mode) in a separate ui_state.json so a report reproduces
            // the exact view, not just the document.
            if (t.uiState) {
              writeFileSync(
                join(dir, "ui_state.json"),
                JSON.stringify(t.uiState, null, 2),
              );
            }

            // Replayable user-action trace (semantic actions + raw input) in a
            // separate actions.json so a report carries the steps that led to
            // the bug, not just the end state.
            if (t.actionLog) {
              writeFileSync(
                join(dir, "actions.json"),
                JSON.stringify(t.actionLog, null, 2),
              );
            }

            // on3k: the project rides along as a base64 .fdraft ZIP — decode
            // and write the real binary file (document JSON + font assets).
            if (t.projectFdraft) {
              writeFileSync(
                join(dir, "project.fdraft"),
                Buffer.from(t.projectFdraft, "base64"),
              );
            }

            if (
              typeof t.screenshot === "string" &&
              t.screenshot.startsWith("data:image/")
            ) {
              const b64 = t.screenshot.slice(t.screenshot.indexOf(",") + 1);
              writeFileSync(
                join(dir, "screenshot.png"),
                Buffer.from(b64, "base64"),
              );
            }

            server.config.logger.info(
              `ticket saved: tickets/open/${id} — ${heading}`,
            );
            res.statusCode = 200;
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify({ ticket_id: id }));
          } catch (err) {
            server.config.logger.error(`ticket write failed: ${err}`);
            res.statusCode = 400;
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify({ error: "invalid ticket" }));
          }
        });
      });
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

// https://vitejs.dev/config/
export default defineConfig(({ mode }) => ({
  plugins: [react(), liveVersion(), kernelStamp(), ticketWriter(), fileReader()],
  server: {
    port: webPort(mode),
    strictPort: true,
    allowedHosts: true,
    https: hasTailscaleCert ? { cert: certFile, key: keyFile } : undefined,
  },
  optimizeDeps: {
    exclude: ["./src/wasm/fielddraft_kernel.js"],
  },
  assetsInclude: ["**/*.wasm"],
}));
