"""E2: Harte konkave Naht (min-Union zweier Kreise).

URSPRUENGLICHE Hypothese: im Schattenkegel der konkaven Ecke unterschaetzt
min(d1,d2) um den Faktor cos(gamma/2). ERGEBNIS: WIDERLEGT — die Messung
zeigt rel-err ~ 0. Grund (nachtraeglich klar): dist(x, A∪B) = min(dist(x,A),
dist(x,B)) ist fuer AUSSENPUNKTE eine exakte Mengen-Identitaet; der
min-Feld-Fehler der Union sitzt ausschliesslich INNEN (Tiefe) — und bei
Intersection/Difference AUSSEN (siehe exp2b_boolean_gaps.py).

Was BLEIBT aus diesem Experiment: die FD-Gradienten-Statistik an der
Tie-Flaeche (d1=d2). Dort mittelt der zentrale Differenzenquotient beide
Aeste → |g_fd| = cos(gamma/2) < 1 → Sampson f/|g_fd| verletzt die
Sphere-Trace-Schranke um bis zu 1/cos(gamma/2), OBWOHL f selbst exakt ist.
"""

import numpy as np

from lib import circle, gradnorm_fd, hard_union_truth, sampson

R = 1.0

print("Union zweier Kreise R=1, Zentren (±cx,0). Konkave Ecke am Schnittpunkt.")
print(f"{'cx':>6} {'gamma[deg]':>10} {'pred 1-cos(g/2)':>16} {'gemessen max rel-err':>20}")

rng = np.random.default_rng(3)

for cx in [0.3, 0.5, 0.7, 0.9, 0.98]:
    c1 = np.array([-cx, 0.0])
    c2 = np.array([+cx, 0.0])
    f1, f2 = circle(c1, R), circle(c2, R)

    def field(p):
        return np.minimum(f1(p), f2(p))

    truth = hard_union_truth(c1, R, c2, R)
    # Schnittpunkt oben: (0, h), Normalkegel-Oeffnung gamma:
    h = np.sqrt(R * R - cx * cx)
    P = np.array([0.0, h])
    # Aussennormalen der beiden Boegen im Punkt P: (P-ci)/R
    n1 = (P - c1) / R
    n2 = (P - c2) / R
    gamma = np.arccos(np.clip(n1 @ n2, -1, 1))
    pred = 1 - np.cos(gamma / 2)

    # Punkte im Normalkegel: P + rho * Richtung zwischen n1 und n2
    rho = rng.uniform(1e-3, 0.3, 20000)
    t = rng.uniform(0, 1, 20000)
    ang1, ang2 = np.arctan2(n1[1], n1[0]), np.arctan2(n2[1], n2[0])
    lo, hi = min(ang1, ang2), max(ang1, ang2)
    ang = lo + t * (hi - lo)
    pts = P + rho[:, None] * np.stack([np.cos(ang), np.sin(ang)], axis=-1)

    d = truth(pts)
    f = field(pts)
    rel = (d - f) / np.maximum(d, 1e-12)
    print(f"{cx:>6.2f} {np.degrees(gamma):>10.2f} {pred:>16.4f} {rel.max():>20.4f}")

# ── |grad f| und Sampson an der Naht ───────────────────────────────
print("\n|grad min|-Statistik im Schattenkegel (cx=0.7) und Sampson-Wirkung:")
cx = 0.7
c1, c2 = np.array([-cx, 0.0]), np.array([cx, 0.0])
f1, f2 = circle(c1, R), circle(c2, R)


def field(p):
    return np.minimum(f1(p), f2(p))


truth = hard_union_truth(c1, R, c2, R)
h_apex = np.sqrt(R * R - cx * cx)
P = np.array([0.0, h_apex])
n1, n2 = (P - c1) / R, (P - c2) / R
lo = min(np.arctan2(n1[1], n1[0]), np.arctan2(n2[1], n2[0]))
hi = max(np.arctan2(n1[1], n1[0]), np.arctan2(n2[1], n2[0]))
rho = rng.uniform(1e-3, 0.3, 40000)
ang = rng.uniform(lo, hi, 40000)
pts = P + rho[:, None] * np.stack([np.cos(ang), np.sin(ang)], axis=-1)
d = truth(pts)
f = field(pts)

print(f"{'h_FD':>8} {'med |g|':>9} {'min |g|':>9} {'Anteil |g|<0.99':>16} "
      f"{'max (f/|g|)/d':>14} {'Anteil f/|g|>d':>15}")
for h_fd in [1e-6, 1e-4, 1e-2, 3e-2]:
    gn = gradnorm_fd(field, pts, h=h_fd)
    s = f / np.maximum(gn, 1e-12)
    ratio = s / np.maximum(d, 1e-12)
    print(f"{h_fd:>8.0e} {np.median(gn):>9.4f} {gn.min():>9.4f} "
          f"{(gn < 0.99).mean():>16.1%} {ratio.max():>14.3f} {(s > d + 1e-12).mean():>15.1%}")

print("""
Interpretation (nach Messung):
 - Union AUSSEN ist exakt (Mengen-Identitaet) — rel-err-Spalte ~0, die
   cos(gamma/2)-Vorhersage betrifft Intersection/Difference (exp2b).
 - Im 2h-Band um die Tie-Flaeche mittelt der FD-Gradient beide Aeste →
   |g|<1 → f/|g| > f = d_true: max (f/|g|)/d ≈ 1.4 bei h>=1e-4 —
   FD-Sampson verletzt die Sphere-Trace-Schranke an JEDER Naht, auch wo
   das rohe Feld exakt ist. AD (Winner-Branch, h→0) zeigt das nicht.
""")
