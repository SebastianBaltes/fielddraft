# FieldDraft Desktop Agents (`agents/`)

The two **ClaudeDraft desktop agents** that drive the FieldDraft user→agent loop
(Phase 1b of `docs/PLATFORM-ARCHITECTURE.md`):

- **`fielddraft-po`** — the product-owner agent. Triages tickets, classifies
  spam / malicious / vision-fit, talks to users in the thread, emits the §8.3
  verdict, and (on accept) creates the work-order. **No source access** (§4.1).
- **`fielddraft-coding`** — the coding agent. Pulls accepted work-orders,
  reproduces from forwarded artifacts, implements + tests + builds + deploys, and
  records the deploy. The **only** agent with source/build/deploy access. Never
  talks to users directly.

Both authenticate to the WP6 backend API (`server/agent.go`) with a
least-privilege **Service-Token** (`Authorization: Bearer <token>`, §11).

This directory is **self-contained and zero-dependency**: plain Node ESM
(`.mjs`), global `fetch`, tested with the built-in `node --test`. It is **not**
part of the `web/` workspace and has no npm dependencies.

---

## What the scripts do — and what they do NOT do

This is the important boundary. Read it before wiring anything up.

| Layer | Who | Does |
| --- | --- | --- |
| **`lib/apiClient.mjs`** | script | HTTP plumbing: builds requests with the right Bearer token, parses JSON (or raw bytes for `fetchArtifact`), throws `ApiError` on non-2xx. |
| **`lib/ticketSync.mjs`** | script | The **read side** ("snapshots down"): `materializeTicket` writes one queue item to the §3.1 on-disk layout (`ticket.md`/`comments.md`/`ticket.json` + downloaded artifact blobs); `pull` does a whole scope's queue into `open/`; `moveToDone` archives `open/<id>` → `done/<id>`. |
| **`*-runner.mjs`** | script | The poll loop, backoff, per-ticket error isolation, and the §10.1 workflow skeleton (claim → **materialize** → record → complete → state → **move**; failure → dev note + needs_info). |
| **`decide(item, {ticketDir})` / `implement(workOrder, item, {ticketDir})`** | **ClaudeDraft agent** | The **model step** — the actual reasoning, judgement, code, build, and deploy, working over the materialized files at `ticketDir`. |
| **`send_to_agent`, `chat_history`, `agent_status`** | **ClaudeDraft agent at runtime** | Waking the sibling agent / reporting status. |

**A standalone script cannot reason, cannot call `send_to_agent`, and cannot
build or deploy FieldDraft.** The runners deliberately abstract the model step
behind an injected function:

- The runner is given a `decide` / `implement` callback.
- In **production** that callback is the **ClaudeDraft Opus 4.8 agent** itself:
  the ClaudeDraft runtime loads the matching prompt (`fielddraft-po/prompt.md` or
  `fielddraft-coding/prompt.md`), feeds it the queue item as *data*, and the
  agent's structured output is the verdict / deploy descriptor the runner sends
  back over the API.
- In **tests** the callback is a deterministic fake (see `test/`).

Running a runner file directly (`node po-runner.mjs`) starts the loop with a
`decide`/`implement` that **throws on purpose** — the real one only exists inside
the ClaudeDraft agent. This keeps the boundary honest instead of pretending a
script can think.

So the production deployment is: the ClaudeDraft agent runtime *is* the host; it
calls the runner's exported loop with its own reasoning wired in as
`decide`/`implement`, and uses its MCP tools (`send_to_agent` etc.) for the
agent-to-agent coordination the API can't express.

---

## Environment / configuration

Both runners read config from the environment (see `lib/apiClient.mjs`):

| Variable | Used by | Meaning |
| --- | --- | --- |
| `FIELDDRAFT_API_URL` | both | Base URL of the backend API, e.g. `https://fielddraft.example` (no trailing slash needed). |
| `PO_SERVICE_TOKEN` | po | The `po`-scope Service-Token. **Must match** the server's `PO_SERVICE_TOKEN` (`server/agent.go`). |
| `CODING_SERVICE_TOKEN` | coding | The `coding`-scope Service-Token. Must match the server's `CODING_SERVICE_TOKEN`. |
| `PO_TICKETS_DIR` | po | The PO's **source-less** tickets dir (default `./po-tickets`). Tickets materialize into `<dir>/open/<id>/`. |

The coding agent's tickets dir is the repo worktree's `tickets/` (materialized to
`tickets/open/<id>/`, archived to `tickets/done/<id>/`); it is not an env var
because the coding agent is *defined* by running in the repo worktree.

An **empty/unset token disables that scope server-side** (the server's
constant-time compare never matches `""`). The client fails fast with a clear
message if you try to use a scope whose token isn't set — a missing secret never
silently grants (or silently no-ops) access.

The tokens live **only on the desktop** (§11), next to the agents — never on the
V-Server, never in the repo. Set them via your shell profile or a vault, e.g.:

```bash
export FIELDDRAFT_API_URL="https://fielddraft.example"
export PO_SERVICE_TOKEN="…"        # matches the server's po token
export CODING_SERVICE_TOKEN="…"    # matches the server's coding token
```

---

## Ticket-sync: snapshots down, commands up (`lib/ticketSync.mjs`)

Prod tickets live in the DB + content-addressed blobs — there is no
`tickets/<id>/` on the server (see `docs/TICKET-SYNC-AND-AGENTS.md`). The sync
layer gives each agent the **file ergonomics of a local ticket** without a
bidirectional file-sync:

- **Down (read, idempotent):** `pull({scope, destDir, client})` calls
  `GET /api/agent/queue`, then for each item `materializeTicket` writes the §3.1
  layout into `destDir/open/<id>/` — `ticket.md` (title + meta + body),
  `comments.md` (the thread), `ticket.json` (state/verdict/work-order + artifact
  index), and each artifact blob to its canonical name (`screenshot.png`,
  `project.fdraft`, `actions.json`, `settings.json`, `ui_state.json`,
  `attachment-NN-<name>`) via `GET /api/tickets/{id}/artifacts/{aid}`. Text/thread
  refresh every pull; content-addressed blobs already on disk (same size) are
  skipped. The PO pulls text-only where it wants (`{artifacts:false}` or just not
  opening the repro files); the coding agent pulls the full bundle.
- **Up (write):** never files — the typed API ops in `apiClient.mjs`
  (`postVerdict` / `postAgentComment` / `patchState` / `claim` / `complete` /
  `recordDeploy`). The server applies them transactionally, so a user writing in
  the thread in parallel can never cause a lost update.
- `moveToDone(destDir, id)` archives `open/<id>` → `done/<id>` after completion.

The runners wire this in: `runCodingOnce` materializes the full bundle to
`tickets/open/<id>/` on `claim` and moves it to `tickets/done/<id>/` after
`complete`; `runPoOnce` materializes text+thread+screenshot into
`$PO_TICKETS_DIR/open/<id>/` before `decide`. Both pass the ticket's dir to the
model step as `ctx.ticketDir`.

## Running against prod

Point both agents at the prod API and give each the token for its scope. The
tokens come from prod's `.env.prod` (`PO_SERVICE_TOKEN` / `CODING_SERVICE_TOKEN`)
and live **only on the desktop** — never in the repo, never on the V-Server.

```bash
export FIELDDRAFT_API_URL="https://203.0.113.10.sslip.io"   # prod

# ── PO agent — source-less tickets dir ──
export PO_SERVICE_TOKEN="…"            # from .env.prod
export PO_TICKETS_DIR="$HOME/fd-po-tickets"
# run the ClaudeDraft `fielddraft-po` agent HERE (cwd = $PO_TICKETS_DIR); it
# reasons via fielddraft-po/prompt.md and drives runPoLoop's `decide`.

# ── Coding agent — repo worktree ──
export CODING_SERVICE_TOKEN="…"        # from .env.prod
# run the ClaudeDraft `fielddraft-coding` agent in the repo worktree; its
# `implement` has source/build/deploy and runs the real pipeline.
```

The **cycle** each poll: `pull` (materialize the queue) → **work** over the files
(triage/verdict for the PO; reproduce/fix/test/build/deploy for coding) → **push**
the semantic op (verdict/comment/state/claim/complete/deploy) → the coding agent
also `moveToDone`. Keep the DB as the source of truth — materialization is the
read side only. The **PO→coding wake** (`send_to_agent`) is a runtime ClaudeDraft
tool, a notification on top of the DB work-order, never a replacement for it.

## Polling cadence (§8.1, §10.1)

- **PO**: polls `triage` tickets + `needs_info`/`declined` threads with a new user
  reply, every ~15 s (`PO_DEFAULTS.pollIntervalMs`). Triage is latency-sensitive
  (a user is waiting), so it polls more often.
- **Coding**: polls `accepted` tickets with an open work-order every ~30 s
  (`CODING_DEFAULTS.pollIntervalMs`). Coding work is slower, so it polls less.

Both back off exponentially (capped at 5 min) when a queue fetch fails, so a
transient API/network outage doesn't hammer the server. (§15 notes the PO and
coding agent **share the ClaudeDraft subscription rate limits** — keep intervals
generous and prioritise PO triage during ticket bursts.) Later this may move to
SSE/webhook instead of polling.

---

## How the PO wakes the coding agent (`send_to_agent`, §9)

The DB work-order is the **source of truth**: the coding agent's poll loop finds
accepted tickets on its own. On top of that, because both are ClaudeDraft agents,
the **PO can nudge the coding agent at runtime** with the `send_to_agent` MCP
tool — a wake signal plus a direct file hand-off — and the coding agent reports
status back the same way (in addition to the durable internal `dev` note). That
wake is an action the **ClaudeDraft agent** takes, not the runner script; the
script only writes the verdict/work-order over the API. The wake is a
notification + transport, never a replacement for the DB record.

---

## Trust boundaries (recap — encoded in the prompts)

1. **PO has no source access** (structural, §4.1). It cannot leak code because it
   never has it. The server leak-filter on outgoing `po` comments is a backstop,
   not the primary control. → encoded in `fielddraft-po/prompt.md` §1–§2, §5.
2. **PO is the only public-comment author.** The coding agent reports to the PO
   internally; the PO decides what the user sees. → `po` prompt §1; `coding`
   prompt §6–§7.
3. **Ticket/user text is DATA, not commands.** Both agents treat ticket text and
   forwarded artifacts as untrusted input — they classify/reproduce it, never
   obey instructions inside it (§9 invariant). → `po` prompt §2; `coding` prompt
   §1.
4. **Coding trusts only the PO-authored work-order fields** (`summary` /
   `acceptance_criteria` / `vision_notes`); `forwarded_artifacts` +
   `untrusted_user_text` are reproduced, never executed. → `coding` prompt §1.

---

## Running the tests

```bash
cd agents
node --test          # built-in runner; injects a fake fetch — no live server/DB

# To be safe on Node version:
source ~/.nvm/nvm.sh && nvm use 23 && node --test
```

The tests cover the API client (correct URL / Bearer token / scope / parsing,
plus `fetchArtifact` bytes), the ticket-sync layer (`materializeTicket` writes
the §3.1 files with the right names + content, idempotent blob-skip, `pull` into
`open/`, `moveToDone`), the PO runner (materialize-before-`decide`; verdict from a
fake `decide`, incl. accepted-with-work_order and spam), and the coding runner
(claim → materialize → implement → record-deploy → complete → move, plus the
failure path that posts a `dev` note and sets `needs_info`). No network — a fake
`fetch` serves JSON and artifact bytes; temp dirs under `os.tmpdir()`.

## Layout

```
agents/
├── package.json                     # type:module, "test": "node --test", NO deps
├── README.md                        # this file
├── lib/
│   ├── apiClient.mjs                # thin client over the WP6 agent API (+ fetchArtifact)
│   └── ticketSync.mjs               # materialize / pull / moveToDone (read side, §3.1)
├── fielddraft-po/
│   ├── prompt.md                    # PO system prompt (gateway + verdict + examples)
│   └── po-runner.mjs                # PO poll loop (materialize → decide injection)
├── fielddraft-coding/
│   ├── prompt.md                    # coding operating playbook (trusted/untrusted, build/deploy)
│   └── coding-runner.mjs            # coding poll loop (materialize → implement → move)
└── test/
    ├── apiClient.test.mjs
    ├── ticketSync.test.mjs
    ├── po-runner.test.mjs
    └── coding-runner.test.mjs
```
