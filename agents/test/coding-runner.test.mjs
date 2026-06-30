import { test } from "node:test";
import assert from "node:assert/strict";
import { createApiClient } from "../lib/apiClient.mjs";
import { runCodingOnce, runCodingLoop } from "../fielddraft-coding/coding-runner.mjs";
import { makeFakeFetch, TEST_ENV } from "./helpers.mjs";

// A fixture that records the server-side effects of the coding workflow.
function codingFixture(queueItems) {
  const effects = {
    claimed: [],
    deploys: [],
    completed: [],
    comments: [],
    states: [],
  };
  const { fetch, calls } = makeFakeFetch({
    "GET /api/agent/queue": () => ({ json: { scope: "coding", items: queueItems } }),
    "POST /api/work-orders/t1/claim": () => {
      effects.claimed.push("t1");
      return { json: { work_order: { ticket_id: "t1" }, ticket: { id: "t1", state: "in_progress" } } };
    },
    "POST /api/deploys": (req) => {
      effects.deploys.push(req.body);
      return { status: 201, json: { id: "deploy-1", ...req.body } };
    },
    "POST /api/work-orders/t1/complete": (req) => {
      effects.completed.push(req.body);
      return { json: { ticket: { id: "t1", state: "resolved" } } };
    },
    "POST /api/tickets/t1/agent-comment": (req) => {
      effects.comments.push(req.body);
      return { status: 201, json: { id: "c1", ...req.body } };
    },
    "PATCH /api/tickets/t1/agent-state": (req) => {
      effects.states.push(req.body.state);
      return { json: { id: "t1", state: req.body.state } };
    },
  });
  const client = createApiClient({ env: TEST_ENV, fetch });
  return { client, effects, calls };
}

const ITEM = {
  id: "t1",
  state: "accepted",
  work_order: {
    ticket_id: "t1",
    type: "bug",
    summary: "Extrude depth edit does not re-evaluate the solid",
    acceptance_criteria: ["depth edit updates the body"],
    vision_notes: "core T1",
    forwarded_artifacts: ["project.fdraft", "actions.json"],
    untrusted_user_text: "depth change doesn't update the solid",
  },
};

test("success path: claim → implement → recordDeploy → deploying → complete", async () => {
  const { client, effects } = codingFixture([ITEM]);

  // The fake `implement` stands in for the ClaudeDraft coding agent. It returns
  // a deploy descriptor on success.
  const implement = (workOrder, item) => {
    // Sanity: the runner hands the trusted task + untrusted data through.
    assert.equal(workOrder.summary, ITEM.work_order.summary);
    assert.equal(item.id, "t1");
    return {
      deploy: { version: "v0.6.340", gitSha: "abc123", ticketIds: ["t1"], status: "live" },
      nextState: "resolved",
    };
  };

  const results = await runCodingOnce({ client, implement });
  assert.equal(results[0].ok, true);
  assert.equal(results[0].deployId, "deploy-1");

  assert.deepEqual(effects.claimed, ["t1"]);
  assert.equal(effects.deploys.length, 1);
  assert.equal(effects.deploys[0].version, "v0.6.340");
  assert.deepEqual(effects.deploys[0].ticket_ids, ["t1"]);
  assert.equal(effects.completed.length, 1);
  assert.equal(effects.completed[0].deploy_id, "deploy-1");
  assert.equal(effects.completed[0].next_state, "resolved");
  // No failure comment on the happy path; exactly one state hop: → deploying
  // (the state machine requires in_progress → deploying → resolved, §7.2).
  assert.equal(effects.comments.length, 0);
  assert.deepEqual(effects.states, ["deploying"]);
});

test("deploy ticketIds defaults to the ticket when omitted", async () => {
  const { client, effects } = codingFixture([ITEM]);
  const implement = () => ({ deploy: { version: "v1" } }); // minimal
  await runCodingOnce({ client, implement });
  assert.deepEqual(effects.deploys[0].ticket_ids, ["t1"]);
  assert.equal(effects.deploys[0].status, "live");
});

test("failure path: implement throws → dev comment + needs_info, no deploy", async () => {
  const { client, effects } = codingFixture([ITEM]);

  const implement = () => {
    throw new Error("just test red: extrude_depth_updates_solid failed");
  };

  const results = await runCodingOnce({ client, implement });
  assert.equal(results[0].ok, false);
  assert.match(results[0].error.message, /just test red/);

  // It claimed first…
  assert.deepEqual(effects.claimed, ["t1"]);
  // …but never recorded a deploy or completed.
  assert.equal(effects.deploys.length, 0);
  assert.equal(effects.completed.length, 0);
  // It posted an internal dev note…
  assert.equal(effects.comments.length, 1);
  assert.equal(effects.comments[0].kind, "dev");
  assert.match(effects.comments[0].body, /just test red/);
  // …and moved the ticket to needs_info (PO relays to the user).
  assert.deepEqual(effects.states, ["needs_info"]);
});

test("rejects an implement result without a deploy.version", async () => {
  const { client, effects } = codingFixture([ITEM]);
  const implement = () => ({ deploy: {} }); // missing version
  const results = await runCodingOnce({ client, implement });
  assert.equal(results[0].ok, false);
  assert.match(results[0].error.message, /deploy.*version/);
  // Claimed, then failed → dev note + needs_info, no deploy recorded.
  assert.equal(effects.deploys.length, 0);
  assert.equal(effects.comments[0].kind, "dev");
  assert.deepEqual(effects.states, ["needs_info"]);
});

test("an empty coding queue does nothing", async () => {
  const { client, effects } = codingFixture([]);
  const results = await runCodingOnce({ client, implement: () => ({ deploy: { version: "v1" } }) });
  assert.equal(results.length, 0);
  assert.equal(effects.claimed.length, 0);
});

test("runCodingLoop runs an iteration then stops on abort", async () => {
  const { client, effects } = codingFixture([ITEM]);
  const implement = () => ({ deploy: { version: "v1" } });

  const ac = new AbortController();
  const sleep = async () => ac.abort(); // one iteration then stop
  await runCodingLoop({ client, implement, signal: ac.signal, sleep });

  assert.equal(effects.deploys.length, 1);
  assert.ok(ac.signal.aborted);
});
