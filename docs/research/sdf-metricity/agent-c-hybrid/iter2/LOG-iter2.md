# LOG — Agent C (Hybrid), Iteration 2: B-Rep-Proxy gegen ECHTE Kernel-Artefakte

Experiment 2 der gemeinsamen Iteration-2-Charta (SYNTHESIS §4.2). Kernfrage
(Haupt-Implementierungsrisiko aus Iteration 1, LOG.md „Wo der Ansatz bricht"):
**Erscheinen Edge-Fillet-Zonen im Weave-Output als gerundete Faces, sodass die
Proxy-Distanz auch dort exakt ist?**

Harness: `crates/metricity-lab` (Ground Truth validiert ≤ 2.4e-4 rel.,
HARNESS.md). Experiment-Code: `weave_proxy.patch` in diesem Verzeichnis
(neues Binary `crates/metricity-lab/src/bin/weave_proxy.rs`); Lauf
`cargo run -p metricity-lab --release --bin weave_proxy -- <outdir>`.
Seeds: uniform 42, near-surface 43, Gradienten 44, Fillet-Band 45.
Punktmengen je Szene: 3000 uniform + 3000 near (±5 % Modellgröße),
Fillet-Band zusätzlich 2500 Punkte in der Kapsel r ≤ 2.5·r_f um die
maskierte Kante. Domain je Szene um 20 % der größten AABB-Kante gepolstert
(sonst liegen die Phantom-Regionen aus Iteration 1 außerhalb der Messung).
Daten: `summary.csv`, `fillet_band.csv`, `blend_rule.csv`, `weave_facts.json`.

## Quellen (Codebasis-Grounding, diese Iteration)

- `crates/kernel/src/body.rs`: `weave_all` → `Body::ensure_brep` →
  `build_bounded_for_body_local(features, indices, placement)` — diese
  Funktion hat **keinen `edge_fillets`-Parameter**. Nur die Mesh-/GPU-SDF-
  Pfade (`build_bounded_*_rounded`, #350) wenden Fillets an. Der
  **Produktions-Weave webt also den UNGEFILLETETEN Körper** — strukturell,
  nicht als Bug: `export_mesh.rs::check_analytically_exact` gate't Bodies
  mit Fillets explizit auf die generische MDC-Route („Blends, warps and
  edge fillets shape the SDF but not the woven faces").
- `crates/kernel/src/brep/tessellate.rs::tessellate_face`:
  `HalfSpace::Blend { .. } => Err("procedural blend carrier")` — selbst wenn
  ein Weave die Blend-Face trägt, **verweigert `tessellate_brep` sie** (wie
  auch Gyroid-Carrier, getrimmte Torus-Patches, `Tessellated`-Faces).
- Synchroner Test-Weave-Weg (aus `features_tests.rs`, DL-61/c-M1):
  `weave_all` fürs Kanten-Query-Hash; gefilleteter Direkt-Weave via
  `BoundedCsg::from_csg(csg.apply_edge_fillets(…))` → `Octree::build` →
  `weaving::weave` — nur dieser Weg materialisiert die Blend-Face
  (`HalfSpace::Blend`, c-M1).
- Tape = `sdf_compiler::compile_body` + `eval_opcodes_with_volumes`
  (CPU-Referenz des GPU-Interpreters), inkl. #382-Render-Fade (÷√2).

## Hypothesen

- **H8** (das Risiko aus Iteration 1): Die Fillet-Zone erscheint NICHT im
  tesselierten Weave-Proxy; die Proxy-Distanz misst dort zur scharfen Kante,
  Fehler ≈ O(r_fillet).
- **H9**: Auf fillet-freien echten Weaves ist der Proxy-Fehler ≡ tol_mm
  (Iteration-1-E2, jetzt an echten Artefakten statt Hand-Meshes).
- **H10**: Die Blend-Regel „Band ≥ 4× Diskrepanz" aus E4 hält an echten
  Szenen (Marsch-Sicherheit + ∇-Spikes ~ Δ/Band).

## Aufbau (echte Kernel-API, keine Hand-Meshes)

Szenen über `FeatureRegistry::regenerate` (identity placement):

| Szene | Rezept | Charakter |
|---|---|---|
| `pocket` | Box he=10 − Box he=5 @ (0,0,10), Subtrahend ragt 5 mm über | Tasche: einspringende Kanten + Phantom-Flächen |
| `box_union_cyl` | Box he=10 ∪ Zylinder r=6, hz=14 @ (8,0,0) — durchstößt beide Deckel | konkave Nähte, durchstoßende Union |
| `box_boss_cyl` | Box he=10 ∪ Zylinder r=6, hz=8 @ (0,0,10) — Boss auf der Oberseite | konkave Kreis-Naht (c-M2-Bandtopologie) |
| `fillet_step` | `pocket` + `fillet_edge` r=3 auf der Feature-1×2-Rim-Kante (qEdge-Hash aus synchronem `weave_all`) | MaskedBlend im Feld |

Gemessene Felder je Szene: `field_visual` (`CsgNode::sdf`, Visual),
`tape` (`eval_opcodes`), `proxy_tess` (= `tessellate_brep(weave_all-BRep,
tol)`-Mesh + BVH aus Iteration 1, Vorzeichen aus dem Feld) bei tol ∈
{0.2, 0.05, 0.01}, sowie für Fillet-/Fehlschlag-Szenen `proxy_dc128/256`
(DC-Render-Mesh `dc::dual_contour` — das einzige Kernel-Artefakt, das die
Fillet-Geometrie trägt). GT auf der Nullmenge des Visual-Felds (Grid 96,
Fillet 128).

## Messwerte

### 1. Struktur-Befunde zur Kernfrage (weave_facts.json)

- `fillet_step`, Produktions-Weave (`weave_all`): **11 Faces, alle `plane`
  — KEINE Blend-Face.** Der Weave ist der scharfe Körper; die 28-Tri-
  Tesselation ist bei allen drei tol-Stufen identisch mit der der
  ungefilleteten `pocket`.
- `fillet_step`, Direkt-Weave des gefilleteten CSG (c-M1-Route): 12 Faces,
  **1 Blend-Face wird gewoben** — aber `tessellate_brep` scheitert:
  `face 3: loop is not a connected ring` (Weave emittiert zudem 4×
  „could not order face loops … unordered fallback loop"; der Step-Fillet
  hat Runout, anders als der manifold-saubere c-M2-Kreisfall). Selbst ohne
  diesen Loop-Fehler wäre die Blend-Face ein harter Stopp:
  `Err("procedural blend carrier")`.
- `box_union_cyl`: `tessellate_brep` scheitert bei ALLEN tol-Stufen —
  `face 6: unsupported winding configuration (outer -1, 2 inner)` (die
  Zylinderwand durchstößt beide Box-Deckel). **Abdeckungslücke der
  Tesselation auch ganz ohne Fillet.** Der Boss-Fall (eine geschlossene
  Naht) tesseliert dagegen sauber.

### 2. Distanzfehler |f − d_wahr| in mm (summary.csv, Auszug)

`near` = |d| < 2 % Modellgröße, `far` > 10 %; out/außen, in/innen.

| Szene | Variante | near_out max | near_in max | mid_in max | far_out max | far_in max | ns/Eval |
|---|---|---|---|---|---|---|---|
| pocket | field_visual = tape | 0.050 | 0.000017 | 0.00055 | **2.002** | 0.0006 | 26 |
| pocket | proxy_tess (alle tol) | **0.0014** | 0.000017 | 0.00055 | **0.013** | 0.0006 | 238 |
| box_union_cyl | field_visual = tape | 0.00008 | 0.196 | 0.966 | 0.004 | **2.189** | 34 |
| box_union_cyl | proxy_dc256 | 0.0064 | 0.0083 | 0.029 | 0.0062 | **0.036** | 13 419 |
| box_boss_cyl | field_visual = tape | 0.00009 | 0.094 | 0.991 | 0.003 | **3.546** | 74 |
| box_boss_cyl | proxy_tess tol 0.2 | 0.170 | 0.176 | 0.174 | 0.110 | 0.177 | 1 531 |
| box_boss_cyl | proxy_tess tol 0.05 | 0.044 | 0.046 | 0.047 | 0.027 | 0.123 | 2 625 |
| box_boss_cyl | proxy_tess tol 0.01 | **0.0076** | 0.0095 | 0.044 | 0.0075 | 0.123 | 4 501 |
| fillet_step | proxy_tess (sharp) | 0.85 | 0.85 | 0.046 | 0.0005 | 0.0004 | 1 217 |
| fillet_step | proxy_dc128 | 0.056 | 0.053 | 0.0069 | 0.0067 | 0.0069 | 8 025 |
| fillet_step | proxy_dc256 | 0.071 | 0.059 | 0.0059 | 0.0011 | 0.0011 | 8 675 |

(fillet_step-Feld/Tape-Zeilen: siehe summary.csv; die Fillet-Zone selbst
steht gesondert in §3. Kosten: naiver f64-Median-Split-BVH, 1 Thread —
Iteration-1-E3-Basis; SAH/f32/fcpw ≥5–10× Luft. Tris: pocket 28;
boss 458/2 614/34 038 bei tol 0.2/0.05/0.01; dc256 97k–136k.)

Kernbefunde:

- **H9 bestätigt, mit einem neuen Floor.** Auf tesselierbaren echten Weaves
  ist der Proxy-Nahfeld-Fehler ≈ Sehnenfehler: boss near_out max
  0.170/0.044/0.0076 mm bei tol 0.2/0.05/0.01 (Sehnenfehler ≲ tol, wie
  E2). ABER: far_in plateaut bei **0.123 mm für tol 0.05 UND 0.01** —
  tol-unabhängig. Der Boden kommt aus der **gewobenen Kantengeometrie**
  (Naht-Polyline des Weaves), nicht aus der Face-Tesselation: die
  Proxy-Genauigkeit ist `max(tol_mm, Weave-Kanten-Genauigkeit)`.
- **Phantom-Regionen real bestätigt** (Iteration-1-Mechanik an echter
  Kernel-Szene): pocket-Feld über der Taschenöffnung bis 2.0 mm daneben
  (Punkt über dem Mund misst gegen die weggebooleante Subtrahend-Kappe);
  Union innen bis 2.2/3.5 mm (verdeckte Operanden-Wände). Der Proxy
  drückt beides auf 0.013–0.036 mm.
- Feld und Tape sind identisch, AUSSER bei MaskedBlend: der #382-Fade
  staucht das Tape global ÷√2 (fillet_step Fernfeld mean ~1.9 mm —
  konsistent mit Baseline-Befund 5).

### 3. Fillet-Zone (fillet_band.csv — 2500 Punkte in der Kapsel r ≤ 7.5 mm um die verrundete Kante, GT auf dem gefilleteten Feld)

| Variante | mean | p95 | max | bias | ∇-dev p95 | ∇-dev max |
|---|---|---|---|---|---|---|
| field_visual | 0.043 | 0.239 | 2.391 | −0.018 | 0.0002 | 30.0 |
| tape (#382-Fade) | 0.601 | 1.769 | 3.577 | −0.465 | 0.46 | 0.93 |
| **proxy_tess (scharfer Weave)** | **0.391** | **1.206** | **1.243** | −0.380 | 0.125 | 32.7 |
| proxy_dc128 | 0.0012 | 0.0036 | 0.236 | +0.000009 | 0.0021 | 0.95 |
| proxy_dc256 | **0.0008** | **0.0012** | 0.223 | +0.00014 | 0.0015 | 0.91 |

- **H8 bestätigt — die Kernfrage ist mit NEIN beantwortet.** Der heutige
  tessellate-Proxy misst in der Fillet-Zone zur scharfen Kante: max-Fehler
  1.243 mm ≡ r·(√2−1) = 3·0.4142 = 1.243 — auf 4 Stellen der geometrische
  Abstand Ecke↔Fillet-Fläche. Der Fehler ist tol-UNABHÄNGIG (identisch bei
  0.2/0.05/0.01) und skaliert mit r_fillet.
- Das DC-Render-Mesh (das reale Artefakt, das die Fillet-Geometrie trägt,
  #350) liefert dagegen mean 0.0008–0.0012 mm, p95 ~0.001–0.004, max
  0.22–0.24 (≈ 1.3 DC-Zellen an den Runout-Ecken) — **die Proxy-IDEE
  funktioniert in der Fillet-Zone, nur die Proxy-QUELLE fehlt**.
- Bonus-Befund: das Visual-FELD selbst hat in der Fillet-Zone max 2.39 mm
  Metrikfehler (Blend-Tiefe), das Tape 3.58 mm — der DC-Proxy ist dort um
  3 Größenordnungen (mean) besser als beide.

### 4. Hybrid-Blend-Regel (blend_rule.csv; h = mix(sign(f)·d_mesh, f, smoothstep(τ_in=1mm, τ_in+Band, |f|)), Band = mult·Δ, Δ = max Nahfeld-|f−d|)

| Szene (Proxy) | Δ | mult | Überschätzung außen | innen | max ‖∇h‖ |
|---|---|---|---|---|---|
| pocket (tess 0.05) | 0.050 | 1–8 | 0.000001 | 0.0006 | 1.0005 |
| box_union_cyl (dc256) | 0.196 | 1–8 | 0.0064 | 2.189 | 1.044→1.012 |
| box_boss_cyl (tess 0.05) | 0.094 | 1–8 | 0.044 | 3.546 | 1.7295 |
| fillet_step (tess scharf) | 0.990 | 1 | 1.165 | 0.168 | 39.4 |
| fillet_step (tess scharf) | 0.990 | 8 | 0.850 | 0.012 | 39.4 |
| fillet_step (dc256) | 0.990 | 1 | 1.165 | 0.209 | 14.6 |
| fillet_step (dc256) | 0.990 | 4 | 0.781 | 0.209 | 13.4 |
| fillet_step (dc256) | 0.990 | 8 | 0.312 | 0.209 | 6.03 |

Einordnung (revidiert die Iteration-1-Lesart in zwei Punkten):

1. **Die E4-Prämisse „h überschätzt nie" gilt NUR, solange f ein
   Unterschätzer ist — und genau das bricht beim Edge-Fillet.** Ein
   Differenz-Fillet FÜLLT die Kehle: die wahre Fläche rückt zu
   Außenpunkten hin, das harte Feld jenseits des Mask-Bands misst weiter
   zur scharfen Geometrie und ÜBERSCHÄTZT bis 1.16 mm (Marsch-Overstep-
   Klasse; der strukturelle Grund, warum der Render-Pfad den #382-Fade
   braucht). Das Blend-Band kann nur reparieren, was es abdeckt: mit
   korrektem Proxy fällt die Außen-Überschätzung monoton 1.16 → 0.31 mm
   (mult 1→8), mit dem falschen (scharfen) Proxy bleibt sie bei 0.85 mm
   hängen.
2. **∇-Spikes an der Nullmenge sind Vorzeichen-Konsistenz, nicht
   Bandbreite.** sign(f)·d_mesh springt an der Fläche um
   2·(Geometrie-Abweichung Proxy↔Feld): scharfer Proxy im Fillet
   → Sprung ~2.5 mm → ‖∇h‖ ≈ 39 bei JEDER Bandbreite; DC-Proxy
   (Abweichung ~0.2 mm) → 14.6→6.0; exakter Proxy (pocket) → 1.0005.
   Die Iteration-1-Kaveat-Schranke „Diskontinuität ≤ 2·tol" ist damit
   in vivo bestätigt — sie ist die dominante ∇-Störung, NICHT der
   Δ/Band-Spike, sobald der Proxy geometrisch stimmt.
3. Die 4×-Regel selbst (∇-Glättung des Δ/Band-Anteils) bleibt für
   korrekte Proxies plausibel (boss: 1.73 konstant = reiner
   Vorzeichen-Sprung ≈ 2·sag/2h + 1; union: 1.04→1.01 monoton), ist aber
   sekundär gegenüber Punkt 2.
4. Innen-Überschätzung (union 2.19 / boss 3.55 mm) ist der Feld-Fehler
   JENSEITS des Bands (h=f dort) — ein Nahfeld-Blend repariert
   definitionsgemäß nur das Nahfeld. Für Shell/Wandstärke muss der
   Konsument im d_near-Regime bleiben (Iteration-1-Architektur §3).

## Schlussfolgerung

**Verdict zur Kernfrage: NEIN — Fillet-Zonen erscheinen heute in KEINEM
tesselierbaren Weave-Output.** Dreifacher Stopp, empirisch belegt:
(1) der Produktions-Weave (`weave_all`/`ensure_brep`) wendet Edge-Fillets
gar nicht an (11/11 Plane-Faces, 28-Tri-Mesh ≡ scharfe pocket);
(2) der Direkt-Weave des gefilleteten CSG webt zwar genau eine Blend-Face
(c-M1 bestätigt), scheitert aber an `tessellate_brep` — Blend-Carrier ist
dort ein expliziter `Err`, und der Step-Fillet-Weave hat zusätzlich
degenerierte Loops; (3) im Proxy aus dem scharfen Weave beträgt der
Fillet-Zonen-Fehler exakt r·(√2−1) (gemessen 1.243 mm bei r=3),
tol-unabhängig.

**Tragfähigkeit des Proxy-Pfads als Nahfeld-Wahrheitsschicht: JA, mit
einer klaren Bedingung.** Auf allem, was heute tesseliert (planare
Booleans, saubere Rotations-Nähte), ist der Proxy an echten Artefakten so
gut wie an den Hand-Meshes der Iteration 1: Nahfeld ≈ tol_mm, Phantom-
und Union-Innen-Fehler des Felds (2.0–3.5 mm) fallen auf 0.013–0.036 mm,
Kosten 0.2–13 µs/Query (naiver BVH). Die Bedingung: **die Fillet-Zone
braucht eine Proxy-Quelle, die die Blend-Geometrie trägt.** Der Beweis,
dass das reicht, ist da — das DC-Render-Mesh als Proxy liefert in der
Fillet-Zone mean ≤ 0.0012 mm (vs 0.39 mm scharfer Proxy, 0.043 mm Feld,
0.60 mm Tape). Wege, geordnet nach Sauberkeit: (a) Blend-Face-Tesselation
in `tessellate_brep` (Carrier ist analytisch — `HalfSpace::Blend` hat eine
Feld-Definition; Marching über das Face-UV oder Projektion der
Octree-Patches), (b) hybrides Proxy-Mesh: tesselierte analytische Faces +
feld-gemarchte Patches nur für Blend-Faces (die `Tessellated`-Carrier-
Infrastruktur existiert), (c) pragmatisch sofort: DC/MDC-Mesh des Bodies
als Proxy überall dort, wo (wie beim Export heute) das Fillet-Gate zieht —
Genauigkeitspreis O(Zelle) ≈ 0.2 mm statt O(tol).

Zusätzliche Negativbefunde, sauber dokumentiert: `tessellate_brep`
scheitert auch fillet-frei an durchstoßenden Unions (outer −1 / 2 inner);
die Proxy-Genauigkeit hat einen tol-unabhängigen Boden aus der gewobenen
Naht-Polyline (0.123 mm im Boss); und die E4-Marsch-Sicherheits-Garantie
der Iteration 1 gilt bei Edge-Fillets NICHT — das harte Feld ist dort
lokal ein ÜBERSCHÄTZER (bis 1.16 mm), was den Hybrid-Pfad nicht schwächt,
sondern seine Notwendigkeit verschärft (nur ein geometrisch korrekter
Nahfeld-Proxy beseitigt den Overstep, gemessen 1.16 → 0.31 mm).

Nächste Schritte (Vorschlag für Iteration 3): Blend-Face-Tesselation als
Spike (Weg a/b) und danach Wiederholung DIESES Laufs — Erwartung:
Fillet-Band-Fehler fällt von r·(√2−1) auf ≤ tol; plus Offset-End-to-End
(Iteration-1-Vorschlag 2) auf der dann vollständigen Proxy-Quelle.
