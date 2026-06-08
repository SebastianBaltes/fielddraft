# FieldDraft developer command surface.
# All targets are safe to run from a clean checkout; nothing here talks to a
# remote, a database, or CI — M0 is strictly local.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Load per-instance config (ports, instance name) from a gitignored .env so
# that `fielddraft` and `fielddraft-a` can run side by side. Optional: a clean
# checkout with no .env just falls back to the in-file/Vite defaults.
# See .env.example.
set dotenv-load := true

# Default: print the available recipes.
default:
    @just --list

# Install web dependencies.
install:
    cd web && npm install

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
    cd web && npm run dev

# Build the full web bundle (WASM + Vite).
build: build-wasm
    cd web && npm run build

# Run the fast local test suite: Rust unit tests + Vitest.
# Playwright smoke tests are intentionally *not* included here —
# they launch a headed Chromium with real WebGPU and are meant to
# run on demand (`just test-e2e`) at integration points, not on
# every feature change.
test: test-rust test-web

test-rust:
    cargo test --workspace

test-web: build-wasm
    cd web && npm run test

# E2E (Playwright, headed Chromium + real WebGPU). Catches
# integration-level regressions that Vitest can't — material /
# pipeline incompatibilities, drawIndexed crashes, pointer-event
# routing through the canvas, etc. Requires `just install-e2e`
# once to fetch the Chromium binary.
test-e2e: build-wasm
    cd web && npm run e2e

install-e2e:
    cd web && npm run e2e:install

# Format everything.
fmt:
    cargo fmt --all
    cd web && npx prettier --write "src/**/*.{ts,tsx,css}" "tests/**/*.ts"

# Clean all build artifacts.
clean:
    cargo clean
    rm -rf web/src/wasm web/dist web/node_modules/.vite
