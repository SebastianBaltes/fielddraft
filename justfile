# FieldDraft developer command surface.
# All targets are safe to run from a clean checkout; nothing here talks to a
# remote, a database, or CI — M0 is strictly local.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Default: print the available recipes.
default:
    @just --list

# Install web dependencies.
install:
    cd web && npm install

# Build the Rust kernel to WASM and drop the output into web/src/wasm.
# Stamp the binary with the web app version (single source of truth =
# web/package.json) so the app can detect a stale kernel module on boot.
build-wasm:
    cd crates/kernel && FIELDDRAFT_VERSION="$(node -p "require('../../web/package.json').version")" wasm-pack build --target web --out-dir ../../web/src/wasm --out-name fielddraft_kernel

# Start the Vite dev server (WebGPU-capable Chromium recommended).
dev: build-wasm
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
