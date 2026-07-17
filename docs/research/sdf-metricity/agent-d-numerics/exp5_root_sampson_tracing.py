"""E5: Wurzel-Sampson praktisch — AD (Dual Numbers) vs. FD, Sphere-Trace-
Sicherheit, benoetigter Daempfungsfaktor.

Prototyp des 'Zweikanal-Evals' fuer den Bytecode-Interpreter: jede Operation
propagiert (val, grad) analytisch (Winner-Branch bei min/max, blend_min_weights
bei smin) — exakt das, was OP-weise im Tape machbar waere.

Korpus (aussen, d_true ∈ (1e-3, 1.0)):
  a) smin-Union moderat (cx=0.6, r=0.25)
  b) smin-Union tiefer Ueberlapp (cx=0.3, r=0.25)   — fast-koinzident
  c) koinzidenter smin-Stack m=4 (r=0.3)
  d) harte Union (cx=0.7)
  e) harte Differenz: duenne Sichel (ecc=0.15, R_B=0.9)
  f) harte Intersection (Linse, cx=0.7)

Metriken je Feldvariante F ∈ {roh, FD-Sampson, AD-Sampson, AD-Sampson-clamped}:
  max F/d_true   (>1 = Sphere-Trace-Schranke verletzt; 1/max = noetiger relax)
  mean F/d_true  (<1 = Kriechen; Schrittweiten-Effizienz)
"""

import numpy as np

from lib import GroundTruth, boundary_by_rays, boundary_by_scanlines, circle, smin_poly

rng = np.random.default_rng(42)
R = 1.0


# ── Dual-Number-Feld-Kombinatoren: (val, grad) ────────────────────

def d_circle(c, Rr):
    c = np.asarray(c, float)

    def f(p):
        rel = p - c
        rn = np.linalg.norm(rel, axis=-1)
        g = rel / np.maximum(rn, 1e-300)[..., None]
        return rn - Rr, g

    return f


def d_min(a, b):
    va, ga = a
    vb, gb = b
    take_a = va <= vb
    return np.where(take_a, va, vb), np.where(take_a[..., None], ga, gb)


def d_neg(a):
    return -a[0], -a[1]


def d_max(a, b):
    return d_neg(d_min(d_neg(a), d_neg(b)))


def d_smin(a, b, r):
    va, ga = a
    vb, gb = b
    ua = np.maximum(r - va, 0.0)
    ub = np.maximum(r - vb, 0.0)
    L = np.sqrt(ua * ua + ub * ub)
    mn, gmn = d_min(a, b)
    base = np.maximum(mn, r)
    gbase = np.where((mn >= r)[..., None], gmn, 0.0)
    val = base - L
    safeL = np.maximum(L, 1e-300)
    gval = gbase + (ua[..., None] * ga + ub[..., None] * gb) / safeL[..., None]
    return np.where(L > 0, val, mn), np.where((L > 0)[..., None], gval, gmn)


# ── Korpus ─────────────────────────────────────────────────────────

def make_corpus():
    corpus = {}

    c1, c2 = np.array([-0.6, 0.0]), np.array([0.6, 0.0])
    corpus["a) smin moderat"] = lambda p: d_smin(d_circle(c1, R)(p), d_circle(c2, R)(p), 0.25)

    c3, c4 = np.array([-0.3, 0.0]), np.array([0.3, 0.0])
    corpus["b) smin tief"] = lambda p: d_smin(d_circle(c3, R)(p), d_circle(c4, R)(p), 0.25)

    def stack4(p):
        acc = d_circle(np.zeros(2), R)(p)
        for _ in range(3):
            acc = d_smin(acc, d_circle(np.zeros(2), R)(p), 0.3)
        return acc

    corpus["c) koinzident m=4"] = stack4

    c5, c6 = np.array([-0.7, 0.0]), np.array([0.7, 0.0])
    corpus["d) harte Union"] = lambda p: d_min(d_circle(c5, R)(p), d_circle(c6, R)(p))

    cB = np.array([0.15, 0.0])
    corpus["e) Sichel-Diff"] = lambda p: d_max(d_circle(np.zeros(2), R)(p), d_neg(d_circle(cB, 0.9)(p)))

    corpus["f) Linse"] = lambda p: d_max(d_circle(c5, R)(p), d_circle(c6, R)(p))
    return corpus


def fd_gradnorm(F, p, h=1e-5):
    ex, ey = np.array([h, 0.0]), np.array([0.0, h])
    gx = (F(p + ex)[0] - F(p - ex)[0]) / (2 * h)
    gy = (F(p + ey)[0] - F(p - ey)[0]) / (2 * h)
    return np.hypot(gx, gy)


print(f"{'Feld':<18} {'Variante':<18} {'max F/d':>8} {'mean F/d':>9} {'nötig relax':>11}")
for name, dfield in make_corpus().items():
    F = lambda p: dfield(p)[0]
    # Ground Truth zur eigenen Nullflaeche
    if "Sichel" in name or "Linse" in name:
        bpts = boundary_by_scanlines(F, ((-2.0, -2.0), (2.0, 2.0)), n_lines=4000, n_samples=4096)
    else:
        bpts = boundary_by_rays(F, np.zeros(2) if "koinz" in name or "b)" in name else np.array([0.0, 0.0]), n_rays=300_000, r_max=4.0)
    truth = GroundTruth(bpts, F)
    pts = rng.uniform(-2.0, 2.0, (400_000, 2))
    d = truth(pts)
    m = (d > 1e-3) & (d < 1.0)
    pts, d = pts[m], d[m]
    val, grad = dfield(pts)
    gn_ad = np.linalg.norm(grad, axis=-1)
    gn_fd = fd_gradnorm(F, pts)
    variants = {
        "roh": val,
        "FD-Sampson": val / np.maximum(gn_fd, 1e-12),
        "AD-Sampson": val / np.maximum(gn_ad, 1e-12),
        "AD-Samp-clamp": val / np.maximum(gn_ad, 1.0),  # nur daempfen, nie strecken
    }
    for vname, vv in variants.items():
        ratio = vv / d
        print(f"{name:<18} {vname:<18} {ratio.max():>8.3f} {ratio.mean():>9.3f} "
              f"{1.0/max(ratio.max(),1.0):>11.3f}")
    print()

# ── Schritt-Zaehlung + Overshoot: 1D-Trace adversarialer Strahlen ─
print("\nSphere-Trace (tol 1e-4, max 2000): Schritte + Endzustand.")
print("  'pen' = Endpunkt liegt IM Koerper (Overshoot durch Ueberschaetzung).")


def trace(F, origin, direction, relax=1.0, tol=1e-4, max_steps=2000):
    t = 0.0
    for i in range(max_steps):
        v = float(F((origin + t * direction)[None, :])[0])
        if abs(v) < tol:
            return i, t, "ok"
        if v < 0:
            return i, t, f"pen {v:+.2e}"
        t += relax * v
        if t > 6.0:
            return i, np.inf, "miss"
    return max_steps, t, "MAXSTEPS"


corpus = make_corpus()
# Horn-Geometrie der Sichel (ecc=0.15, R_B=0.9):
ecc, RB = 0.15, 0.9
a_h = (ecc * ecc + 1.0 - RB * RB) / (2 * ecc)
hh = np.sqrt(1.0 - a_h * a_h)
P = np.array([a_h, hh])
nA = P / np.linalg.norm(P)
nBf = -(P - np.array([ecc, 0.0])) / RB
bis = nA + nBf
bis /= np.linalg.norm(bis)

cases = [
    ("e) Sichel: in den Horn-Kegel", corpus["e) Sichel-Diff"], P + 0.7 * bis, -bis),
    ("b) smin tief: auf den Neck", corpus["b) smin tief"], np.array([0.0, 2.0]),
     np.array([0.0, -1.0])),
    ("c) koinzident m=4: radial", corpus["c) koinzident m=4"], np.array([0.0, 2.0]),
     np.array([0.0, -1.0])),
]
for cname, dfield, o, dirn in cases:
    F = lambda p: dfield(p)[0]
    Fs = lambda p: (lambda vg: vg[0] / np.maximum(np.linalg.norm(vg[1], axis=-1), 1e-12))(dfield(p))
    r_raw = trace(F, o, dirn)
    r_rel = trace(F, o, dirn, relax=0.75)
    r_s = trace(Fs, o, dirn)
    r_s75 = trace(Fs, o, dirn, relax=0.75)
    print(f"  {cname}")
    print(f"    roh:            {r_raw[0]:>4} Schritte  {r_raw[2]}")
    print(f"    relax 0.75:     {r_rel[0]:>4} Schritte  {r_rel[2]}")
    print(f"    AD-Sampson:     {r_s[0]:>4} Schritte  {r_s[2]}")
    print(f"    AD-Samp.+0.75:  {r_s75[0]:>4} Schritte  {r_s75[2]}")
