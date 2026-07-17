#!/usr/bin/env python3
"""E1: smin-Varianten — Distanzfehler, Lipschitz, Sphere-Trace-Sicherheit.

Ground truth = Distanz zur bisektions-verfeinerten Nullkontur der Variante
SELBST (jede Variante definiert ihre eigene Geometrie; Sicherheit heißt
f(p) <= d(p, Z_f) außen).

Ausgabe: Tabelle pro Konfiguration × Variante.
"""
import numpy as np
from scipy.spatial import cKDTree

SQ2 = np.sqrt(2.0)

# ── smin-Varianten (alle: Inputs a,b = signed distances, r = Radius/Band) ──

def smin_hard(a, b, r):
    return np.minimum(a, b)

def smin_chamfer(a, b, r):  # FieldDraft csg.rs::chamfer
    return np.minimum(np.minimum(a, b), (a + b - r) / SQ2)

def smin_circ_geom(a, b, r):  # FieldDraft csg.rs::smin_poly (IQ round union)
    ua = np.maximum(r - a, 0.0)
    ub = np.maximum(r - b, 0.0)
    return np.maximum(np.minimum(a, b), r) - np.sqrt(ua * ua + ub * ub)

def smin_circ_cd(a, b, r):  # IQ 2024 "circular" (CD family), k_int = r
    h = np.maximum(r - np.abs(a - b), 0.0) / r
    return np.minimum(a, b) - r * 0.5 * (1.0 + h - np.sqrt(1.0 - h * (h - 2.0)))

def smin_quad_cd(a, b, r):  # IQ quadratic polynomial, k_int = r
    h = np.maximum(r - np.abs(a - b), 0.0) / r
    return np.minimum(a, b) - h * h * r * 0.25

def smin_cubic_cd(a, b, r):  # IQ cubic polynomial, k_int = r
    h = np.maximum(r - np.abs(a - b), 0.0) / r
    return np.minimum(a, b) - h * h * h * r / 6.0

def smin_root(a, b, r):  # IQ root (DD family), k_int = r
    x = b - a
    return 0.5 * (a + b - np.sqrt(x * x + r * r))

def smin_exp(a, b, r):  # IQ exponential (DD family)
    return -r * np.log2(np.exp2(-a / r) + np.exp2(-b / r))

def smin_r0(a, b, r):  # Rvachev R0-Union (inside-negative Konvention)
    return a + b - np.sqrt(a * a + b * b)

VARIANTS = [
    ("hard_min", smin_hard),
    ("chamfer", smin_chamfer),
    ("circ_geom", smin_circ_geom),
    ("circ_cd", smin_circ_cd),
    ("quad_cd", smin_quad_cd),
    ("cubic_cd", smin_cubic_cd),
    ("root_dd", smin_root),
    ("exp_dd", smin_exp),
    ("rvachev_r0", smin_r0),
]

# ── Konfigurationen: Feld-Fabriken f(x, y) ────────────────────────────────

def halfplane(nx, ny):
    """Signed distance, inside-negative: d = n·p (Material dort, wo d<0)."""
    def d(x, y):
        return nx * x + ny * y
    return d

def circle(cx, cy, R):
    def d(x, y):
        return np.sqrt((x - cx) ** 2 + (y - cy) ** 2) - R
    return d

def make_config(kind, gamma_deg, smin, r):
    """kind: union | intersect | sdiff. gamma = Winkel zwischen den Normalen."""
    g = np.radians(gamma_deg)
    # Normalen symmetrisch um die y-Achse
    na = (np.sin(g / 2), np.cos(g / 2))
    nb = (-np.sin(g / 2), np.cos(g / 2))
    da = halfplane(*na)
    db = halfplane(*nb)
    if kind == "union":
        def f(x, y):
            return smin(da(x, y), db(x, y), r)
    elif kind == "intersect":
        def f(x, y):
            return -smin(-da(x, y), -db(x, y), r)
    elif kind == "sdiff":  # A minus B  (FieldDraft: -blend_min(-da, db, r))
        def f(x, y):
            return -smin(-da(x, y), db(x, y), r)
    else:
        raise ValueError(kind)
    return f

def make_circles_union(smin, r):
    da = circle(-1.5, 0.0, 2.0)
    db = circle(1.5, 0.0, 2.0)
    def f(x, y):
        return smin(da(x, y), db(x, y), r)
    return f

# ── Nullkontur-Extraktion mit Bisektion ───────────────────────────────────

def zero_contour(f, lo=-5.0, hi=5.0, n=2000, iters=12):
    # Asymmetrischer Sub-Zellen-Shift: verhindert, dass achsen-/diagonal-
    # ausgerichtete Nullmembranen exakt durch Gitterknoten laufen (f==0 am
    # Knoten -> a*b<0 schlaegt fehl, Kontur-Luecke; beobachtet bei sdiff_90).
    step = (hi - lo) / (n - 1)
    xs = np.linspace(lo, hi, n) + 0.137 * step
    ys = np.linspace(lo, hi, n) - 0.291 * step
    X, Y = np.meshgrid(xs, ys, indexing="ij")
    F = f(X, Y)
    pts = []
    # x-Kanten (Vorzeichenwechsel entlang Achse 0)
    for axis in (0, 1):
        if axis == 0:
            A, B = F[:-1, :], F[1:, :]
            P0x, P0y = X[:-1, :], Y[:-1, :]
            P1x, P1y = X[1:, :], Y[1:, :]
        else:
            A, B = F[:, :-1], F[:, 1:]
            P0x, P0y = X[:, :-1], Y[:, :-1]
            P1x, P1y = X[:, 1:], Y[:, 1:]
        m = (A * B) < 0
        if not m.any():
            continue
        a = A[m]
        p0x, p0y = P0x[m].copy(), P0y[m].copy()
        p1x, p1y = P1x[m].copy(), P1y[m].copy()
        for _ in range(iters):
            mx, my = 0.5 * (p0x + p1x), 0.5 * (p0y + p1y)
            fm = f(mx, my)
            take_lo = (a * fm) < 0  # Wurzel in [p0, mid]
            p1x = np.where(take_lo, mx, p1x)
            p1y = np.where(take_lo, my, p1y)
            p0x = np.where(take_lo, p0x, mx)
            p0y = np.where(take_lo, p0y, my)
            a = np.where(take_lo, a, fm)
        pts.append(np.column_stack([0.5 * (p0x + p1x), 0.5 * (p0y + p1y)]))
    if not pts:
        return np.zeros((0, 2))
    return np.vstack(pts)

# ── Metriken ──────────────────────────────────────────────────────────────

def evaluate(f, r, lo=-2.0, hi=2.0, n=400):
    contour = zero_contour(f)
    assert len(contour) > 100, "Kontur leer?"
    tree = cKDTree(contour)
    xs = np.linspace(lo, hi, n)
    X, Y = np.meshgrid(xs, ys := np.linspace(lo, hi, n), indexing="ij")
    P = np.column_stack([X.ravel(), Y.ravel()])
    F = f(X, Y).ravel()
    d_true, _ = tree.query(P, workers=-1)
    sd_true = np.sign(F) * d_true  # Vorzeichen von f übernehmen (gleiche Topologie)

    h = 1e-4
    gx = (f(X + h, Y) - f(X - h, Y)) / (2 * h)
    gy = (f(X, Y + h) - f(X, Y - h)) / (2 * h)
    gn = np.sqrt(gx * gx + gy * gy).ravel()

    out = {}
    # Lipschitz (Diskontinuitäts-Punkte des hard-min inklusive — max ist fair,
    # da FD-Felder überall ausgewertet werden)
    out["grad_max"] = float(np.max(gn))
    out["grad_p999"] = float(np.quantile(gn, 0.999))
    # Sicherheit außen
    mask_out = (F > 0) & (d_true > 0.02)
    if mask_out.any():
        ratio = F[mask_out] / d_true[mask_out]
        out["over_max_ratio"] = float(np.max(ratio))
        out["over_max_abs"] = float(np.max(F[mask_out] - d_true[mask_out]))
        out["march_eff_mean"] = float(np.mean(ratio))
    else:
        out["over_max_ratio"] = float("nan")
        out["over_max_abs"] = float("nan")
        out["march_eff_mean"] = float("nan")
    # Nahfeld-Metrik (Offset/Shell): |f - sd| in Bändern
    for band, key in ((0.5 * r, "near50"), (0.1 * r, "near10")):
        m = (np.abs(sd_true) < band) & (d_true > 1e-3)
        err = np.abs(F[m] - sd_true[m])
        out[f"{key}_max"] = float(np.max(err)) if m.any() else float("nan")
        out[f"{key}_mean"] = float(np.mean(err)) if m.any() else float("nan")
    return out, contour

def run():
    r = 1.0
    configs = []
    for gamma in (30, 60, 90, 120, 150):
        configs.append((f"union_hp_{gamma}", "union", gamma))
        configs.append((f"intersect_hp_{gamma}", "intersect", gamma))
        configs.append((f"sdiff_hp_{gamma}", "sdiff", gamma))
    header = (
        f"{'config':<18} {'variant':<11} {'|∇f|max':>8} {'|∇f|p999':>9} "
        f"{'over_ratio':>10} {'over_abs':>9} {'eff':>6} "
        f"{'near50max':>10} {'near10max':>10}"
    )
    print(header)
    print("-" * len(header))
    contours = {}
    for cname, kind, gamma in configs:
        for vname, smin in VARIANTS:
            f = make_config(kind, gamma, smin, r)
            m, contour = evaluate(f, r)
            contours[(cname, vname)] = contour
            print(
                f"{cname:<18} {vname:<11} {m['grad_max']:8.3f} {m['grad_p999']:9.3f} "
                f"{m['over_max_ratio']:10.3f} {m['over_max_abs']:9.4f} {m['march_eff_mean']:6.3f} "
                f"{m['near50_max']:10.4f} {m['near10_max']:10.4f}"
            )
        # Zero-Set-Identität circ_geom vs circ_cd
        ca = contours[(cname, "circ_geom")]
        cb = contours[(cname, "circ_cd")]
        ta, tb = cKDTree(ca), cKDTree(cb)
        h1 = np.max(ta.query(cb, workers=-1)[0])
        h2 = np.max(tb.query(ca, workers=-1)[0])
        print(f"  -> zero-set Hausdorff(circ_geom, circ_cd) = {max(h1, h2):.2e}")
        # Diagnose: wo unterscheiden sich die Vorzeichen der beiden Felder?
        fg = make_config(kind, gamma, smin_circ_geom, r)
        fc = make_config(kind, gamma, smin_circ_cd, r)
        xs = np.linspace(-3, 3, 900)
        X, Y = np.meshgrid(xs, xs, indexing="ij")
        FG, FC = fg(X, Y), fc(X, Y)
        mm = (np.sign(FG) != np.sign(FC)) & (np.abs(FG) > 1e-4)
        if mm.any():
            print(f"     SIGN-MISMATCH: {mm.sum()} Punkte, max|f_geom|={np.abs(FG[mm]).max():.3f}, "
                  f"Beispiel p=({X[mm][0]:.2f},{Y[mm][0]:.2f})")
    # Kreis-Konfiguration
    cname = "circles_union"
    for vname, smin in VARIANTS:
        f = make_circles_union(smin, r)
        m, contour = evaluate(f, r)
        contours[(cname, vname)] = contour
        print(
            f"{cname:<18} {vname:<11} {m['grad_max']:8.3f} {m['grad_p999']:9.3f} "
            f"{m['over_max_ratio']:10.3f} {m['over_max_abs']:9.4f} {m['march_eff_mean']:6.3f} "
            f"{m['near50_max']:10.4f} {m['near10_max']:10.4f}"
        )
    ca = contours[(cname, "circ_geom")]
    cb = contours[(cname, "circ_cd")]
    ta, tb = cKDTree(ca), cKDTree(cb)
    print(f"  -> zero-set Hausdorff(circ_geom, circ_cd) = "
          f"{max(np.max(ta.query(cb, workers=-1)[0]), np.max(tb.query(ca, workers=-1)[0])):.2e}")

if __name__ == "__main__":
    run()
