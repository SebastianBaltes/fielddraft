//! Ehrliche Mesh-Metriken für die Lichen-Abnahme.
//!
//! Vier Metriken, die ein Oberflächen-Netz gegen die ECHTE Fläche (die SDF)
//! prüfen — bewusst getrennt von `lichen::mesh_quality`, das nur die
//! Kombinatorik des Netzes sieht und nichts über die Geometrie weiß:
//!
//! 1. **Hausdorff** — beidseitig. `mesh→surface` misst, ob das Netz auf der
//!    Fläche LIEGT; `surface→mesh` misst, ob es die Fläche ÜBERDECKT. Nur
//!    beide zusammen schließen sowohl Ausreißer als auch Löcher aus: ein Netz
//!    mit einem fehlenden Stück hat perfektes mesh→surface.
//! 2. **Dreieckszahl** — gegen die aus der Fläche geschätzte Soll-Zahl.
//! 3. **Manifold** — aus `lichen::mesh_quality`.
//! 4. **Aspect Ratio** — Formgüte der Dreiecke.
//!
//! Determinismus ist Pflicht (Regressions-Gate): die Stichproben kommen aus
//! einem festen LCG, nicht aus `rand`.

/// Beidseitige Hausdorff-Distanz in mm.
#[derive(Debug, Clone, Copy, Default)]
pub struct Hausdorff {
    /// max |sdf| über Stichproben AUF dem Netz — Netz liegt auf der Fläche?
    pub mesh_to_surface: f32,
    /// max Abstand Flächenpunkt → nächstes Dreieck — Netz überdeckt die Fläche?
    pub surface_to_mesh: f32,
    /// 95-Perzentil von `surface_to_mesh` — robust gegen einzelne Ausreißer.
    pub surface_to_mesh_p95: f32,
    /// Ort des schlimmsten surface→mesh-Fehlers (Loch-Lokalisierung).
    pub worst_surface_point: [f32; 3],
}

/// Formgüte-Statistik. `aspect` = Umkreisradius / (2·Inkreisradius); 1.0 für
/// das gleichseitige Dreieck, → ∞ für Nadeln und Kappen.
#[derive(Debug, Clone, Copy, Default)]
pub struct AspectStats {
    pub max: f32,
    pub p99: f32,
    pub mean: f32,
    /// Kleinster Innenwinkel über alle Dreiecke, in Grad.
    pub min_angle_deg: f32,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

/// Formgüte über alle Dreiecke.
pub fn aspect_stats(pts: &[[f32; 3]], tris: &[[u32; 3]]) -> AspectStats {
    let mut vals: Vec<f32> = Vec::with_capacity(tris.len());
    let mut min_angle = 180.0f32;
    let mut sum = 0.0f64;
    for t in tris {
        let (a, b, c) = (
            pts[t[0] as usize],
            pts[t[1] as usize],
            pts[t[2] as usize],
        );
        let (la, lb, lc) = (len(sub(b, c)), len(sub(a, c)), len(sub(a, b)));
        let s = 0.5 * (la + lb + lc);
        // Heron, gegen Auslöschung geschützt.
        let area2 = (s * (s - la) * (s - lb) * (s - lc)).max(0.0);
        let area = area2.sqrt();
        if area <= 1e-20 || s <= 0.0 {
            vals.push(f32::MAX);
            min_angle = 0.0;
            continue;
        }
        // R = abc/4A, r = A/s  →  R/(2r) = abc / (8 · A²/s) … kompakt:
        let circum = la * lb * lc / (4.0 * area);
        let inr = area / s;
        let aspect = circum / (2.0 * inr);
        vals.push(aspect);
        sum += aspect as f64;
        // kleinster Winkel liegt der kürzesten Seite gegenüber
        for (opp, x, y) in [(la, lb, lc), (lb, la, lc), (lc, la, lb)] {
            let cosv = ((x * x + y * y - opp * opp) / (2.0 * x * y)).clamp(-1.0, 1.0);
            min_angle = min_angle.min(cosv.acos().to_degrees());
        }
    }
    if vals.is_empty() {
        return AspectStats::default();
    }
    let mut sorted = vals.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p99 = sorted[((sorted.len() as f32 * 0.99) as usize).min(sorted.len() - 1)];
    AspectStats {
        max: *sorted.last().unwrap(),
        p99,
        mean: (sum / vals.len() as f64) as f32,
        min_angle_deg: min_angle,
    }
}

/// Baryzentrische Stichproben je Dreieck (Ordnung `k` → k·(k+1)/2 Punkte,
/// inkl. Ecken und Kanten-Mitten).
fn bary_samples(k: usize) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    for i in 0..=k {
        for j in 0..=(k - i) {
            let l = k - i - j;
            out.push([
                i as f32 / k as f32,
                j as f32 / k as f32,
                l as f32 / k as f32,
            ]);
        }
    }
    out
}

/// mesh→surface: max |f| über dichte Stichproben auf allen Dreiecken.
///
/// Das ist die EINSEITIGE Hausdorff-Distanz vom Netz zur Fläche — sie sieht
/// Ausreißer (Punkte/Dreiecke neben dem Körper) und Sehnen-Fehler (ein
/// Dreieck, das eine stark gekrümmte Fläche überspannt), aber KEINE Löcher.
pub fn hausdorff_mesh_to_surface<F>(pts: &[[f32; 3]], tris: &[[u32; 3]], f: F) -> f32
where
    F: Fn([f32; 3]) -> f32,
{
    let bs = bary_samples(4);
    let mut worst = 0.0f32;
    for t in tris {
        let (a, b, c) = (
            pts[t[0] as usize],
            pts[t[1] as usize],
            pts[t[2] as usize],
        );
        for w in &bs {
            let p = [
                a[0] * w[0] + b[0] * w[1] + c[0] * w[2],
                a[1] * w[0] + b[1] * w[1] + c[1] * w[2],
                a[2] * w[0] + b[2] * w[1] + c[2] * w[2],
            ];
            worst = worst.max(f(p).abs());
        }
    }
    worst
}

/// Quadrierter Abstand Punkt → Dreieck (Ericson, Real-Time Collision Detection).
fn point_tri_dist2(p: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return dot(ap, ap);
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return dot(bp, bp);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3).max(1e-20);
        let q = [a[0] + ab[0] * v, a[1] + ab[1] * v, a[2] + ab[2] * v];
        let d = sub(p, q);
        return dot(d, d);
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return dot(cp, cp);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6).max(1e-20);
        let q = [a[0] + ac[0] * w, a[1] + ac[1] * w, a[2] + ac[2] * w];
        let d = sub(p, q);
        return dot(d, d);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6)).max(1e-20);
        let q = [
            b[0] + (c[0] - b[0]) * w,
            b[1] + (c[1] - b[1]) * w,
            b[2] + (c[2] - b[2]) * w,
        ];
        let d = sub(p, q);
        return dot(d, d);
    }
    let denom = 1.0 / (va + vb + vc).max(1e-20);
    let v = vb * denom;
    let w = vc * denom;
    let q = [
        a[0] + ab[0] * v + ac[0] * w,
        a[1] + ab[1] * v + ac[1] * w,
        a[2] + ab[2] * v + ac[2] * w,
    ];
    let d = sub(p, q);
    dot(d, d)
}

/// Uniformes Gitter über die Dreiecks-AABBs — macht die Nächste-Nachbar-Suche
/// bei ~10^5 Dreiecken × ~10^5 Stichproben überhaupt erst bezahlbar.
struct TriGrid {
    cells: std::collections::HashMap<(i32, i32, i32), Vec<u32>>,
    cell: f32,
}

impl TriGrid {
    fn build(pts: &[[f32; 3]], tris: &[[u32; 3]], cell: f32) -> Self {
        let mut cells: std::collections::HashMap<(i32, i32, i32), Vec<u32>> =
            std::collections::HashMap::new();
        for (ti, t) in tris.iter().enumerate() {
            let v = [
                pts[t[0] as usize],
                pts[t[1] as usize],
                pts[t[2] as usize],
            ];
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            for p in &v {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            let i0 = [
                (lo[0] / cell).floor() as i32,
                (lo[1] / cell).floor() as i32,
                (lo[2] / cell).floor() as i32,
            ];
            let i1 = [
                (hi[0] / cell).floor() as i32,
                (hi[1] / cell).floor() as i32,
                (hi[2] / cell).floor() as i32,
            ];
            for x in i0[0]..=i1[0] {
                for y in i0[1]..=i1[1] {
                    for z in i0[2]..=i1[2] {
                        cells.entry((x, y, z)).or_default().push(ti as u32);
                    }
                }
            }
        }
        Self { cells, cell }
    }

    /// Abstand zum nächsten Dreieck. Ringweise wachsende Suche: erst der
    /// eigene Ring, dann weiter — abgebrochen, sobald der garantierte
    /// Mindestabstand des nächsten Rings die beste Distanz übersteigt.
    fn nearest(&self, p: [f32; 3], pts: &[[f32; 3]], tris: &[[u32; 3]], max_ring: i32) -> f32 {
        let base = [
            (p[0] / self.cell).floor() as i32,
            (p[1] / self.cell).floor() as i32,
            (p[2] / self.cell).floor() as i32,
        ];
        let mut best = f32::MAX;
        for r in 0..=max_ring {
            // Sobald der nächste Ring geometrisch nicht mehr näher kommen kann,
            // ist `best` bewiesen minimal.
            if best.sqrt() <= (r as f32 - 1.0) * self.cell && r > 0 {
                break;
            }
            for x in (base[0] - r)..=(base[0] + r) {
                for y in (base[1] - r)..=(base[1] + r) {
                    for z in (base[2] - r)..=(base[2] + r) {
                        // nur die SCHALE des Rings
                        let on_shell = (x - base[0]).abs() == r
                            || (y - base[1]).abs() == r
                            || (z - base[2]).abs() == r;
                        if !on_shell {
                            continue;
                        }
                        let Some(list) = self.cells.get(&(x, y, z)) else { continue };
                        for &ti in list {
                            let t = tris[ti as usize];
                            let d2 = point_tri_dist2(
                                p,
                                pts[t[0] as usize],
                                pts[t[1] as usize],
                                pts[t[2] as usize],
                            );
                            best = best.min(d2);
                        }
                    }
                }
            }
        }
        if best == f32::MAX {
            f32::MAX
        } else {
            best.sqrt()
        }
    }
}

/// Deterministischer LCG — Regressions-Gates dürfen nicht würfeln.
struct Lcg(u64);
impl Lcg {
    fn next_f32(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u32 << 24) as f32
    }
}

/// Beidseitige Hausdorff-Distanz.
///
/// `surface→mesh` ist die Richtung, die LÖCHER sieht: wir streuen Punkte in
/// die Bounding-Box, projizieren sie per Newton entlang des Gradienten auf
/// `f = 0` (das ist billig und exakt, weil wir die echte SDF haben — kein
/// Referenz-Netz nötig!) und messen von dort zum Netz. Wo das Netz ein Loch
/// hat, schlägt genau hier der Abstand aus.
pub fn hausdorff<F, G>(
    pts: &[[f32; 3]],
    tris: &[[u32; 3]],
    f: F,
    grad: G,
    lo: [f32; 3],
    hi: [f32; 3],
    samples: usize,
    cell: f32,
) -> Hausdorff
where
    F: Fn([f32; 3]) -> f32,
    G: Fn([f32; 3]) -> [f32; 3],
{
    let m2s = hausdorff_mesh_to_surface(pts, tris, &f);

    let grid = TriGrid::build(pts, tris, cell);
    let mut rng = Lcg(0x5EED_1CE5);
    let mut dists: Vec<f32> = Vec::with_capacity(samples);
    let mut worst = 0.0f32;
    let mut worst_p = [0.0f32; 3];
    let mut tries = 0usize;
    while dists.len() < samples && tries < samples * 40 {
        tries += 1;
        let mut p = [
            lo[0] + (hi[0] - lo[0]) * rng.next_f32(),
            lo[1] + (hi[1] - lo[1]) * rng.next_f32(),
            lo[2] + (hi[2] - lo[2]) * rng.next_f32(),
        ];
        // Newton auf f = 0
        let mut ok = false;
        for _ in 0..24 {
            let d = f(p);
            if d.abs() < 1e-4 {
                ok = true;
                break;
            }
            let g = grad(p);
            let gl = len(g);
            if gl < 1e-6 {
                break;
            }
            let step = (d / gl).clamp(-2.0, 2.0);
            p = [
                p[0] - g[0] / gl * step,
                p[1] - g[1] / gl * step,
                p[2] - g[2] / gl * step,
            ];
        }
        if !ok {
            continue;
        }
        // Nur Punkte INNERHALB der Box zählen — sonst misst man den Rand des
        // Sample-Fensters statt das Netz.
        if (0..3).any(|k| p[k] < lo[k] || p[k] > hi[k]) {
            continue;
        }
        let d = grid.nearest(p, pts, tris, 6);
        if d == f32::MAX {
            continue;
        }
        if d > worst {
            worst = d;
            worst_p = p;
        }
        dists.push(d);
    }
    dists.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p95 = if dists.is_empty() {
        0.0
    } else {
        dists[((dists.len() as f32 * 0.95) as usize).min(dists.len() - 1)]
    };
    Hausdorff {
        mesh_to_surface: m2s,
        surface_to_mesh: worst,
        surface_to_mesh_p95: p95,
        worst_surface_point: worst_p,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ein exakt trianguliertes Quadrat auf z=0 gegen die Ebene z=0: beide
    /// Hausdorff-Richtungen müssen ~0 sein. Sanity-Check für das Messwerkzeug
    /// selbst — ein Gate, das sein eigenes Maß nicht kalibriert, misst nichts.
    #[test]
    fn hausdorff_is_zero_for_exact_plane() {
        let n = 12;
        let mut pts = Vec::new();
        for i in 0..=n {
            for j in 0..=n {
                pts.push([i as f32 - 6.0, j as f32 - 6.0, 0.0]);
            }
        }
        let idx = |i: usize, j: usize| (i * (n + 1) + j) as u32;
        let mut tris = Vec::new();
        for i in 0..n {
            for j in 0..n {
                tris.push([idx(i, j), idx(i + 1, j), idx(i + 1, j + 1)]);
                tris.push([idx(i, j), idx(i + 1, j + 1), idx(i, j + 1)]);
            }
        }
        let h = hausdorff(
            &pts,
            &tris,
            |p| p[2],
            |_| [0.0, 0.0, 1.0],
            [-5.0, -5.0, -1.0],
            [5.0, 5.0, 1.0],
            2000,
            1.0,
        );
        assert!(h.mesh_to_surface < 1e-5, "{h:?}");
        assert!(h.surface_to_mesh < 1e-3, "{h:?}");
    }

    /// Ein LOCH im Netz muss die surface→mesh-Richtung ausschlagen lassen —
    /// und die mesh→surface-Richtung NICHT. Genau dafür messen wir beidseitig.
    #[test]
    fn hausdorff_sees_a_hole_only_in_the_surface_to_mesh_direction() {
        let n = 12;
        let mut pts = Vec::new();
        for i in 0..=n {
            for j in 0..=n {
                pts.push([i as f32 - 6.0, j as f32 - 6.0, 0.0]);
            }
        }
        let idx = |i: usize, j: usize| (i * (n + 1) + j) as u32;
        let mut tris = Vec::new();
        for i in 0..n {
            for j in 0..n {
                // 3×3-Loch in der Mitte
                if (4..7).contains(&i) && (4..7).contains(&j) {
                    continue;
                }
                tris.push([idx(i, j), idx(i + 1, j), idx(i + 1, j + 1)]);
                tris.push([idx(i, j), idx(i + 1, j + 1), idx(i, j + 1)]);
            }
        }
        let h = hausdorff(
            &pts,
            &tris,
            |p| p[2],
            |_| [0.0, 0.0, 1.0],
            [-5.0, -5.0, -1.0],
            [5.0, 5.0, 1.0],
            4000,
            1.0,
        );
        assert!(h.mesh_to_surface < 1e-5, "Netz liegt trotzdem exakt: {h:?}");
        assert!(
            h.surface_to_mesh > 1.0,
            "Loch (3×3 mm) MUSS surface→mesh ausschlagen: {h:?}"
        );
    }

    /// Das gleichseitige Dreieck hat Aspect Ratio 1, die Nadel eine große.
    #[test]
    fn aspect_ratio_is_one_for_equilateral() {
        let s = 3.0f32.sqrt() / 2.0;
        let pts = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.5, s, 0.0]];
        let a = aspect_stats(&pts, &[[0, 1, 2]]);
        assert!((a.max - 1.0).abs() < 1e-3, "{a:?}");
        assert!((a.min_angle_deg - 60.0).abs() < 0.1, "{a:?}");

        let pts2 = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.5, 0.01, 0.0]];
        let a2 = aspect_stats(&pts2, &[[0, 1, 2]]);
        assert!(a2.max > 10.0, "Nadel muss ausschlagen: {a2:?}");
    }
}
