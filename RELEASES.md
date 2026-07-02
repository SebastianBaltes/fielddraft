# FieldDraft — Release & Archive-Deploy

How an app-agent builds a release artifact for the central-ingress prod host.
Deploy/host orchestration itself is owned by **devops** (Ansible IaC,
`ansible-playbook deploy.yml -e only_app=fielddraft`); this file covers only the
**app-side** contract: how images are named and packed.

## Version = single source of truth

The version lives in **`web/package.json`** (`version`) and is stamped into the
WASM kernel (`FIELDDRAFT_VERSION`) at build time. Both images ship under the
**same** version. Bump it per the semver policy (see `CLAUDE.md`) and add a
`web/src/releaseNotes.ts` entry (enforced by the `commit-msg` hook).

## Build the artifact

```
just release-image
```

This:

1. builds `fielddraft/server:<version>` (from `server/Dockerfile`)
2. builds `fielddraft/web:<version>` (from `web/Dockerfile.prod`, context = repo
   root; self-contained: rebuilds the WASM kernel + Vite bundle from source)
3. `docker save fielddraft/server:<version> fielddraft/web:<version> \
      | gzip > dist/fielddraft-<version>.image.tar.gz`

`dist/` is gitignored — the artifact is never committed. Ship the tar to the
server (e.g. `scp`), where devops' deploy path loads it and runs the compose:

```
gunzip -c fielddraft-<version>.image.tar.gz | docker load
# devops: ansible-playbook deploy.yml -e only_app=fielddraft   (compose refs fielddraft/{server,web}:${TAG})
```

## Image contract (what devops' compose expects)

- **Tags:** `fielddraft/server:<version>` and `fielddraft/web:<version>` — local
  tags, no registry prefix (archive-deploy, not a registry pull). Later, once a
  `read:packages` PAT exists, the same images can be pushed/pulled via GHCR.
- **`fielddraft/web`** is the app's own inner Caddy. It bakes `web/Caddyfile.internal`:
  serves the SPA on **plain HTTP `:80`** with SPA fallback, proxies `/api/*` →
  `server:8080`, and sets the **correctness-critical** app headers — especially
  `Cross-Origin-Opener-Policy: same-origin` + `Cross-Origin-Embedder-Policy:
  require-corp` (required for SharedArrayBuffer / WASM threads). **TLS + HSTS are
  the edge Caddy's job, not this image's.** The edge does `reverse_proxy` to this
  container on `:80` over the shared `edge` network; `db`/`server` stay on the
  app's private internal network (no host ports).
- **`fielddraft/server`** runs its embedded DB migrations on startup
  (`server/migrations.go`). Deploying a new server tag **auto-migrates** — there
  is no separate migration step. Consequence: migrations are **forward-only**;
  rolling back to an older server image after a forward migration is **not**
  automatically safe and needs a DB restore from backup.

## Pre-cutover note

Until the coordinated central-ingress cutover, the live compose still
bind-mounts the edge `./Caddyfile` (`{$DOMAIN}` + TLS) over the baked
`/etc/caddy/Caddyfile`, so it keeps winning and current prod is unaffected. The
edge-ingress compose (no bind-mount) activates the baked `:80` config.
