# Plan: Thicken/Offset auf allgemeinem SDF-Feld (Pivot weg vom analytischen Sonderfall)

**Status:** Spike ✅ + M0 ✅ (0.5.36, additiv) + M1 **neu ausgerichtet** auf Kantenzug-Sweep (Nachbar-Halbraum-Ansatz verworfen) + **M1-Vorbedingung „saubere Kantenzüge" ✅ (DL-77, 0.5.37)**. **Datum:** 2026-06-07. **Branch-Ziel:** `agents/fielddraft-a-dev`.

> **M1-Neuausrichtung (2026-06-07) — Lateral-Trim = Kantenzug-Sweep, nicht Nachbar-Halbräume**
>
> **Korrigierte Definition (verbindlich):** Eine B-Rep-Fläche ist **nichts weiter** als ein durch einen
> **geschlossenen Kantenzug (Edge-Loop)** berandeter Teil der `d=0`-Isofläche — beliebige Form, gekrümmt
> oder nicht. **Die Nachbarflächen müssen nicht existieren** (koplanare Split-Kante, eingeprägte Kante,
> freiform-getrimmter Rand → es gibt keinen Nachbar-Halbraum). Lateral-Trim ist damit die **direkte
> Verallgemeinerung von Sketch→Extrude**: der planare Fall *ist* „2D-Sketch-Region extrudieren", und der
> Sketch-Rand ist der Kantenzug; für gekrümmte Träger wird der Rand zu einem **3D-Kantenzug auf der
> Trägerfläche**.
>
> **Trim-Mechanik:** den Kantenzug der gepickten Face **entlang der Offset-Richtung sweepen** → eine
> verallgemeinerte, **offene** Sweep-Wand (keine End-Caps, an beiden Enden übers Band hinaus) → Band damit
> auf das Loop-Innere intersecten. Offen-gehalten besteht die Sweep-Wand **nur aus den echten
> Schnitt-Surfaces** (genau die lateralen Begrenzungen des Ergebnisses) → **keine Phantom-Flächen.**
>
> **Verworfen (warum):** Der frühere Slice-1a-Ansatz „Band ∩ analytische Nachbar-Halbräume, Halbraum als
> `Box` materialisiert" war **falsch verallgemeinert** und **blockiert**. Falsch, weil er Surfaces
> referenziert, die eine Face gar nicht definieren und oft nicht existieren. Blockiert, weil eine `Box`
> sechs Flächen hat: ihre nicht-schneidenden Flächen queren die Weave-Domain und setzen
> **Phantom-Branch-Surfaces** (SDF korrekt, B-Rep falsch). Der Würfel-Fall lief nur, weil die 4
> hochgezogenen Kanten *zufällig* mit den 4 Seitenflächen-Ebenen zusammenfielen. → `brep/face_trim.rs`
> (neighbour_gates / materialise_inside / trimmed_band) und der Slice-1a-Gate-Test **gelöscht**.
> Nachbar-Halbräume bleiben höchstens eine spätere opportunistische *Fast-Path*-Optimierung für
> rein-analytische Loops, **nicht** das Fundament.
>
> **Fließender Fall (kein Sonderfall-Bruch):** geschlossene Fläche (Kugel, Volltorus) = **kein** Edge-Loop
> → nichts zu sweepen → reines `Shell(Offset(..))` = M0. Sobald ein Loop existiert, wird er gesweept; eine
> Face, der nur ein kleiner Biss fehlt, hat einen kleinen Loop, eine Halbkugel einen großen — der Trim
> skaliert stetig. Geschlossen ist also der **No-Loop-Grenzfall** derselben *einen* Mechanik. Zylinder-
> Seitenfläche (zwei Rand-Kreise) → zwei Loops gesweept → axial begrenztes Rohr, ebenfalls dieselbe Mechanik.

> **Fortschritt**
> - **Spike (Gate) ✅** — `CsgNode::Offset` end-to-end, nicht-analytischer Träger webt selektierbar (Commit `86f7cd2`).
> - **M0 ✅ (0.5.36)** — der **Schlüssel-Vereinfacher**: eine Wand ist `Shell(Offset(carrier, out), out+in)`,
>   d. h. **Komposition zweier bereits bewiesener Operatoren** (Spike-`Offset` + Shell DL-75) statt neuer
>   Flächen-Maschinerie. Damit fällt der dual-offset-Branch-Konflikt komplett weg (genau ein Träger in genau
>   einem Shell; Shell liefert die Inner-Wall-0x800-Klassifikation gratis). Neuer Code: `brep/face_offset.rs`
>   (Träger-Lookup aus dem Spine + Leaf-Id-Remap + Band-Bau). `ThickenFeature` routet **Tessellated** Faces über
>   das allgemeine Band; **analytische** Faces behalten ihren exakten Pfad (Beweis „analytisch exakt wo faltbar":
>   `Shell(Offset(Sphere(r),out),out+in)` faltet zu `Sphere(r+out)`/`Sphere(r−in)`).
> - **Scope-Entscheidung M0 (bewusst, nicht faul):** M0 ist **additiv** — die neue Fähigkeit ist „geschlossene
>   **nicht-analytische** Fläche verdickbar" (revolved/swept Blob; analytisch unmöglich). Der analytische Pfad
>   bleibt unverändert als Fast-Path; er wird im **Park-Schritt** nur dort entfernt, wo wirklich redundant.
>   **Bewusst auf M1 verschoben** (brauchen die laterale Begrenzung, um korrekt zu sein): Zylinder/Kegel über das
>   *allgemeine* Feld (das Offsetten des begrenzten Zylinder-*Solids* würde fälschlich die Deckel mitverdicken —
>   die saubere Lösung ist Flächen-Offset + Lateral-Trim = M1) und `up-to-face` als Lens. Das deckt sich mit der
>   M0/M1-Grenze des Plans (partielle Zylinder/Kegel sind ohnehin M1).
> - **Komposit-Träger** (Boolean-Ergebnis) → klarer Err, deferred (würde sonst jede Fläche verdicken).

**Auslöser:** Designentscheidung — das analytische, pro-Primitiv `face_thicken.rs` deckt nur die
4 „Spielzeug"-Grundformen exakt ab. Reale Flächen entstehen aus Sketch→Extrude/Revolve/Twist→Boolean
und sind ohnehin nicht analytisch. Der allgemeine Feld-Offset ist breiter **und robuster** und
läuft bei uns bereits produktiv als **Shell** (`open_shell_sdf`/`OpenSurface`, erprobt auf
gekrümmten Körpern, DL-75). Ziel: den allgemeinen Weg zur Grundlage machen, Analytik nur noch als
opportunistische Optimierung.

---

## 0. Leitidee & Invariante

Eine Wand ist der **Offset eines Feldes**, nicht die Differenz zweier Primitive.

- **Ein neuer Operator** trägt alles: `Offset(child, δ) := child.sdf(p) − δ` (Dilatation δ>0,
  Erosion δ<0). Allgemein für *jedes* Kindfeld, selbstüberschneidungsfrei (Kernvorteil ggü.
  B-Rep-Offset, der an konkaven Stellen self-intersected und Cleanup braucht).
- **Wand/Band einer Fläche** = `Difference(Offset(carrier, out), Offset(carrier, −in))`
  = `max(g − out, −(g + in))` — ein Schalenband der Dicke `out+in` um die Trägerfläche `g`.
- **`carrier` = Trägerfeld der gepickten Face:**
  - analytisch → **`HalfSpace`** (unbegrenzte Trägerfläche, aus `branch_halfspace_map`),
  - nicht-analytisch → **Leaf-Subtree** des Features (aus `branch_leaf_map` / `Field::Leaf`).
  Beide Maps baut der Weave **heute schon** pro Branch-ID — genau das Tor, durch das die Wand
  wieder **selektierbare B-Rep-Faces** bekommt.
- **Lateral-Trim** isoliert die gepickte Face aus dem (i. A. unbegrenzten) Band.
- **Analytik bleibt als Fast-Path**, aber ohne Sonder-Feature: der Offset eines Sphere-/Cylinder-/
  Plane-Feldes *faltet sich wieder zu einem `HalfSpace`* → exakte Faces dort, wo es geht.

---

## 1. Kern-Baustein `CsgNode::Offset(Box<CsgNode>, f32)`

Der minimale, breit nützliche Operator (Dilate/Erode beliebiger Körper, weit über Thicken hinaus).

**Zu implementieren in `csg.rs`:**
- `evaluate` / Dual-SDF (`sdf_dual.rs`): `child − δ` (Wert und Gradient unverändert, nur Offset).
- `aabb`/`bbox`: child-AABB isotrop um `δ` aufblasen.
- Lipschitz-/Eigenschafts-Propagation: `is_non_lipschitz`/`is_space_filling`/L-Schätzung des Kindes
  erben (DL-70/71-Infrastruktur — Offset eines nicht-Lipschitz-Feldes bleibt nicht-Lipschitz).
- Tree-Visitor/`lower`/Edge-Fillet-Recursion-Arme.

**Ripple (neuer Variant) — bekannte, begrenzte Liste (~7 Dateien):**
`csg.rs` (10 Match-Stellen), `body.rs`, `bvh.rs`, `sdf_compiler.rs` (WGSL-Codegen!),
`step_classifier.rs`, `brep/weaving.rs`, `brep/extraction.rs`. Jede ist ein mechanischer Arm —
kompiliert erst wieder, wenn alle erschöpfend gematcht sind (gut: der Compiler erzwingt Vollständigkeit).

**DIE zentrale Integration — Branch-Maps müssen `Offset` durchschauen:**
- `branch_halfspace_map`: ist das Kind eine analytische Leaf-Fläche und `δ` faltbar, den
  **verschobenen `HalfSpace`** emittieren → exakte analytische Face:
  - `Sphere(r) ∓ δ` → `Sphere(r∓δ)`; `InfiniteCylinder(r)` → `r∓δ`; `Plane(off)` → `off±δ`;
    `Torus(R,r)` minor → `r∓δ`. (`Cone`: Apex-Versatz → vorerst Leaf-Fallback, vgl. DL-76.)
- Sonst `branch_leaf_map` liefert `Field::Leaf{ Offset-Subtree }` → der **Tracer behandelt es
  bereits** (DL-69-Leaf-Pfad). ⇒ Wand-Faces entstehen für analytisch *und* nicht-analytisch ohne
  neuen Weave-Code.

Genau hier wird die im Memo behauptete „Generalität gratis" eingelöst: **die Maps existieren, sie
müssen nur einen Knoten weiter laufen.**

---

## 2. M0 — Geschlossene Flächen über das allgemeine Band (ersetzt `face_thicken.rs`)

**Ziel:** den analytischen Pro-Primitiv-Pfad durch *einen* allgemeinen Band-Bau ersetzen; bei
geschlossenen Flächen ist **kein** Lateral-Trim nötig.

- Neuer Resolve (ersetzt `resolve_thicken_faces`/`resolve_thicken_to_face`): gepickte Face →
  Trägerfeld (HalfSpace **oder** Leaf-Subtree) → Knoten
  `Difference(Offset(carrier, out), Offset(carrier, −in))`.
- **Geschlossene Träger** (Sphere, voller Torus, voll-revolvierter Zylinder via vorhandene
  Rim-Erkennung): Band = fertige Wand.
- **Deckt sofort mehr ab als heute:** geschlossene *nicht-analytische* Flächen (revolvierter Blob,
  geschlossener Sweep) werden verdickbar — analytisch unmöglich.
- **`guard_inner`-Äquivalent** bleibt: `in ≥ lokaler Krümmungsradius` ⇒ Erosion verschluckt das
  Feature → Err/Warn (numerisch statt algebraisch geprüft).
- **TNP/Identität:** Wand hängt weiter am Quell-`id_hash` (`qFace`), regenerationsstabil.

**`up-to-face` wird allgemeiner *und* restriktionsärmer.** Onshape „bis zur Fläche" = der Lens
**zwischen** Quell- und Zielfläche, nicht ein Konstant-Offset:
`Intersection(Material-Seite-der-Quelle, bis-zur-Ziel-Seite)` — eine CSG der beiden Trägerfelder.
Das **entfernt die Koaxial-/Gleiche-Art-Einschränkung komplett** (füllt auch den Spalt zweier
nicht-paralleler Freiform-Flächen, mit korrekt variabler Dicke). Konstant-Offset-Sonderfall wird
numerisch erkannt (Gap-Varianz ≈ 0), nicht pro Primitivpaar algebraisch.

**Migration:** analytische Faltlogik (Sphere/Cyl-Offset → HalfSpace) **wandert** nach §1
(`branch_halfspace_map`-Offset-Faltung) — sie wird *umgezogen, nicht gelöscht*. Der Rest von
`face_thicken.rs` (Pro-Primitiv-Rim-Rekonstruktion, ThickenSolid-Paarbau) entfällt.

---

## 3. M1 — Laterale Begrenzung über **Kantenzug-Sweep** (löst das vertagte „partielle Wand")

**Ziel:** offene/berandete Flächen — ein Patch, ein nur 90°-revolvierter Zylinder/Kegel, eine
Extrude-Seitenwand — auf genau die gepickte Face beschränken.

**Definition (verbindlich, siehe Neuausrichtungs-Block oben):** Eine Face = `d=0`-Isofläche, berandet
durch ihre **Edge-Loops**. Nur der Loop definiert sie; Nachbarflächen sind irrelevant und müssen nicht
existieren. Lateral-Trim ist die Verallgemeinerung von Sketch→Extrude.

**Mechanik (der eine Pfad):**
1. **Kantenzug holen:** die Edge-Loops der gepickten Face als 3D-Polylinien auf der Trägerfläche
   abtasten (`brep/edge_sampling.rs::sample_edge` liefert die Loop-Punkte — Rand-Kreise haben keine
   Vertices und müssen gesampelt werden).
2. **Offen sweepen:** den Loop entlang der **Offset-Richtung** (Träger-Normalenfeld, ein-/auswärts über
   die ganze Banddicke und etwas darüber hinaus) zu einer **offenen Sweep-Wand** ohne End-Caps machen.
   Offen = nur die lateralen Schnitt-Surfaces, **keine Phantom-Flächen** (das war der ganze Blocker).
3. **Intersecten:** `Intersection(Band, Sweep-Wand-Innenraum)` = nur die Wand der gepickten Face.

Das ist die einzige genuin *neue* Geometrie — als härteste Teilaufgabe gesondert verifizieren
(Loop-Sampling/Weld, konkave Loops, mehrere Loops pro Face, Drift entlang gekrümmter Träger).

**M1-Vorbedingung GELÖST (DL-77, 0.5.37): saubere Kantenzüge aus dem Weave.** Der Schritt-1-Sample
setzt *konsolidierte* Edge-Loops voraus — die der Weave für nicht-analytische Faces vorher NICHT lieferte:
eine Blob-Extrude (4 Quad-Bézier-Wände) wob die Rim-Naht (Wand ∩ analytischer Cap) in ~70 Ein-Kanten-
Müll-Loops je Face statt in *einen* sauberen Bogen (empirisch: 6 Faces, 290 Kanten, 14 Verts, jede Face
mit 72–143 Innen-Loops). Ursache: die `leaf_involved && same_feature`-Sperre schickte die eigene Rim-Naht
auf die Sample-Fits (Linie/Kreis), die an einem Bézier-Bogen scheitern → `chain_group`-Zersplitterung;
zusätzlich lagen die Rim-Ecken ~1 Zelle daneben (`refine_triple_vertex` kann nur analytische Branches).
Fix in drei Teilen: (a) **`Field::ExtrudeWall`** — das cap-clamp-freie, *vorzeichenbehaftete* Profil-SDF
über die Closest-Segment-Geometrie (Normale = 90°-gedrehte Kurventangente, sauber auch *auf* der Kurve,
wo `∇‖p−closest‖` 0/0 ist und das Flatten-Polygon-Vorzeichen inkonsistent wird); die Naht Wand∩Cap traced
damit als *ein* offener Bogen. (b) **`extrude_rim_corner`** — die Rim-Ecke ist exakt der geteilte
G1-Gruppen-Segment-Endpunkt, auf die Cap-Ebene gehoben → resolution-unabhängig, fusioniert die
verdoppelten/spurious Ecken via `merge_refined_by_branch_set`. (c) Sperre verfeinert: nur Wand∩Wand bleibt
auf dem Fallback. Ergebnis Blob jetzt **8 Verts / 12 Kanten / 6 Faces**, jede Face genau 4 Rand-Kanten,
0 Innen-Loops, watertight. Sector-Wände (kein Segment-Rand) behalten bewusst den `Leaf`-Pfad (DL-69).
Damit liefert Schritt 1 jetzt echte Kantenzüge; der Sweep selbst (Schritt 2/3) ist der nächste Schritt.

- **Auflösung:** partiell-revolvierte Zylinder/Kegel-Wände, Extrude-Seitenwände, beliebige Patches —
  alle bisher „vertagt".
- **Verworfen:** „Band ∩ analytische Nachbar-Halbräume" (Halbraum als `Box`) — falsch verallgemeinert
  (referenziert nicht-existente Nachbarn) und Weave-blockiert (Box-Phantom-Flächen). Nur als spätere
  opportunistische Fast-Path-Option für rein-analytische Loops denkbar, nicht als Fundament.

---

## 4. M2 — Thicken/Shell-Konsolidierung (nach Stress-Erfahrung, NICHT vorziehen)

Shell = einwärts-Offset eines ganzen Körpers mit Open-Face-Taper (`e` = variabler Offset). Prüfen,
ob ein vereinheitlichtes Offset-Feature mit Modi {Shell-einwärts, Thicken-Band, Dilate} lohnt, oder
ob beide Features getrennt bleiben, aber auf *einem* Substrat (Offset + Branch-Map-Plumbing). Wie
M2/Merge: **erst Erfahrung sammeln, nicht autonom starten.**

---

## 5. Parken & Versionierung

- Tag/Branch `archive/thicken-analytic-offset` auf aktuellem HEAD (`2349d6b`, v0.5.35) als
  abrufbarer Stand.
- Dann analytisches `face_thicken.rs` aus der Dev-Linie entfernen (kein Backward-Compat-Schleppen).
  **Faltlogik migriert** nach §1, nicht gelöscht.
- Breaking (Thicken-Repr/-Semantik ändert sich) → 0.x-Minor → **0.6.0**. `just build-wasm`,
  Cargo.toml + web/package.json + web/src/wasm/package.json im Gleichschritt, WASM-Artefakt committen.

---

## 6. Risiken & offene Punkte (vor Zusage zu klären)

1. **Leaf-Feld-Verfügbarkeit pro Feature.** DL-69 hatte Fallbacks (koaxialer equal-radius-Node,
   cross-feature-Crease). Träger-Extraktion für Extrude/Revolve/Twist/Boolean-Ergebnisse muss
   **vor** der Zusage voller Abdeckung verifiziert werden. → **Spike zuerst** (§7).
2. **Lipschitz.** Offset von Twist/Thread/Gyroid muss L-Baking tragen (DL-70/71) — sonst
   Renderer/Weave-March falsch.
3. **Selbstüberschneidung.** `out > Krümmungsradius` fängt der SDF-Offset *robust* ab (Vorteil);
   `in` zu groß verschluckt dünne Features → Guard.
4. **Trim-Prisma-Robustheit** auf konkaven Loops (M1-Kern).
5. **Determinismus/`id_hash`-Stabilität** der Wand-Faces (TNP) erhalten.
6. **WGSL-Codegen** (`sdf_compiler.rs`) für `Offset` — Render-Pfad muss den Knoten kennen.

---

## 7. Validierung & Reihenfolge

**Spike (Gate vor dem Parken):** `Offset`-Knoten minimal + `branch_*_map`-Durchlauf; an *einer*
geschlossenen **nicht-analytischen** Face (z. B. revolvierter Querschnitt) zeigen, dass das Band
webt und selektierbare Faces liefert. Erst wenn das grün ist, analytisch parken.

**Pro Milestone:** Kernel-Unit + Integration; Weave-Wasserdichtheit (Euler F/E/V); Headful-WebGPU-E2E
(Band rendert, AABB stimmt); `tsc`/`vitest`; `cargo clippy`; `just build-wasm`.

| Schritt | Inhalt | Gate |
|--------|--------|------|
| Spike | `Offset` + Map-Durchlauf, 1 nicht-analytische geschlossene Face | webt + selektierbar |
| M0 | allg. Band ersetzt `face_thicken.rs`; up-to-face als Lens; Analytik-Faltung migriert | alte + neue Thicken-Tests grün, analytisch exakt wo faltbar |
| Parken | `archive/…`-Tag, analytischen Rest entfernen, 0.6.0 | volles Gate |
| M1 | Lateral-Trim (Gates + Prisma), partielle Wände | partiell-revolvierte/Extrude-Wand-Tests |
| M2 | Shell/Thicken-Konsolidierung prüfen | nach Erfahrung |

**Entscheidungs-Gates:** nach Spike (Träger-Extraktion tragfähig?), nach M0 (parken?), nach M1
(M2 lohnend?).
