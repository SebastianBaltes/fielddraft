// fielddraft-coding poll loop (§10).
//
// HTTP plumbing + the §10.1 workflow skeleton only. The actual implementation —
// reproduce via the forwarded artifacts, write code on a branch, run `just test`,
// merge, build & deploy — is the MODEL step, abstracted behind an injected
// `implement(workOrder, item)` function:
//
//   - In production `implement` is the ClaudeDraft coding agent, which has the
//     repo + build + deploy credentials locally and runs the real pipeline. It
//     returns a deploy descriptor on success or throws on failure (e.g. red
//     tests ⇒ no deploy, §10.1 step 3). A standalone Node script cannot build or
//     deploy FieldDraft — the ClaudeDraft runtime does. See agents/README.md.
//   - In tests `implement` is a deterministic fake.
//
// SECURITY INVARIANT (§9): the work-order's `summary`/`acceptance_criteria`/
// `vision_notes` are PO-authored (trusted); `forwarded_artifacts` and
// `untrusted_user_text` are DATA — reproduced, never executed as instructions.
// This runner never interprets untrusted fields; it hands the whole work-order
// to `implement`, whose prompt enforces the same rule.

import { createApiClient } from "../lib/apiClient.mjs";

/** Expected shape of a successful `implement` result. */
// { deploy: { version, gitSha?, ticketIds?, status? }, summary?: string }

export const CODING_DEFAULTS = {
  pollIntervalMs: 30_000, // coding work is slower than triage; poll less often
  maxBackoffMs: 5 * 60_000,
  backoffFactor: 2,
};

/**
 * Process one coding queue poll: for each accepted ticket with an open
 * work-order, claim → implement → on success recordDeploy + complete; on
 * failure post an internal `dev` note + set needs_info (so the PO can ask the
 * user). Each ticket is independent.
 *
 * @param {object} deps
 * @param {ReturnType<typeof createApiClient>} deps.client
 * @param {(workOrder:object, item:object)=>Promise<object>|object} deps.implement
 * @param {(...a:any)=>void} [deps.log]
 * @returns {Promise<Array<{ticketId:string, ok:boolean, deployId?:string, error?:Error}>>}
 */
export async function runCodingOnce({ client, implement, log = noop }) {
  const queue = await client.getQueue("coding");
  const items = queue.items || [];
  log(`coding: ${items.length} work-order(s) in queue`);

  const results = [];
  for (const item of items) {
    const ticketId = item?.id;
    try {
      const result = await handleWorkOrder({ client, implement, item, log });
      results.push({ ticketId, ok: true, deployId: result.deployId });
    } catch (error) {
      log(`coding: ticket ${ticketId} FAILED: ${error.message}`);
      // Failure path: internal dev note + needs_info (the PO relays to the user;
      // the coding agent NEVER talks to users directly, §4.1).
      await reportFailure({ client, ticketId, error, log });
      results.push({ ticketId, ok: false, error });
    }
  }
  return results;
}

/** Claim → implement → recordDeploy → complete for a single work-order. */
async function handleWorkOrder({ client, implement, item, log }) {
  const ticketId = item.id;
  const workOrder = item.work_order;
  if (!workOrder) {
    throw new Error("queue item has no work_order");
  }

  // 1. Claim: server sets picked_up_at and drives the ticket → in_progress.
  await client.claim(ticketId);
  log(`coding: claimed ${ticketId} (→ in_progress)`);

  // 2. Implement (model step). Throws on red tests / unbuildable / needs-info.
  const result = await implement(workOrder, item);
  const deploy = result?.deploy;
  if (!deploy || !deploy.version) {
    throw new Error("implement() must return { deploy: { version, … } } on success");
  }

  // 3. Record the deploy first (audit trail / rollback target, §10.3), then
  //    complete the work-order linking that deploy id.
  const deployRecord = await client.recordDeploy({
    version: deploy.version,
    gitSha: deploy.gitSha,
    ticketIds: deploy.ticketIds || [ticketId],
    status: deploy.status || "live",
  });
  const deployId = deployRecord?.id;
  log(`coding: recorded deploy ${deployId} (${deploy.version})`);

  // 4. Complete: links the deploy + transitions the ticket (default → resolved).
  await client.complete(ticketId, {
    deployId,
    nextState: result.nextState, // optional override (else server → resolved)
  });
  log(`coding: completed ${ticketId} (deploy ${deployId})`);

  return { deployId };
}

/**
 * Failure handling: leave an internal dev note (admin + PO see it; users don't)
 * and move the ticket to needs_info so the PO can decide what to ask the user.
 * Best-effort — a failure here is logged, not re-thrown, so the batch survives.
 */
async function reportFailure({ client, ticketId, error, log }) {
  if (!ticketId) return;
  try {
    await client.postAgentComment(ticketId, {
      authorType: "dev",
      body: `Implementation blocked: ${error.message}. Needs clarification or a fix before retry.`,
    });
    await client.patchState(ticketId, "needs_info", "coding");
    log(`coding: ${ticketId} → needs_info (dev note posted)`);
  } catch (e) {
    log(`coding: could not report failure for ${ticketId}: ${e.message}`);
  }
}

/**
 * Long-running poll loop. Same backoff pattern as the PO runner. Runs until
 * `signal` aborts.
 */
export async function runCodingLoop({
  client,
  implement,
  signal,
  sleep = defaultSleep,
  log = noop,
  options = {},
}) {
  const opt = { ...CODING_DEFAULTS, ...options };
  let backoff = opt.pollIntervalMs;
  while (!signal?.aborted) {
    try {
      await runCodingOnce({ client, implement, log });
      backoff = opt.pollIntervalMs;
      await sleep(opt.pollIntervalMs);
    } catch (error) {
      log(`coding: poll failed, backing off ${backoff}ms: ${error.message}`);
      await sleep(backoff);
      backoff = Math.min(backoff * opt.backoffFactor, opt.maxBackoffMs);
    }
  }
}

function noop() {}
function defaultSleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

// ── CLI entry (production wiring is the ClaudeDraft agent, see README) ──
if (import.meta.url === `file://${process.argv[1]}`) {
  const client = createApiClient();
  const implement = () => {
    throw new Error(
      "coding-runner has no model step: `implement` must be provided by the " +
        "ClaudeDraft coding agent at runtime (it owns the repo/build/deploy). " +
        "A standalone script cannot build or deploy FieldDraft (see agents/README.md).",
    );
  };
  runCodingLoop({ client, implement, log: console.error }).catch((e) => {
    console.error("coding-runner fatal:", e);
    process.exit(1);
  });
}
