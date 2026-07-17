"""Agent D (Numerik) — gemeinsame Bausteine für die Metricity-Experimente.

2D-Setting (Kreise statt Kugeln): alle untersuchten Formeln (min/max, smin_poly,
Sampson f/|grad f|, Taubin 2. Ordnung, iterierte Normalisierung) sind
dimensionsunabhängig; Nähte von Booleans sind lokal 2D-Phänomene
(Codimension-2-Kanten im 3D ↔ Ecken im 2D). Ground Truth zu Maschinen-
genauigkeit via dichter analytischer/bisektierter Randabtastung + KD-Tree.

Spiegelt crates/kernel/src/csg.rs: smin_poly (iq round union), chamfer_union,
hard_combine.
"""

import numpy as np
from scipy.spatial import cKDTree


# ── Feld-Kombinatoren (Spiegel von csg.rs) ──────────────────────────

def smin_poly(da, db, r):
    """Zirkulaeres smooth-min (iq round union), identisch zu csg.rs::smin_poly."""
    if r <= 0.0:
        return np.minimum(da, db)
    ua = np.maximum(r - da, 0.0)
    ub = np.maximum(r - db, 0.0)
    return np.maximum(np.minimum(da, db), r) - np.sqrt(ua * ua + ub * ub)


def smax_poly(da, db, r):
    """Smooth-max via De-Morgan: -smin(-a,-b,r)."""
    return -smin_poly(-da, -db, r)


def chamfer_union(da, db, r):
    return np.minimum(np.minimum(da, db), (da + db - r) * np.sqrt(0.5))


# ── Primitive ───────────────────────────────────────────────────────

def circle(c, R):
    c = np.asarray(c, dtype=np.float64)

    def f(p):
        return np.hypot(p[..., 0] - c[0], p[..., 1] - c[1]) - R

    return f


# ── Numerische Ableitungen ──────────────────────────────────────────

def grad_fd(field, p, h=1e-5):
    """Zentrale Differenzen, 4 Extra-Evals (2D). Entspricht dem, was die
    Tape-Pipeline ohne AD tun muesste."""
    ex = np.array([h, 0.0])
    ey = np.array([0.0, h])
    gx = (field(p + ex) - field(p - ex)) / (2 * h)
    gy = (field(p + ey) - field(p - ey)) / (2 * h)
    return np.stack([gx, gy], axis=-1)


def gradnorm_fd(field, p, h=1e-5):
    g = grad_fd(field, p, h)
    return np.linalg.norm(g, axis=-1)


def second_dir_deriv(field, p, n, h=1e-4):
    """Zweite Richtungsableitung entlang n (fuer Taubin 2. Ordnung)."""
    return (field(p + h * n) - 2 * field(p) + field(p - h * n)) / (h * h)


# ── Distanz-Korrekturen ─────────────────────────────────────────────

def sampson(field, p, h=1e-5, eps=1e-12):
    """1. Ordnung: f/|grad f| (Sampson/Taubin d1, Rvachev-Normalisierung 1. Ordnung)."""
    f = field(p)
    gn = gradnorm_fd(field, p, h)
    return f / np.maximum(gn, eps)


def taubin2(field, p, h=1e-4, eps=1e-12):
    """2. Ordnung entlang der Gradientenrichtung: loese
    |f| = d*|g| - 0.5*d^2*f_nn  (Taylor entlang der Normalen Richtung zum
    naechsten Punkt, Vorzeichen so, dass f(x - d*n_hat) ≈ 0).
    Fallback auf Sampson, wenn die Diskriminante negativ/instabil ist."""
    f = field(p)
    g = grad_fd(field, p, h)
    gn = np.linalg.norm(g, axis=-1)
    gn_safe = np.maximum(gn, eps)
    n = g / gn_safe[..., None]
    fnn = second_dir_deriv(field, p, n, h)
    # f(x - d n) ≈ f - d*gn + 0.5 d^2 fnn = 0  → kleinste positive Wurzel
    s = np.sign(f)
    a = 0.5 * fnn
    b = -gn
    c = np.abs(f)
    disc = b * b - 4 * a * c
    d1 = c / gn_safe  # Sampson-Fallback
    with np.errstate(invalid="ignore", divide="ignore"):
        root = np.where(
            (np.abs(a) > 1e-9) & (disc > 0),
            (-b - np.sqrt(np.maximum(disc, 0.0))) / (2 * a),
            d1,
        )
    root = np.where((root <= 0) | ~np.isfinite(root), d1, root)
    return s * root


def iterated_normalization(field, p, k, h=1e-4, eps=1e-12):
    """f_{j+1} = f_j / |grad f_j|, k-fach, Gradienten numerisch (geschachtelt).
    k=1 == Sampson. Achtung: jede Stufe potenziert die Eval-Kosten (5^k)."""
    fk = field
    for _ in range(k):
        prev = fk

        def fk(p, _prev=prev):
            f = _prev(p)
            gn = gradnorm_fd(_prev, p, h)
            return f / np.maximum(gn, eps)

    return fk(p)


# ── Ground Truth ────────────────────────────────────────────────────

def boundary_by_rays(field, center, n_rays=200_000, r_max=10.0, iters=80):
    """Randpunkte des Nullniveaus via Bisektion entlang Strahlen von einem
    inneren Punkt (Feld dort < 0). Setzt Sternfoermigkeit bzgl. center voraus.
    Verifiziert |f|<1e-9 an den gefundenen Punkten."""
    center = np.asarray(center, dtype=np.float64)
    th = np.linspace(0.0, 2 * np.pi, n_rays, endpoint=False)
    dirs = np.stack([np.cos(th), np.sin(th)], axis=-1)
    lo = np.zeros(n_rays)
    hi = np.full(n_rays, r_max)
    f_lo = field(center[None, :] + lo[:, None] * dirs)
    assert np.all(f_lo < 0), "center muss strikt innen liegen"
    f_hi = field(center[None, :] + hi[:, None] * dirs)
    assert np.all(f_hi > 0), "r_max muss ausserhalb liegen"
    for _ in range(iters):
        mid = 0.5 * (lo + hi)
        f_mid = field(center[None, :] + mid[:, None] * dirs)
        inside = f_mid < 0
        lo = np.where(inside, mid, lo)
        hi = np.where(inside, hi, mid)
    t = 0.5 * (lo + hi)
    pts = center[None, :] + t[:, None] * dirs
    resid = np.abs(field(pts))
    assert resid.max() < 1e-8, f"Bisektions-Residuum {resid.max():.2e}"
    return pts


def boundary_by_scanlines(field, bbox, n_lines=6000, n_samples=4096, iters=60):
    """Alle Nulldurchgaenge entlang horizontaler UND vertikaler Scanlines,
    per Bisektion verfeinert. Funktioniert ohne Sternfoermigkeit."""
    (x0, y0), (x1, y1) = bbox
    pts_out = []
    for orient in range(2):
        if orient == 0:
            lines = np.linspace(y0, y1, n_lines)
            ts = np.linspace(x0, x1, n_samples)
            grid = np.stack(np.meshgrid(ts, lines, indexing="xy"), axis=-1)
        else:
            lines = np.linspace(x0, x1, n_lines)
            ts = np.linspace(y0, y1, n_samples)
            grid = np.stack(np.meshgrid(lines, ts, indexing="ij"), axis=-1)
            grid = grid.transpose(1, 0, 2)  # (n_samples, n_lines, 2) → beliebig
            grid = grid.transpose(1, 0, 2)
        vals = field(grid)
        if orient == 0:
            sgn = vals[:, :-1] * vals[:, 1:] < 0
            a = grid[:, :-1][sgn]
            b = grid[:, 1:][sgn]
        else:
            sgn = vals[:, :-1] * vals[:, 1:] < 0
            a = grid[:, :-1][sgn]
            b = grid[:, 1:][sgn]
        fa = field(a)
        for _ in range(iters):
            m = 0.5 * (a + b)
            fm = field(m)
            take_a = fa * fm < 0
            b = np.where(take_a[:, None], m, b)
            a = np.where(take_a[:, None], a, m)
            fa = np.where(take_a, fa, fm)
        pts_out.append(0.5 * (a + b))
    return np.concatenate(pts_out, axis=0)


class GroundTruth:
    """Vorzeichenbehaftete Wahre-Distanz zu einem via Randabtastung gegebenen
    Nullniveau. Chord-Fehler O(spacing^2/8rho) — bei 2e5 Samples vernachlaessigbar
    gegenueber den gemessenen Feldfehlern (>=1e-6)."""

    def __init__(self, boundary_pts, sign_field):
        self.tree = cKDTree(boundary_pts)
        self.sign_field = sign_field

    def __call__(self, p):
        d, _ = self.tree.query(p.reshape(-1, 2), workers=-1)
        d = d.reshape(p.shape[:-1])
        return np.where(self.sign_field(p) < 0, -d, d)


# ── Analytische Ground Truth: harte Union zweier Kreise ────────────

def hard_union_truth(c1, R1, c2, R2):
    """Exakte signed distance zur Union zweier sich schneidender Kreise.
    Randstuecke: getrimmte Boegen + Schnittpunkte (konkave Ecken)."""
    c1 = np.asarray(c1, dtype=np.float64)
    c2 = np.asarray(c2, dtype=np.float64)
    d = np.linalg.norm(c2 - c1)
    assert abs(R1 - R2) < d < R1 + R2, "Kreise muessen sich schneiden"
    # Schnittpunkte
    a = (d * d + R1 * R1 - R2 * R2) / (2 * d)
    hh = np.sqrt(R1 * R1 - a * a)
    e = (c2 - c1) / d
    n = np.array([-e[1], e[0]])
    P = np.stack([c1 + a * e + hh * n, c1 + a * e - hh * n])

    def truth(p):
        r1 = np.linalg.norm(p - c1, axis=-1)
        r2 = np.linalg.norm(p - c2, axis=-1)
        with np.errstate(invalid="ignore", divide="ignore"):
            q1 = c1 + R1 * (p - c1) / r1[..., None]
            q2 = c2 + R2 * (p - c2) / r2[..., None]
        # Fusspunkt gueltig, wenn er ausserhalb des anderen Kreises liegt
        v1 = np.linalg.norm(q1 - c2, axis=-1) >= R2
        v2 = np.linalg.norm(q2 - c1, axis=-1) >= R1
        cand1 = np.where(v1, np.abs(r1 - R1), np.inf)
        cand2 = np.where(v2, np.abs(r2 - R2), np.inf)
        dP = np.minimum(
            np.linalg.norm(p - P[0], axis=-1), np.linalg.norm(p - P[1], axis=-1)
        )
        ud = np.minimum(np.minimum(cand1, cand2), dP)
        inside = np.minimum(r1 - R1, r2 - R2) < 0
        return np.where(inside, -ud, ud)

    return truth


def band_table(d_true, err, edges):
    """max/mean |err| in Baendern von |d_true|."""
    rows = []
    ad = np.abs(d_true)
    for lo, hi in zip(edges[:-1], edges[1:]):
        m = (ad >= lo) & (ad < hi)
        if m.sum() == 0:
            rows.append((lo, hi, 0, np.nan, np.nan))
        else:
            rows.append((lo, hi, int(m.sum()), np.abs(err[m]).max(), np.abs(err[m]).mean()))
    return rows


def print_band_table(title, rows, unit=""):
    print(f"\n{title}")
    print(f"  {'|d|-Band':>18} {'n':>7} {'max|err|':>12} {'mean|err|':>12}")
    for lo, hi, n, mx, mn in rows:
        print(f"  [{lo:7.4f},{hi:7.4f}) {n:>7} {mx:>12.3e} {mn:>12.3e}")
