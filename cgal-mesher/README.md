# cgal-mesher — nativer Server-Mesher (CGAL Mesh_3)

Serverseitiges Export-Meshing (Plan-SSOT: `docs/CGAL-SERVER-MESHING-PLAN.md`).
CGAL `Mesh_3` (Restricted-Delaunay-Refinement) mesht das analytische SDF als
implizite Domain; die B-Rep-Kanten kommen als geschützte 1D-Polyline-Features
(Schutzkugeln) hinein — Ergebnis: wasserdicht, 2-manifold, kanten-/eckentreu.

Historie: entstand als Wasm/Emscripten-Spike (Branch `feat/cgal-mesher`,
verworfen als Client-Integration am 2026-07-07); `mesher.cpp` und
`sdf_interp.h` sind daraus übernommen und nativ umgebaut. Das
Data-Shipping-Modell bleibt: der Rust-Kernel kompiliert die CSG zu einem
flachen f32-Opcode-Buffer, C++ evaluiert die SDF autonom über einen
1:1-Interpreter-Port — kein per-Sample-FFI.

## Bausteine

- `src/sdf_interp.h` — **1:1-Port des Rust-Opcode-Interpreters**
  (`crates/kernel/src/sdf_compiler.rs::eval_opcodes`). Muss bit-nah synchron
  bleiben; Drift wird vom Golden-Test gefangen (siehe unten). Bei JEDER
  Opcode-Änderung im Rust-Kernel hier nachziehen.
- `src/mesher.cpp` — CGAL-Kern: `fd_reset()` / `fd_add_edge(polyline)` /
  `fd_mesh(opcodes, aabb, resolution, facet_dist_mult)`; Kriterien
  curvature-getrieben (`facet_distance` treibt, `facet_size` locker),
  `params::manifold()`, Feature-Protection via
  `Mesh_domain_with_polyline_features_3::add_features`.
- `src/cli.cpp` — CLI `fd-cgal-mesh <job.fdmesh> <out.stl>`: parst das
  Interchange-File von `fd-prepare` (bit-exakte f32-Hex), mesht jeden Body,
  richtet die Orientierung nach außen (signiertes Volumen) und schreibt EIN
  binäres STL; Report-JSON auf stderr.
- `crates/kernel/src/bin/fd_prepare.rs` + `native_prepare.rs` — die
  Rust-Vorstufe: `document.json` → Opcodes + AABB + B-Rep-Kanten-Polylines.
- `serve/main.go` — minimaler HTTP-Wrapper (M2): `POST /mesh` mit
  `{document, params}` → fd-prepare → fd-cgal-mesh → binäres STL zurück
  (Report-JSON im `X-Fd-Report`-Header), `GET /healthz`. Kein Auth (nur
  internes Compose-Netz); Limits: `FD_MESH_CONCURRENCY` (default 2),
  `FD_MESH_TIMEOUT_SEC` (300), `FD_MESH_MAX_BODY_MB` (50).

## Pipeline lokal

    cargo build -p fielddraft-kernel --bin fd-prepare
    ./cgal-mesher/build-native.sh
    ./target/debug/fd-prepare part.json '{"cellSizeMm":0.5,"facetDistMult":0.15}' > job.fdmesh
    ./cgal-mesher/build/fd-cgal-mesh job.fdmesh out.stl
    python3 cgal-mesher/test/verify_stl.py job.fdmesh out.stl   # M1-Prüfung

`verify_stl.py` prüft: jede Kante exakt 2× (wasserdicht/manifold),
konsistente Orientierung + positives Volumen, und jeden geschützten
Kanten-Punkt < 1,5·cell an einem Mesh-Vertex (Kantentreue).

## Golden-Test (Interpreter-Sync, Pflicht in CI)

    cargo run -p fielddraft-kernel --example gen_golden > /tmp/fd_golden.txt
    g++ -O2 -std=c++17 -I cgal-mesher/src -o /tmp/check_interp cgal-mesher/test/check_interp.cpp
    /tmp/check_interp /tmp/fd_golden.txt

Hat beim Nativ-Umbau sofort zwei echte Drifts gefunden (Cone `wall_scale`,
Shell inward statt symmetrisch) — nicht optional.

## Tape-Spezialisierung (tape_spec.h)

Zell-lokale Verkürzung der Opcode-Tape (C++-Gegenstück zu
`crates/kernel/src/cell_csg.rs`): Octree über dem Mesh-AABB, pro Zelle
werden Boolean-Zweige per 1-Lipschitz-Ball-Bound (ein Center-Eval pro
Subtree) geprunt, die kurze Tape wird an Kindzellen vererbt und vom
Mesh_3-Oracle evaluiert. Warp-Klammern (Extrude/Revolve/Helix/Sweep)
bleiben opake Leaf-Spans — nur erkannte Boolean-Knoten werden geprunt,
unbekannte Tape-Formen fallen komplett auf die volle Tape zurück.
`FD_NO_SPEC=1` schaltet ab. Wert-Exaktheits-Harness:

    g++ -O2 -std=c++17 -I src -o /tmp/check_spec test/check_spec.cpp
    /tmp/check_spec job.fdmesh   # vergleicht voll vs. spezialisiert

Benchmark 512-Kugel-Baugruppe (cell 0,8): 36,1s → 16,3s wall,
466s → 117s CPU; Tape 6656 → Ø 30 Records pro Zelle.

## Build-Varianten

- `build-native.sh` (M1, lokal): header-only — CGAL-6.0.1- + Boost-1.86-
  Header (default `~/cgal-wasm-spike/`, via `CGAL_DIR`/`BOOST_DIR`
  übersteuerbar), `-DCGAL_DISABLE_GMP -DCGAL_USE_BOOST_MP`,
  `Sequential_tag`. Keine Systempakete nötig, aber langsame exakte
  Arithmetik.
- `Dockerfile` (M2, Kontext = Repo-Root): Debian-trixie-CGAL + GMP/MPFR +
  TBB (`Parallel_tag`), Thread-Deckel pro Job via `FD_MESH_THREADS`
  (tbb::global_control, 0 = alle Kerne). Der Golden-Test läuft als
  Pflicht-Schritt IM Image-Build — Interpreter-Drift bricht den Build.
  Compose-Service `mesher` (docker-compose.yml, internes Netz, kein Port):

      docker compose up --build -d db server mesher
      # Smoke-Test aus dem server-Container heraus:
      # POST http://mesher:9090/mesh {"document":…, "params":{…}} → STL

## Bekannte Grenzen (Plan §4)

- Gyroid/TPMS: implizites Initial-Seeding verfehlt die periodische
  Mehrblatt-Fläche (0 Dreiecke) — braucht eigenen Seeding-Schritt.
- Baked Volumes (`OP_PRIM_TEX3D`): `fd-prepare` lehnt solche Bodies ab.
- Boost.MP-Build ist deutlich langsamer als GMP — Produktionslaufzeiten
  erst mit dem Docker-Build bewerten.
