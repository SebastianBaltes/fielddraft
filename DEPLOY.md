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
server + a Vite dev container). It is NOT for production — the web service runs
the Vite dev server and ports are exposed on the host for convenience.

```bash
docker compose up --build -d
# db:     MariaDB 11   (host :3306)
# server: Go backend   (host :8080)
# web:    Vite dev      (host :5173)
```

Dev env vars (host port overrides) live in `.env` — see `.env.example`.

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

### Build → push → deploy (PLATFORM-ARCHITECTURE §10.2)

On the **desktop** (the only host with source):

```bash
# 1. Build + push both images to GHCR (private), tagged with the app version.
TAG=v$(node -p "require('./web/package.json').version")
docker build -f web/Dockerfile.prod -t ghcr.io/sorokan/fielddraft/web:$TAG .
docker build -f server/Dockerfile  -t ghcr.io/sorokan/fielddraft/server:$TAG ./server
docker push ghcr.io/sorokan/fielddraft/web:$TAG
docker push ghcr.io/sorokan/fielddraft/server:$TAG

# 2. Deploy to the V-server (pulls the tag, brings the stack up, healthchecks).
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
(set in the compose file) so session cookies carry `Secure` behind TLS.

### TLS / domain

Caddy auto-issues a Let's Encrypt cert for `{$DOMAIN}` and redirects HTTP→HTTPS.
Point the domain's A/AAAA record at the V-server first. For local testing set
`DOMAIN=localhost` and leave `TLS_EMAIL` unset → Caddy uses its internal CA.

### Rollback (1 command)

Every deploy is a tagged image; the previous tag stays in GHCR. Roll back by
re-deploying the prior version:

```bash
ansible-playbook -i inventory.ini deploy.yml --ask-vault-pass -e version=<previous-tag>
```

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
