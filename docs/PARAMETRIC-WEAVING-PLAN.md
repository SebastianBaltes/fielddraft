# Parametric Weaving — Umbau auf CSG-Branch-Paar-Seeding

Stand: 2026-06-08. Single Source of Truth für den Umbau des Edge-Findings von
octree-primärer Suche auf **CSG-Branch-Paar-Seeding**. Entstanden aus der
Architektur-Diskussion (Opus 4.8 max thinking) und gegen den echten Code
verifiziert.

## Motivation

Der adaptive Octree zahlt eine strukturelle Steuer: Er verfeinert das gesamte
Narrow-Band um die 2D-Fläche und klassifiziert jede Oberflächenzelle per
Branch-Zählung, nur um die Kanten zu finden — eine 1D-Teilmenge. Bei 0,1 mm
Detail auf 1 m großen Körpern sind das ~14 Octree-Ebenen und Millionen
Face-Zellen, um eine 1D-Kantenmenge abzulesen. Das ist „3D abtasten, um 1D zu
gewinnen". DL-82 (Geisterbrücke über offenem Nutmund) ist ein Symptom genau
dieser octree-primären Suche.

Umkehrung: Kanten **direkt aus der CSG-Branch-Adjazenz seeden**, das
Narrow-Band-Klassifizieren für die Kantensuche überspringen. Der Octree
schrumpft auf eine Verifikations-/Vertex-Rolle.

## Verifizierte Code-Faktenlage

- **`weave_parametric` (weaving.rs:1222)** ist heute nur ein Satz hartcodierter
  Shortcuts: `build_box_wireframe`, `build_cylinder_wireframe`,
  `build_edgeless_wireframe` (Box, Zylinder, Sphäre, Torus). Greift keiner,
  gibt es `None` → Octree übernimmt. Für reale Geometrie (Boolean ×
  Extrude/Revolve/Sweep × Sketch) fällt der Flag schlicht durch. Branch-Paar-
  Seeding ist die **allgemeine Implementierung dessen, was der Flag immer sein
  wollte**.
- **`ensure_brep(…, parametric)` (body.rs:158)** wählt: space-filling → leer;
  `parametric && weave_parametric().is_some()` → analytisch ohne Octree; sonst
  Octree `weave` (weaving.rs:1022). `parametric_weave` default `false`
  (lib.rs:955) → heute zwingt der Default bereits alle Körper auf den Octree.
- **Konvergenzpunkt**: Beide Routen münden in `finish_weave` (weaving.rs:1035)
  und erzeugen identische `id_hash`-Handles (body.rs:156). Die Verdrahtung am
  Ausgang ist geschenkt; der Greenfield-Teil sitzt davor.
- **`trace_edge` (edge_trace.rs:473)** nimmt einen Innen-Seed, projiziert ihn
  auf die Kante, marschiert forward + backward, stitcht
  `reverse(backward) ++ seed ++ forward`, erkennt geschlossene Ringe und
  protokolliert die begrenzenden Vertices. Mid-curve-bidirektionales Seeding
  ist also nativ vorhanden — der Kern-Trace muss nicht angefasst werden.
- **`both_branches_active` (extraction.rs:592)** ist ein `body.evaluate(p, eps,
  Logical)` + zwei `contains`-Checks → O(1) pro Punkt, kein Octree. Dicht
  entlang der Kurve auswertbar.

### Drei Präzisierungen, die den Plan prägen

1. **`validate()` (mod.rs:462) ist ein reiner Referenz-Integritäts-Stub**:
   Handle-Bounds + Signatur-Sortierung. KEIN Manifold-/Loop-Closure-/Euler-
   Check. Deshalb kam die DL-82-Geisterbrücke durch. Die Manifold-Invariante
   (Stufe 1 der Teststrategie) ist nicht vorhanden — sie ist Greenfield in
   Phase 0a.
2. **DL-75-Gating**: Der analytische Fast-Path `project_branch_analytic`
   (extraction.rs:250) ist über eine `HalfspaceMap` gegated, die nur für
   *shelled bodies* befüllt wird — unabhängig vom `parametric_weave`-Flag. Für
   ein gültiges Differential-Oracle muss der Force-Pure-Octree-Modus die
   `HalfspaceMap` leeren, sonst vergleicht man in geschalten Regionen
   analytisch gegen analytisch und verfehlt genau die analytischen Bugs.
3. **Basis-Force-Octree existiert** als `parametric_weave = false`. Was fehlt,
   ist die *Reinheit* (Punkt 2) und die Manifold-Invariante (Punkt 1), nicht
   der Schalter.

## Multi-Segment: zwei Quellen, getrennt zu behandeln

Ein Paar `(a, b)` liefert disjunkte Kurven aus zwei völlig verschiedenen
Gründen:

- **(A) Intrinsische Multiplizität** — die ungetrimmte Schnittmenge a∩b ist
  schon mehrkomponentig (Ebene ∩ achsenparalleler Zylinder = zwei Geraden;
  diverse Torus-Fälle). Hängt nur an den Flächentypen. Wird in der
  **Branch-Paar-Aufzählung** (Phase 1) per closed-form-SSI-Komponentenzählung
  bzw. 2D-Param-Scan aufgelöst.
- **(B) Trimming-Multiplizität** — a∩b ist eine Kurve, aber ein dritter Branch
  schneidet ihre Mitte heraus → zwei kollineare Segmente, getrennt durch eine
  Vertex-Lücke. Dominiert bei realen Teilen. Löst sich gratis über den
  **Aktiv-Intervall-Marsch** (Phase 2): `both_branches_active` entlang der
  bekannten Kurve ablesen; zwei aktive Intervalle = zwei Segmente, die
  Lückengrenzen sind die ≥3-Branch-Vertices. Die Vollständigkeit ist hier nur
  durch einen 1D-Nyquist begrenzt (Schrittweite entlang einer Kurve), nicht
  durch den 3D-Nyquist des Narrow-Bands.

Die affine Arithmetik (3D, volumetrisch) ist damit **optional, nicht Pflicht**:
Sie kommt erst mit der freien-NURBS-Spine-Erweiterung, wo keine saubere
Vorwärtsmap existiert. Box-Subdivision wird gestrichen — sie nutzt weder die
analytische Struktur noch die 2D-Param-Reduktion und behält 3D-Nyquist-Löcher.

## Teststrategie (dreistufig)

Der Octree ist KEIN sauberes Ground Truth (DL-82-Geist, DL-75-Analytik-Leck),
also kein einzelnes Oracle, sondern drei Stufen:

1. **Absolute Invarianten — kein Oracle nötig.** Manifold (jede Kante in genau
   zwei Schleifen, DL-24), Wasserdichtigkeit, Euler-χ. Beide Pfade bedingungslos.
   Stark, aber nicht vollständig (ein Miss + kompensierendes Merge kann zufällig
   manifold und falsch sein) → Stufe 2/3.
2. **Analytisches Ground Truth — kuratierte Körper.** Box = 12 Kanten bekannter
   Länge; Zylinder−Box = berechenbare Schnittkurven bekannten Typs. Stärkster
   echter Korrektheitscheck, wo die Geometrie geschlossen-form bekannt ist.
3. **Differentiell Octree ↔ Branch-Paar — alles andere.** Abweichungen sind
   Triage-Signale, kein Pass/Fail. Jede Abweichung ist genau eines von:
   (a) Branch-Paar-Bug (Octree findet, Branch-Paar nicht) → Coverage-Lücke,
   meist fehlende intrinsische Multiplizität (A) → Arbeitsliste;
   (b) Octree-Auflösungslimit (Branch-Paar findet Kante dünner als Octree-Tiefe)
   → Sieg neu;
   (c) Octree-Geist à la DL-82 (Branch-Paar lässt korrekt weg) → Beleg für den
   Umbau.
   Gültigkeit von Stufe 3 setzt den **DL-75-reinen** Force-Octree voraus
   (HalfspaceMap leer).

## Umsetzungsreihenfolge — Harness vor dem riskanten Code

### Phase 0 — Korrektheits-Infrastruktur (validiert gegen heutigen Octree)

- **0a Manifold-Invariante**: `validate()` um Loop-Closure (jede Kante in genau
  zwei Schleifen, DL-24), Wasserdichtigkeit und Euler-χ erweitern. Tests gegen
  heutigen Output als Baseline.
- **0b Force-Pure-Octree-Modus**: Flag durch `weave` → `extraction` reichen, der
  die `HalfspaceMap` leert → echter numerischer Octree ohne analytische
  Schnellpfade.
- **0c Differential-Komparator**: Octree ↔ Branch-Paar mit (a)/(b)/(c)-
  Klassifikation. Jetzt validierbar: Octree-vs-Octree identisch + Manifold-
  Baseline auf heutigem Output.

### Phase 1 — Branch-Paar-Aufzählung + Seeds-pro-Paar (der echte Greenfield)

- An jedem Boolean-Knoten Kandidatenpaare `(BranchId_a, BranchId_b)` aufzählen:
  (aktive Flächen links) × (aktive Flächen rechts) plus intrinsische Paare jedes
  Operanden. Per world-AABB-Überlappung prunen (`BoundedCsg` trägt sie an jedem
  Knoten).
- **Intrinsische Multiplizität (A)** auflösen: closed-form-SSI-Komponenten-
  zählung pro Flächentyp-Paar (Ebene/Sphäre/Zylinder/Kegel); für Sweep/Helix
  der 2D-Param-Rechteck-Scan über `g(S(u,s)) = 0`. Ein Seed pro Komponente
  (Überlappungsbox-Zentroid).

### Phase 2 — Aktiv-Intervall-Marsch in `trace_edge`

- **2a — Aktiv-Prädikat im Tracer ✅ (v0.6.2)**: `march`/`trace_edge` nehmen ein
  optionales `active: Option<&dyn Fn([f64;3]) -> bool>`. Pro akzeptiertem Schritt
  wird `active(np)` ausgewertet; beim Aktiv→Inaktiv-Übergang bisektiert der Marsch
  `[cur, np]` auf die On-Surface-Grenze und endet mit `MarchEnd::Trimmed` →
  `EdgeEnd::Trim([f64;3])`. **Trimming-Multiplizität (B)** fällt damit gratis:
  Lückengrenzen = dritter Branch erscheint = neue Vertices. Inaktiver Seed →
  `None`. `edge_trace` bleibt geometrie-only (Prädikat ist opakes Closure); der
  Octree-Pfad reicht `None` durch (verhaltensidentisch). `TracedEdge.ends` ist
  jetzt `(EdgeEnd, EdgeEnd)` statt `(Option<usize>, Option<usize>)`.
- **2b — Branch-Seed-Weaver (offen)**: `EdgeSeed`s → Fields aus der
  Half-Space-Map → `trace_edge` mit `active = both_branches_active(body, ba, bb)`
  → `finish_weave`. Ersetzt die vier `weave_parametric`-Sonderfälle.
- Headroom: auf analytischen Segmenten nicht dicht auswerten, sondern auf
  Übergänge bisektieren; geometrischen Korrektor auf rein-analytischen
  Segmenten droppen.

Ergebnis: Branch-Paar-Seeding ersetzt die vier `weave_parametric`-Sonderfälle —
gleicher `finish_weave`-Ausgang, gleiche `id_hash`-Handles. Der
`parametric_weave`-Flag wird zum ersten Mal für echte Geometrie bedeutsam.

## Scope-Split (Körperklassen)

- **Branch-Paar-Seeding**: Boolean × {Extrude / Revolve / Helix} × Sketch.
- **Octree-Fallback**: Twist / Bend / Displace und freie-NURBS-Spine (kein
  sauberer Vorwärtsmap); hier wird später ggf. 3D-affine Arithmetik wertvoll.

## Sweep-Repräsentation (Greenfield, Vorausentscheidung)

Sweep/Helix stehen noch nicht in der `CsgNode`-Op-Liste. Repräsentation so
anlegen, dass **Spine + Frame + Profil** erhalten bleiben → intrinsische Kanten
`E(s) = C(s) + R(s)·c` fallen forward closed-form raus. Frame: **Rotation-
Minimizing (Parallel-Transport / Double-Reflection), nicht Frenet** (Frenet
flippt an Wendepunkten, ist bei κ=0 undefiniert). Spine **stückweise analytisch
(Linie/Bogen/Helix) zuerst**, freie NURBS-Spine später — Letzteres bringt die
Fußpunkt-Inversion + affine Arithmetik mit und ist bewusst nach hinten geschoben.

## Offene Designfrage

Multi-Segment-Abdeckung für (A) bei Sweep/Helix: reicht der 2D-Param-Scan, oder
braucht es 2D-Intervallarithmetik für eine Garantie? Entscheidet sich an realen
Sweep-Teilen in Phase 1.
