//! fd-prepare — native Vorstufe des serverseitigen CGAL-Meshings
//! (docs/CGAL-SERVER-MESHING-PLAN.md, P2).
//!
//! Aufruf:  fd-prepare <document.json> [params-json-oder-@datei]
//! Ausgabe: das Interchange-File (stdout); Fehler auf stderr, Exit ≠ 0.
//!
//! Beispiel: fd-prepare part.json '{"cellSizeMm":0.5}' > job.fdmesh

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || args.len() > 3 {
        eprintln!("usage: fd-prepare <document.json> [params-json-or-@file]");
        return ExitCode::from(2);
    }
    let document = match std::fs::read_to_string(&args[1]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("fd-prepare: cannot read {}: {e}", args[1]);
            return ExitCode::from(2);
        }
    };
    let params = match args.get(2).map(String::as_str) {
        None => "{}".to_string(),
        Some(p) if p.starts_with('@') => match std::fs::read_to_string(&p[1..]) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("fd-prepare: cannot read {}: {e}", &p[1..]);
                return ExitCode::from(2);
            }
        },
        Some(p) => p.to_string(),
    };
    match fielddraft_kernel::native_prepare::prepare_interchange(&document, &params) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("fd-prepare: {e}");
            ExitCode::FAILURE
        }
    }
}
