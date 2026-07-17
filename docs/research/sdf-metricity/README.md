# Forschungsprogramm: SDF-Distanzerhaltung unter Modellieroperationen

Auftrag des Betreibers (2026-07-17), Ticket #384. Ziel: belastbare Antwort auf die
Frage, wie FieldDrafts Distanzfelder unter Modellieroperationen — allen voran den
Booleschen — ihre metrische Treue behalten oder zurückgewinnen können.

## Warum

Fast alles stromabwärts verlässt sich darauf, dass das Feld (annähernd) echte
Distanz liefert: Sphere-Trace-Schrittweiten, Offset/Shell, Rundungen, robustes
Meshing, analytische Abfragen. min/max-Booleans liefern aber nur Schranken, und
mit jeder Verschachtelungsebene degradiert die Treue weiter. Skalierung und
Fillets gelten als beherrschbar; die Distanzerhaltung ist das offene Kernproblem.

## Bekannte Ausgangslage (Codebasis-Fakten, Stand v0.16.0)

- Harte Booleans (`hard_combine`, min/max in `crates/kernel/src/csg.rs`):
  1-Lipschitz, aber nur Distanz-Schranke; nahe konkaven Nähten über-/unterschätzt.
- `blend_min`/`smin_poly` (MaskedBlend, SmoothUnion): zirkuläres smin, inhärent
  √2-Lipschitz. Nur der Render-Pfad normalisiert (`masked_blend_render_fade`,
  ÷√2 global); der Mesher-/Logical-Pfad (`EvalMode::Visual`) ist unnormalisiert.
- Weitere Lipschitz-Brecher mit Ad-hoc-Dämpfung: `Displace` (L≈1.8),
  `Revolve` (`REVOLVE_MARCH_RELAX 0.7`), Helix/Gyroid; Mechanismus `march_relax`
  in `sdf_compiler.rs`.
- `Offset` (= `d − δ`) und `Shell` (= `max(d, −(d+t))`) sind nur korrekt, wenn
  `d` echte euklidische Distanz ist — beim √2-Feld liegt die Isofläche `d = δ`
  real bei ≈ δ/√2.
- Evaluationspipeline: Rust-Kernel kompiliert Bytecode-Tape
  (`sdf_compiler.rs`), gespiegelt als WGSL/TSL-Interpreter
  (`web/src/sdfVmInterpreter.ts`); CPU-Referenz `eval_opcodes`.

## Struktur

- `agent-a-pde/` — Level-Set/PDE-Sicht: Eikonal, Redistancing (Fast Marching /
  Fast Sweeping), Viskositätslösungen, Übertragbarkeit auf analytischen
  GPU-Kernel.
- `agent-b-graphics/` — Grafik/SDF-Literatur: exakte/korrigierte Booleans,
  smin-Varianten und ihre Distanzfehler, Lipschitz-Schranken, libfive/Fidget,
  Intervall-/affine Arithmetik.
- `agent-c-hybrid/` — Hybrid-Sicht: Distanzabfragen gegen exakte
  Randrepräsentation (BREP-Proxy, Branch-Pair-Strukturen) als Korrektur- oder
  Ersatzpfad im Nahfeld.
- `agent-d-numerics/` — Numerische Sicht: Gradientennorm-Korrekturen,
  Normalisierungsoperatoren, Fehlerfortpflanzung über Verschachtelungstiefe.
- `HARNESS.md` + `crates/metricity-lab/` — gemeinsames Experiment-Harness:
  Ground-Truth-Distanz, Metriken, adversarialer Testkorpus.

Jeder Agent führt `LOG.md` in seinem Verzeichnis: Quellen, Hypothese,
Experimentaufbau, Messergebnisse, Schlussfolgerung — pro Iteration. Negative
Ergebnisse werden genauso sorgfältig dokumentiert wie Erfolge.

## Pflicht-Metriken für jedes Experiment

1. max./mittlerer Distanzfehler |f − d_wahr|, besonders im Nahfeld.
2. Abweichung |∇f| von 1; Gradientenkontinuität an Nahtstellen.
3. Fehlerentwicklung über CSG-Verschachtelungstiefe.
4. Rechenkosten + Eignung für die Pipeline (WGSL-Interpreter/Bytecode/Tape).

Testkorpus (adversarial): dünne Wände, nahezu tangentiale Boolesche Schnitte,
tiefe CSG-Bäume, Kombinationen mit Domain-Warps (Twist/Bend, Gyroid).

## Vorarbeit

Opus 4.8 hat in Ticket #384 Hypothesen gesammelt (Voxel-Fast-Sweeping on-demand
via `api/voxel.rs`, PDE-Reinit verworfen, analytische lokale Schranken für den
Render-Pfad). Diese gelten als zu prüfende Hypothesen, nicht als Antworten.
