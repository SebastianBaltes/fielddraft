//! Kleine Helfer: deterministischer RNG (SplitMix64) und ℝ³-Vektorrechnung.

/// SplitMix64 — deterministisch, seedbar, gut genug für Sample-Streuung.
/// Bewusst handgerollt statt `rand`-Dependency: die Messbank braucht nur
/// reproduzierbare Uniform-Samples.
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `[lo, hi)`.
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }

    /// Uniform verteilte Richtung auf der Einheitskugel.
    pub fn unit_dir(&mut self) -> [f64; 3] {
        loop {
            let v = [
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
            ];
            let n2 = dot(v, v);
            if n2 > 1e-6 && n2 <= 1.0 {
                let n = n2.sqrt();
                return [v[0] / n, v[1] / n, v[2] / n];
            }
        }
    }
}

// ── ℝ³ ─────────────────────────────────────────────────────────────

pub fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

pub fn normalize(a: [f64; 3]) -> Option<[f64; 3]> {
    let n = norm(a);
    if n > 1e-12 {
        Some(scale(a, 1.0 / n))
    } else {
        None
    }
}

/// Orthonormalbasis (e1, e2) senkrecht zu `u` (|u| = 1).
pub fn ortho_basis(u: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let pick = if u[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let e1 = normalize(cross(u, pick)).expect("ortho_basis: u degenerate");
    let e2 = cross(u, e1);
    (e1, e2)
}

pub fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
