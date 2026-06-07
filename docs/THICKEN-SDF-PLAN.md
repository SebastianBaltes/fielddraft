# Plan: Thicken/Offset auf allgemeinem SDF-Feld (Pivot weg vom analytischen Sonderfall)

**Status:** Spike ✅ + M0 ✅ (0.5.36, additiv) + M1 Slice-0 ✅ / Slice-1a ⛔ blockiert. **Datum:** 2026-06-07. **Branch-Ziel:** `agents/fielddraft-a-dev`.

> **M1-Fortschritt & Blocker (2026-06-07)**
> - **Slice-0 (Nachbar-Discovery) ✅** — `brep/face_trim.rs::neighbour_gates` liest die berandenden
>   analytischen Nachbar-Halbräume direkt aus der gewebten BRep (über `edge.signature`), ohne
>   `BoundedCsg`/`branch_halfspace_map`-Umweg. Plus `materialise_inside` (Halbraum→bounded Primitive)
>   und `face_boundary_extent`. Unit- + Real-Topologie-Tests grün.
> - **Slice-1a-Gate ✅ (commit `a8a3ea4`)** — `Intersection(Shell(Offset(oversized carrier)),
>   Nachbar-Halbräume)` webt im Kleinen wasserdicht + selektierbar; **kein** neuer Knoten nötig, weil
>   jeder `HalfSpace` ein echtes bounded `Primitive` ist.
> - **Slice-1a ⛔ BLOCKER (Weave-Extraktion):** Die *Materialisierung eines Halbraums als `Box`* ist
>   die **falsche Trim-Repräsentation** für den Octree-Weave. Eine Box hat sechs Flächen; ihre
>   nicht-schneidenden Flächen (tiefer Deckel entlang der Normale, Seitenflächen) queren die
>   Weave-Domain und setzen **Phantom-Branch-Surfaces** → der Weave extrahiert falsch (SDF korrekt, aber
>   nur Phantom-Plane + Innenwand, Außenwand & echte Caps fehlen). Das Gate überlebte nur, weil seine
>   *eine enge* Box knapp außerhalb einer winzigen Domain lag. `trimmed_band` ist gebaut, aber
>   `#[ignore]` und **nicht** in `resolve_thicken_faces` verdrahtet.
> - **Design-Fork (Entscheidung nötig):** (a) dedizierter `CsgNode`-Trim-Knoten mit `OpenSurface`-Gates
>   = reine Halbraum-SDF ohne Phantom-Flächen (die ursprüngliche R1a des Plans, voller Ripple + Weave
>   muss die Schnitt-Cut-Faces selektierbar machen), oder (b) dem Weave beibringen, die
>   nicht-schneidenden Flächen eines Trim-Primitivs zu ignorieren. (a) ist sauberer, mehr Arbeit.

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

## 3. M1 — Laterale Begrenzung (löst das vertagte „partielle Wand")

**Ziel:** offene/berandete Flächen — ein Patch, ein nur 90°-revolvierter Zylinder/Kegel, eine
Extrude-Seitenwand — auf genau die gepickte Face beschränken.

- Eine Face ist von **Edge-Loops** berandet. Lateral-Bound = `Intersection` von Kanten-Gates.
- **Analytische Nachbarn** (Caps, angrenzende Planes): jede Nachbarfläche liefert ein
  `OpenSurface`-artiges Euklid-Gate (∇g≈1, wohlkonditioniert) → Band ∩ (innerhalb aller
  Nachbar-Gates) = nur die Wand der gepickten Face. **Direkte Wiederverwendung** des Shell-Gate-Codes.
- **Allgemeiner Fallback:** Edge-Loop-Polylinien entlang der Träger-Normale zu einem
  **Begrenzungs-Prisma** (Loop-Extrude/Kapsel-SDF) sweepen, `Intersection` mit dem Band. Das ist die
  einzige genuin *neue* Geometrie — als härteste Teilaufgabe gesondert verifizieren (Tracer/Weld,
  konkave Loops).
- **Auflösung:** partiell-revolvierte Zylinder/Kegel-Wände, Extrude-Seitenwände, beliebige Patches —
  alle bisher „vertagt".

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
