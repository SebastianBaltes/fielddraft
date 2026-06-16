import { defineConfig, loadEnv, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { existsSync, readFileSync, mkdirSync, writeFileSync } from "node:fs";
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
        return { define: { __KERNEL_EXPECTED__: JSON.stringify(readKernelStamp()) } };
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
      if (!id.includes("node_modules") && code.includes("__KERNEL_EXPECTED__")) {
        stampModules.add(id);
        return code.replaceAll("__KERNEL_EXPECTED__", JSON.stringify(readKernelStamp()));
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
              projectJson?: string;
              screenshot?: string | null;
              attachments?: { name?: string; mime?: string; dataUrl?: string }[];
              version?: string;
              timestamp?: string;
              userAgent?: string;
              url?: string;
              settings?: Record<string, unknown>;
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
              if (typeof a?.dataUrl !== "string" || !a.dataUrl.startsWith("data:")) return;
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
                  : "") +
                // Per-device settings (renderMode, controlsPreset, …) as a JSON
                // block so a report carries the environment (helps reproduce
                // renderer-/controls-dependent bugs).
                (t.settings
                  ? `\n## Settings\n\n\`\`\`json\n${JSON.stringify(t.settings, null, 2)}\n\`\`\`\n`
                  : ""),
            );

            if (t.projectJson) {
              writeFileSync(join(dir, "project.fdraft"), t.projectJson);
            }

            if (typeof t.screenshot === "string" && t.screenshot.startsWith("data:image/")) {
              const b64 = t.screenshot.slice(t.screenshot.indexOf(",") + 1);
              writeFileSync(join(dir, "screenshot.png"), Buffer.from(b64, "base64"));
            }

            server.config.logger.info(`ticket saved: tickets/open/${id} — ${heading}`);
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
  plugins: [react(), liveVersion(), kernelStamp(), ticketWriter()],
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
