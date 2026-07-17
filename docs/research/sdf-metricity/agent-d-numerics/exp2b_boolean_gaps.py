"""E2b: Wo die harten Booleans WIRKLICH Distanz verlieren.

Mengenlehre-Fakten (nach dem Negativ-Resultat von exp2):
  dist(x, A∪B) = min(dist A, dist B)   → Union AUSSEN exakt.
  dist(x, A∩B) ≥ max(dist A, dist B)   → Intersection AUSSEN unterschaetzt
                                          (Schattenkegel konvexer Ecken).
  Tiefe innen(A∪B) ≥ |min(d1,d2)|      → Union INNEN unterschaetzt Tiefe
                                          (Shell/Inward-Offset betroffen).
  Difference = A ∩ ¬B                   → wie Intersection, Horn-Ecken
                                          werden bei fast-tangentialen
                                          Schnitten beliebig spitz.

Messungen:
 1. Lens (Intersection): max relative Unterschaetzung aussen vs Vorhersage
    1 - cos(gamma/2) (gamma = Normalkegel-Oeffnung an der Ecke).
 2. Union innen: relative Tiefen-Unterschaetzung nahe Neck; Konsequenz fuer
    die Innenflaeche einer Shell {f = -t}: wahre Tiefe dort (soll: t).
 3. Difference fast-tangential: Kollaps des Feldes im Horn (Grey-Wall-Naehe).
"""

import numpy as np

from lib import circle, hard_union_truth

R = 1.0
rng = np.random.default_rng(11)


def lens_truth(c1, R1, c2, R2):
    """Signed distance zur Intersection (Linse) zweier Kreisscheiben."""
    c1 = np.asarray(c1, float)
    c2 = np.asarray(c2, float)
    d = np.linalg.norm(c2 - c1)
    a = (d * d + R1 * R1 - R2 * R2) / (2 * d)
    hh = np.sqrt(R1 * R1 - a * a)
    e = (c2 - c1) / d
    n = np.array([-e[1], e[0]])
    P = np.stack([c1 + a * e + hh * n, c1 + a * e - hh * n])

    def truth(p):
        r1 = np.linalg.norm(p - c1, axis=-1)
        r2 = np.linalg.norm(p - c2, axis=-1)
        q1 = c1 + R1 * (p - c1) / np.maximum(r1, 1e-300)[..., None]
        q2 = c2 + R2 * (p - c2) / np.maximum(r2, 1e-300)[..., None]
        # Randstueck von C1 gehoert zur Linse, wo es in Scheibe 2 liegt
        v1 = np.linalg.norm(q1 - c2, axis=-1) <= R2
        v2 = np.linalg.norm(q2 - c1, axis=-1) <= R1
        cand1 = np.where(v1, np.abs(r1 - R1), np.inf)
        cand2 = np.where(v2, np.abs(r2 - R2), np.inf)
        dP = np.minimum(
            np.linalg.norm(p - P[0], axis=-1), np.linalg.norm(p - P[1], axis=-1)
        )
        ud = np.minimum(np.minimum(cand1, cand2), dP)
        inside = np.maximum(r1 - R1, r2 - R2) < 0
        return np.where(inside, -ud, ud)

    return truth


# ── 1) Intersection aussen: Schattenkegel ──────────────────────────
print("1) Lens = Intersection zweier Kreise R=1, Zentren (±cx,0), Punkte im")
print("   Normalkegel der Ecke, rho in [1e-3, 0.3].")
print(f"{'cx':>6} {'gamma[deg]':>10} {'pred 1-cos(g/2)':>16} {'max rel-err':>12} {'@rho→0':>8}")
for cx in [0.3, 0.5, 0.7, 0.9, 0.98]:
    c1, c2 = np.array([-cx, 0.0]), np.array([cx, 0.0])
    f1, f2 = circle(c1, R), circle(c2, R)
    truth = lens_truth(c1, R, c2, R)
    h_apex = np.sqrt(R * R - cx * cx)
    P = np.array([0.0, h_apex])
    n1, n2 = (P - c1) / R, (P - c2) / R
    gamma = np.arccos(np.clip(n1 @ n2, -1, 1))
    ang1, ang2 = np.arctan2(n1[1], n1[0]), np.arctan2(n2[1], n2[0])
    lo, hi = min(ang1, ang2), max(ang1, ang2)
    rho = rng.uniform(1e-3, 0.3, 30000)
    ang = rng.uniform(lo, hi, 30000)
    pts = P + rho[:, None] * np.stack([np.cos(ang), np.sin(ang)], axis=-1)
    f = np.maximum(f1(pts), f2(pts))
    d = truth(pts)
    rel = (d - f) / np.maximum(d, 1e-12)
    small = rho < 0.01
    print(f"{cx:>6.2f} {np.degrees(gamma):>10.2f} {1-np.cos(gamma/2):>16.4f} "
          f"{rel.max():>12.4f} {rel[small].max():>8.4f}")

# ── 2) Union innen: Tiefen-Unterschaetzung & Shell-Wirkung ─────────
print("\n2) Union innen (cx=0.7): Tiefe |f| vs wahre Tiefe; Shell-Innenflaeche {f=-t}.")
cx = 0.7
c1, c2 = np.array([-cx, 0.0]), np.array([cx, 0.0])
f1, f2 = circle(c1, R), circle(c2, R)
truth = hard_union_truth(c1, R, c2, R)


def field(p):
    return np.minimum(f1(p), f2(p))


# Innenpunkte
pts = rng.uniform(-2, 2, (400000, 2))
pts = pts[field(pts) < 0]
d = truth(pts)
f = field(pts)
rel = (np.abs(d) - np.abs(f)) / np.maximum(np.abs(d), 1e-12)
print(f"   Innen: max rel Tiefen-Unterschaetzung = {rel.max():.4f} "
      f"(Anteil >5%: {(rel > 0.05).mean():.1%})")

# Shell-Innenflaeche: sample {f=-t} durch Bisektion entlang y-Linien
for t in [0.1, 0.2, 0.3]:
    xs = np.linspace(-1.6, 1.6, 4000)
    # von Mittelachse (y=0, tief innen) nach oben bis aussen
    y_lo = np.zeros_like(xs)
    y_hi = np.full_like(xs, 2.5)
    g_lo = field(np.stack([xs, y_lo], -1)) + t
    valid = g_lo < 0
    xs_v = xs[valid]
    y_lo, y_hi = y_lo[valid], y_hi[valid]
    for _ in range(60):
        mid = 0.5 * (y_lo + y_hi)
        gm = field(np.stack([xs_v, mid], -1)) + t
        y_lo = np.where(gm < 0, mid, y_lo)
        y_hi = np.where(gm < 0, y_hi, mid)
    contour = np.stack([xs_v, 0.5 * (y_lo + y_hi)], -1)
    depth = -truth(contour)
    print(f"   t={t}: wahre Tiefe auf {{f=-t}}: min={depth.min():.4f} "
          f"max={depth.max():.4f}  (soll {t}; max Wand-Verdickung "
          f"{(depth.max()-t)/t:+.1%})")

# ── 3) Difference fast-tangential: Horn-Kollaps ────────────────────
print("\n3) Difference A\\B fast-tangential: A=Kreis R=1 um 0, B=Kreis R=1 um (2-gap,0).")
print("   Feld f = max(d1, -d2). Punkte auf der x-Achse im Horn-Vorfeld.")
for gap in [0.5, 0.2, 0.05, 0.01]:
    cB = np.array([2 - gap, 0.0])
    fA, fB = circle(np.zeros(2), R), circle(cB, R)

    def fdiff(p):
        return np.maximum(fA(p), -fB(p))

    # Horn-Ecken: Schnittpunkte der Kreise
    dcc = 2 - gap
    a = dcc / 2
    hh = np.sqrt(R * R - a * a)
    Pcorner = np.array([a, hh])
    # Normalkegel der Ecke von A\B: zwischen n_A (Aussennormale von A) und
    # -n_B (Aussennormale der geflippten B-Flaeche)
    nA = Pcorner / np.linalg.norm(Pcorner)
    nBflip = -(Pcorner - cB) / R
    gamma = np.arccos(np.clip(nA @ nBflip, -1, 1))
    angA, angB = np.arctan2(nA[1], nA[0]), np.arctan2(nBflip[1], nBflip[0])
    lo_a, hi_a = min(angA, angB), max(angA, angB)
    rho = rng.uniform(1e-3, 0.3, 30000)
    ang = rng.uniform(lo_a, hi_a, 30000)
    pts = Pcorner + rho[:, None] * np.stack([np.cos(ang), np.sin(ang)], -1)
    # nur Punkte, deren naechster Punkt wirklich die Ecke ist: ausserhalb A
    # oder innerhalb B (weggeschnitten) → konservativ: d_true per Sampling
    # der zwei getrimmten Randstuecke + Ecke; hier reicht: im Kegelinneren
    # ist die Ecke der Fusspunkt (beide Kreis-Fusspunkte ungueltig).
    f = fdiff(pts)
    dtrue = np.linalg.norm(pts - Pcorner, axis=-1)
    ratio = f / np.maximum(dtrue, 1e-12)
    print(f"   gap={gap:>5}: gamma={np.degrees(gamma):6.2f}°  "
          f"pred min-ratio cos(g/2)={np.cos(gamma/2):.4f}  "
          f"gemessen min f/d_true={ratio.min():.4f}")
print("""   Ueberraschung: flache Anschnitte (gap→0 bei aussenliegenden Zentren)
   haben KLEINE Normalkegel → fast kein Fehler. Der Adversar ist die
   INTERNE Fast-Tangenz (duennwandige Sichel), gamma → 180°:""")

print("\n3b) Sichel: A=Kreis R=1 um 0, B=Kreis R_B um (ecc,0) — duenne Wand links.")
for ecc, RB in [(0.3, 0.8), (0.2, 0.85), (0.15, 0.9), (0.12, 0.93)]:
    cB = np.array([ecc, 0.0])
    fA, fB = circle(np.zeros(2), R), circle(cB, RB)

    def fdiff(p):
        return np.maximum(fA(p), -fB(p))

    # Schnittpunkte
    dcc = ecc
    a = (dcc * dcc + R * R - RB * RB) / (2 * dcc)
    hh = np.sqrt(R * R - a * a)
    Pcorner = np.array([a, hh])
    nA = Pcorner / np.linalg.norm(Pcorner)
    nBflip = -(Pcorner - cB) / RB
    gamma = np.arccos(np.clip(nA @ nBflip, -1, 1))
    angA, angB = np.arctan2(nA[1], nA[0]), np.arctan2(nBflip[1], nBflip[0])
    lo_a, hi_a = min(angA, angB), max(angA, angB)
    rho = rng.uniform(1e-3, 0.3, 30000)
    ang = rng.uniform(lo_a, hi_a, 30000)
    pts = Pcorner + rho[:, None] * np.stack([np.cos(ang), np.sin(ang)], -1)
    f = fdiff(pts)
    dtrue = np.linalg.norm(pts - Pcorner, axis=-1)
    ratio = f / np.maximum(dtrue, 1e-12)
    wall = 1 - (RB - ecc)  # Wanddicke der Sichel links
    print(f"   ecc={ecc:>5} R_B={RB}: Wand={wall:.2f}  gamma={np.degrees(gamma):6.2f}°  "
          f"pred cos(g/2)={np.cos(gamma/2):.4f}  gemessen min f/d={ratio.min():.4f}")

print("""   → Horn-Vorfeld-Kollaps f ≈ cos(gamma/2)·d_true, gamma→180° bei duennen
   Sicheln: Feld beliebig weit unter der wahren Distanz. Sphere-Tracing
   kriecht ('Grey Wall'), Offsets im Horn beliebig falsch. KEINE
   Gradientennorm-Korrektur hilft: |grad f| = 1 a.e. auch hier —
   der Fehler ist ein Fusspunkt-/Mengen-Fehler, kein Skalenfehler.""")
