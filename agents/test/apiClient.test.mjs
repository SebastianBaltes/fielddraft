import { test } from "node:test";
import assert from "node:assert/strict";
import { createApiClient, ApiError, configFromEnv } from "../lib/apiClient.mjs";
import { makeFakeFetch, TEST_ENV } from "./helpers.mjs";

function client(routes) {
  const { fetch, calls } = makeFakeFetch(routes);
  const api = createApiClient({ env: TEST_ENV, fetch });
  return { api, calls };
}

test("configFromEnv strips trailing slashes and reads tokens", () => {
  const cfg = configFromEnv({
    FIELDDRAFT_API_URL: "https://api.test///",
    PO_SERVICE_TOKEN: "po",
    CODING_SERVICE_TOKEN: "cod",
  });
  assert.equal(cfg.apiUrl, "https://api.test");
  assert.equal(cfg.poToken, "po");
  assert.equal(cfg.codingToken, "cod");
});

test("createApiClient throws without an API URL", () => {
  assert.throws(
    () => createApiClient({ env: { PO_SERVICE_TOKEN: "x" }, fetch: () => {} }),
    /FIELDDRAFT_API_URL/,
  );
});

test("getQueue('po') sends the PO Bearer token to /api/agent/queue", async () => {
  const { api, calls } = client({
    "GET /api/agent/queue": () => ({ json: { scope: "po", items: [{ id: "t1" }] } }),
  });
  const res = await api.getQueue("po");
  assert.equal(res.scope, "po");
  assert.equal(res.items[0].id, "t1");

  assert.equal(calls.length, 1);
  assert.equal(calls[0].method, "GET");
  assert.equal(calls[0].url, "https://api.test/api/agent/queue");
  assert.equal(calls[0].headers.Authorization, "Bearer po-token-abc");
});

test("getQueue('coding') uses the coding token", async () => {
  const { api, calls } = client({
    "GET /api/agent/queue": () => ({ json: { scope: "coding", items: [] } }),
  });
  await api.getQueue("coding");
  assert.equal(calls[0].headers.Authorization, "Bearer coding-token-xyz");
});

test("getQueue rejects an unknown scope", () => {
  const { api } = client({});
  assert.throws(() => api.getQueue("admin"), /scope must be/);
});

test("postVerdict POSTs the verdict JSON with the po token", async () => {
  const { api, calls } = client({
    "POST /api/tickets/t1/verdict": (req) => ({
      json: { ticket: { id: "t1" }, work_order: null },
    }),
  });
  const verdict = { spam: false, next_state: "needs_info", public_reply: "hi" };
  const res = await api.postVerdict("t1", verdict);
  assert.equal(res.ticket.id, "t1");

  const c = calls[0];
  assert.equal(c.method, "POST");
  assert.equal(c.url, "https://api.test/api/tickets/t1/verdict");
  assert.equal(c.headers.Authorization, "Bearer po-token-abc");
  assert.equal(c.headers["Content-Type"], "application/json");
  assert.deepEqual(c.body, verdict);
});

test("postAgentComment(po) uses the po token and kind=po", async () => {
  const { api, calls } = client({
    "POST /api/tickets/t1/agent-comment": () => ({ status: 201, json: { id: "c1" } }),
  });
  await api.postAgentComment("t1", { authorType: "po", body: "public reply" });
  assert.equal(calls[0].headers.Authorization, "Bearer po-token-abc");
  assert.deepEqual(calls[0].body, { kind: "po", body: "public reply" });
});

test("postAgentComment(dev) uses the coding token and kind=dev", async () => {
  const { api, calls } = client({
    "POST /api/tickets/t1/agent-comment": () => ({ status: 201, json: { id: "c2" } }),
  });
  await api.postAgentComment("t1", { authorType: "dev", body: "internal note" });
  assert.equal(calls[0].headers.Authorization, "Bearer coding-token-xyz");
  assert.deepEqual(calls[0].body, { kind: "dev", body: "internal note" });
});

test("patchState PATCHes the target state (coding scope by default)", async () => {
  const { api, calls } = client({
    "PATCH /api/tickets/t1/agent-state": () => ({ json: { id: "t1", state: "needs_info" } }),
  });
  await api.patchState("t1", "needs_info");
  assert.equal(calls[0].method, "PATCH");
  assert.equal(calls[0].headers.Authorization, "Bearer coding-token-xyz");
  assert.deepEqual(calls[0].body, { state: "needs_info" });
});

test("claim POSTs to the work-order claim route with the coding token", async () => {
  const { api, calls } = client({
    "POST /api/work-orders/t1/claim": () => ({ json: { work_order: { ticket_id: "t1" } } }),
  });
  await api.claim("t1");
  assert.equal(calls[0].url, "https://api.test/api/work-orders/t1/claim");
  assert.equal(calls[0].headers.Authorization, "Bearer coding-token-xyz");
});

test("complete maps deployId/nextState to snake_case body", async () => {
  const { api, calls } = client({
    "POST /api/work-orders/t1/complete": () => ({ json: { ticket: { id: "t1" } } }),
  });
  await api.complete("t1", { deployId: "d9", nextState: "resolved" });
  assert.deepEqual(calls[0].body, { deploy_id: "d9", next_state: "resolved" });
});

test("recordDeploy maps camelCase to the snake_case deploy body", async () => {
  const { api, calls } = client({
    "POST /api/deploys": () => ({ status: 201, json: { id: "d1" } }),
  });
  const res = await api.recordDeploy({
    version: "v1",
    gitSha: "abc",
    ticketIds: ["t1"],
    status: "live",
  });
  assert.equal(res.id, "d1");
  assert.deepEqual(calls[0].body, {
    version: "v1",
    git_sha: "abc",
    ticket_ids: ["t1"],
    status: "live",
  });
});

test("listDeploys GETs /api/deploys with the coding token", async () => {
  const { api, calls } = client({
    "GET /api/deploys": () => ({ json: [{ id: "d1" }] }),
  });
  const res = await api.listDeploys();
  assert.equal(res[0].id, "d1");
  assert.equal(calls[0].headers.Authorization, "Bearer coding-token-xyz");
});

test("ids are URL-encoded into the path", async () => {
  const { api, calls } = client({
    "POST /api/tickets/a%2Fb/verdict": () => ({ json: { ticket: { id: "a/b" } } }),
  });
  await api.postVerdict("a/b", { next_state: "needs_info" });
  assert.equal(calls[0].url, "https://api.test/api/tickets/a%2Fb/verdict");
});

test("non-2xx throws an ApiError carrying status + parsed body", async () => {
  const { api } = client({
    "GET /api/agent/queue": () => ({ status: 401, json: { error: "agent unauthorized" } }),
  });
  await assert.rejects(
    () => api.getQueue("po"),
    (err) => {
      assert.ok(err instanceof ApiError);
      assert.equal(err.status, 401);
      assert.equal(err.method, "GET");
      assert.equal(err.body.error, "agent unauthorized");
      return true;
    },
  );
});

test("a disabled scope (empty token) fails before any fetch", async () => {
  const { fetch, calls } = makeFakeFetch({});
  const api = createApiClient({
    env: { FIELDDRAFT_API_URL: "https://api.test", PO_SERVICE_TOKEN: "po" },
    fetch,
  });
  // claim() needs the coding token, which is unset → rejects, and never fetches.
  await assert.rejects(() => api.claim("t1"), /CODING_SERVICE_TOKEN is not set/);
  assert.equal(calls.length, 0);
});
