import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createApiClient } from "../lib/apiClient.mjs";
import { materializeTicket, pull, moveToDone } from "../lib/ticketSync.mjs";
import { makeFakeFetch, TEST_ENV } from "./helpers.mjs";

// A fresh temp dir per test, cleaned up after.
function tmp(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fd-ticketsync-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function read(dir, ...parts) {
  return fs.readFileSync(path.join(dir, ...parts), "utf8");
}
function exists(dir, ...parts) {
  return fs.existsSync(path.join(dir, ...parts));
}

// A queue item with a full artifact set + a thread.
const FULL_ITEM = {
  id: "t-bug",
  author_id: "u42",
  title: "Extrude depth doesn't update the solid",
  body: "I edit the depth, the tree updates but the body keeps the old height.",
  state: "accepted",
  category: "bug",
  version: "0.5.81",
  created_at: "2026-06-30T10:00:00Z",
  updated_at: "2026-06-30T10:05:00Z",
  url: "https://app.test/p/abc",
  user_agent: "Mozilla/5.0 Test",
  po_spam: false,
  po_vision_fit: "fit",
  work_order: { ticket_id: "t-bug", type: "bug", summary: "fix extrude" },
  comments: [
    { author_type: "user", created_at: "2026-06-30T10:00:00Z", body: "See repro." },
    { author_type: "po", created_at: "2026-06-30T10:03:00Z", body: "Thanks, passed on." },
  ],
  artifacts: [
    { id: "a-shot", kind: "screenshot", mime: "image/png", size: 3, name: "shot.png" },
    { id: "a-fdraft", kind: "fdraft", mime: "application/json", size: 4, name: "project.fdraft" },
    { id: "a-actions", kind: "actions", mime: "application/json", size: 2, name: "actions.json" },
    { id: "a-settings", kind: "settings", mime: "application/json", size: 2, name: "settings.json" },
    { id: "a-ui", kind: "ui_state", mime: "application/json", size: 2, name: "ui_state.json" },
    { id: "a-att", kind: "attachment", mime: "text/plain", size: 5, name: "my notes.txt" },
  ],
};

// Route table that serves each artifact's bytes from its id.
const ARTIFACT_BYTES = {
  "a-shot": [0x89, 0x50, 0x4e], // PNG-ish
  "a-fdraft": [1, 2, 3, 4],
  "a-actions": [91, 93], // "[]"
  "a-settings": [123, 125], // "{}"
  "a-ui": [123, 125],
  "a-att": [104, 101, 108, 108, 111], // "hello"
};

function fakeClient(extraRoutes = {}) {
  const routes = {
    "GET /api/agent/queue": () => ({ json: { scope: "coding", items: [FULL_ITEM] } }),
    ...extraRoutes,
  };
  for (const [aid, bytes] of Object.entries(ARTIFACT_BYTES)) {
    routes[`GET /api/tickets/t-bug/artifacts/${aid}`] = () => ({ bytes });
  }
  const { fetch, calls } = makeFakeFetch(routes);
  const client = createApiClient({ env: TEST_ENV, fetch });
  return { client, calls };
}

test("fetchArtifact builds the right URL and returns raw bytes", async () => {
  const { client, calls } = fakeClient();
  const bytes = await client.fetchArtifact("t-bug", "a-shot", "coding");
  assert.ok(bytes instanceof Uint8Array);
  assert.deepEqual([...bytes], [0x89, 0x50, 0x4e]);
  const c = calls.at(-1);
  assert.equal(c.method, "GET");
  assert.equal(c.url, "https://api.test/api/tickets/t-bug/artifacts/a-shot");
  assert.equal(c.headers.Authorization, "Bearer coding-token-xyz");
});

test("fetchArtifact uses the PO token when scope=po", async () => {
  const { client, calls } = fakeClient();
  await client.fetchArtifact("t-bug", "a-shot", "po");
  assert.equal(calls.at(-1).headers.Authorization, "Bearer po-token-abc");
});

test("materializeTicket writes the §3.1 layout with canonical filenames", async (t) => {
  const dir = tmp(t);
  const { client } = fakeClient();
  const res = await materializeTicket(dir, FULL_ITEM, { client, scope: "coding" });

  const tdir = path.join(dir, "t-bug");
  assert.equal(res.dir, tdir);

  // Text files.
  assert.ok(exists(tdir, "ticket.md"));
  assert.ok(exists(tdir, "comments.md"));
  assert.ok(exists(tdir, "ticket.json"));

  // ticket.md — heading + meta + body.
  const md = read(tdir, "ticket.md");
  assert.match(md, /^# Extrude depth doesn't update the solid/);
  assert.match(md, /\| ID \| t-bug \|/);
  assert.match(md, /\| State \| accepted \|/);
  assert.match(md, /\| Version \| 0\.5\.81 \|/);
  assert.match(md, /\| URL \| https:\/\/app\.test\/p\/abc \|/);
  assert.match(md, /the body keeps the old height/);

  // comments.md — chronological thread.
  const cmts = read(tdir, "comments.md");
  assert.match(cmts, /## user · 2026-06-30T10:00:00Z/);
  assert.match(cmts, /See repro\./);
  assert.match(cmts, /## po · 2026-06-30T10:03:00Z/);
  assert.ok(cmts.indexOf("See repro") < cmts.indexOf("passed on"));

  // Canonical artifact filenames.
  assert.deepEqual(read(tdir, "screenshot.png").length, 3);
  assert.ok(exists(tdir, "project.fdraft"));
  assert.ok(exists(tdir, "actions.json"));
  assert.ok(exists(tdir, "settings.json"));
  assert.ok(exists(tdir, "ui_state.json"));
  // Attachment: numbered + sanitized original name.
  assert.ok(exists(tdir, "attachment-01-my-notes.txt"));
  assert.equal(read(tdir, "attachment-01-my-notes.txt"), "hello");

  // Artifact bytes are the served bytes.
  assert.deepEqual([...fs.readFileSync(path.join(tdir, "screenshot.png"))], [0x89, 0x50, 0x4e]);

  // ticket.json — push-context + artifact index.
  const j = JSON.parse(read(tdir, "ticket.json"));
  assert.equal(j.id, "t-bug");
  assert.equal(j.state, "accepted");
  assert.equal(j.category, "bug");
  assert.equal(j.author_id, "u42");
  assert.equal(j.po_vision_fit, "fit");
  assert.equal(j.work_order.summary, "fix extrude");
  assert.equal(j.artifacts.length, 6);
  const shot = j.artifacts.find((a) => a.kind === "screenshot");
  assert.equal(shot.file, "screenshot.png");
  const att = j.artifacts.find((a) => a.kind === "attachment");
  assert.equal(att.file, "attachment-01-my-notes.txt");
});

test("materializeTicket is idempotent and skips already-present blobs on re-pull", async (t) => {
  const dir = tmp(t);
  let downloads = 0;
  const routes = {};
  for (const [aid, bytes] of Object.entries(ARTIFACT_BYTES)) {
    routes[`GET /api/tickets/t-bug/artifacts/${aid}`] = () => {
      downloads += 1;
      return { bytes };
    };
  }
  const { fetch } = makeFakeFetch(routes);
  const client = createApiClient({ env: TEST_ENV, fetch });

  await materializeTicket(dir, FULL_ITEM, { client });
  assert.equal(downloads, 6);
  // Re-pull: text refreshes, but content-addressed blobs (same size) are skipped.
  await materializeTicket(dir, FULL_ITEM, { client });
  assert.equal(downloads, 6, "blobs already on disk are not re-downloaded");
  // Text still present/valid after refresh.
  assert.match(read(path.join(dir, "t-bug"), "ticket.md"), /# Extrude depth/);
});

test("materializeTicket({artifacts:false}) writes text+thread only, no blobs", async (t) => {
  const dir = tmp(t);
  const { client } = fakeClient();
  await materializeTicket(dir, FULL_ITEM, { client, scope: "po", artifacts: false });
  const tdir = path.join(dir, "t-bug");
  assert.ok(exists(tdir, "ticket.md"));
  assert.ok(exists(tdir, "comments.md"));
  assert.ok(exists(tdir, "ticket.json"));
  assert.ok(!exists(tdir, "screenshot.png"));
  // The index is still recorded in ticket.json (refs), just not downloaded.
  const j = JSON.parse(read(tdir, "ticket.json"));
  assert.equal(j.artifacts.length, 6);
});

test("materializeTicket falls back to first body line for the heading when no title", async (t) => {
  const dir = tmp(t);
  const { client } = fakeClient();
  const item = { id: "t2", body: "first line\nsecond line", comments: [], artifacts: [] };
  await materializeTicket(dir, item, { client });
  assert.match(read(path.join(dir, "t2"), "ticket.md"), /^# first line/);
  assert.match(read(path.join(dir, "t2"), "comments.md"), /_\(no comments\)_/);
});

test("materializeTicket throws without an id", async () => {
  await assert.rejects(() => materializeTicket("/tmp/x", {}, { client: {} }), /has no id/);
});

test("pull materializes every queue item into open/", async (t) => {
  const dir = tmp(t);
  const items = [
    { id: "t-a", title: "A", body: "a", comments: [], artifacts: [] },
    { id: "t-b", title: "B", body: "b", comments: [], artifacts: [] },
  ];
  const { fetch } = makeFakeFetch({
    "GET /api/agent/queue": () => ({ json: { scope: "coding", items } }),
  });
  const client = createApiClient({ env: TEST_ENV, fetch });

  const ids = await pull({ scope: "coding", destDir: dir, client });
  assert.deepEqual(ids, ["t-a", "t-b"]);
  assert.ok(exists(dir, "open", "t-a", "ticket.md"));
  assert.ok(exists(dir, "open", "t-b", "ticket.md"));
});

test("pull rejects a bad scope", async () => {
  await assert.rejects(() => pull({ scope: "nope", destDir: "/tmp", client: {} }), /scope must be/);
});

test("moveToDone moves open/<id> → done/<id>", async (t) => {
  const dir = tmp(t);
  const { client } = fakeClient();
  await materializeTicket(path.join(dir, "open"), FULL_ITEM, { client });
  assert.ok(exists(dir, "open", "t-bug", "ticket.md"));

  const res = moveToDone(dir, "t-bug");
  assert.equal(res.moved, true);
  assert.ok(!exists(dir, "open", "t-bug"));
  assert.ok(exists(dir, "done", "t-bug", "ticket.md"));
});

test("moveToDone is a no-op when the ticket isn't in open/", (t) => {
  const dir = tmp(t);
  const res = moveToDone(dir, "ghost");
  assert.equal(res.moved, false);
  assert.ok(!exists(dir, "done", "ghost"));
});
