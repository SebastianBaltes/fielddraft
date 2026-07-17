//! CLI der Messbank.
//!
//! ```text
//! cargo run -p metricity-lab --release -- list
//! cargo run -p metricity-lab --release -- all [--out DIR] [--quick]
//! cargo run -p metricity-lab --release -- <szene> [--out DIR] [--grid N] [--points N] [--seed S] [--no-bench]
//! ```
//!
//! Default-Ausgabeverzeichnis: `docs/research/sdf-metricity/results/`
//! (relativ zum Repo, aus `CARGO_MANIFEST_DIR` abgeleitet); `all`
//! schreibt nach `<out>/baseline/`.

use std::path::PathBuf;

use metricity_lab::report::{
    summary_row, sweep_row, write_csv, write_json, SUMMARY_HEADER, SWEEP_HEADER,
};
use metricity_lab::runner::{run_scene, RunConfig};
use metricity_lab::scenes;

fn default_out() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/research/sdf-metricity/results")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage_and_exit();
    }
    let cmd = args[0].clone();
    let mut out = default_out();
    let mut cfg = RunConfig::default();
    let mut quick = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                out = PathBuf::from(args.get(i + 1).unwrap_or_else(|| usage_and_exit()));
                i += 2;
            }
            "--grid" => {
                cfg.grid = parse(args.get(i + 1));
                i += 2;
            }
            "--points" => {
                let n: usize = parse(args.get(i + 1));
                cfg.n_uniform = n / 2;
                cfg.n_near = n - n / 2;
                i += 2;
            }
            "--seed" => {
                cfg.seed = parse(args.get(i + 1));
                i += 2;
            }
            "--no-bench" => {
                cfg.bench_evals = 0;
                i += 1;
            }
            "--no-refine" => {
                cfg.refine = false;
                i += 1;
            }
            "--quick" => {
                quick = true;
                i += 1;
            }
            other => {
                eprintln!("unknown flag: {other}");
                usage_and_exit();
            }
        }
    }
    if quick {
        cfg.n_uniform = 1500;
        cfg.n_near = 1500;
        cfg.bench_evals = cfg.bench_evals.min(200_000);
    }

    match cmd.as_str() {
        "list" => {
            for s in scenes::corpus() {
                println!("{:26} {}", s.name, s.description);
            }
            println!("{:26} parametrische Familie (N in {:?})", "deep_csg_<N>", scenes::DEEP_CSG_SWEEP);
            println!("{:26} parametrische Familie (N in {:?})", "smin_chain_<N>", scenes::SMIN_CHAIN_SWEEP);
        }
        "all" => run_all(&out.join("baseline"), &cfg),
        name => match scenes::scene_by_name(name) {
            Some(scene) => {
                let r = run_scene(&scene, &cfg);
                let path = out.join("scenes").join(format!("{}.json", scene.name));
                write_json(&path, &r).expect("write scene json");
                println!("{}", summary_row(&r));
                println!("→ {}", path.display());
            }
            None => {
                eprintln!("unknown scene: {name} (see `list`)");
                std::process::exit(2);
            }
        },
    }
}

fn run_all(out: &std::path::Path, cfg: &RunConfig) {
    let mut summary: Vec<String> = Vec::new();

    for scene in scenes::corpus() {
        eprintln!("[metricity-lab] scene {} …", scene.name);
        let t = std::time::Instant::now();
        let r = run_scene(&scene, cfg);
        eprintln!(
            "[metricity-lab]   {:.1}s  gt_pts={}  near_out_p95={:.4}mm",
            t.elapsed().as_secs_f64(),
            r.gt.n_surface_points,
            r.distance.near_out.p95
        );
        write_json(&out.join("scenes").join(format!("{}.json", r.name)), &r)
            .expect("write scene json");
        summary.push(summary_row(&r));
    }
    write_csv(&out.join("summary.csv"), SUMMARY_HEADER, &summary).expect("write summary");

    // Sweeps: Fehler über CSG-Tiefe (harte Booleans) und smin-Kettenlänge.
    let sweep_cfg = RunConfig {
        n_uniform: cfg.n_uniform.min(3000),
        n_near: cfg.n_near.min(3000),
        bench_evals: cfg.bench_evals.min(300_000),
        ..cfg.clone()
    };
    for (file, family, depths) in [
        (
            "sweep_hard_depth.csv",
            &scenes::deep_csg as &dyn Fn(usize) -> scenes::Scene,
            &scenes::DEEP_CSG_SWEEP[..],
        ),
        (
            "sweep_smin_chain.csv",
            &scenes::smin_chain as &dyn Fn(usize) -> scenes::Scene,
            &scenes::SMIN_CHAIN_SWEEP[..],
        ),
    ] {
        let mut rows = Vec::new();
        for &n in depths {
            let scene = family(n);
            eprintln!("[metricity-lab] sweep {} …", scene.name);
            let r = run_scene(&scene, &sweep_cfg);
            rows.push(sweep_row(n, &r));
        }
        write_csv(&out.join(file), SWEEP_HEADER, &rows).expect("write sweep");
    }
    eprintln!("[metricity-lab] done → {}", out.display());
}

fn parse<T: std::str::FromStr>(v: Option<&String>) -> T {
    v.and_then(|s| s.parse().ok()).unwrap_or_else(|| usage_and_exit())
}

fn usage_and_exit() -> ! {
    eprintln!(
        "usage: metricity-lab <list|all|SCENE> [--out DIR] [--grid N] [--points N] [--seed S] [--no-bench] [--no-refine] [--quick]"
    );
    std::process::exit(2);
}
