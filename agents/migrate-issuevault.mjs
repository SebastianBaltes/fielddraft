#!/usr/bin/env node
// FieldDraft → IssueVault one-shot ticket migration.
//
// Reads a checkout's `tickets/done/<id>/` folders and emits IssueVault import
// payloads, PRESERVING the old FieldDraft ticket id VERBATIM as IssueVault's
// `id` (IssueVault's tickets.id is a TEXT PK — see docs/MIGRATION.md on its
// side; the import endpoint added caller-supplied `id` in issuevault 0.2.0).
//
// Two phases (decoupled so the offline `generate` is fully testable before the
// live token / box exist):
//   generate  (default) — parse tickets → write import-<installation>.json +
//                          stage every attachment blob sha256-flat under blobs/.
//   push      (--push)  — POST each payload to the import endpoint (needs an
//                          import-capable token) and rsync the staged blobs into
//                          IssueVault's flat blob store.
//
// Attachments follow IssueVault's manifest model: the import JSON references
// blobs by sha256 (name/kind/mime/size), the bytes travel separately (rsync).
//
// Usage:
//   node migrate-issuevault.mjs [--out DIR] [--push] [DIR=INSTALLATION ...]
//   IV_URL, IV_TOKEN, IV_PROJECT, IV_BLOB_TARGET  via env (see below).
//
// Defaults (this desktop's three FieldDraft installations; prod DB has 0
// tickets so it is omitted):
//   /home/sorokan/ClaudeDraft/fielddraft   = fielddraft     (peer dev checkout)
//   /home/sorokan/ClaudeDraft/fielddraft-a = fielddraft-a   (this dev checkout)

import { readFileSync, readdirSync, existsSync, statSync, mkdirSync, writeFileSync } from "node:fs";
import { join, basename } from "node:path";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";

const REPO_URL = "https://github.com/SebastianBaltes/fielddraft";
const REPO = "SebastianBaltes/fielddraft";

// checkout dir → IssueVault installation id
const DEFAULT_INSTALLATIONS = [
  ["/home/sorokan/ClaudeDraft/fielddraft", "fielddraft"],
  ["/home/sorokan/ClaudeDraft/fielddraft-a", "fielddraft-a"],
];

// filename → { kind, mime } for IssueVault X-Kind / FileMeta
const ATTACHMENTS = {
  "screenshot.png": { kind: "screenshot", mime: "image/png" },
  "project.fdraft": { kind: "fdraft", mime: "application/zip" },
  "actions.json": { kind: "actions", mime: "application/json" },
  "settings.json": { kind: "attachment", mime: "application/json" },
  "ui_state.json": { kind: "attachment", mime: "application/json" },
};

const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");

// ── ticket.md parser (fallback when ticket.json is absent) ──────────────────
function parseTicketMd(md) {
  const lines = md.split(/\r?\n/);
  const titleLine = lines.find((l) => l.startsWith("# "));
  const title = titleLine ? titleLine.slice(2).trim() : "(untitled)";
  const meta = {};
  for (const l of lines) {
    const m = l.match(/^-\s+\*\*(\w+):\*\*\s+(.*)$/);
    if (m) meta[m[1].toLowerCase()] = m[2].trim();
  }
  const descIdx = lines.findIndex((l) => /^##\s+Description/i.test(l));
  const description = descIdx >= 0 ? lines.slice(descIdx + 1).join("\n").trim() : "";
  return {
    title,
    description,
    version: meta.version || "",
    timestamp: meta.time || "",
    userAgent: meta.ua || "",
    url: meta.url || "",
    status: "resolved",
  };
}

// ── read a done-ticket dir into an IssueVault import-ticket record ───────────
function parseTicket(dir) {
  const id = basename(dir);
  let t;
  if (existsSync(join(dir, "ticket.json"))) {
    t = JSON.parse(readFileSync(join(dir, "ticket.json"), "utf8"));
  } else if (existsSync(join(dir, "ticket.md"))) {
    t = parseTicketMd(readFileSync(join(dir, "ticket.md"), "utf8"));
  } else if (existsSync(join(dir, "info.md"))) {
    // Hand-authored design-note "ticket" (no Report-Issue metadata). Parse the
    // markdown as best-effort: first heading = title, whole file = body.
    const info = readFileSync(join(dir, "info.md"), "utf8");
    const head = info.split(/\r?\n/).find((l) => l.startsWith("# "));
    t = { title: head ? head.slice(2).trim() : id, description: info.trim(), status: "resolved" };
  } else {
    throw new Error(`no ticket.json/ticket.md/info.md in ${dir}`);
  }

  const createdAt = t.timestamp || new Date(statSync(dir).mtime).toISOString();

  // Fold the client context the IssueVault schema has no dedicated field for
  // (version / URL / UA) into the markdown body so nothing is lost.
  const ctx = [];
  if (t.version) ctx.push(`Version: ${t.version}`);
  if (t.url) ctx.push(`URL: ${t.url}`);
  if (t.userAgent) ctx.push(`UA: ${t.userAgent}`);
  const body = (t.description || "").trim() + (ctx.length ? `\n\n---\n_${ctx.join(" · ")}_` : "");

  // resolution.md → one agent comment + linked commit(s)
  const comments = [];
  const commits = [];
  let closedAt = createdAt;
  const resPath = join(dir, "resolution.md");
  if (existsSync(resPath)) {
    const res = readFileSync(resPath, "utf8");
    closedAt = new Date(statSync(resPath).mtime).toISOString();
    comments.push({ author: "coding-agent", author_kind: "agent", body: res.trim(), created_at: closedAt });
    const cm = res.match(/\*\*Commit:\*\*\s*([0-9a-f]{7,40})/i);
    if (cm) commits.push({ repo: REPO, sha: cm[1], url: `${REPO_URL}/commit/${cm[1]}` });
  }

  // attachments → sha256 manifest + raw bytes (for staging/rsync)
  const files = [];
  const blobs = []; // { sha256, buf }
  for (const [name, meta] of Object.entries(ATTACHMENTS)) {
    const p = join(dir, name);
    if (!existsSync(p)) continue;
    const buf = readFileSync(p);
    const h = sha256(buf);
    files.push({ sha256: h, name, kind: meta.kind, mime: meta.mime, size: buf.length, created_at: createdAt });
    blobs.push({ sha256: h, buf });
  }

  const ticket = {
    id, // VERBATIM old FieldDraft ticket id → IssueVault id (TEXT PK)
    title: t.title || "(untitled)",
    body,
    state: "closed",
    status: t.status || "resolved",
    category: "",
    author: "user",
    assignee: "",
    created_at: createdAt,
    updated_at: closedAt,
    closed_at: closedAt,
    comments,
    commits,
    files,
  };
  return { ticket, blobs };
}

function loadInstallation(checkoutDir) {
  const doneDir = join(checkoutDir, "tickets", "done");
  if (!existsSync(doneDir)) return { tickets: [], blobs: new Map() };
  const dirs = readdirSync(doneDir).map((d) => join(doneDir, d)).filter((d) => statSync(d).isDirectory());
  const tickets = [];
  const blobs = new Map(); // sha256 → buf (deduped across tickets)
  const skipped = [];
  for (const d of dirs) {
    try {
      const { ticket, blobs: tb } = parseTicket(d);
      tickets.push(ticket);
      for (const b of tb) if (!blobs.has(b.sha256)) blobs.set(b.sha256, b.buf);
    } catch (e) {
      skipped.push(`${basename(d)} (${e.message})`);
    }
  }
  return { tickets, blobs, skipped };
}

// ── CLI ─────────────────────────────────────────────────────────────────────
const args = process.argv.slice(2);
const push = args.includes("--push");
const outIdx = args.indexOf("--out");
const outDir = outIdx >= 0 ? args[outIdx + 1] : "/tmp/iv-migration";
const pairs = args
  .filter((a) => a.includes("=") && !a.startsWith("--"))
  .map((a) => a.split("="));
const installations = pairs.length ? pairs : DEFAULT_INSTALLATIONS;

const IV_URL = process.env.IV_URL || "https://issuevault.203.0.113.10.sslip.io/api/v1";
const IV_TOKEN = process.env.IV_TOKEN || "";
const IV_PROJECT = process.env.IV_PROJECT || "fielddraft";
const IV_BLOB_TARGET = process.env.IV_BLOB_TARGET || "root@203.0.113.10:/data/issuevault/blobs/";
// IssueVault's blob dir is owned by the container user (uid 10001); rsync runs
// as root on the box, so force ownership or the container can't read the blobs.
const IV_BLOB_OWNER = process.env.IV_BLOB_OWNER || "10001:10001";

mkdirSync(join(outDir, "blobs"), { recursive: true });

let grandTickets = 0;
const allBlobs = new Map();
const payloads = [];

for (const [checkoutDir, installationId] of installations) {
  const { tickets, blobs, skipped } = loadInstallation(checkoutDir);
  const payload = { installation_id: installationId, tickets };
  const file = join(outDir, `import-${installationId}.json`);
  writeFileSync(file, JSON.stringify(payload, null, 2));
  payloads.push({ installationId, file, payload });
  for (const [h, buf] of blobs) if (!allBlobs.has(h)) allBlobs.set(h, buf);
  grandTickets += tickets.length;
  const withFiles = tickets.reduce((n, t) => n + t.files.length, 0);
  console.log(`✔ ${installationId}: ${tickets.length} tickets, ${blobs.size} unique blobs, ${withFiles} file refs → ${file}`);
  if (skipped.length) console.log(`  ⚠ skipped ${skipped.length}: ${skipped.join("; ")}`);
}

// stage unique blobs sha256-flat for rsync
for (const [h, buf] of allBlobs) writeFileSync(join(outDir, "blobs", h), buf);
console.log(`✔ staged ${allBlobs.size} unique blobs → ${join(outDir, "blobs")}`);
console.log(`Σ ${grandTickets} tickets across ${installations.length} installations`);

if (!push) {
  console.log(`\n[generate only] to push: IV_TOKEN=… node ${basename(process.argv[1])} --push`);
  console.log(`  (needs an import-capable token; box must run issuevault ≥ 0.2.0)`);
  process.exit(0);
}

// ── push phase ──────────────────────────────────────────────────────────────
if (!IV_TOKEN) {
  console.error("✗ --push needs IV_TOKEN (import-capable). Aborting.");
  process.exit(1);
}
console.log(`\n▶ rsync ${allBlobs.size} blobs → ${IV_BLOB_TARGET} (chown ${IV_BLOB_OWNER})`);
execFileSync("rsync", ["-a", `--chown=${IV_BLOB_OWNER}`, join(outDir, "blobs") + "/", IV_BLOB_TARGET], { stdio: "inherit" });

for (const { installationId, payload } of payloads) {
  const res = await fetch(`${IV_URL}/projects/${IV_PROJECT}/import/tickets`, {
    method: "POST",
    headers: { authorization: `Bearer ${IV_TOKEN}`, "content-type": "application/json" },
    body: JSON.stringify(payload),
  });
  const txt = await res.text();
  if (!res.ok) {
    console.error(`✗ ${installationId}: ${res.status} ${txt}`);
    continue;
  }
  const out = JSON.parse(txt);
  console.log(`✔ ${installationId}: imported ${out.imported ?? out.ids?.length ?? "?"} tickets`);
}
