//! Brute-Force-Referenzdistanz ("Ground Truth") zur exakten Nullmenge
//! eines beliebigen [`Field`]s.
//!
//! Definition: Die Ground Truth misst die **euklidische Distanz zur
//! Nullmenge des übergebenen Feldes** — also zur Oberfläche, die das
//! Feld tatsächlich definiert. Das ist die richtige Referenz für die
//! Metrizitätsfrage: "ist f(p) die echte Distanz zu {f = 0}?". Für
//! Felder mit exakter Nullmenge, aber verzerrter Metrik (min/max-
//! Booleans, smin) fällt die Nullmenge mit der gewollten Oberfläche
//! zusammen; der gemessene Fehler ist genau der Metrikverlust.
//!
//! Verfahren (zweistufig):
//!
//! 1. **Oberflächen-Punktwolke**: dichte Achsen-Linien über dem
//!    (gepolsterten) AABB in allen drei Richtungen; jeder
//!    Vorzeichenwechsel entlang einer Linie wird per Bisektion auf die
//!    Nullstelle verfeinert (Vorzeichen sind auch bei metrisch
//!    verzerrten Feldern exakt). Nearest-Neighbor-Abfragen laufen über
//!    einen k-d-Tree (kiddo).
//! 2. **Richtungs-Raffinierung**: der NN-Treffer liefert nur eine obere
//!    Schranke mit O(Gitterabstand)-Fehler (im Nahfeld bis ~h/2). Darum
//!    wird pro Abfragepunkt die Distanz als Minimum der ERSTEN
//!    Nullstelle über Strahlrichtungen minimiert (Kompass-Suche auf der
//!    Richtungskugel, Nullstellen wieder per Bisektion). Das Segment vom
//!    Punkt zu seinem nächsten Oberflächenpunkt kreuzt die Oberfläche
//!    nicht vorher — die erste Nullstelle in optimaler Richtung IST die
//!    exakte Distanz. Genauigkeit damit ~1e-5·Modellgröße statt ~h/2
//!    (validiert in `tests/ground_truth_validation.rs`).

use kiddo::{ImmutableKdTree, SquaredEuclidean};
use rayon::prelude::*;

use crate::field::Field;
use crate::util::{add, normalize, ortho_basis, scale, sub};

/// Bau-Parameter der Ground Truth.
#[derive(Debug, Clone)]
pub struct GtOptions {
    /// Linien pro Achse (n² Linien je Richtung, n Samples je Linie).
    pub grid: usize,
    /// AABB-Polster relativ zur Modellgröße (fängt Oberflächen exakt
    /// auf dem AABB-Rand ein).
    pub padding_rel: f64,
    /// Richtungs-Raffinierung aktiv (aus = roher NN-Abstand, nur für
    /// Debug/Schnellläufe).
    pub refine: bool,
}

impl Default for GtOptions {
    fn default() -> Self {
        Self { grid: 96, padding_rel: 0.04, refine: true }
    }
}

/// Referenzdistanz-Orakel für die Nullmenge von `field`.
pub struct GroundTruth<'a> {
    field: &'a dyn Field,
    tree: ImmutableKdTree<f64, 3>,
    points: Vec<[f64; 3]>,
    /// Gepolsterter Messbereich.
    pub lo: [f64; 3],
    pub hi: [f64; 3],
    /// Modellgröße = größte Ausdehnung des (ungepolsterten) AABB.
    /// Referenz für alle relativen Toleranzen und Band-Grenzen.
    pub model_size: f64,
    grid_h: f64,
    tol: f64,
    refine: bool,
}

impl<'a> GroundTruth<'a> {
    /// Baut die Oberflächen-Punktwolke von `field` über dem AABB
    /// `[aabb_min, aabb_max]` (typisch `CsgNode::aabb()`).
    pub fn build(
        field: &'a dyn Field,
        aabb_min: [f64; 3],
        aabb_max: [f64; 3],
        opts: &GtOptions,
    ) -> Self {
        let ext = sub(aabb_max, aabb_min);
        let model_size = ext[0].max(ext[1]).max(ext[2]);
        assert!(
            model_size.is_finite() && model_size > 0.0,
            "GroundTruth::build: degenerate AABB"
        );
        let pad = model_size * opts.padding_rel;
        let lo = [aabb_min[0] - pad, aabb_min[1] - pad, aabb_min[2] - pad];
        let hi = [aabb_max[0] + pad, aabb_max[1] + pad, aabb_max[2] + pad];
        let n = opts.grid.max(8);
        let tol = model_size * 1e-7;

        let step = [
            (hi[0] - lo[0]) / (n - 1) as f64,
            (hi[1] - lo[1]) / (n - 1) as f64,
            (hi[2] - lo[2]) / (n - 1) as f64,
        ];
        let grid_h = step[0].max(step[1]).max(step[2]);

        // Alle 3·n² Linien, parallel gemarcht. Jede Linie bekommt einen
        // deterministischen Sub-Step-Jitter der March-Samples: bei
        // achsparalleler Geometrie (Box-Schalen!) kann sonst ein
        // kommensurables Gitter eine dünne Wand GLOBAL verfehlen (kein
        // Gitterwert landet im Wandband → null Kreuzungen).
        let lines: Vec<(usize, usize, usize)> = (0..3)
            .flat_map(|axis| (0..n).flat_map(move |i| (0..n).map(move |j| (axis, i, j))))
            .collect();
        let points: Vec<[f64; 3]> = lines
            .par_iter()
            .flat_map_iter(|&(axis, i, j)| {
                let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
                let mut base = [0.0f64; 3];
                base[a1] = lo[a1] + i as f64 * step[a1];
                base[a2] = lo[a2] + j as f64 * step[a2];
                let jitter = {
                    let mut rng = crate::util::SplitMix64::new(
                        0x11E5_7A61 ^ ((axis as u64) << 40 | (i as u64) << 20 | j as u64),
                    );
                    rng.next_f64()
                };
                let mut out: Vec<[f64; 3]> = Vec::new();
                let mut t_prev = lo[axis];
                base[axis] = t_prev;
                let mut f_prev = field.eval(base);
                for k in 1..=n {
                    let t = (lo[axis] + (k as f64 - 1.0 + jitter) * step[axis]).min(hi[axis]);
                    base[axis] = t;
                    let f = field.eval(base);
                    if f_prev.is_finite() && f.is_finite() && (f_prev < 0.0) != (f < 0.0) {
                        let mut p = base;
                        let root = bisect_axis(field, &mut p, axis, t_prev, t, f_prev, tol);
                        p[axis] = root;
                        out.push(p);
                    }
                    t_prev = t;
                    f_prev = f;
                }
                out.into_iter()
            })
            .collect();

        assert!(
            !points.is_empty(),
            "GroundTruth::build: no surface crossings found — empty body or AABB mismatch"
        );
        let tree = ImmutableKdTree::new_from_slice(&points);
        Self {
            field,
            tree,
            points,
            lo,
            hi,
            model_size,
            grid_h,
            tol,
            refine: opts.refine,
        }
    }

    pub fn n_surface_points(&self) -> usize {
        self.points.len()
    }

    pub fn surface_points(&self) -> &[[f64; 3]] {
        &self.points
    }

    /// Signierte Referenzdistanz: Vorzeichen vom Feld (bei gültigen
    /// Implicits auch metrisch verzerrt korrekt), Betrag = exakte
    /// euklidische Distanz zur Nullmenge.
    pub fn distance(&self, p: [f64; 3]) -> f64 {
        let sign = if self.field.eval(p) < 0.0 { -1.0 } else { 1.0 };
        let nn = self.tree.nearest_one::<SquaredEuclidean>(&p);
        let d0 = nn.distance.sqrt();
        if !self.refine || d0 <= self.tol * 10.0 {
            return sign * d0;
        }

        // Seeds: Richtungen zu den k nächsten Oberflächenpunkten plus
        // (negierte) Feld-Gradientenrichtung. Mehrere NN-Seeds sind
        // wichtig an Ecken/Kanten (Punkt-Features), wo die erste
        // Nullstelle über der Richtung ein enges Tal hat und eine
        // einzelne Startrichtung die Kompass-Suche stranden lässt.
        let mut best = d0;
        let knn = self
            .tree
            .nearest_n::<SquaredEuclidean>(&p, std::num::NonZero::new(4).unwrap());
        let g = central_gradient(self.field, p, self.model_size * 1e-4);
        let u_grad = normalize(scale(g, -sign));

        let mut start = normalize(sub(self.points[nn.item as usize], p));
        for u in knn
            .iter()
            .map(|n| normalize(sub(self.points[n.item as usize], p)))
            .chain([u_grad])
            .flatten()
        {
            if let Some(t) = self.first_crossing(p, u, sign, best * (1.0 + 1e-6)) {
                if t < best {
                    best = t;
                    start = Some(u);
                }
            }
        }
        if let Some(u0) = start {
            best = self.compass(p, sign, u0, best);
        }
        sign * best
    }

    /// Kompass-Suche auf der Richtungskugel: minimiert die erste
    /// Nullstelle über die Strahlrichtung. Winkel-Schrittweite halbiert
    /// sich, sobald keine der vier Nachbarrichtungen verbessert.
    fn compass(&self, p: [f64; 3], sign: f64, u0: [f64; 3], mut best: f64) -> f64 {
        let mut u = u0;
        let mut alpha: f64 = 0.8;
        let mut rounds = 0;
        const INV_SQRT2: f64 = std::f64::consts::FRAC_1_SQRT_2;
        while alpha > 5e-4 && rounds < 300 {
            rounds += 1;
            let (e1, e2) = ortho_basis(u);
            let diag = |s1: f64, s2: f64| {
                [
                    (e1[0] * s1 + e2[0] * s2) * INV_SQRT2,
                    (e1[1] * s1 + e2[1] * s2) * INV_SQRT2,
                    (e1[2] * s1 + e2[2] * s2) * INV_SQRT2,
                ]
            };
            let mut improved = false;
            for d in [
                e1,
                scale(e1, -1.0),
                e2,
                scale(e2, -1.0),
                diag(1.0, 1.0),
                diag(1.0, -1.0),
                diag(-1.0, 1.0),
                diag(-1.0, -1.0),
            ] {
                let cand = match normalize(add(scale(u, alpha.cos()), scale(d, alpha.sin()))) {
                    Some(c) => c,
                    None => continue,
                };
                if let Some(t) = self.first_crossing(p, cand, sign, best * (1.0 + 1e-6)) {
                    if t < best * (1.0 - 1e-12) {
                        best = t;
                        u = cand;
                        improved = true;
                    }
                }
            }
            if !improved {
                alpha *= 0.5;
            }
        }
        best
    }

    /// Erste Nullstelle des Feldes entlang `p + t·u`, `t ∈ (0, t_max]`,
    /// per Vorzeichen-March + Bisektion. `None` wenn kein Wechsel bis
    /// `t_max`. Die Schrittweite ist an den GT-Gitterabstand gekoppelt,
    /// damit dünne Features nicht übersprungen werden.
    fn first_crossing(&self, p: [f64; 3], u: [f64; 3], sign: f64, t_max: f64) -> Option<f64> {
        if !(t_max > 0.0) {
            return None;
        }
        let step = (t_max / 24.0)
            .min(self.grid_h * 0.5)
            .max(t_max / 4096.0)
            .max(self.tol);
        let mut t_prev = 0.0f64;
        let mut s_prev = sign;
        let mut t = step;
        while t_prev < t_max {
            let tc = t.min(t_max);
            let f = self.field.eval(add(p, scale(u, tc)));
            let s = if f < 0.0 { -1.0 } else { 1.0 };
            if f.is_finite() && s != s_prev {
                // Bracket [t_prev, tc] → Bisektion.
                let (mut a, mut b) = (t_prev, tc);
                let mut sa = s_prev;
                for _ in 0..80 {
                    if b - a <= self.tol {
                        break;
                    }
                    let m = 0.5 * (a + b);
                    let fm = self.field.eval(add(p, scale(u, m)));
                    let sm = if fm < 0.0 { -1.0 } else { 1.0 };
                    if sm == sa {
                        a = m;
                        sa = sm;
                    } else {
                        b = m;
                    }
                }
                return Some(0.5 * (a + b));
            }
            t_prev = tc;
            s_prev = s;
            t += step;
        }
        None
    }
}

/// Bisektion einer Achsen-Linie: `p` variiert nur in `axis`,
/// Vorzeichenwechsel zwischen `a` (Feldwert-Vorzeichen `fa`) und `b`.
fn bisect_axis(
    field: &dyn Field,
    p: &mut [f64; 3],
    axis: usize,
    mut a: f64,
    mut b: f64,
    fa: f64,
    tol: f64,
) -> f64 {
    let mut sa = fa < 0.0;
    for _ in 0..80 {
        if b - a <= tol {
            break;
        }
        let m = 0.5 * (a + b);
        p[axis] = m;
        let sm = field.eval(*p) < 0.0;
        if sm == sa {
            a = m;
            sa = sm;
        } else {
            b = m;
        }
    }
    0.5 * (a + b)
}

/// Zentraler Differenzen-Gradient von `field` bei `p` mit Schrittweite `h`.
pub fn central_gradient(field: &dyn Field, p: [f64; 3], h: f64) -> [f64; 3] {
    let mut g = [0.0f64; 3];
    for (axis, ga) in g.iter_mut().enumerate() {
        let mut pp = p;
        let mut pm = p;
        pp[axis] += h;
        pm[axis] -= h;
        *ga = (field.eval(pp) - field.eval(pm)) / (2.0 * h);
    }
    g
}
