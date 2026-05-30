# RFTA Full Port — Implementierungsplan

Sellán, Ren, Batty, Stein. "Reach for the Arcs: Reconstructing Surfaces from SDFs via Tangent Points." SIGGRAPH 2024.

Paper: `docs/Literatur/reach-for-the-arcs.pdf`
Reference-Code (C++/Python): github.com/odedstein/reach-for-the-arcs-code
gpytoolbox API: gpytoolbox.org/latest/reach_for_the_arcs/

## Status Quo

Die aktuelle Implementierung in `crates/kernel/src/rfta/` hat die richtige Grundstruktur (Spheres → Point Cloud → sPSR → Mesh), aber der Kernalgorithmus — Feasibility Arcs, Free-Space Sampling, Collision Resolution — wurde durch eine einfache Gradient-Projektion (`a_i = p_i − s_i · ∇SDF`) ersetzt. Das ist **nicht** RFTA, sondern nur eine Normalenprojektion plus sPSR.

## Ziel

Treue Umsetzung des Paper-Algorithmus, sodass die Qualitätsvorteile von RFTA (bessere Rekonstruktion bei niedrigen/mittleren Auflösungen, scharfe CSG-Kanten) tatsächlich erreicht werden.

## Modulstruktur

    rfta/
      mod.rs            — Pipeline-Orchestrierung (REWRITE)
      rasterize.rs      — NEU: Free-Space Sampling via Sphere-Rasterization
      feasibility.rs    — NEU: locally_make_feasible + resolve_collisions_on_sphere
      finetune.rs       — REWRITE: Echtes Fine-Tuning mit Arc-Move + Line-Search
      sphere_tree.rs    — ERWEITERN: k-nearest + containing queries
      closest_point.rs  — Beibehalten (bereits korrekt)
      poisson.rs        — Beibehalten (bereits korrekt)

## Algorithmus-Schritte

### Schritt 0: Normalisierung

Ref: `reach_for_the_arcs.py`, Zeilen 1-30

- Alle SDF-Punkte `U` und -Werte `S` auf `[0,1]^3` normalisieren (AABB berechnen, skalieren).
- Am Ende Output-Mesh zurückskalieren.
- **Warum:** Die Rasterization und Toleranzen im Paper arbeiten in normalisierten Koordinaten.

### Schritt 1: SDF Spheres aufbauen

Ref: Paper Section 3, `reach_for_the_arcs.py`

- Jedes SDF-Sample `(p_i, s_i)` wird als Sphere `S_i` mit Zentrum `p_i` und Radius `|s_i|` interpretiert.
- **Sign Separation:** Positive (`s_i > 0`) und negative (`s_i < 0`) Spheres werden **getrennt** verarbeitet. Die beiden Mengen interferieren per SDF-Definition nie.
- Spheres mit `|s_i| < tol` (≈ 1e-10) überspringen.

### Schritt 2: Free-Space Sampling (Rasterization)

Ref: Paper Section 4.1 (Figure 8, 10), `outside_points_from_rasterization.cpp`

**Input:** Menge von Spheres (nur eine Sign-Klasse)
**Output:** Punkte `w_1, ..., w_m` die außerhalb aller Spheres liegen

Algorithmus:
1. Rasterization-Grid mit Auflösung `r = 64 · ⌈(n/16)^{1/3}⌉` erstellen (3D).
2. Grid-Zellmittelpunkte mit zufälligem Offset (uniform in [0,1)) jittern.
3. Für jede Sphere: Bounding Box in Grid-Koordinaten berechnen, alle enthaltenen Zellen als "belegt" markieren. Sphere wird um Faktor `raster_shrink = 0.95` geschrumpft (konservativ).
4. **Narrow-Band Filter:** Unbesetzte Zellen, deren komplette 3×3×3-Nachbarschaft ebenfalls unbesetzt ist, werden als "zu weit weg" markiert und verworfen. Nur der schmale Streifen nahe der Sphere-Grenzen bleibt.
5. Alle verbleibenden unbesetzten Zellen → Outside Points `W`.

### Schritt 3: Initiale Tangency-Punkte

Ref: Paper Section 4.1 (Figure 8), `locally_make_feasible.cpp`

**Input:** Spheres + Outside Points `W`
**Output:** Für jede Sphere ein feasibler Tangency-Punkt (oder null)

Algorithmus pro Sphere `S_i`:
1. **Nächsten Outside-Point finden:** KD-Tree über `W`, finde `w^i = argmin_j d(w_j, S_i)` — den nächsten Punkt im leeren Raum.
2. **Auf Sphere projizieren:** `candidate = p_i + |s_i| · (w^i − p_i) / ‖w^i − p_i‖`
3. **Feasibility prüfen:** Liegt `candidate` innerhalb einer anderen Sphere? (AABB-Tree Query)
4. **Falls infeasible → `locally_make_feasible`:**
   - `k_search = ⌈2 · n^{1/3}⌉` nächste Spheres einsammeln
   - `resolve_collisions_on_sphere` aufrufen (Sphere-Intersection-Geometrie)
   - Bis zu `local_search_iters = 20` Iterationen
   - Falls nach 20 Iterationen immer noch infeasible → Sphere hat keinen Tangency-Punkt (null)
5. **Normale berechnen:** `n_i = sign(s_i) · (a_i − p_i) / ‖a_i − p_i‖` (zeigt nach außen bei negativen, nach innen bei positiven Spheres)

### Schritt 4: resolve_collisions_on_sphere

Ref: `resolve_collisions_on_sphere.cpp`, Paper Section 4.1

**Input:** Punkt `p` auf Sphere `(c, r)`, Liste blockierender Spheres `(d_j, s_j)`
**Output:** Neuer Punkt auf Sphere `(c, r)` der keine blockierende Sphere schneidet

**3D-Fall:**

**1 blockierende Sphere** → Sphere-Sphere-Intersection-Kreis:
- `dist = ‖c − d_0‖`
- `a = (r² − s_0² + dist²) / (2·dist)`
- `h = √(r² − a²)`
- Kreismittelpunkt: `mid = c + a · (d_0 − c) / dist`
- Kreisnormale: `(d_0 − c) / dist`
- `p` auf die Kreisebene projizieren, dann radial auf den Kreis: `mid + h · (proj_p − mid).normalized()`

**2+ blockierende Spheres** → Trilateration (Fang 1986):
- Für jedes Paar `(d_i, d_j)` die Trilateration mit der Home-Sphere `(c, r)` durchführen:
  - Lokales Koordinatensystem: `î = (d_i−c)/‖d_i−c‖`, `ĵ = ((d_j−c) − dot·î).normalized()`, `k̂ = î × ĵ`
  - `X = (r² − s_i² + ‖d_i−c‖²) / (2·‖d_i−c‖)`
  - `Y = (r² − s_j² + ‖d_j−c‖² − 2·(d_j−c)·î·X) / (2·(d_j−c)·ĵ)`
  - Diskriminante: `Z² = r² − X² − Y²`
  - Falls `Z² ≥ 0`: zwei Kandidaten `c + X·î + Y·ĵ ± Z·k̂`
- Aus allen Kandidaten: den wählen der (a) in bounds [0,1]³ liegt, (b) nicht in einer anderen blockierenden Sphere liegt, (c) am nächsten zu `p` ist.
- Fallback bei kollinearen Zentren: paarweise Sphere-Sphere-Intersection statt Trilateration.

### Schritt 5: Fine-Tuning Loop

Ref: Paper Section 4.2 (Figure 9, 13), `fine_tune_point_cloud_iter.cpp`

**Parameter:** `k_max = 10`, `τ = 0.01`, `line_search_iters = 20`, `max_points_per_sphere = 3`

Für `k = 0` bis `k_max − 1`:

1. **sPSR-Mesh rekonstruieren:** `Ω^k = PSR(P, N, screening_weight=10)`
2. **AABB-Baum für Mesh aufbauen** (ClosestPointBvh)
3. **Pro Sphere `S_i`:**
   a. Closest Point `c_i` auf `Ω^k` zum Sphere-Zentrum finden, Distanz `t_i`
   b. **Idealen Tangency-Punkt berechnen:**
      - Richtung `N = (c_i − p_i) / ‖c_i − p_i‖`
      - Sign-Korrektur: `N *= sign(s_i) · sign(winding_number)` (Winding Number des Sphere-Zentrums bzgl. `Ω^k`)
      - `q_i = p_i + |s_i| · N`
   c. **Punkt-Bewegung (Arc-Move + Line-Search):**
      - Great-Arc `γ` auf `S_i` von `a_i^k` nach `q_i`
      - Winkel `θ = min(τ/r, 2·arcsin(min(1, 0.5·‖a_i^k − q_i‖/r)))`
      - Rotation um Achse `(a_i^k − c) × (q_i − c)` um Winkel `θ`
      - **Line-Search:** Falls rotierter Punkt infeasible → `θ *= 0.5`, wiederholen (bis zu 20×)
   d. **Preventing Intersections:** Falls `|t_i| < |s_i|` (Mesh schneidet Sphere):
      - Zusätzliche Tangency-Punkte hinzufügen (bis zu `max_points_per_sphere`)
      - Falls schwer (`|t_i| < 0.25·|s_i|`): alle behalten + neuen hinzufügen
      - Falls leicht: weitesten von `Ω^k` verwerfen, neuen hinzufügen
   e. **Feasibility Re-Check** aller neuen/bewegten Punkte via AABB + `resolve_collisions_on_sphere`
4. **Normalen neu berechnen**

### Schritt 6: Finales Mesh + FieldDraft-Integration

- Finaler sPSR-Aufruf: `PSR(P*, N*, screening_weight=10)`
- Zurückskalieren aus [0,1]³ in Original-Koordinaten
- **FieldDraft-spezifisch:** Classifications + Contributors berechnen (wie bisher in mod.rs Step 6)

## Abnahmekriterien

### Funktional

- [ ] Free-Space Sampling via Rasterization produziert Punkte im leeren Raum zwischen Spheres
- [ ] Sign Separation: positive und negative Spheres werden getrennt verarbeitet
- [ ] `resolve_collisions_on_sphere` löst Sphere-Kollisionen analytisch (Kreis-Intersection + Trilateration)
- [ ] `locally_make_feasible` iteriert bis zu 20× um einen feasiblen Punkt auf der Sphere-Oberfläche zu finden
- [ ] Fine-Tuning-Loop läuft mindestens 3 Iterationen (bei resolution > 8) mit Arc-Move + Line-Search
- [ ] Preventing Intersections: bei Sphere-Mesh-Überschneidung werden zusätzliche Punkte hinzugefügt
- [ ] Normalen haben korrekte Orientierung (sign-abhängig)
- [ ] Mesh-Vertices liegen näher an der Oberfläche als die Gradient-Projektion (quantitativer Vergleich im Sphere-Test)
- [ ] Kompiliert ohne Warnings (`cargo build --workspace`)
- [ ] Alle bestehenden Tests in `rfta/mod.rs` bestehen (4 Tests)
- [ ] WASM-Build funktioniert (`just build-wasm` oder äquivalent)

### Nicht-funktional

- [ ] Kein `unsafe` Code
- [ ] Keine neuen externen Crate-Dependencies (KD-Tree selbst bauen oder bestehende sphere_tree.rs erweitern)
- [ ] Kein `#[allow(dead_code)]` — jeder Code wird genutzt

## Implementierungs-Checkliste

- [ ] **A. sphere_tree.rs erweitern**
  - [ ] A1. `k_closest_spheres(point, k)` — k nächste Spheres finden (Priority Queue)
  - [ ] A2. `get_spheres_containing(point, tol)` — alle Spheres die den Punkt enthalten (mit Toleranz)
  - [ ] A3. Bestehende `point_inside_any_same_sign` beibehalten

- [ ] **B. rasterize.rs (NEU)**
  - [ ] B1. `outside_points(spheres, resolution) -> Vec<[f32; 3]>` — Rasterization-Grid, Sphere-Markierung, Narrow-Band-Filter
  - [ ] B2. Rasterization-Resolution: `r = 64 · ⌈(n/16)^{1/3}⌉`
  - [ ] B3. Zufälliger Jitter pro Grid-Zelle (deterministisch via Seed)
  - [ ] B4. `raster_shrink = 0.95` Sphere-Schrumpfung

- [ ] **C. feasibility.rs (NEU)**
  - [ ] C1. `resolve_collisions_on_sphere(point, home_sphere, blockers) -> Option<[f32; 3]>` — analytische Sphere-Intersection
  - [ ] C2. 1-Sphere-Fall: Intersection-Kreis, nächster Punkt zu `p`
  - [ ] C3. 2+-Sphere-Fall: Trilateration (Fang 1986), Kandidaten-Selektion
  - [ ] C4. Bounds-Check [0,1]³ und Feasibility-Check der Kandidaten
  - [ ] C5. `locally_make_feasible(spheres, tree, outside_points, ...) -> Vec<Option<TangentPoint>>` — Orchestrierung der Feasibility-Schleife
  - [ ] C6. KD-Tree über Outside Points (oder lineare Suche falls n klein)

- [ ] **D. finetune.rs (REWRITE)**
  - [ ] D1. `finetune_iteration(spheres, tree, tangent_points, mesh_verts, mesh_tris, ...) -> bool` — ein Fine-Tuning-Schritt
  - [ ] D2. Ideal-Tangent-Punkt-Berechnung mit Sign-Korrektur (Winding Number oder Fallback)
  - [ ] D3. Arc-Move via Rotation auf Great Circle (AngleAxis-Rotation)
  - [ ] D4. Line-Search: θ halbieren bei Infeasibility, bis zu 20 Iterationen
  - [ ] D5. Preventing Intersections: zusätzliche Punkte hinzufügen bei Sphere-Mesh-Overlap
  - [ ] D6. Re-Feasibility-Check aller neuen/bewegten Punkte

- [ ] **E. mod.rs (REWRITE)**
  - [ ] E1. Normalisierung auf [0,1]³
  - [ ] E2. Sign Separation (positive/negative getrennt verarbeiten)
  - [ ] E3. Rasterization → `locally_make_feasible` → initiale Tangency-Punkte
  - [ ] E4. Fine-Tuning-Loop mit `k_max` Iterationen
  - [ ] E5. Finaler sPSR-Aufruf
  - [ ] E6. Rückskalierung + Classifications/Contributors (wie bisher)
  - [ ] E7. Bestehende Tests anpassen falls nötig

- [ ] **F. Kompilierung + Tests**
  - [ ] F1. `cargo build --workspace` ohne Fehler
  - [ ] F2. `cargo test --workspace --release` — alle Tests bestehen
  - [ ] F3. WASM-Build erfolgreich

## Hinweise für die Implementierung

- **Winding Number:** Die Reference-Implementierung nutzt libigl's `fast_winding_number`. Wir haben das nicht. Einfacher Fallback: Sign des SDF-Werts am Sphere-Zentrum verwenden (der ist ja bekannt). Oder: prüfen ob closest-point-Distanz < Sphere-Radius → Sphere schneidet Mesh.
- **KD-Tree für Outside Points:** Kann als einfache lineare Suche starten. Bei Bedarf einen simplen KD-Tree bauen (kein externes Crate).
- **Determinismus:** Zufalls-Jitter im Rasterization-Grid mit festem Seed (z.B. einfacher LCG-PRNG).
- **Performance:** Nicht optimieren. Korrektheit first. Batching (Paper: b=10000) ist optional und kann später kommen.
- **f32 vs f64:** Weiterhin f32 intern, f64 nur an der sPSR-Grenze (wie bisher in poisson.rs).
- **Normalisierung:** Die Paper-Toleranzen (tol=1e-4, raster_shrink=0.95) sind für [0,1]³ kalibriert. Deshalb ist die Normalisierung wichtig.
- **Compile-Checks:** Zwischendurch `cargo check --workspace` ist ok. Tests erst am Ende.
