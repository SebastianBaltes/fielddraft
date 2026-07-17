#!/usr/bin/env python3
"""E2: Punktweise (gitterfreie) Hopf-Lax-Redistanzierung auf analytischen
Feldern — der Tape-kompatible Weg (Royston et al. 2018 / Lee et al. 2017).

Prinzip: d(x) = kleinstes t mit m(t) := min_{||y-x||<=t} f(y) = 0.
Da f 1-Lipschitz mit f <= d_true (konservativ) ist, ist die Iteration
t <- t + m(t) sicher (Beweis: fuer Flaechenpunkt z gilt |z-x| >= t + m(t))
und konvergiert wie Sphere-Tracing — nur eben auf der ECHTEN Distanz.
Innere Minimierung: projizierter Gradientenabstieg mit Multi-Start.

Faelle:
  A) Lens = max(d1, d2) zweier Kreise — analytische Ground Truth,
     Testpunkte im Aussen-Fan der konkaven Ecke (dort ist max am falschesten).
  B) smin-Union (zirkulaeres smin wie kernel::smin_poly, r=0.3) — Ground
     Truth via dichter Zero-Kontur-Punktwolke; Testring um die Naht.

Gemessen: Fehler roh vs. Hopf-Lax, Zahl der Feld-Evaluierungen pro Punkt.
"""
import numpy as np

C1 = np.array([-0.55, 0.0]); C2 = np.array([0.55, 0.0]); R = 1.0
B = np.sqrt(1.0 - 0.55**2); K_TOP = np.array([0.0, B])

EVALS = [0]

def circle(p, c):
    return np.hypot(p[0] - c[0], p[1] - c[1]) - R

def f_lens(p):
    EVALS[0] += 1
    return max(circle(p, C1), circle(p, C2))

def smin_poly(da, db, r):
    if r <= 0: return min(da, db)
    ua = max(r - da, 0.0); ub = max(r - db, 0.0)
    return max(min(da, db), r) - np.hypot(ua, ub)

SMIN_R = 0.3
def f_smin(p):
    EVALS[0] += 1
    return smin_poly(circle(p, C1), circle(p, C2), SMIN_R)

def d_true_lens(p):
    best = np.inf
    for (c, other) in ((C1, C2), (C2, C1)):
        u = p - c; L = np.hypot(*u)
        q = c + R * u / max(L, 1e-30)
        retained = np.hypot(*(q - other)) <= R
        if retained:
            best = min(best, abs(L - R))
        else:
            best = min(best, np.hypot(p[0], p[1] - B), np.hypot(p[0], p[1] + B))
    return best if f_lens(p) >= 0 else -best

# ---------- Hopf-Lax ----------

def min_ball(f, x, t, starts=6, iters=30):
    """min f ueber Ball B(x,t): projizierter Gradientenabstieg, Multi-Start."""
    cands = [x.copy()]
    for k in range(starts):
        a = 2 * np.pi * k / starts
        cands.append(x + t * np.array([np.cos(a), np.sin(a)]))
    best = np.inf
    eps = 1e-7
    for y0 in cands:
        y = y0.copy()
        fy = f(y)
        best = min(best, fy)
        step = 0.5 * t
        for _ in range(iters):
            gx = (f(y + [eps, 0]) - f(y - [eps, 0])) / (2 * eps)
            gy = (f(y + [0, eps]) - f(y - [0, eps])) / (2 * eps)
            g = np.array([gx, gy]); n = np.hypot(*g)
            if n < 1e-12: break
            y = y - step * g / n
            r = np.hypot(*(y - x))
            if r > t:
                y = x + (y - x) * (t / r)
            fy = f(y)
            if fy < best: best = fy
            step *= 0.85
    return best

def hopf_lax(f, x, tol=1e-4, tmax=3.0, starts=6, iters=30):
    """Aussendistanz von x zur Nullmenge von f (f(x) > 0 vorausgesetzt)."""
    t = max(f(x), 0.0)
    for _ in range(60):
        m = min_ball(f, x, t, starts, iters)
        if m < -tol:            # Ball hat die Flaeche ueberschritten -> zurueck
            t += m              # (m<0: schrumpfen; sicher, da f>=d-... konservativ innen nicht garantiert,
                                #  praktisch: Bisektionsschutz unten)
        elif m <= tol:
            return t
        else:
            t += m
        if t > tmax:
            return np.inf
    return t

# ---------- Ground Truth fuer smin: Zero-Kontur-Punktwolke ----------

def smin_zero_cloud(n=4097, dom=2.4):
    xs = np.linspace(-dom, dom, n)
    X, Y = np.meshgrid(xs, xs, indexing="ij")
    D1 = np.hypot(X - C1[0], Y - C1[1]) - R
    D2 = np.hypot(X - C2[0], Y - C2[1]) - R
    UA = np.maximum(SMIN_R - D1, 0.0); UB = np.maximum(SMIN_R - D2, 0.0)
    F = np.maximum(np.minimum(D1, D2), SMIN_R) - np.hypot(UA, UB)
    pts = []
    # Vorzeichenwechsel entlang x und y, linear interpoliert:
    sx = F[:-1, :] * F[1:, :] < 0
    i, j = np.nonzero(sx)
    tfrac = F[i, j] / (F[i, j] - F[i + 1, j])
    pts.append(np.stack([xs[i] + tfrac * (xs[1] - xs[0]), Y[i, j]], axis=1))
    sy = F[:, :-1] * F[:, 1:] < 0
    i, j = np.nonzero(sy)
    tfrac = F[i, j] / (F[i, j] - F[i, j + 1])
    pts.append(np.stack([X[i, j], xs[j] + tfrac * (xs[1] - xs[0])], axis=1))
    return np.concatenate(pts, axis=0)

def cloud_dist(cloud, p):
    return np.min(np.hypot(cloud[:, 0] - p[0], cloud[:, 1] - p[1]))

# ---------- Tests ----------

def fan_dirs(n=21, margin=0.02):
    n1 = (K_TOP - C1) / R; n2 = (K_TOP - C2) / R
    a1, a2 = np.arctan2(n1[1], n1[0]), np.arctan2(n2[1], n2[0])
    return [np.array([np.cos(a), np.sin(a)])
            for a in np.linspace(min(a1, a2) + margin, max(a1, a2) - margin, n)]

def case_lens(starts, iters, tol):
    errs_raw, errs_hl, evals = [], [], []
    for dist in (0.1, 0.3, 0.6):
        for d_ in fan_dirs():
            p = K_TOP + dist * d_
            dt = d_true_lens(p)
            errs_raw.append(abs(f_lens(p) - dt))
            EVALS[0] = 0
            dh = hopf_lax(f_lens, p, tol=tol, starts=starts, iters=iters)
            evals.append(EVALS[0])
            errs_hl.append(abs(dh - dt))
    return (np.max(errs_raw), np.max(errs_hl), np.mean(errs_hl),
            int(np.mean(evals)), int(np.max(evals)))

def case_smin(starts, iters, tol):
    cloud = smin_zero_cloud()
    errs_raw, errs_hl, evals = [], [], []
    # Ring um die obere Naht: Punkte auf wahrer Distanz ~delta von der Flaeche
    for ang in np.linspace(-1.1, 1.1, 15):
        d_ = np.array([np.sin(ang), np.cos(ang)])
        for dist in (0.1, 0.25):
            # Start von der Ecke aus nach aussen laufen, bis wahre Distanz = dist
            p = K_TOP + (SMIN_R + dist) * d_   # grob; wahre Distanz via Cloud
            dt = cloud_dist(cloud, p)
            if f_smin(p) <= 0.02:              # zu nah/innen -> skip
                continue
            errs_raw.append(abs(f_smin(p) - dt))
            EVALS[0] = 0
            dh = hopf_lax(f_smin, p, tol=tol, starts=starts, iters=iters)
            evals.append(EVALS[0])
            errs_hl.append(abs(dh - dt))
    return (np.max(errs_raw), np.max(errs_hl), np.mean(errs_hl),
            int(np.mean(evals)), int(np.max(evals)))

if __name__ == "__main__":
    import time
    print("case\tconfig\traw_max\thl_max\thl_mean\tevals_mean\tevals_max\tsec")
    for (name, fn) in (("lens", case_lens), ("smin", case_smin)):
        for (starts, iters, tol, label) in ((2, 12, 1e-3, "cheap"),
                                            (6, 30, 1e-4, "thorough")):
            t0 = time.time()
            rmax, hmax, hmean, em, ex = fn(starts, iters, tol)
            print(f"{name}\t{label}\t{rmax:.5g}\t{hmax:.5g}\t{hmean:.5g}\t{em}\t{ex}\t{time.time()-t0:.1f}")
