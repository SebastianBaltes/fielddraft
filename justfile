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
build-wasm:
    cd crates/kernel && wasm-pack build --target web --out-dir ../../web/src/wasm --out-name fielddraft_kernel

# Start the Vite dev server (WebGPU-capable Chromium recommended).
dev: build-wasm
    cd web && npm run dev

# Build the full web bundle (WASM + Vite).
build: build-wasm
    cd web && npm run build

# Run all local tests: Rust unit tests, Vitest, Playwright smoke.
test: test-rust test-web

test-rust:
    cargo test --workspace

test-web: build-wasm
    cd web && npm run test

# E2E (Playwright) — requires `just install-e2e` once.
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
