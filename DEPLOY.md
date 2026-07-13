# FieldDraft — Self-Hosting Guide

## Prerequisites

- Docker + Docker Compose
- Node.js 22+ (for building the frontend)
- Rust + wasm-pack (for building the WASM kernel)

## Quick Start (Development)

```bash
# 1. Build the WASM kernel
just build-wasm

# 2. Start the frontend dev server
cd web && npm install && npm run dev

# 3. (Optional) Start the backend
cd server && go run .
```

The app runs at `http://localhost:5173` in guest/offline mode.
No backend needed for local modeling — projects save to localStorage.

## Dev stack (Docker Compose)

The committed `docker-compose.yml` is the **development** stack (MariaDB + Go
server). It is NOT for production — ports are exposed on the host for
convenience. The frontend is not containerized in dev: run Vite on the host
(`just dev` / `just dev-full`); it proxies `/api` to the Go server.

```bash
docker compose up --build -d db server
# db:     MariaDB 11   (host :3306)
# server: Go backend   (host :8080)

# Optional: server-side CGAL meshing (export dialog "CGAL Mesh_3 (server)")
docker compose up --build -d db server mesher
# mesher: CGAL meshing service (in-network only, server reaches it via MESHER_URL)
```

Dev env vars (host port overrides) live in `.env` — see `.env.example`.

### Onshape import (optional)

The "Aus Onshape…" import needs an Onshape API key **on the server** — the
browser cannot call Onshape directly (no CORS, and the secret must never reach
the bundle), so `server/onshape.go` proxies the two read endpoints and signs
them. Without the key those routes answer `503 onshape not configured`, and the
import dialog shows exactly that.

Put the key pair in the (gitignored) `.env`; `docker-compose.yml` passes both
through to the server container:

```bash
ONSHAPE_ACCESS_KEY=…
ONSHAPE_SECRET_KEY=…
# ONSHAPE_URL=https://cad.onshape.com   # optional, this is the default
```

Get the pair from <https://dev-portal.onshape.com> → API keys. **The key is an
IDENTITY, not a public-data pass:** it reads everything its Onshape account may
read. That is why the proxy routes sit behind `requireAuth` — never expose them
unauthenticated. Restart the server after adding the key
(`docker compose up -d server`).

### Full local stack with login (`just dev-full`)

For a working local login, run the DB + Go server (via docker compose) and the
Vite dev server together:

```bash
just dev-full
```

This brings up `db` + `server` in the background and starts Vite on
`http://localhost:5173`. Vite proxies `/api` → `http://localhost:8080` (the Go
server), so auth/invites/projects work end-to-end.

A fresh dev DB seeds a default admin — **log in with `admin@local` / `admin`**
(override via `FIELDDRAFT_ADMIN_EMAIL` / `FIELDDRAFT_ADMIN_PASSWORD` in `.env`).
The agent API is enabled too, with dev Service-Tokens `dev-po` / `dev-coding`
(override via `FIELDDRAFT_PO_SERVICE_TOKEN` / `FIELDDRAFT_CODING_SERVICE_TOKEN`).

Stop the backend with `docker compose down` (add `-v` to also wipe the DB and
re-seed the admin on the next boot).

---

## Production Deployment

Production runs a **separate** stack: `docker-compose.prod.yml` + `Caddyfile`,
driven by Ansible. See `docs/PLATFORM-ARCHITECTURE.md` §10–§12 for the full
design. The V-server is source-less: it only runs compiled images.

### Prod stack layout

```
Browser ──TLS──▶ caddy (80/443/443-udp) ──┬──▶ server :8080 (Go API) ──▶ db :3306
                                           └──▶ /srv  (static SPA: WASM + minified JS)
```

| Service | Image | Host ports | Notes |
|---|---|---|---|
| `db` | `mariadb:11` | **none** | in-network only; password from env/Vault |
| `server` | `…/server:<tag>` | **none** | Caddy proxies `/api/*`; runs DB migrations on startup |
| `caddy` | `…/web:<tag>` | 80, 443, 443/udp | auto-TLS; serves the SPA; reverse-proxies the API |
| `mesher` | `…/mesher:<tag>` | **none** | server-side CGAL meshing (fd-prepare + fd-cgal-mesh + HTTP wrapper); only `server` talks to it; scale with `--scale mesher=N` |

Persistent volumes: `db-data`, `blob-data` (`/data/blobs`), `caddy-data`
(ACME certs), `caddy-config`. `restart: unless-stopped` on every service.

### How the WASM/web build is handled

`web/Dockerfile.prod` is a self-contained multi-stage build (context = repo root):

1. **`wasm` stage** (`rust:1-bookworm`): installs `wasm-pack` + the
   `wasm32-unknown-unknown` target and compiles `crates/kernel` into
   `web/src/wasm/` — exactly what `just build-wasm` does locally (this output is
   gitignored, so the image must rebuild it from source).
2. **`web` stage** (`node:22`): `npm ci`, then `tsc -b && vite build --config
   vite.config.prod.ts`. The prod config sets `build.sourcemap: false` (no TS
   source maps shipped) and minifies → `web/dist/`.
3. **runtime stage** (`caddy:2-alpine`): copies `web/dist` to `/srv`. The
   `Caddyfile` is bind-mounted by the compose stack (not baked) so DOMAIN/TLS
   can change without rebuilding.

The Go server image is the existing static-binary `server/Dockerfile`.

### Continuous Integration (test gate)

`.github/workflows/ci.yml` runs on **every push and pull request** to
`agents/fielddraft-a-platform` and `main`. Three jobs run in parallel:

| Job | What it runs |
|---|---|
| `server` | `cd server && go vet ./... && go test ./...` (Go 1.26) |
| `web` | builds the WASM kernel (`just build-wasm` equivalent), then `npm ci && tsc -b && npm test` (Node 23, vitest) |
| `agents` | `cd agents && node --test` (zero-dep Node ESM) |

The `web` job must compile `crates/kernel` → `web/src/wasm/` first: that output
is gitignored (a build artifact), but both `tsc -b` and vitest need the
generated kernel types. CI installs `wasm-pack` pinned to the same version as
`web/Dockerfile.prod` (`0.13.1`) and runs the exact commands `just build-wasm`
uses, with cargo/target caching. **CI red ⇒ no release** (PLATFORM-ARCHITECTURE
§10.1: „Rot ⇒ kein Deploy").

### Release (tag → GHCR) (PLATFORM-ARCHITECTURE §10.2)

Releases are **tag-driven**, not branch-driven. Pushing a `v*` tag triggers
`.github/workflows/release.yml`, which logs in to GHCR with the workflow's
`GITHUB_TOKEN` (`packages: write`) and uses `docker/build-push-action` to build
**and push all three images private to GHCR**, each tagged with the version **and**
`latest`:

| Image | Built from | Context | Pushed as |
|---|---|---|---|
| server | `server/Dockerfile` (static Go binary) | `./server` | `ghcr.io/<owner>/fielddraft/server:<tag>` |
| web | `web/Dockerfile.prod` (wasm → vite → caddy) | repo root `.` | `ghcr.io/<owner>/fielddraft/web:<tag>` |
| mesher | `cgal-mesher/Dockerfile` (fd-prepare + CGAL + wrapper; golden interpreter test runs in-build) | repo root `.` | `ghcr.io/<owner>/fielddraft/mesher:<tag>` |

The owner is lower-cased per GHCR rules; with `REGISTRY` left at its default the
refs equal `ghcr.io/sorokan/fielddraft/{server,web}:<tag>` — exactly what
`docker-compose.prod.yml` and `deploy/ansible/deploy.yml -e version=<tag>` pull.

**Why tags, not a release branch?** A tag is an immutable, named point in
history. It maps 1:1 onto the deploy's `version` var (`deploy.yml -e
version=vX.Y.Z`, `TAG=vX.Y.Z`) and onto the `deploys(version, git_sha, …)`
record (PLATFORM-ARCHITECTURE §10.3) — the image tag, the deployed version, and
the git SHA are all the same coordinate. A moving branch ref could not give that
immutable mapping, and rollback (“deploy the previous tag”) needs each release
to stay individually addressable.

#### Cut a release

```bash
# 0. Land the change; CI on the branch must be green (test gate above).

# 1. Bump the single source of truth and commit.
#    web/package.json "version" feeds both the WASM stamp and the deploy tag.
$EDITOR web/package.json            # e.g. 0.6.339 → 0.6.340

git commit -am "release: v0.6.340"

# 2. Tag and push the tag — this fires release.yml (build + push to GHCR).
git tag v0.6.340
git push origin HEAD --tags

# 3. Wait for the Release workflow to finish (all three images on GHCR), then deploy.
cd deploy/ansible
ansible-playbook -i inventory.ini deploy.yml --ask-vault-pass -e version=v0.6.340
```

> **Tag = `v` + the `web/package.json` version.** Keep them in lock-step so the
> WASM build stamp, the image tag, and the deploy `version` all agree.

### Manual build → push → deploy (no CI / break-glass)

The desktop is the only host with source, so a release can also be cut by hand
(produces the **same** image refs the deploy side pulls):

```bash
TAG=v$(node -p "require('./web/package.json').version")
docker build -f web/Dockerfile.prod -t ghcr.io/sorokan/fielddraft/web:$TAG .
docker build -f server/Dockerfile  -t ghcr.io/sorokan/fielddraft/server:$TAG ./server
docker push ghcr.io/sorokan/fielddraft/web:$TAG
docker push ghcr.io/sorokan/fielddraft/server:$TAG

cd deploy/ansible
ansible-playbook -i inventory.ini deploy.yml --ask-vault-pass -e version=$TAG
```

### Ansible playbooks (`deploy/ansible/`)

| Playbook | Purpose |
|---|---|
| `bootstrap.yml` | One-time: install Docker + compose plugin, lock UFW to 22/80/443, create the `deploy` user, lay out `/opt/fielddraft` + `/data`. |
| `deploy.yml` | Sync compose+Caddyfile, render `.env.prod` from Vault, `docker login` GHCR, pull `version`'s images, `compose up -d`, healthcheck `/api/me`. |
| `backup.yml` | Install the nightly backup (systemd timer): `mysqldump`+gzip, `restic` of `/data/blobs`, retention prune. |

Copy `inventory.example.ini` → `inventory.ini` and `group_vars/all.example.yml`
→ `group_vars/all.yml`; set the V-server host + non-secret config there.

**DB migrations** run automatically: the Go server applies its embedded
`migrations/*.sql` on startup (`server/migrations.go`). Bringing `server` up IS
the migration step — there is no separate migrate task.

### Secrets (Ansible-Vault — never committed)

Non-secret config goes in `group_vars/all.yml`; secrets go in an encrypted
`group_vars/vault.yml`:

```bash
cd deploy/ansible
ansible-vault create group_vars/vault.yml   # define the vault_* vars (see all.example.yml)
```

`deploy.yml` renders `/opt/fielddraft/.env.prod` from those vars. For a manual
(non-Ansible) run, copy **`.env.prod.example`** → `.env.prod` and fill it, then:

```bash
docker compose --env-file .env.prod -f docker-compose.prod.yml up -d
```

Required env (full list in `.env.prod.example`): `DOMAIN`, `TLS_EMAIL`,
`REGISTRY`, `TAG`, `DB_ROOT_PASSWORD`, `DB_NAME`, `DB_USER`, `DB_PASSWORD`,
`ADMIN_EMAIL`, `ADMIN_PASSWORD`. The server gets `SECURE_COOKIES=1`
(set in the compose file) so session cookies carry `Secure` behind TLS, and
`TRUST_PROXY=1` so the per-IP rate limiter reads the client IP from
`X-Forwarded-For` (correct behind Caddy, which strips client-supplied XFF;
without a trusted proxy leave it unset — the header is client-spoofable).

### AI assistant (ticket #347)

The assistant proxies model calls through the Go server, so the **OpenRouter key
lives on the server** — and it is the one secret that spends real money, so it
never travels as an environment variable (an env var shows up in
`docker inspect`, in `/proc/<pid>/environ`, and is inherited by every child
process). Put it in the Vault as `vault_openrouter_api_key`; `deploy.yml` writes
it to `/opt/fielddraft/secrets/openrouter.key` (0600) and the compose file mounts
that read-only at `/run/secrets/openrouter.key`, which is what
`OPENROUTER_API_KEY_FILE` names.

**No key in the Vault = the assistant is simply off** — the server logs
`ai chat: no API key … chat disabled`, comes up healthy, and the app hides the
chat. The key file is still written, empty: it must EXIST before compose starts,
because Docker answers a bind mount whose host path is missing by creating a
root-owned *directory* there — which is not a key the server can read, and which
the next deploy (with a key) can no longer overwrite as the deploy user. The same
holds for a manual run: `touch secrets/openrouter.key` before `compose up`.

Optional knobs in `group_vars/all.yml`: `aichat_models` (comma-separated
allowlist — the **first** entry is the model the chat talks to, and the vision
model must be in the list or a screenshot turns the turn into a 403) and
`aichat_daily_quota` (assistant requests per user and day, default 300, 0 = off —
the minute-level rate limit stops a burst, not a patient script).
`AICHAT_ALLOWED_ORIGINS` is pinned to `https://{{ domain }}` by the template.

Caddy proxies `/aichat/*` to the server with `flush_interval -1` — the answer is
an SSE token stream, and with response buffering the user stares at a frozen
panel until the whole turn is done.

### Tickets (IssueVault)

The ticket domain lives in **IssueVault**, the shared external ticket service —
the ONLY backend (ticket #346). The MariaDB and file-backed stores are gone, and
with them the `TICKET_BACKEND` switch: two idle backends meant three
implementations of one domain, and an unset token used to fall back silently to
an isolated local store — a server that looks healthy while every ticket a user
files lands where nobody reads it. Now a missing token fails loudly (401
upstream). The deploy sets, in the prod env:

```
ISSUEVAULT_URL=…            # optional; falls back to the code default
ISSUEVAULT_PROJECT=…       # optional; falls back to the code default
ISSUEVAULT_INSTALLATION=…  # this deployment's installation id
ISSUEVAULT_TOKEN=…         # installation-bound agent token — SECRET (Vault)
```

These come from the deploy's env (Ansible-Vault → the rendered prod `.env`),
**not** from a committed compose default. `docker-compose.prod.yml` passes them
through with empty defaults, so a break-glass `--env-file .env.prod` run honours
them too.

All FieldDraft installations — production and each developer's dev stack — share
**one** IssueVault project and are isolated per installation (`scope:"own"`):
each sees only its own non-resolved tickets, while **resolved** tickets are read
project-wide (they migrate to `main` and are inherently shareable). A dev stack
opts in the same way via its gitignored `.env` (see *Dev stack* above); each
stack needs its own installation-bound token (ask the IssueVault operator).

### TLS / domain

Caddy auto-issues a Let's Encrypt cert for `{$DOMAIN}` and redirects HTTP→HTTPS.
Point the domain's A/AAAA record at the V-server first. For local testing set
`DOMAIN=localhost` and leave `TLS_EMAIL` unset → Caddy uses its internal CA.

### Rollback (1 command)

Every release is an immutable `v*` tag and a matching set of GHCR images; the
previous tag's images stay in GHCR. **Rollback = deploy the previous tag** — no
rebuild, no revert commit, just point `version` at the prior release:

```bash
ansible-playbook -i inventory.ini deploy.yml --ask-vault-pass -e version=<previous-tag>
```

This works precisely because releases are tag-driven (above): the prior image
tag is still addressable and still in the registry, so `deploy.yml` re-pulls it
and brings the stack back to that exact state.

### Backups (PLATFORM-ARCHITECTURE §12)

`backup.yml` installs a nightly `systemd` timer that runs `mysqldump` (gzip) +
`restic` of `/data/blobs` to an offsite repo, with 7-daily / 4-weekly retention.
Configure `vault_restic_repo` / `vault_restic_password` (+ provider creds in
`vault_restic_env`) in the Vault file. **Run a restore drill periodically — an
untested backup does not count.**

### Validate the configs locally (no live server)

```bash
# Compose parses with the example env:
set -a; . ./.env.prod.example; set +a
docker compose -f docker-compose.prod.yml config

# Playbooks parse:
cd deploy/ansible
for p in bootstrap.yml deploy.yml backup.yml; do ansible-playbook --syntax-check "$p" -i inventory.example.ini; done

# Caddyfile (if a caddy binary/image is available):
docker run --rm -e DOMAIN=fielddraft.example.com -e TLS_EMAIL=ops@example.com \
  -v "$PWD/../../Caddyfile:/etc/caddy/Caddyfile:ro" caddy:2-alpine \
  caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
```

### Without Docker

```bash
# 1. Set up MariaDB
mysql -u root -e "CREATE DATABASE fielddraft; CREATE USER 'fielddraft'@'%' IDENTIFIED BY 'secret'; GRANT ALL ON fielddraft.* TO 'fielddraft'@'%';"

# 2. Build and run the server
cd server && go build -o fielddraft-server . && ./fielddraft-server

# 3. Build and serve the frontend (WASM kernel + Vite bundle)
just build
# `just build` runs `build-wasm` first, so the bundle always carries a
# freshly compiled kernel stamped with the web/package.json version.
# (`cd web && npm run build` alone would NOT rebuild the kernel — the
# wasm-pack output in web/src/wasm/ is a gitignored build artifact and is
# not committed.)
# Serve web/dist/ via nginx/caddy with /api proxied to :8080
```

## API Endpoints

| Method | Path | Auth | Description |
|---|---|---|---|
| POST | `/api/auth/register` | — | Create account |
| POST | `/api/auth/login` | — | Login (sets session cookie) |
| POST | `/api/auth/logout` | ✓ | Logout (clears cookie) |
| GET | `/api/me` | ✓ | Current user |
| GET | `/api/projects` | ✓ | List own projects |
| POST | `/api/projects` | ✓ | Create project |
| GET | `/api/projects/:id` | ✓ | Get project with document |
| PATCH | `/api/projects/:id` | ✓ | Update name/document |
| DELETE | `/api/projects/:id` | ✓ | Delete project |

## Architecture

```
┌─────────────┐     ┌──────────────┐     ┌──────────┐
│  Browser     │────▶│  Go Server   │────▶│ MariaDB  │
│  (Vite/WASM) │     │  (:8080)     │     │ (:3306)  │
└─────────────┘     └──────────────┘     └──────────┘
       │
       │ WASM kernel runs entirely in the browser.
       │ The server stores projects as JSON blobs.
       │ No geometry computation on the server.
```

## Security Notes

- Passwords hashed with Argon2id (16-byte salt, 64 MB memory, 4 threads)
- Session cookies: HttpOnly, SameSite=Lax, 7-day expiry
- Per-IP rate limiting: 60 burst, 1 req/sec refill
- All project data is owner-scoped — no cross-user access
