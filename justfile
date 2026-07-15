# FieldDraft developer command surface.
# All targets are safe to run from a clean checkout; nothing here talks to a
# remote, a database, or CI — M0 is strictly local.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Load per-instance config (ports, instance name) from a gitignored .env so
# that `fielddraft` and `fielddraft-a` can run side by side. Optional: a clean
# checkout with no .env just falls back to the in-file/Vite defaults.
# See .env.example.
set dotenv-load := true

# Activate the Node version pinned in web/.nvmrc (via nvm) before any npm/vite
# call. `just` runs recipes in non-interactive bash where nvm is NOT sourced, so
# the system Node (v18 on this box) would be used — too old for Vite 8 / Vitest
# 3, which then die with cryptic cjs/loader errors. Sources nvm if present and
# `nvm use` reads .nvmrc; falls back to whatever node is on PATH otherwise. Use
# as `cd web && {{nvm_use}} && npm …`.
nvm_use := '. "$HOME/.nvm/nvm.sh" 2>/dev/null && nvm use 2>/dev/null || true'

# Default: print the available recipes.
default:
    @just --list

# Install web dependencies and activate the repo's git hooks.
install: setup-hooks
    cd web && {{nvm_use}} && npm install

# Point git at the tracked .githooks dir so the commit-msg hook runs. The hook
# blocks a feat/fix/perf commit that bumps the version but forgot the matching
# web/src/releaseNotes.ts entry (see CLAUDE.md → "Release Notes pflegen").
# Idempotent — safe to re-run; shared across this repo's worktrees.
setup-hooks:
    git config core.hooksPath .githooks
    @echo "git hooks active (.githooks)"

# Build the Rust kernel to WASM and drop the output into web/src/wasm.
# Stamp the binary with the web app version (single source of truth =
# web/package.json) and mirror that stamp into a `.kernel-version` sidecar.
# The boot-time staleness guard compares the loaded module's stamp against
# this sidecar — i.e. against the kernel artifact actually on disk — NOT
# against the live package.json. That way a cosmetic version bump without a
# rebuild never trips the guard, while a genuinely stale worker still does.
build-wasm:
    #!/usr/bin/env bash
    set -euo pipefail
    V="$(node -p "require('./web/package.json').version")"
    cd crates/kernel && FIELDDRAFT_VERSION="$V" wasm-pack build --target web --out-dir ../../web/src/wasm --out-name fielddraft_kernel
    printf '%s' "$V" > ../../web/src/wasm/.kernel-version

# Start ONLY the Vite dev server (WebGPU-capable Chromium recommended).
# NOTE: /api (auth, projects, TICKETS) proxies to the Go server on
# :${FIELDDRAFT_SERVER_PORT:-8080}. Without a backend (use `just dev-full`)
# there is no ticket store — tickets live in IssueVault via the Go server.
dev: build-wasm
    @echo "▶ ${FIELDDRAFT_INSTANCE:-fielddraft} dev server on port ${FIELDDRAFT_WEB_PORT:-5173}"
    cd web && {{nvm_use}} && npm run dev -- --host 0.0.0.0

# Backend only, with LIVE-RELOAD: db + air-server + mesher, then follow the
# server log. air rebuilds & restarts the Go server on every change under
# server/*.go (docker-compose.air.yml). Use this for backend-only work;
# `just dev-full` is the same live-reloading server plus vite.
dev-air:
    docker compose -f docker-compose.yml -f docker-compose.air.yml up -d db server mesher
    docker compose logs -f server

# Full local stack with a LIVE-RELOADING Go server: MariaDB + air-server +
# mesher (docker compose) + Vite. The Go server rebuilds & restarts ITSELF on
# every server/*.go change (air) — no more stale hand-built containers serving
# old code. Seeds admin@local / admin on a fresh DB; vite proxies /api to the
# server (web/vite.config.ts). Stop the backend with `docker compose down`.
# The first start is slower (air pulls its toolchain + cold build); later
# rebuilds take seconds.
dev-full: build-wasm
    #!/usr/bin/env bash
    set -euo pipefail
    echo "▶ bringing up db + air-server (live-reload) + mesher…"
    docker compose -f docker-compose.yml -f docker-compose.air.yml up -d db server mesher
    echo "▶ waiting for the Go server on :${FIELDDRAFT_SERVER_PORT:-8080} (first air build is slow)…"
    # Any HTTP response (even 401/404) means the server is up and migrations ran.
    for i in $(seq 1 120); do
        if curl -s -o /dev/null "http://localhost:${FIELDDRAFT_SERVER_PORT:-8080}/api/me"; then break; fi
        sleep 1
    done
    echo "▶ login: admin@local / admin — vite on port ${FIELDDRAFT_WEB_PORT:-5173}"
    cd web && {{nvm_use}} && npm run dev -- --host 0.0.0.0

# Build the full web bundle (WASM + Vite).
build: build-wasm
    cd web && {{nvm_use}} && npm run build

# Build the PROD Docker images and pack them into a single archive for the
# archive-deploy path (devops IaC: `deploy.yml -e only_app=fielddraft`). Tags
# both images at web/package.json's version and saves a gzipped tar under dist/.
# See RELEASES.md. Version is the single source of truth = web/package.json.
release-image:
    #!/usr/bin/env bash
    set -euo pipefail
    V="$(node -p "require('./web/package.json').version")"
    echo "▶ building fielddraft/{server,web}:$V"
    # Context is the REPO ROOT, not server/: the Go module `replace`s the AI-chat
    # proxy to the submodule under third_party/ (ticket #347), and a path outside
    # the build context does not exist inside the builder. Check out with
    # --recurse-submodules, or this build has nothing to compile against.
    docker build -t "fielddraft/server:$V" -f server/Dockerfile .
    docker build -t "fielddraft/web:$V"    -f web/Dockerfile.prod .
    mkdir -p dist
    OUT="dist/fielddraft-$V.image.tar.gz"
    # Write to .partial, verify, THEN rename. A shell redirect creates the target
    # BEFORE `docker save` runs, so a failed save leaves a plausible-looking
    # 20-byte empty gzip behind — one of those very nearly went to the deploy as
    # a "release". An archive under dist/ must be loadable, or not exist.
    TMP="$OUT.partial"
    trap 'rm -f "$TMP"' EXIT
    echo "▶ saving $OUT"
    docker save "fielddraft/server:$V" "fielddraft/web:$V" | gzip > "$TMP"
    tar -tzf "$TMP" manifest.json > /dev/null   # loadable archive, or bail
    mv "$TMP" "$OUT"
    echo "✔ $OUT ($(du -h "$OUT" | cut -f1)) — ship to the server, then devops runs deploy.yml -e only_app=fielddraft"

# Run the fast local test suite: Rust unit tests + Vitest.
# Two suites are intentionally *not* included here, both meant to run
# on demand at integration points rather than on every feature change:
#   * Playwright smoke tests (`just test-e2e`) — headed Chromium, real WebGPU.
#   * The slow kernel-acceptance suite (`just test-slow`) — drives the real
#     wasm kernel end-to-end (Onshape import), tens of seconds per file.
test: test-rust test-web

test-rust:
    cargo test --workspace

test-web: build-wasm
    cd web && {{nvm_use}} && npm run test

# Slow acceptance suite (`*.slow.test.ts`): the real wasm kernel driven
# end-to-end. Run it after kernel/query/import changes.
test-slow: build-wasm
    cd web && {{nvm_use}} && npm run test:slow

# E2E (Playwright, headed Chromium + real WebGPU). Catches
# integration-level regressions that Vitest can't — material /
# pipeline incompatibilities, drawIndexed crashes, pointer-event
# routing through the canvas, etc. Requires `just install-e2e`
# once to fetch the Chromium binary.
test-e2e: build-wasm
    cd web && {{nvm_use}} && npm run e2e

install-e2e:
    cd web && {{nvm_use}} && npm run e2e:install

# Format everything.
fmt:
    cargo fmt --all
    cd web && npx prettier --write "src/**/*.{ts,tsx,css}" "tests/**/*.ts"

# Clean all build artifacts.
clean:
    cargo clean
    rm -rf web/src/wasm web/dist web/node_modules/.vite
