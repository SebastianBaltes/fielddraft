// Ticket-sync layer — "snapshots down, commands up" (docs/TICKET-SYNC-AND-AGENTS.md).
//
// The DB is the shared, transactional truth; FILES are the agent's workspace.
// This module is the READ side: it MATERIALIZES a queue item (ticket text +
// thread + content-addressed artifact blobs) into the §3.1 on-disk layout so the
// agent can `rg`/`cat`/load `project.fdraft`/replay `actions.json` exactly like a
// local file-ticket. The mutable side (claim/verdict/comment/state/complete)
// stays as typed API operations in apiClient.mjs — never a file merge.
//
// Design:
//   - Zero deps: only node:fs / node:path.
//   - Artifacts are content-addressed & immutable → re-pull is a pure refresh;
//     an already-present blob with the same size/name is skipped.
//   - Text + thread are a fresh snapshot: they OVERWRITE the local read-view
//     (the local view is never the truth).
//   - The apiClient is injected (`{ client }`) so tests drive it without a
//     network, matching the runner/apiClient injectable-fetch pattern.

import fs from "node:fs";
import path from "node:path";

// Canonical on-disk filename per artifact kind (§3.1). `attachment` is handled
// separately (numbered + original name), so it is intentionally absent here.
const ARTIFACT_FILENAME = {
  screenshot: "screenshot.png",
  fdraft: "project.fdraft",
  actions: "actions.json",
  settings: "settings.json",
  ui_state: "ui_state.json",
};

/**
 * Materialize ONE queue item into `destDir/<id>/` (the §3.1 layout).
 *
 * Writes:
 *   - ticket.md    — heading + meta block + body,
 *   - comments.md  — the thread (chronological, one section per comment),
 *   - ticket.json  — state/category/verdict/author + the artifact index (push-context),
 *   - <kind file>  — each artifact blob, downloaded via client.fetchArtifact.
 *
 * Idempotent: text/thread/json are rewritten every call (fresh snapshot);
 * content-addressed artifact blobs already on disk with the same size are
 * skipped (immutable → re-pull is a refresh, not a re-download).
 *
 * @param {string} destDir  parent dir; the ticket lands in `destDir/<id>/`.
 * @param {object} item     a queue item ({...ticket fields, comments, artifacts}).
 * @param {object} deps
 * @param {{fetchArtifact:(id:string,aid:string,scope?:string)=>Promise<Uint8Array>}} deps.client
 * @param {"po"|"coding"} [deps.scope="coding"]  token scope for artifact download.
 * @param {boolean} [deps.artifacts=true]  set false to skip blob download (text-only).
 * @returns {Promise<{dir:string, files:string[], artifacts:string[]}>}
 */
export async function materializeTicket(destDir, item, { client, scope = "coding", artifacts = true } = {}) {
  const id = item?.id;
  if (!id) throw new Error("materializeTicket: item has no id");
  if (artifacts && !client?.fetchArtifact) {
    throw new Error("materializeTicket: { client } with fetchArtifact is required to pull artifacts");
  }

  const dir = path.join(destDir, String(id));
  fs.mkdirSync(dir, { recursive: true });

  const comments = Array.isArray(item.comments) ? item.comments : [];
  const arts = Array.isArray(item.artifacts) ? item.artifacts : [];

  // ── ticket.md — heading + meta + body (a fresh snapshot) ──
  fs.writeFileSync(path.join(dir, "ticket.md"), renderTicketMd(item), "utf8");

  // ── comments.md — the thread ──
  fs.writeFileSync(path.join(dir, "comments.md"), renderCommentsMd(item, comments), "utf8");

  // ── artifact blobs (immutable, content-addressed) ──
  const written = [];
  let attachmentN = 0;
  const index = [];
  for (const a of arts) {
    const filename = artifactFilename(a, () => ++attachmentN);
    index.push({ id: a.id, kind: a.kind, mime: a.mime, size: a.size, name: a.name, file: filename });
    if (!artifacts) continue;
    const target = path.join(dir, filename);
    if (isAlreadyMaterialized(target, a.size)) {
      written.push(filename);
      continue;
    }
    const bytes = await client.fetchArtifact(id, a.id, scope);
    fs.writeFileSync(target, bytes);
    written.push(filename);
  }

  // ── ticket.json — the push-context (state/category/verdict/author + index) ──
  fs.writeFileSync(path.join(dir, "ticket.json"), JSON.stringify(ticketJson(item, index), null, 2) + "\n", "utf8");

  const files = ["ticket.md", "comments.md", "ticket.json", ...written];
  return { dir, files, artifacts: written };
}

/**
 * Pull a whole scope's queue and materialize every item into `destDir/open/`.
 * @param {object} deps
 * @param {"po"|"coding"} deps.scope
 * @param {string} deps.destDir  base tickets dir (open/ is created under it).
 * @param {object} deps.client   apiClient (getQueue + fetchArtifact).
 * @param {boolean} [deps.artifacts=true]  false → text+thread only (PO triage).
 * @returns {Promise<string[]>}  the materialized ticket ids.
 */
export async function pull({ scope, destDir, client, artifacts = true }) {
  if (scope !== "po" && scope !== "coding") {
    throw new Error(`pull: scope must be "po" or "coding", got ${scope}`);
  }
  const queue = await client.getQueue(scope);
  const items = queue?.items || [];
  const openDir = path.join(destDir, "open");
  const ids = [];
  for (const item of items) {
    await materializeTicket(openDir, item, { client, scope, artifacts });
    ids.push(item.id);
  }
  return ids;
}

/**
 * Move a materialized ticket from `destDir/open/<id>` to `destDir/done/<id>`
 * (after the work-order completes). Overwrites any stale done/ copy.
 * @param {string} destDir  base tickets dir.
 * @param {string} id
 * @returns {{from:string, to:string, moved:boolean}}
 */
export function moveToDone(destDir, id) {
  const from = path.join(destDir, "open", String(id));
  const to = path.join(destDir, "done", String(id));
  if (!fs.existsSync(from)) {
    return { from, to, moved: false };
  }
  fs.mkdirSync(path.join(destDir, "done"), { recursive: true });
  fs.rmSync(to, { recursive: true, force: true }); // replace any stale copy
  fs.renameSync(from, to);
  return { from, to, moved: true };
}

// ─── rendering ──────────────────────────────────────────────────

/** The canonical filename for an artifact ref. `attachment` gets numbered. */
function artifactFilename(a, nextN) {
  const known = ARTIFACT_FILENAME[a.kind];
  if (known) return known;
  if (a.kind === "attachment") {
    const n = String(nextN()).padStart(2, "0");
    return `attachment-${n}-${sanitizeName(a.name) || "file"}`;
  }
  // Unknown kind: keep it, sanitized, so nothing silently vanishes.
  return `artifact-${sanitizeName(a.id || a.kind || "blob")}`;
}

/** ticket.md: heading (title or first body line) + meta block + body. */
function renderTicketMd(item) {
  const title = (item.title && item.title.trim()) || firstLine(item.body) || `Ticket ${item.id}`;
  const meta = [
    ["ID", item.id],
    ["State", item.state],
    ["Category", item.category],
    ["Version", pick(item, "version", "app_version")],
    ["Time", item.created_at],
    ["URL", pick(item, "url", "page_url")],
    ["UA", pick(item, "user_agent", "ua")],
    ["Author", item.author_id],
  ].filter(([, v]) => v !== undefined && v !== null && v !== "");

  const lines = [`# ${title}`, ""];
  if (meta.length) {
    lines.push("| Field | Value |", "| --- | --- |");
    for (const [k, v] of meta) lines.push(`| ${k} | ${escapeCell(String(v))} |`);
    lines.push("");
  }
  lines.push((item.body || "").trimEnd(), "");
  return lines.join("\n");
}

/** comments.md: the thread, chronological, one section per comment. */
function renderCommentsMd(item, comments) {
  const lines = [`# Thread — ${item.id}`, ""];
  if (!comments.length) {
    lines.push("_(no comments)_", "");
    return lines.join("\n");
  }
  for (const c of comments) {
    const when = c.created_at ? ` · ${c.created_at}` : "";
    lines.push(`## ${c.author_type || "unknown"}${when}`, "", (c.body || "").trimEnd(), "");
  }
  return lines.join("\n");
}

/** ticket.json: the machine-readable push-context. */
function ticketJson(item, artifactIndex) {
  return {
    id: item.id,
    state: item.state,
    category: item.category ?? null,
    author_id: item.author_id ?? null,
    po_spam: item.po_spam ?? null,
    po_malicious: item.po_malicious ?? null,
    po_vision_fit: item.po_vision_fit ?? null,
    po_reason: item.po_reason ?? null,
    po_verdict_at: item.po_verdict_at ?? null,
    created_at: item.created_at ?? null,
    updated_at: item.updated_at ?? null,
    work_order: item.work_order ?? null,
    artifacts: artifactIndex,
  };
}

// ─── small helpers ──────────────────────────────────────────────

/** True if a blob is already on disk with the expected size (immutable refresh). */
function isAlreadyMaterialized(target, expectedSize) {
  try {
    const st = fs.statSync(target);
    // size unknown (0/undefined) → can't trust it; re-download to be safe.
    if (!expectedSize) return false;
    return st.size === Number(expectedSize);
  } catch {
    return false; // not present
  }
}

/** First non-empty line of a string (for a heading fallback). */
function firstLine(s) {
  if (!s) return "";
  for (const line of String(s).split(/\r?\n/)) {
    if (line.trim()) return line.trim();
  }
  return "";
}

/** Read the first present key from an object (aliases for optional meta). */
function pick(obj, ...keys) {
  for (const k of keys) {
    if (obj[k] !== undefined && obj[k] !== null && obj[k] !== "") return obj[k];
  }
  return undefined;
}

/** Keep a filename safe: strip path separators + control chars, collapse spaces. */
function sanitizeName(name) {
  return String(name || "")
    .replace(/[/\\]+/g, "-")
    .replace(/[\x00-\x1f]+/g, "")
    .replace(/[^\w.\-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 120);
}

/** Escape a value for a single Markdown table cell. */
function escapeCell(s) {
  return s.replace(/\r?\n/g, " ").replace(/\|/g, "\\|");
}

export default { materializeTicket, pull, moveToDone };
