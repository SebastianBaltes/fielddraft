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

# Start the Vite dev server (WebGPU-capable Chromium recommended).
dev: build-wasm
    @echo "▶ ${FIELDDRAFT_INSTANCE:-fielddraft} dev server on port ${FIELDDRAFT_WEB_PORT:-5173}"
    cd web && {{nvm_use}} && npm run dev

# Full local stack: MariaDB + Go server (docker compose) + Vite dev server.
# Brings up db+server in the background (seeds admin@local / admin on a fresh
# DB), waits for the server to be healthy, then runs vite — which proxies /api
# to the local Go server (see web/vite.config.ts). Gives a working local login.
# Stop the backend afterwards with `docker compose down`.
dev-full: build-wasm
    #!/usr/bin/env bash
    set -euo pipefail
    echo "▶ bringing up db + server (docker compose)…"
    docker compose up --build -d db server
    echo "▶ waiting for the Go server on :${FIELDDRAFT_SERVER_PORT:-8080}…"
    # Any HTTP response (even 401/404) means the server is up and migrations ran.
    for i in $(seq 1 60); do
        if curl -s -o /dev/null "http://localhost:${FIELDDRAFT_SERVER_PORT:-8080}/api/me"; then break; fi
        sleep 1
    done
    echo "▶ login: admin@local / admin — vite on port ${FIELDDRAFT_WEB_PORT:-5173}"
    cd web && {{nvm_use}} && npm run dev

# Build the full web bundle (WASM + Vite).
build: build-wasm
    cd web && {{nvm_use}} && npm run build

# Run the fast local test suite: Rust unit tests + Vitest.
# Playwright smoke tests are intentionally *not* included here —
# they launch a headed Chromium with real WebGPU and are meant to
# run on demand (`just test-e2e`) at integration points, not on
# every feature change.
test: test-rust test-web

test-rust:
    cargo test --workspace

test-web: build-wasm
    cd web && {{nvm_use}} && npm run test

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
