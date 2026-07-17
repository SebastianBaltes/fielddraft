"""E1: Gradientennorm-Korrekturen auf GLATTEN Feldern.

Frage: Welche Genauigkeitsordnung liefern Sampson (f/|grad f|), Taubin 2. Ordnung
und iterierte Normalisierung nahe einer glatten Flaeche — und wie schnell
degradiert das mit dem Abstand?

Testfelder mit analytischer Wahrheit:
  A  algebraischer Kreis  f = (x^2+y^2-R^2)/(2R)   (|grad f| = r/R, glatt)
  B  skaliertes SDF       f = s*(r-R), s=1/sqrt(2)  (globaler Skalenfehler wie
     unnormalisiertes smin-Feld im Visual-Pfad)
  C  smin-Union zweier fast-tangentialer Kreise (der adversarielle sqrt(2)-Fall)
"""

import numpy as np

from lib import (
    GroundTruth,
    boundary_by_rays,
    band_table,
    circle,
    gradnorm_fd,
    iterated_normalization,
    print_band_table,
    sampson,
    smin_poly,
    taubin2,
)

rng = np.random.default_rng(7)
R = 1.0


def sample_annulus(center, r_lo, r_hi, n):
    th = rng.uniform(0, 2 * np.pi, n)
    rr = rng.uniform(r_lo, r_hi, n)
    return np.stack([center[0] + rr * np.cos(th), center[1] + rr * np.sin(th)], axis=-1)


def report(name, field, truth, pts, edges):
    d = truth(pts)
    for label, approx in [
        ("f roh", field(pts)),
        ("Sampson f/|g|", sampson(field, pts)),
        ("Taubin 2. Ord", taubin2(field, pts)),
        ("iterNorm k=2", iterated_normalization(field, pts, 2)),
        ("iterNorm k=3", iterated_normalization(field, pts, 3)),
    ]:
        rows = band_table(d, approx - d, edges)
        print_band_table(f"[{name}] {label}", rows)


def order_fit(name, field, truth, center, direction, ds):
    """Fehler entlang eines Normalstrahls, log-log-Steigung."""
    print(f"\n[{name}] Konvergenzordnung entlang Normalstrahl (log-log-Fit):")
    pts = center + ds[:, None] * direction
    d = truth(pts)
    for label, approx in [
        ("f roh", field(pts)),
        ("Sampson", sampson(field, pts)),
        ("Taubin2", taubin2(field, pts)),
        ("iter2", iterated_normalization(field, pts, 2)),
    ]:
        err = np.abs(approx - d)
        mask = err > 1e-13
        if mask.sum() >= 3:
            slope = np.polyfit(np.log(ds[mask]), np.log(err[mask]), 1)[0]
        else:
            slope = float("nan")
        print(
            f"  {label:>9}: err(d=0.01)={np.interp(0.01, ds, err):.2e} "
            f"err(d=0.3)={np.interp(0.3, ds, err):.2e}  Ordnung≈{slope:.2f}"
        )


edges = np.array([0.001, 0.01, 0.05, 0.1, 0.2, 0.4])

# ── A: algebraischer Kreis ─────────────────────────────────────────
print("=" * 72)
print("A) algebraischer Kreis f=(r^2-R^2)/(2R)")


def field_a(p):
    return (p[..., 0] ** 2 + p[..., 1] ** 2 - R * R) / (2 * R)


def truth_a(p):
    return np.hypot(p[..., 0], p[..., 1]) - R


pts = sample_annulus((0, 0), 0.6, 1.4, 20000)
report("A", field_a, truth_a, pts, edges)
order_fit("A", field_a, truth_a, np.array([R, 0.0]), np.array([1.0, 0.0]),
          np.geomspace(1e-3, 0.4, 40))

# ── B: skaliertes SDF ──────────────────────────────────────────────
print("\n" + "=" * 72)
print("B) skaliertes SDF f = (r-R)/sqrt(2)  — Modell des unnormalisierten smin-Visual-Pfads")


def field_b(p):
    return (np.hypot(p[..., 0], p[..., 1]) - R) / np.sqrt(2.0)


report("B", field_b, truth_a, pts, edges)

# ── C: fast-tangentiale smin-Union ─────────────────────────────────
print("\n" + "=" * 72)
print("C) smin-Union zweier fast-tangentialer Kreise, r_blend=0.25")
# Zentren so, dass sich die Kreise gerade eben schneiden (fast tangential):
gap = 0.02  # Überlappungstiefe: d_centers = 2R - gap
c1 = np.array([-(R - gap / 2), 0.0])
c2 = np.array([+(R - gap / 2), 0.0])
r_blend = 0.25
f1 = circle(c1, R)
f2 = circle(c2, R)


def field_c(p):
    return smin_poly(f1(p), f2(p), r_blend)


bpts = boundary_by_rays(field_c, np.array([0.0, 0.0]), n_rays=400_000, r_max=4.0)
truth_c = GroundTruth(bpts, field_c)

# Punkte im Aussen-Nahfeld um die Tangentialzone (dort ist |grad f|≈sqrt(2))
pts_c = sample_annulus((0, 0), 0.0, 0.5, 40000)
pts_c = pts_c[field_c(pts_c) > 0]  # nur aussen
gn = gradnorm_fd(field_c, pts_c)
print(f"\n|grad f| im Aussenfeld nahe Tangentialzone: max={gn.max():.4f} "
      f"(sqrt(2)={np.sqrt(2):.4f}), Anteil >1.2: {(gn > 1.2).mean():.1%}")
report("C", field_c, truth_c, pts_c, np.array([0.001, 0.02, 0.05, 0.1, 0.2]))
