#!/usr/bin/env python3
"""E1: 2D-Redistancing von min/max-Boolean-Feldern zweier Kreise.

Geometrie: Kreise r=1 um (+-0.55, 0); Nahtecken bei (0, +-b), b=sqrt(1-0.55^2).
Zwei Faelle:
  union     f = min(d1, d2)  — Fehler erwartet INNEN (Shell/Innen-Offset),
                                aussen beweisbar exakt;
  intersect f = max(d1, d2)  — Fehler erwartet AUSSEN im Fan hinter der
                                (von aussen) konkaven Ecke (Offset/Dilation).

Gemessen: Bandfehler roh vs. nach Gitter-Redistancing (monotones
Godunov-Jacobi, Interface-Band eingefroren), Fan-Strahl-/Offset-Test an der
Ecke (nur intersect), Zero-Set-Verschiebung, Konvergenz ueber h.
"""
import numpy as np

C1 = np.array([-0.55, 0.0])
C2 = np.array([0.55, 0.0])
R = 1.0
B = np.sqrt(1.0 - 0.55**2)
K_TOP = np.array([0.0, B])
DOM = 2.4

def circle_sdf(px, py, c):
    return np.hypot(px - c[0], py - c[1]) - R

def f_raw(px, py, op):
    d1, d2 = circle_sdf(px, py, C1), circle_sdf(px, py, C2)
    return np.minimum(d1, d2) if op == "union" else np.maximum(d1, d2)

def d_true(px, py, op):
    """Analytische signierte Distanz zum Rand von Union bzw. Intersection."""
    px = np.asarray(px, dtype=float); py = np.asarray(py, dtype=float)
    best = np.full(px.shape, np.inf)
    for (c, other) in ((C1, C2), (C2, C1)):
        ux, uy = px - c[0], py - c[1]
        L = np.hypot(ux, uy)
        L = np.where(L == 0.0, 1e-30, L)
        qx, qy = c[0] + R * ux / L, c[1] + R * uy / L
        qo = np.hypot(qx - other[0], qy - other[1])
        retained = (qo >= R) if op == "union" else (qo <= R)
        arc = np.abs(L - R)
        corner = np.minimum(np.hypot(px, py - B), np.hypot(px, py + B))
        best = np.minimum(best, np.where(retained, arc, corner))
    sign = np.where(f_raw(px, py, op) < 0.0, -1.0, 1.0)  # Zero-Set exakt
    return sign * best

def godunov_update(u, h):
    big = 1e10
    up = np.pad(u, 1, constant_values=big)
    a = np.minimum(up[:-2, 1:-1], up[2:, 1:-1])
    b = np.minimum(up[1:-1, :-2], up[1:-1, 2:])
    lo, hi = np.minimum(a, b), np.maximum(a, b)
    one_sided = lo + h
    quad = 0.5 * (a + b + np.sqrt(np.maximum(2*h*h - (a - b)**2, 0.0)))
    unew = np.where(hi - lo >= h, one_sided, quad)
    return np.minimum(u, unew)

def redistance(f, h, tol=1e-12, maxiter=100000):
    s = np.sign(f)
    band = np.zeros(f.shape, bool)
    band[:-1, :] |= s[:-1, :] != s[1:, :]
    band[1:, :]  |= s[:-1, :] != s[1:, :]
    band[:, :-1] |= s[:, :-1] != s[:, 1:]
    band[:, 1:]  |= s[:, :-1] != s[:, 1:]
    u = np.where(band, np.abs(f), 1e10)
    it = 0
    while it < maxiter:
        it += 1
        un = godunov_update(u, h)
        un[band] = np.abs(f)[band]
        change = np.max(np.abs(un - u))
        u = un
        if change < tol:
            break
    return np.where(f < 0, -u, u), band, it

def bilinear(grid, xs, ys, x0, h):
    gx = (np.asarray(xs) - x0) / h
    gy = (np.asarray(ys) - x0) / h
    i0 = np.clip(np.floor(gx).astype(int), 0, grid.shape[0] - 2)
    j0 = np.clip(np.floor(gy).astype(int), 0, grid.shape[1] - 2)
    tx, ty = gx - i0, gy - j0
    return ((grid[i0, j0] * (1-tx) + grid[i0+1, j0] * tx) * (1-ty)
            + (grid[i0, j0+1] * (1-tx) + grid[i0+1, j0+1] * tx) * ty)

def ray_root(fun, origin, direction, lo, hi, iters=80):
    d = np.asarray(direction, dtype=float)
    d = d / np.hypot(*d)
    flo = fun(*(origin + lo * d))
    for _ in range(iters):
        mid = 0.5 * (lo + hi)
        fm = fun(*(origin + mid * d))
        if (fm < 0) == (flo < 0):
            lo, flo = mid, fm
        else:
            hi = mid
    return 0.5 * (lo + hi)

def fan_directions(n=21, margin=0.02):
    """Aussen-Normalenkegel der Lens-Ecke (zwischen den Arc-Normalen)."""
    n1 = (K_TOP - C1) / R
    n2 = (K_TOP - C2) / R
    a1, a2 = np.arctan2(n1[1], n1[0]), np.arctan2(n2[1], n2[0])
    lo, hi = min(a1, a2) + margin, max(a1, a2) - margin
    return [np.array([np.cos(a), np.sin(a)]) for a in np.linspace(lo, hi, n)]

def run(N, op):
    h = 2 * DOM / (N - 1)
    x0 = -DOM
    xs = np.linspace(-DOM, DOM, N)
    X, Y = np.meshgrid(xs, xs, indexing="ij")
    F = f_raw(X, Y, op)
    D = d_true(X, Y, op)
    err_raw = F - D
    ext = (D > 0) & (D <= 0.5)
    intr = (D < 0) & (D >= -0.5)
    res = {
        "op": op, "N": N, "h": h,
        "raw_ext_max": np.max(np.abs(err_raw[ext])),
        "raw_int_max": np.max(np.abs(err_raw[intr])),
    }

    G, band, iters = redistance(F, h)
    err_g = G - D
    res.update({
        "iters": iters,
        "fsm_ext_max": np.max(np.abs(err_g[ext])),
        "fsm_ext_mean": np.mean(np.abs(err_g[ext])),
        "fsm_int_max": np.max(np.abs(err_g[intr])),
    })

    if op == "intersect":
        dirs = fan_directions()
        delta = 0.2
        raw_off, fsm_off = [], []
        for d_ in dirs:
            t_raw = ray_root(lambda x, y: f_raw(x, y, op) - delta, K_TOP, d_, 0.0, 1.5)
            raw_off.append(t_raw - delta)
            t_fsm = ray_root(lambda x, y: bilinear(G, x, y, x0, h) - delta, K_TOP, d_, 0.0, 1.5)
            fsm_off.append(t_fsm - delta)
        res["off_raw_max"] = np.max(np.abs(raw_off))
        res["off_fsm_max"] = np.max(np.abs(fsm_off))
        pts = [K_TOP + 0.3 * d_ for d_ in dirs]
        px = np.array([p[0] for p in pts]); py = np.array([p[1] for p in pts])
        res["fan_raw_max"] = np.max(np.abs(f_raw(px, py, op) - 0.3))
        res["fan_fsm_max"] = np.max(np.abs(bilinear(G, px, py, x0, h) - 0.3))
        phis = np.linspace(np.pi - 0.85, np.pi - 0.15, 15)
        inside_pred = lambda p: np.hypot(*(p - C1)) <= R      # retained: in C1
    else:
        res.update({"off_raw_max": np.nan, "off_fsm_max": np.nan,
                    "fan_raw_max": np.nan, "fan_fsm_max": np.nan})
        phis = np.linspace(0.3, 1.0, 15)
        inside_pred = lambda p: np.hypot(*(p - C1)) > R       # retained: ausserhalb C1

    zdisp = []
    for phi in phis:
        p = C2 + R * np.array([np.cos(phi), np.sin(phi)])
        if not inside_pred(p):
            continue
        nrm = (p - C2) / R
        t = ray_root(lambda x, y: bilinear(G, x, y, x0, h), p, nrm, -3*h, 3*h)
        zdisp.append(abs(t))
    res["zero_disp_max"] = max(zdisp)

    bis = np.array([0.0, 1.0])
    t_corner = ray_root(lambda x, y: bilinear(G, x, y, x0, h), K_TOP, bis, -6*h, 6*h)
    res["corner_disp"] = abs(t_corner)
    return res

if __name__ == "__main__":
    import time
    for op in ("union", "intersect"):
        keys = None
        for N in (65, 129, 257, 513):
            t0 = time.time()
            r = run(N, op)
            r["sec"] = time.time() - t0
            if keys is None:
                keys = list(r.keys())
                print("\t".join(keys))
            print("\t".join(f"{r[k]:.6g}" if isinstance(r[k], float) else str(r[k]) for k in keys))
        print()
