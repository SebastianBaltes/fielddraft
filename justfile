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

# --- Tests -------------------------------------------------------------
#
# Die Suite ist GESTUFT, weil ihre Kosten extrem ungleich verteilt sind:
# 89 % der Rust-Test-CPU stecken in 103 von 1444 Tests (Lichen-Vermascher,
# Mesh-Export, Metricity-Forschung). Welcher Test in welche Stufe gehört
# und warum, steht in `.config/nextest.toml` — das ist die SSOT der
# Aufteilung, nicht dieses Justfile.
#
#   just test        Rust-Kern + Vitest         12 s    ← Edit-Schleife
#   just test-mesh   Vermascher                 88 s    ← nach Mesher-Arbeit
#   just test-offset Offset + Metricity         25 s    ← nach Offset-Arbeit
#   just test-full   alles, beide Seiten        ~4 min  ← Gate vor dem Commit
#   just test-deep   + Diagnose-Harnische       ~20 min ← selten, auf Verdacht
#   just test-slow   wasm-Abnahme (Onshape)     ~2 min  ← nach Import-Arbeit
#   just test-e2e    Playwright, echtes WebGPU          ← vor dem Deploy
#
# Gemessen am 2026-07-27 auf 24 Kernen. Vorher lag `just test` bei 36 min —
# nicht wegen der Menge der Tests, sondern weil sie unoptimiert gebaut
# wurden (siehe `[profile.test]` in Cargo.toml) und `cargo test` die
# Test-Binaries seriell fährt.

# Bewusst NICHT dabei: test-slow (echter wasm-Kernel) und test-e2e (headed
# Chromium) — Integrationspunkte, keine Edit-Schleife.
#
# Standard-Suite für die Edit-Schleife: Rust-Kern + Vitest, ~12 s.
test: test-rust test-web

# Schnelle Rust-Suite: alles außer Lichen, Mesh-Export und Metricity.
test-rust: _nextest
    cargo nextest run --workspace

# Thematisch: Vermascher — Lichen, Dual-Contouring, Export-Route.
test-mesh: _nextest
    cargo nextest run --workspace --profile mesh

# Thematisch: Offset, Shell, Rim und die Metricity-Forschung (#384).
test-offset: _nextest
    cargo nextest run --workspace --profile offset

# Voll: jeder Rust-Test plus beide Web-Suiten. Gate vor einem Kern-Commit.
test-full: _nextest build-wasm
    cargo nextest run --workspace --profile full
    cd web && {{nvm_use}} && npm run test
    cd web && {{nvm_use}} && npm run test:slow

# Die 27 `#[ignore]`-Diagnose-Harnische: Gyroid-Baselines, Reach-Proben,
# Timing-Profile, `zz_*`-Untersuchungen. Das sind instrumentierte Befunde,
# keine Regressionstests — ein roter Lauf hier ist ein Messergebnis, kein
# Regress (zwei sind ausdrücklich `_wip` und bekanntermaßen rot). Einen
# einzelnen Harnisch mit Ausgabe fahren:
#   cargo nextest run --profile deep --run-ignored ignored-only \
#     -E 'test(zz_gyroid_baseline)' --no-capture
#
# Zusätzlich zur vollen Suite die Diagnose-Harnische fahren.
test-deep: _nextest
    cargo nextest run --workspace --profile deep --run-ignored all

# cargo-nextest ist der Runner: er fährt alle Test-Binaries in EINEM
# Parallel-Pool (cargo test fährt sie seriell — allein dadurch blockierte
# das Kernel-Binary die Tests von metricity-lab), misst jeden Test einzeln
# und markiert neue Ausreißer als SLOW. `cargo test --workspace` funktioniert
# unverändert weiter, ist aber die langsamere Variante.
#
# nextest bereitstellen (vorgebautes Binary, kein Compile).
_nextest:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v cargo-nextest >/dev/null && exit 0
    echo "▶ cargo-nextest fehlt — installiere es nach ~/.cargo/bin"
    curl -sSLf https://get.nexte.st/latest/linux | tar -xz -C "${CARGO_HOME:-$HOME/.cargo}/bin"

# Nur die schnellen Vitest-Tests (ohne `*.slow.test.ts`).
test-web: _wasm-present
    cd web && {{nvm_use}} && npm run test

# Die schnellen Vitest-Tests laden den wasm-Kernel NIE — das tun per
# Konvention nur die `*.slow.test.ts` (siehe web/vitest.config.ts). Sie
# brauchen die Dateien in web/src/wasm trotzdem, weil `src/kernel.ts` sie
# importiert und Vite das Modul auflösen muss. Ein FRISCHER Build ist dafür
# aber unnötig, und er kostete 53 s von 75 s Gesamtlaufzeit — mehr als alle
# 3346 Tests zusammen (wasm-bindgen + wasm-opt laufen auch bei unveränderten
# Quellen). Deshalb hier nur: bauen, falls nicht da.
#
# ACHTUNG, bewusster Handel: dadurch prüft `just test` nicht mehr, ob der
# Kern fürs wasm-Target überhaupt kompiliert (target-gated Code kann so
# durchrutschen). `test-slow`, `test-e2e` und `test-full` hängen weiterhin
# hart an `build-wasm` — dort ist Frische Pflicht, nicht Komfort.
_wasm-present:
    #!/usr/bin/env bash
    set -euo pipefail
    [ -f web/src/wasm/fielddraft_kernel_bg.wasm ] && exit 0
    echo "▶ web/src/wasm fehlt — baue den Kernel einmalig"
    just build-wasm

# Die `*.slow.test.ts` treiben den echten wasm-Kernel end-to-end; erzwingt
# deshalb einen frischen Build.
#
# wasm-Abnahme: echter Kernel end-to-end (Onshape-Import), nach Kern-Arbeit.
test-slow: build-wasm
    cd web && {{nvm_use}} && npm run test:slow

# E2E (Playwright, headed Chromium + real WebGPU). Catches
# integration-level regressions that Vitest can't — material /
# pipeline incompatibilities, drawIndexed crashes, pointer-event
# routing through the canvas, etc. Requires `just install-e2e`
# once to fetch the Chromium binary.
#
# E2E: Playwright, headed Chromium mit echtem WebGPU.
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
