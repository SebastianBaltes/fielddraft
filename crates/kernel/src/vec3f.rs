//! Kernel-weite `[f32; 3]`-Vektor-Algebra — das f32-Pendant zu
//! [`crate::brep::vec3`] (f64, B-Rep-Numerik).
//!
//! Diese Zwei-Zeilen-Routinen waren zuvor in `sweep_path`, `raycast`,
//! `dc` und `brep::face_profile` kopiert (teils unter anderen Namen wie
//! `cross3`/`dot3`). Hier ist die Single Source of Truth; Module
//! importieren sie und aliasen bei Bedarf.

pub type V3 = [f32; 3];

pub(crate) fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub(crate) fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn norm(a: V3) -> f32 {
    dot(a, a).sqrt()
}

/// Einheitsvektor; ein (nahezu) Null-Vektor bleibt der Null-Vektor, damit
/// Aufrufer nie NaN/∞ produzieren.
pub(crate) fn normalize(a: V3) -> V3 {
    let l = norm(a);
    if l > 1e-12 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// Einheitsvektor oder `None` für einen (nahezu) Null-Vektor — für
/// Aufrufer, die Degeneration explizit behandeln (z.B. Ray-Richtung).
pub(crate) fn try_normalize(a: V3) -> Option<V3> {
    let l = norm(a);
    if l < 1e-12 {
        None
    } else {
        Some([a[0] / l, a[1] / l, a[2] / l])
    }
}
