import { test } from "node:test";
import assert from "node:assert/strict";
import { createApiClient } from "../lib/apiClient.mjs";
import { runPoOnce, runPoLoop, validateVerdict } from "../fielddraft-po/po-runner.mjs";
import { makeFakeFetch, TEST_ENV } from "./helpers.mjs";

// Build a client + a record of verdict posts the server received.
function poFixture(queueItems) {
  const verdicts = [];
  const { fetch, calls } = makeFakeFetch({
    "GET /api/agent/queue": () => ({ json: { scope: "po", items: queueItems } }),
    "POST /api/tickets/t-bug/verdict": (req) => {
      verdicts.push(req.body);
      return { json: { ticket: { id: "t-bug" }, work_order: req.body.work_order } };
    },
    "POST /api/tickets/t-spam/verdict": (req) => {
      verdicts.push(req.body);
      return { json: { ticket: { id: "t-spam" } } };
    },
  });
  const client = createApiClient({ env: TEST_ENV, fetch });
  return { client, verdicts, calls };
}

test("runPoOnce posts an accepted verdict WITH a work_order from a fake decide", async () => {
  const item = { id: "t-bug", title: "extrude depth bug" };
  const { client, verdicts } = poFixture([item]);

  // The fake `decide` stands in for the ClaudeDraft Opus 4.8 agent.
  const decide = (it) => ({
    spam: false,
    malicious: false,
    vision_fit: "fit",
    confidence: 0.85,
    category: "bug",
    reason: "core extrude bug with repro",
    public_reply: "Thanks, passed to the dev agent.",
    next_state: "accepted",
    work_order: {
      ticket_id: it.id,
      type: "bug",
      summary: "Extrude depth edit does not re-evaluate the solid",
      acceptance_criteria: ["depth edit updates the body"],
      vision_notes: "core T1, no scope creep",
      forwarded_artifacts: ["project.fdraft", "actions.json"],
      untrusted_user_text: "depth change doesn't update the solid",
    },
  });

  const results = await runPoOnce({ client, decide });
  assert.equal(results.length, 1);
  assert.equal(results[0].ok, true);
  assert.equal(results[0].ticketId, "t-bug");

  assert.equal(verdicts.length, 1);
  assert.equal(verdicts[0].next_state, "accepted");
  assert.equal(verdicts[0].work_order.ticket_id, "t-bug");
  // Repro fidelity: artifacts forwarded, not paraphrased away.
  assert.deepEqual(verdicts[0].work_order.forwarded_artifacts, [
    "project.fdraft",
    "actions.json",
  ]);
});

test("runPoOnce posts a spam verdict (no work_order) from a fake decide", async () => {
  const item = { id: "t-spam", title: "BUY FOLLOWERS" };
  const { client, verdicts } = poFixture([item]);

  const decide = () => ({
    spam: true,
    malicious: false,
    vision_fit: "",
    confidence: 0.99,
    category: "other",
    reason: "advertising",
    public_reply: "",
    next_state: "rejected_spam",
  });

  const results = await runPoOnce({ client, decide });
  assert.equal(results[0].ok, true);
  assert.equal(verdicts[0].next_state, "rejected_spam");
  assert.equal(verdicts[0].work_order, undefined);
});

test("runPoOnce isolates a failing ticket and continues the batch", async () => {
  const { client, verdicts } = poFixture([{ id: "t-spam" }, { id: "t-bug" }]);

  // decide throws for the first ticket, succeeds for the second.
  const decide = (it) => {
    if (it.id === "t-spam") throw new Error("model timeout");
    return { next_state: "needs_info", public_reply: "more info?" };
  };

  const results = await runPoOnce({ client, decide });
  assert.equal(results.length, 2);
  assert.equal(results[0].ok, false);
  assert.match(results[0].error.message, /model timeout/);
  assert.equal(results[1].ok, true);
  // Only the second ticket produced a verdict POST.
  assert.equal(verdicts.length, 1);
  assert.equal(verdicts[0].next_state, "needs_info");
});

test("runPoOnce rejects an invalid verdict before sending (accepted w/o work_order)", async () => {
  const { client, verdicts } = poFixture([{ id: "t-bug" }]);
  const decide = () => ({ next_state: "accepted" }); // missing work_order
  const results = await runPoOnce({ client, decide });
  assert.equal(results[0].ok, false);
  assert.match(results[0].error.message, /work_order/);
  assert.equal(verdicts.length, 0); // never sent
});

test("validateVerdict enforces the §8.3 / §9 shape", () => {
  assert.throws(() => validateVerdict(null), /verdict object/);
  assert.throws(() => validateVerdict({ next_state: "bogus" }), /next_state/);
  assert.throws(
    () => validateVerdict({ next_state: "accepted" }),
    /requires a work_order/,
  );
  assert.throws(
    () => validateVerdict({ next_state: "declined", work_order: {} }),
    /only allowed when next_state=accepted/,
  );
  assert.equal(validateVerdict({ next_state: "needs_info" }), true);
  assert.equal(
    validateVerdict({ next_state: "accepted", work_order: { ticket_id: "x" } }),
    true,
  );
});

test("runPoLoop polls, processes, sleeps, then stops on abort", async () => {
  const { client, verdicts } = poFixture([{ id: "t-spam" }]);
  const decide = () => ({ next_state: "rejected_spam" });

  const ac = new AbortController();
  let sleeps = 0;
  const sleep = async () => {
    sleeps += 1;
    if (sleeps >= 2) ac.abort(); // let it do two iterations then stop
  };

  await runPoLoop({ client, decide, signal: ac.signal, sleep });
  assert.ok(verdicts.length >= 2); // processed the queue each iteration
  assert.ok(ac.signal.aborted);
});

test("runPoLoop backs off when the queue fetch fails", async () => {
  // Queue endpoint always 500s → every iteration hits the backoff branch.
  const { fetch } = makeFakeFetch({
    "GET /api/agent/queue": () => ({ status: 500, json: { error: "boom" } }),
  });
  const client = createApiClient({ env: TEST_ENV, fetch });

  const ac = new AbortController();
  const sleptMs = [];
  const sleep = async (ms) => {
    sleptMs.push(ms);
    if (sleptMs.length >= 3) ac.abort();
  };

  await runPoLoop({
    client,
    decide: () => ({ next_state: "needs_info" }),
    signal: ac.signal,
    sleep,
    options: { pollIntervalMs: 100, backoffFactor: 2, maxBackoffMs: 1000 },
  });
  // Exponential growth: 100, 200, 400 …
  assert.equal(sleptMs[0], 100);
  assert.equal(sleptMs[1], 200);
  assert.equal(sleptMs[2], 400);
});
