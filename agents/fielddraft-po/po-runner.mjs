// fielddraft-po poll loop (§8).
//
// This script does ONLY the HTTP plumbing, error handling, and the poll/backoff
// loop. The actual judgement — reading the ticket + thread + the FieldDraft
// vision and emitting the §8.3 verdict JSON — is the MODEL step, abstracted
// behind an injected `decide(item)` function:
//
//   - In production `decide` is the ClaudeDraft Opus 4.8 agent reasoning over
//     fielddraft-po/prompt.md with the queue item as DATA. (A standalone Node
//     script cannot run an LLM; the ClaudeDraft runtime drives it. See
//     agents/README.md for the boundary.)
//   - In tests `decide` is a deterministic fake.
//
// A `decide` result is the §8.3 verdict object. The runner posts it via
// postVerdict; the server validates it, sets state, writes the (leak-filtered)
// public_reply, and — on accepted — creates the work-order from verdict.work_order.

import { createApiClient } from "../lib/apiClient.mjs";
import { materializeTicket } from "../lib/ticketSync.mjs";

/** Default backoff/cadence (ms). Override per call for tests. */
export const PO_DEFAULTS = {
  pollIntervalMs: 15_000, // §8.1: poll every ~N seconds
  maxBackoffMs: 5 * 60_000,
  backoffFactor: 2,
  // The PO's source-less tickets dir (§4.1). Text + thread + screenshot are
  // materialized here so the PO triages over files, never touching the repo.
  ticketsDir: process.env.PO_TICKETS_DIR || "./po-tickets",
};

/**
 * Process exactly one queue poll: fetch the PO queue, run `decide` on each item,
 * and apply the verdict. Returns a per-item result list for logging/tests.
 *
 * Each item is handled independently — one bad ticket never aborts the batch.
 *
 * @param {object} deps
 * @param {ReturnType<typeof createApiClient>} deps.client
 * @param {(item:object, ctx:{ticketDir:string})=>Promise<object>|object} deps.decide  the model step.
 * @param {string} [deps.ticketsDir]  PO tickets dir (default PO_DEFAULTS.ticketsDir).
 * @param {(...a:any)=>void} [deps.log]
 * @returns {Promise<Array<{ticketId:string, ok:boolean, verdict?:object, error?:Error}>>}
 */
export async function runPoOnce({ client, decide, ticketsDir = PO_DEFAULTS.ticketsDir, log = noop }) {
  const queue = await client.getQueue("po");
  const items = queue.items || [];
  log(`po: ${items.length} ticket(s) in queue`);

  const results = [];
  for (const item of items) {
    const ticketId = item?.id;
    try {
      // Materialize the item into the PO's own source-less dir (§4.1): ticket.md
      // + comments.md + screenshot.png — enough to triage/answer/spam-judge. The
      // PO reads FILES, never the repo. (Full item is fine; the PO won't open
      // project.fdraft.)
      const { dir: ticketDir } = await materializeTicket(ticketsDir + "/open", item, {
        client,
        scope: "po",
      });
      const verdict = await decide(item, { ticketDir });
      validateVerdict(verdict);
      await client.postVerdict(ticketId, verdict);
      log(
        `po: ticket ${ticketId} → ${verdict.next_state}` +
          (verdict.next_state === "accepted" ? " (+work_order)" : ""),
      );
      results.push({ ticketId, ok: true, verdict });
    } catch (error) {
      // Don't let one ticket's failure stop the batch.
      log(`po: ticket ${ticketId} FAILED: ${error.message}`);
      results.push({ ticketId, ok: false, error });
    }
  }
  return results;
}

/**
 * The long-running poll loop. Polls, applies verdicts, sleeps `pollIntervalMs`
 * on success; on a queue-fetch error it backs off exponentially (capped). Runs
 * until `signal` aborts (so tests can stop it) — without a signal it runs forever.
 *
 * @param {object} deps
 * @param {ReturnType<typeof createApiClient>} deps.client
 * @param {(item:object)=>Promise<object>|object} deps.decide
 * @param {AbortSignal} [deps.signal]
 * @param {(ms:number)=>Promise<void>} [deps.sleep]
 * @param {(...a:any)=>void} [deps.log]
 * @param {Partial<typeof PO_DEFAULTS>} [deps.options]
 */
export async function runPoLoop({
  client,
  decide,
  signal,
  sleep = defaultSleep,
  log = noop,
  options = {},
}) {
  const opt = { ...PO_DEFAULTS, ...options };
  let backoff = opt.pollIntervalMs;
  while (!signal?.aborted) {
    try {
      await runPoOnce({ client, decide, ticketsDir: opt.ticketsDir, log });
      backoff = opt.pollIntervalMs; // reset on a clean poll
      await sleep(opt.pollIntervalMs);
    } catch (error) {
      // A queue-fetch failure (network/5xx) — back off, don't hammer.
      log(`po: poll failed, backing off ${backoff}ms: ${error.message}`);
      await sleep(backoff);
      backoff = Math.min(backoff * opt.backoffFactor, opt.maxBackoffMs);
    }
  }
}

/**
 * Shape-check a verdict before sending. The server is authoritative, but a
 * client-side guard catches an obviously broken `decide` early with a clear
 * message (e.g. accepted without a work_order).
 */
export function validateVerdict(v) {
  if (!v || typeof v !== "object") {
    throw new Error("decide() must return a verdict object");
  }
  const states = [
    "accepted",
    "declined",
    "needs_info",
    "rejected_spam",
    "rejected_malicious",
  ];
  if (!states.includes(v.next_state)) {
    throw new Error(`verdict.next_state must be one of ${states.join("|")}`);
  }
  if (v.next_state === "accepted" && !v.work_order) {
    throw new Error("verdict.next_state=accepted requires a work_order (§9)");
  }
  if (v.next_state !== "accepted" && v.work_order) {
    throw new Error("work_order only allowed when next_state=accepted");
  }
  return true;
}

function noop() {}
function defaultSleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

// ── CLI entry (production wiring is the ClaudeDraft agent, see README) ──
// Running this file directly starts the loop with a `decide` that refuses to
// run — the real `decide` is supplied by the ClaudeDraft Opus 4.8 agent at
// runtime. This guard makes the boundary explicit instead of silently no-op'ing.
if (import.meta.url === `file://${process.argv[1]}`) {
  const client = createApiClient();
  const decide = () => {
    throw new Error(
      "po-runner has no model step: `decide` must be provided by the ClaudeDraft " +
        "Opus 4.8 agent at runtime (see agents/README.md). A standalone script " +
        "cannot reason or call send_to_agent.",
    );
  };
  runPoLoop({ client, decide, log: console.error }).catch((e) => {
    console.error("po-runner fatal:", e);
    process.exit(1);
  });
}
