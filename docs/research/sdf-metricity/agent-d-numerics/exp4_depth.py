"""E4: Fehlerfortpflanzung ueber smin-Verschachtelungstiefe.

Formales Modell (hergeleitet aus den blend_min_weights: (w_a,w_b) ist
EINHEITSVEKTOR im Argumentraum):
  |∇ smin(a,b)|² = w_a²|∇a|² + w_b²|∇b|² + 2 w_a w_b ∇a·∇b
  → Fehler kombiniert in QUADRATUR, nicht multiplikativ.
Vorhersagen:
  P1 (koinzident): m identische Kreise gefaltet → |∇f| = √m im Blend,
     Flaechen-Inflation d* = r(1−1/√m), Ueberschaetzung f/d_true → √m.
     Sampson korrigiert exakt (reiner Normfehler).
  P2 (gestaffelt-adversarial): L_{k} = (L_{k-1}+1)/√2 → Fixpunkt √2+1 ≈ 2.414.
  P3 (realistische Kette): Blend-Zonen disjunkt → keine Akkumulation,
     max|∇f| ≈ √2-nah konstant ueber N.
  P4 (Zufallskonfigurationen): praktisch nie ueber P1 hinaus.

Fazit-Kandidat: Fehler waechst hoechstens wie √(Tiefe), NICHT (√2)^Tiefe —
und NUR bei koinzidenten Flaechen; sonst saturiert er.
"""

import numpy as np

from lib import (
    GroundTruth,
    boundary_by_rays,
    circle,
    gradnorm_fd,
    sampson,
    smin_poly,
)

R = 1.0
rng = np.random.default_rng(23)

# ── P1: koinzidenter Stack ─────────────────────────────────────────
print("P1) m identische Kreise (R=1), sequentiell smin-gefaltet, r=0.3:")
print(f"  {'m':>3} {'pred √m':>8} {'max|∇f|':>9} {'max f/d':>9} "
      f"{'nach Sampson max |f/|g|-d|':>26}")
r_blend = 0.3
for m in [2, 4, 8, 16]:
    f0 = circle(np.zeros(2), R)

    def field(p, m=m):
        acc = f0(p)
        for _ in range(m - 1):
            acc = smin_poly(acc, f0(p), r_blend)
        return acc

    bpts = boundary_by_rays(field, np.zeros(2), n_rays=100_000, r_max=4.0)
    truth = GroundTruth(bpts, field)
    th = rng.uniform(0, 2 * np.pi, 40_000)
    rr = rng.uniform(0.9, 1.6, 40_000)
    pts = np.stack([rr * np.cos(th), rr * np.sin(th)], -1)
    d = truth(pts)
    out = d > 1e-3
    f = field(pts)
    gn = gradnorm_fd(field, pts[out])
    s = sampson(field, pts[out])
    print(f"  {m:>3} {np.sqrt(m):>8.3f} {gn.max():>9.3f} "
          f"{np.max(f[out]/d[out]):>9.3f} {np.abs(s - d[out]).max():>26.2e}")

# ── P2: gestaffelt-adversarial ─────────────────────────────────────
print("\nP2) gestaffelt: Kreis k mit Radius so, dass d_k ≈ akkumulierter Wert")
print("    im Probepunkt (1.05, 0); Fixpunkt-Vorhersage L* = √2+1 ≈ 2.414")
probe = np.array([[1.05, 0.0]])
radii = [R]
acc_val = float(circle(np.zeros(2), radii[0])(probe)[0])
for k in range(1, 12):
    # neuer Kreis: Radius so, dass seine Distanz am Probepunkt = acc_val
    radii.append(float(np.linalg.norm(probe[0]) - acc_val))

    def field(p, radii=tuple(radii)):
        acc = circle(np.zeros(2), radii[0])(p)
        for Rk in radii[1:]:
            acc = smin_poly(acc, circle(np.zeros(2), Rk)(p), r_blend)
        return acc

    acc_val = float(field(probe)[0])
    gn = float(gradnorm_fd(field, probe)[0])
    if k in (1, 2, 3, 5, 8, 11):
        print(f"  Tiefe {k:>2}: |∇f|(probe) = {gn:.4f}")

# ── P3: realistische Kette ─────────────────────────────────────────
print("\nP3) Kette: N Kreise R=1 bei x=0,1.2,2.4,…, smin r=0.25, Grid-Statistik:")
print(f"  {'N':>3} {'max|∇f| (aussen)':>17} {'max f/d (aussen)':>17}")
for N in [2, 4, 8]:
    centers = [np.array([1.2 * i, 0.0]) for i in range(N)]

    def field(p, centers=tuple(map(tuple, centers))):
        acc = circle(np.array(centers[0]), R)(p)
        for c in centers[1:]:
            acc = smin_poly(acc, circle(np.array(c), R)(p), 0.25)
        return acc

    mid = np.array([0.6 * (N - 1), 0.0])
    bpts = boundary_by_rays(field, mid, n_rays=300_000, r_max=3.0 + 0.6 * N)
    truth = GroundTruth(bpts, field)
    lo = np.array([-2.0, -2.0])
    hi = np.array([1.2 * (N - 1) + 2.0, 2.0])
    pts = rng.uniform(0, 1, (300_000, 2)) * (hi - lo) + lo
    d = truth(pts)
    out = (d > 1e-3) & (d < 0.6)
    gn = gradnorm_fd(field, pts[out])
    f = field(pts)
    print(f"  {N:>3} {gn.max():>17.4f} {np.max(f[out]/d[out]):>17.4f}")

# ── P4: Zufallskonfigurationen ─────────────────────────────────────
print("\nP4) 40 Zufallskonfigs je N (Zentren u.a.r. in [-1,1]^2, R in [0.5,1.2],")
print("    r_blend in [0.1,0.4]): Verteilung von max|∇f| (aussen, nahe):")
for N in [2, 4, 8]:
    maxg = []
    for trial in range(40):
        centers = rng.uniform(-1, 1, (N, 2))
        rads = rng.uniform(0.5, 1.2, N)
        rb = rng.uniform(0.1, 0.4)

        def field(p, centers=centers, rads=rads, rb=rb):
            acc = circle(centers[0], rads[0])(p)
            for c, Rk in zip(centers[1:], rads[1:]):
                acc = smin_poly(acc, circle(c, Rk)(p), rb)
            return acc

        pts = rng.uniform(-2.5, 2.5, (40_000, 2))
        f = field(pts)
        m = (f > 1e-3) & (f < 0.5)
        if m.sum():
            maxg.append(gradnorm_fd(field, pts[m]).max())
    maxg = np.array(maxg)
    print(f"  N={N}: median={np.median(maxg):.3f}  p90={np.quantile(maxg,0.9):.3f} "
          f" max={maxg.max():.3f}   (√2={np.sqrt(2):.3f}, √N={np.sqrt(N):.3f})")
