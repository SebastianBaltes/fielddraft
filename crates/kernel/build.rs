// Stamp the kernel binary with the *web* app version so the running
// app can detect a stale WASM module (e.g. a long-open tab whose
// worker still holds a binary built before the last `just build-wasm`).
//
// `just build-wasm` exports `FIELDDRAFT_VERSION` from web/package.json —
// the single source of truth, the same value vite injects as
// `__APP_VERSION__`. A plain `cargo build` without the env (unit tests,
// editor checks) falls back to "dev", which never matches a real app
// version and therefore reads as "unstamped" rather than a false match.
fn main() {
    println!("cargo:rerun-if-env-changed=FIELDDRAFT_VERSION");
    let version = std::env::var("FIELDDRAFT_VERSION").unwrap_or_else(|_| "dev".to_string());
    println!("cargo:rustc-env=FIELDDRAFT_VERSION={version}");
}
