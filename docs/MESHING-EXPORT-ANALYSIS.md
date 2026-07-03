# Meshing-Analyse für den STL/3MF-Export

Stand: 2026-07-02 · Quellen: 6 Paper (docs/Literatur/), Web-Recherche (CGAL, Manifold,
fidget, fTetWild, OpenVDB, libfive, Rust-Ökosystem), Repo-Ist-Analyse (dc/mdc/rfta,
B-Rep, CGAL-Spike-Branch). Detailnotizen der Recherche: /tmp/meshing-research/ (Sitzung
2026-07-02); die Kernaussagen sind hier vollständig eingearbeitet.

## 1. Auftrag & Kriterien

Qualitäts-Meshes für den Export (STL, 3MF, …) mit Prioritäten:

1. **Watertight/2-manifold** — garantiert, nicht empirisch.
2. **Exakte Abbildung** — Zieltoleranz **0,1 mm** (chordale Abweichung zur wahren Fläche).
3. **Sehr gute Kanten** — scharfe Feature-Kanten liegen auf den echten Kantenkurven.
4. **Sparsame Dreieckszahl** — adaptiv; Ebenen kosten fast nichts.

Sonderchance: Körper(teile) mit analytischem B-Rep **parametrisch** meshen; die
vorhandenen B-Reps (Flächen-Carrier + getracete Kanten) einspannen.

## 2. Ist-Zustand (ernüchternd, aber mit starken Assets)

- **Der Export schreibt heute das Viewport-Mesh raus.** `export.rs`/`export_3mf.rs`
  exportieren exakt das Render-Mesh: MDC auf **uniformem** Grid, Auflösung 64 (aktiver
  Body) bzw. 32 (Kontext-Bodies), gesetzt in `web/src/kernel.ts`. Bei einem 100-mm-Teil
  sind das ~1,6 mm Zellen — **die 0,1-mm-Anforderung wird um ~16× verfehlt**. Kein
  Export-Pass, kein Weld/Decimate, keine Manifold-Prüfung.
- **`mdc.rs` ist bereits Manifold-DC** (Schaefer/Ju/Warren: Konturzyklen per Union-Find,
  ein QEF-Vertex pro Zyklus → 2-manifold per Konstruktion; QEF f64 + Jacobi-SVD,
  Kantenschnitte per Bisektion). `dc.rs` ist Wrapper + Newton-Projektion der
  Face-Vertices auf die exakte SDF + planare/developable Simplification. Das ist ein
  sehr gutes Fundament für Prio 1 — es fehlt „nur" Auflösung, Adaptivität und Validierung.
- **RFTA ist vollständig portiert** (`rfta/`), aber auf 16³ gekappt und prinzipbedingt
  glättend → Preview/Fallback-Nische, kein Export-Kandidat (siehe §3).
- **CGAL-Mesh_3-Spike existiert** (Branch `feat/cgal-mesher`, 6 Commits): Feature-Edges/
  Protecting-Balls sind bereits mit unseren B-Rep-Kanten-Polylines verdrahtet, der
  SDF-Opcode-Interpreter ist bit-identisch nach C++ portiert (Data-Shipping, kein
  per-Sample-FFI). Lücken: kein `manifold()`-Kriterium, ersetzt nur die Render-Geometrie
  (nicht den Export), Basis ~640 Commits hinter main, ImportedMesh/TEX3D fehlen.
- **Die B-Reps sind der Trumpf:** analytische Carrier (Plane, InfiniteCylinder, Sphere,
  Cone, Torus, RevolvedProfile), Kantenkurven (NURBS/Hermite, f64, Toleranz ~µm),
  STEP-Export existiert. Ein per-Face-Tessellierungspfad fehlt, ist aber in
  `ADAPTIVE-RENDERING-PLAN.md` bereits als Modus vorgesehen.
- **CsgNode-Trennlinie** (entscheidet parametrisch vs. SDF):
  - *Analytisch weavbar:* Leaf(Box/Cylinder/Cone/Sphere/Torus), Union/Difference/
    Intersection, Extrude, Revolve, Shell-Innenwände, Offset (Carrier falten zurück).
  - *SDF-only:* SmoothUnion/SmoothDifference/MaskedBlend (Blendzonen), Twist/Bend/
    Displace/Morph (`needs_full_sdf`), Helix/Sweep (weave-excluded), Gyroid (Weave
    übersprungen), Leafs ohne Carrier (Thread, Ellipsoid, Capsule, HexPrism,
    ImportedMesh).
- Bekannte Qualitätsschulden: Helix im Export pathologisch grob (Ticket 55ea — der
  Viewport-Fix war parametrisch, der Export blieb DC), Mesh-Speckles an 2-mm-Wänden
  (Vase), Wunsch-Tickets für parametrische Geometrie (mdo0/qmvn).

## 3. Verfahrenslandschaft — Bewertung gegen die 4 Kriterien

Legende: ✓✓ stark/garantiert · ✓ gut · ○ neutral/steuerbar · ✗ schwach/fehlt.
WASM = Browser-Tauglichkeit inkl. Speicher; Aufwand = Integrationsaufwand für uns.

| Verfahren / Route | 1 Watertight | 2 Exakt (0,1 mm) | 3 Kanten | 4 Ökonomie | WASM | Aufwand | Anmerkung |
|---|---|---|---|---|---|---|---|
| **Manifold-DC (Schaefer 07)** — Basis liegt in `mdc.rs` | ✓✓ bewiesen (2-manifold, Genus-erhaltend) | ○ über Grid+QEF-Schwelle; mit exakter Hermite sehr gut | ✓✓ QEF/Hermite | ✓✓ Octree-Clustering, LOD ohne Re-Cluster | ✓✓ pure Rust | **M** (Octree-Clustering + Export-Pass fehlen) | Selbstschnitte möglich → Check nötig; Features < Basiszelle werden verpasst |
| **DC mit exakten Hermite-Daten** (Ju 02) | ✗ allein (non-manifold-Fälle) | ✓✓ | ✓✓ | ○ | ✓✓ | — | Qualitäts-OBERGRENZE laut DCDD-Paper — wir HABEN exakte Hermite (∇f + B-Rep) |
| **DCDD** (SIGGRAPH 26) | ✗ (Selbstschnitte, keine Garantie) | ✓ (SOTA auf Samples) | ✓✓ | ○ | ✓ | M–L | Löst ein härteres Problem als unseres (nur Samples); für uns nur Fallback für gebakte Felder |
| **SDF-CWF** (CAD 25) | ○ empirisch 0 Defekte, unbewiesen | ✓ (kein Bound) | ✓✓ inkl. weak features | ✗ uniform | ✗ (CVT/Power-Diagramm, 10–60 s) | L | „Schönheits-Remesh"-Referenz; Power-Diagramm-Trick merken |
| **RFTA** (im Kernel, 16³-Cap) | ○ via sPSR, unbewiesen | ✗ kein Regler | ✗ verrundet (dokumentiert) | ✗ | ○ langsam | vorhanden | Nische: robuste Low-Res-Preview; Tangentialkugel-Test = billiger Validitäts-Check |
| **Marching Tetrahedra auf adaptivem Delaunay-Tet-Gitter** (SDFRaster/TetWeave-Blaupause) | ✓ konstruktiv | ✓ via Refinement | ✗ (Lerp; nur mit Kanten-Seeding) | ✓✓ | ○ (robuste 3D-Delaunay = Knackpunkt) | L | Idee mitnehmen: B-Rep-Kanten als Gitter-Vertices seeden |
| **CGAL Mesh_3 + Protecting Balls + `manifold()`** | ✓✓ | ✓✓ `facet_distance` | ✓✓ Balls auf unseren Kanten-Polylines | ✓ kriterien-adaptiv | ✓ (Spike existiert!) | M (Spike-Rebase + manifold() + Export-Anbindung) | **Einzige Fertiglösung, die alle 4 trifft** — aber GPL + Laufzeit |
| CGAL Isosurfacing_3 (TMC/DC) | TMC ✓✓ / DC ✗ | ○ | TMC ✗ / DC ✓ | ✗ uniform | ✓ | S–M | Bestätigt offiziell die Lücke „manifold ODER sharp" |
| CGAL Alpha_wrap_3 | ✓✓ | ✗ (einseitiger Offset) | ✗ | ✗ | ○ teuer | S | Nur Repair-Fallback |
| **Manifold-Library** (Apache-2.0, offizielles WASM-npm) | ✓✓ CSG watertight-by-construction | ✓ (Input-getreu) | ✓ (Input-getreu) | ✓ (Input-getreu) | ✓✓ | S–M | Mesh-CSG für Hybrid-Fälle (analytisch ∪ SDF-only); LevelSet-Modul selbst: uniform, keine Kanten |
| fidget (Rust, MPL-2.0) | ○ „not bulletproof" (Keeter) | ✓ | ✓ QEF | ✓ Octree | ✓✓ | M | Beste lebende Rust-Referenz für adaptives Sharp-Meshing |
| fTetWild | ✓✓ ε-Envelope | ✓✓ | ✓ | ✓ | ✗ kein Port, Laufzeit | L | Ungeeignet für Browser |
| OpenVDB volumeToMesh | ✓ | ✓ | ✗ | ✓ | ✗ kein realer Port | L | — |
| fast-surface-nets u.a. Rust-Crates | ✗/○ | ○ | ✗ | ✗ | ✓✓ | S | Nur Preview-Klasse |
| Decimation: CGAL SMS / meshoptimizer | erhält ✓ (Link-Condition) / Best-Effort | ✓ Fehlerschranke | ✓ Kanten lockbar | ✓✓ | ○/✓✓ | S | Nachbrenner für Kriterium 4 |

**Schlüssel-Erkenntnisse der Recherche:**

1. **Kein fertiges Paket erfüllt alle vier Kriterien** — außer CGAL Mesh_3 mit
   Protecting Balls + `manifold()` (GPL, Laufzeit). CGAL selbst dokumentiert die Lücke:
   TMC manifold ohne Kanten, DC Kanten ohne manifold.
2. **„DC mit exakten Hermite-Daten ist die Qualitätsobergrenze"** (DCDD-Befund). Die
   gesamte jüngere Literatur (DCDD, RFTA, SDF-CWF) löst das Problem *fehlender* exakter
   Daten — das Problem haben wir nicht: exakte SDF, exakte Gradienten, analytische
   Kantenkurven. Wir können direkt an der Obergrenze arbeiten.
3. **Adaptivität ist alternativlos** (industriebestätigt, nTop): uniform 0,1 mm auf
   600 mm Bauraum = 6000³ — nicht darstellbar. Narrow-Band + Octree (fein nur an
   Krümmung/Kanten) bzw. parametrische Flächen (Ebenen: 2 Dreiecke) sind Pflicht.
4. **Unser `mdc.rs` implementiert bereits den richtigen Kern** (Manifold-DC). Der Weg zu
   Prio 1 ist kein Verfahrenswechsel, sondern: Export-Pass, Adaptivität, Kanten-Pinning,
   Validierung.

## 4. Architektur-Optionen

### Option A — Export-Pass „Exakt-Hermite-MDC" im eigenen Kernel (Backbone)

Eigener Export-Mesher (getrennt vom Viewport!), aufbauend auf `mdc.rs`:

- **Basis-Grid toleranzgetrieben**: Zellgröße aus 0,1-mm-Sag-Bound und minimaler
  Wandstärke; Narrow-Band-Auswertung; Octree-Verfeinerung nur an Krümmung/Kanten.
- **Exakte Hermite-Daten**: Kantenschnitte per Newton/Bisektion auf der exakten SDF,
  Normalen aus ∇f; wo B-Rep-Carrier existieren, Normale/Projektion analytisch.
- **Kanten-Pinning**: Zellen, die eine getracete B-Rep-Kante schneiden, werden vom
  Clustering ausgenommen; Kanten-Vertices auf die Kantenkurve projiziert → Prio 3
  garantiert statt heuristisch.
- **Octree-Vertex-Clustering nach Schaefer 07** (χ- und Schnittzahl-Kriterium, bewiesen
  manifold-erhaltend) mit QEF-Schwelle ≈ Toleranz² → Prio 4; Ebenen kollabieren fast
  vollständig (die planare/developable Simplification aus `dc.rs` bleibt als Feinschliff).
- **Validator + Report**: Euler/Manifold-Check, Selbstschnitt-Test (BVH),
  Tangentialkugel-Stichprobe gegen die exakte SDF (RFTA-Idee) als Toleranz-Check.
  Ergebnis erscheint im Export-Dialog („watertight ✓, max. Abweichung 0,07 mm, N Dreiecke").

*Trifft:* 1 ✓✓ (bewiesen manifold; Selbstschnitte werden geprüft), 2 ✓ (Hermite exakt,
Interieur per Sag-Bound), 3 ✓✓ (Pinning), 4 ✓✓. Funktioniert für **alle** Körper, auch
Gyroid/Helix/Blends. Reine Rust/WASM-Arbeit, keine neuen Abhängigkeiten.

### Option B — Parametrische per-Face-Tessellation aus dem B-Rep (Königsklasse für CAD-Teile)

Für Körper, deren Weave **alle** Flächen analytisch abdeckt (Box/Zylinder-Booleans,
Extrude, Revolve — der Großteil klassischer CAD-Teile):

- Jede B-Rep-Fläche wird auf ihrem Carrier parametrisch trianguliert (Ebene: Ohrclipping
  der Loops, ~2 Dreiecke + Löcher; Zylinder/Kegel/Torus/Revolve: Sag-Formel
  `θ ≤ 2·acos(1 − tol/r)` → Winkelschritt pro 0,1 mm).
- **Invariante Wasserdichtheit:** jede Kantenkurve wird **einmal** diskretisiert und die
  Polyline von beiden angrenzenden Flächen **identisch** übernommen (shared vertices) —
  watertight by construction, Kanten exakt (Prio 1–3 zugleich), minimale Dreieckszahl
  (Prio 4 optimal: Ebenen kosten ~nichts).
- Auto-Gate pro Body: 100 % Flächen analytisch + Weave-Invarianten grün → Option B;
  sonst Option A. (Mischkörper siehe Option D.)

*Risiko:* hängt an der Weave-Vollständigkeit (die offenen Weave-Baustellen aus dem
Klassifikations-Plan gelten hier fort); daher immer mit Option A als Fallback + demselben
Validator.

### Option C — CGAL Mesh_3 reaktivieren (Premium-Pfad / Referenz)

Spike auf main rebasen, `manifold()`-Kriterium ergänzen, an den **Export** (nicht die
Render-Geometrie) anbinden. Liefert garantierte Qualität inkl. Protecting Balls auf
unseren Kantenkurven — als optionaler „High-Quality (langsam)"-Export und als
**Referenz-/Validierungs-Mesher** für die eigenen Pfade. GPL ist für uns (AGPL-Projekt)
kompatibel; Kosten sind WASM-Größe (~MB) und Laufzeit.

### Option D — Manifold-Library für Mischkörper (Ergänzung)

Körper mit analytischem Rumpf + SDF-only-Anteil (Box ∪ Gyroid): Teil-Meshes aus B (Rumpf)
und A (Gyroid-Region) per Manifold-Library (Apache-2.0, offizielles WASM-npm) boolesch
vereinigen — watertight by construction. Nur nötig, wenn Mischkörper-Qualität Priorität
bekommt; bis dahin gehen Mischkörper komplett über Option A.

## 5. Empfehlung: gestufter Plan

```mermaid
graph LR
    P0["Phase 0\nExport vom Viewport\nentkoppeln + Validator"] --> P1["Phase 1\nOption A:\nExakt-Hermite-MDC\n(adaptiv, Kanten-Pinning)"]
    P1 --> P2["Phase 2\nOption B:\nper-Face-Tessellation\nfür analytische Bodies"]
    P2 --> P3["Phase 3 (optional)\nC: CGAL-Premium/Referenz\nD: Manifold für Mischkörper"]
```

- **Phase 0 (klein, sofortiger Gewinn):** Export-Pass mit eigener, toleranzgetriebener
  Auflösung (unabhängig vom Viewport-Budget; Kontext-Bodies nicht mehr mit res 32
  exportieren!) + Validator/Report. Noch ohne neue Algorithmik — dieselbe MDC-Pipeline,
  nur richtig dimensioniert und geprüft, im Worker mit Fortschrittsanzeige.
- **Phase 1 (M, der eigentliche Hub):** Option A vollständig — Octree-Clustering
  (Schaefer 07) auf `mdc.rs` aufsetzen, exakte Hermite-Daten, B-Rep-Kanten-Pinning,
  Selbstschnitt-Check. Erfüllt alle vier Kriterien generisch.
- **Phase 2 (M):** Option B für weave-vollständige Bodies — der Qualitäts- und
  Ökonomiesprung für klassische CAD-Teile (und erledigt Ticket-Klasse 55ea/mdo0
  strukturell: Helix-`parametric_brep`-Pfad kann hier ebenfalls exakt tessellieren).
- **Phase 3 (optional, nach Bedarf):** CGAL-Premium-Export/Referenz-Validierung;
  Manifold-Library für Mischkörper; SDF-CWF-artiger „Beautify"-Remesh nur falls je
  gewünscht.

## 6. Offene Fragen / Risiken

1. **Dünne Wände < Zellgröße** (Speckles-Klasse): Basis-Grid muss die minimale Wandstärke
   sehen; braucht eine billige Wandstärken-Heuristik (SDF-Doppelabtastung entlang ∇f)
   oder Nutzer-Hinweis im Report.
2. **Gyroid/TPMS bei 0,1 mm auf großem Bauraum**: auch adaptiv teuer (Fläche wächst ~
   Bauraum³ / Zellperiode). Pragmatik: TPMS-Toleranz separat (z. B. 0,25 mm) oder
   Perioden-adaptive Sag-Steuerung; klar im Export-Dialog ausweisen.
3. **Selbstschnitt-Politik**: Manifold-DC kann geometrische Selbstdurchdringung erzeugen
   (Paper-Limitation). Erst messen (Validator), dann entscheiden: lokale Vertex-Relaxation,
   Intersection-free-DC-Constraints (Ju/Udeshi 06) oder Alpha-Wrap-Repair als letzte Instanz.
4. **Kontext-/Multi-Body-Export**: pro Body eigener Shell (3MF-Objekte, STL-Konkat) —
   Klärung, ob Berührflächen zweier Bodies koplanar doppelt ok sind (Slicer: ja).
5. **Weave-Vollständigkeit als Gate für Option B**: die bekannten roten Gate-Szenarien
   (Kombi-Deckel etc.) begrenzen die B-Abdeckung — Fallback-Rate im Telemetrie-Report
   mitloggen, damit wir wissen, wie viele Bodies wirklich parametrisch gehen.
6. **Langfristtrend Direct-Slicing** (nTop): Implicit → Slices ohne Mesh. Für uns kein
   Jetzt-Thema, aber ein Grund, den Export-Pass sauber hinter einer Schnittstelle zu
   kapseln (Mesh ist ein Backend, nicht die Architektur).
