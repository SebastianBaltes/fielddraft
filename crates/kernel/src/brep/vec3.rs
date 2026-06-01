//! Shared small `[f64; 3]` vector helpers for the B-Rep modules.
//!
//! These two-line routines were previously copy-pasted into `weaving`,
//! `edge_walker` and `naming` (under slightly different names). This is the
//! single source of truth; the modules import them, aliasing as needed.

pub(crate) fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub(crate) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

/// Unit vector; returns the input unchanged for a near-zero (degenerate)
/// vector so callers never produce NaNs/infinities.
pub(crate) fn normalize(a: [f64; 3]) -> [f64; 3] {
    let n = norm(a);
    if n > 1e-12 {
        scale(a, 1.0 / n)
    } else {
        a
    }
}

pub(crate) fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = sub(a, b);
    dot(d, d)
}

pub(crate) fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    dist2(a, b).sqrt()
}
