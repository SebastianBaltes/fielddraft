#!/usr/bin/env python3
"""E2: (A) Sphere-Trace-Simulation konkave Ecke (#386-Szenario),
(B) Fehlerwachstum über smin-Verschachtelungstiefe,
(C) Sampson-Korrektur f/|∇f| — wo sie hilft und wo nicht.
"""
import numpy as np
from scipy.spatial import cKDTree
from exp1_smin_variants import (
    smin_circ_geom, smin_circ_cd, smin_hard, make_config, zero_contour, circle,
)

rng = np.random.default_rng(7)

# ── A: Sphere-Trace-Simulation ────────────────────────────────────────────

def first_crossing(f, o, d, t_max=8.0, n=8000, iters=40):
    """Ground-Truth-Schnittparameter je Ray (dichte Abtastung + Bisektion)."""
    ts = np.linspace(0.0, t_max, n)
    P = o[:, None, :] + ts[None, :, None] * d[:, None, :]
    F = f(P[..., 0], P[..., 1])
    sgn = F[:, :-1] * F[:, 1:] < 0
    has = sgn.any(axis=1)
    idx = np.argmax(sgn, axis=1)
    t0 = ts[idx]
    t1 = ts[idx + 1]
    a = F[np.arange(len(o)), idx]
    for _ in range(iters):
        tm = 0.5 * (t0 + t1)
        pm = o + tm[:, None] * d
        fm = f(pm[:, 0], pm[:, 1])
        lo = a * fm < 0
        t1 = np.where(lo, tm, t1)
        t0 = np.where(lo, t0, tm)
        a = np.where(lo, a, fm)
    t = 0.5 * (t0 + t1)
    return np.where(has, t, np.inf)

def sphere_trace(f, o, d, relax=1.0, eps=1e-4, t_max=8.0, max_steps=2000):
    n = len(o)
    t = np.zeros(n)
    steps = np.zeros(n, dtype=int)
    done = np.zeros(n, dtype=bool)
    hit = np.zeros(n, dtype=bool)
    for _ in range(max_steps):
        p = o + t[:, None] * d
        fv = f(p[:, 0], p[:, 1])
        newly_hit = ~done & (fv < eps)
        hit |= newly_hit
        done |= newly_hit | (t > t_max)
        if done.all():
            break
        adv = ~done
        t[adv] += np.maximum(fv[adv] * relax, eps)
        steps[adv] += 1
    return t, steps, hit

def run_trace(label, f, o, d, t_true, relax):
    t, steps, hit = sphere_trace(f, o, d, relax=relax)
    finite = np.isfinite(t_true)
    tunnel = finite & (~hit | (t > t_true + 0.01))
    ok = finite & hit & ~tunnel
    print(f"  {label:<28} tunnel/miss: {tunnel.sum():4d}/{finite.sum()} "
          f"({100*tunnel.mean():5.2f}%)  steps(mean/p95): "
          f"{steps[ok].mean():6.1f}/{np.quantile(steps[ok],0.95):6.0f}")

def part_a():
    print("== A: Sphere-Trace, Union zweier Halbebenen γ=30° (konkave Luft-Ecke) ==")
    r = 1.0
    f_geom = make_config("union", 30, smin_circ_geom, r)
    f_cd = make_config("union", 30, smin_circ_cd, r)
    f_geom_n = lambda x, y: f_geom(x, y) / np.sqrt(2.0)
    # Luft = Keil zwischen den Ebenen, oben (beide d > 0). Rays von oben nach
    # unten in die Ecke, leicht gestreut (Grazing-Fälle inklusive).
    n = 4000
    ang0 = np.pi / 2  # Luft-Bisektrix zeigt nach +y
    o_ang = ang0 + rng.uniform(-0.20, 0.20, n)  # Luftkeil γ=30 → Halböffnung ~15°=0.26
    R0 = 5.0
    o = np.column_stack([R0 * np.cos(o_ang), R0 * np.sin(o_ang)])
    tgt = np.column_stack([rng.uniform(-1.5, 1.5, n), rng.uniform(-0.5, 0.5, n)])
    d = tgt - o
    d /= np.linalg.norm(d, axis=1, keepdims=True)
    # Ground truth je Variante GEGEN DAS EIGENE Feld (identische Nullmenge,
    # aber sauber ist sauber)
    t_true = first_crossing(f_geom, o, d)
    run_trace("geom raw (Mesher-Feld)", f_geom, o, d, t_true, 1.0)
    run_trace("geom /sqrt2 (#382)", f_geom_n, o, d, t_true, 1.0)
    run_trace("geom /sqrt2 + relax 0.75", f_geom_n, o, d, t_true, 0.75)
    run_trace("circ_cd raw", f_cd, o, d, t_true, 1.0)
    print("== A2: Intersection γ=30° (konvexe Kante, grazing) ==")
    g_geom = make_config("intersect", 30, smin_circ_geom, r)
    g_cd = make_config("intersect", 30, smin_circ_cd, r)
    g_geom_n = lambda x, y: g_geom(x, y) / np.sqrt(2.0)
    # Luft ist fast überall; Rays streifen die konvexe Ecke
    o2_ang = rng.uniform(0, 2 * np.pi, n)
    o2 = np.column_stack([5 * np.cos(o2_ang), 5 * np.sin(o2_ang)])
    tgt2 = np.column_stack([rng.uniform(-1, 1, n), rng.uniform(-1, 1, n)])
    d2 = tgt2 - o2
    d2 /= np.linalg.norm(d2, axis=1, keepdims=True)
    t_true2 = first_crossing(g_geom, o2, d2)
    run_trace("geom raw", g_geom, o2, d2, t_true2, 1.0)
    run_trace("geom /sqrt2", g_geom_n, o2, d2, t_true2, 1.0)
    run_trace("geom /sqrt2 + relax 0.75", g_geom_n, o2, d2, t_true2, 0.75)
    run_trace("circ_cd raw", g_cd, o2, d2, t_true2, 1.0)

# ── B: Verschachtelungstiefe ──────────────────────────────────────────────

def chain_field(smin, n_circ, r):
    cs = [circle(1.1 * i, 0.35 * ((-1) ** i), 0.8) for i in range(n_circ)]
    def f(x, y):
        acc = cs[0](x, y)
        for c in cs[1:]:
            acc = smin(acc, c(x, y), r)
        return acc
    return f

def part_b():
    print("== B: max|∇f| und Überschätzung über Kettenlänge N (smin-Union-Kette) ==")
    r = 0.6
    print(f"{'N':>3} {'geom |∇f|max':>13} {'geom over':>10} {'cd |∇f|max':>11} {'cd over':>8}")
    for n_circ in (2, 3, 4, 6, 8, 12):
        row = [n_circ]
        for smin in (smin_circ_geom, smin_circ_cd):
            f = chain_field(smin, n_circ, r)
            contour = zero_contour(f, lo=-3.0, hi=3.0 + 1.1 * n_circ, n=1600)
            tree = cKDTree(contour)
            xs = np.linspace(-2, 1.1 * n_circ + 2, 500)
            ys = np.linspace(-3, 3, 260)
            X, Y = np.meshgrid(xs, ys, indexing="ij")
            F = f(X, Y).ravel()
            P = np.column_stack([X.ravel(), Y.ravel()])
            d_true, _ = tree.query(P, workers=-1)
            h = 1e-4
            gx = (f(X + h, Y) - f(X - h, Y)) / (2 * h)
            gy = (f(X, Y + h) - f(X, Y - h)) / (2 * h)
            gn = np.sqrt(gx**2 + gy**2).ravel()
            m = (F > 0) & (d_true > 0.02)
            row += [float(gn.max()), float((F[m] / d_true[m]).max())]
        print(f"{row[0]:>3} {row[1]:>13.3f} {row[2]:>10.3f} {row[3]:>11.3f} {row[4]:>8.3f}")

# ── C: Sampson-Korrektur f/|∇f| ───────────────────────────────────────────

def sampson(f, h=1e-4):
    def g(x, y):
        fv = f(x, y)
        gx = (f(x + h, y) - f(x - h, y)) / (2 * h)
        gy = (f(x, y + h) - f(x, y - h)) / (2 * h)
        gn = np.sqrt(gx * gx + gy * gy)
        return fv / np.maximum(gn, 1e-9)
    return g

def measure(f, contour, lo=-2.0, hi=2.0, n=400, r=1.0):
    tree = cKDTree(contour)
    xs = np.linspace(lo, hi, n)
    X, Y = np.meshgrid(xs, xs, indexing="ij")
    F = f(X, Y).ravel()
    P = np.column_stack([X.ravel(), Y.ravel()])
    d_true, _ = tree.query(P, workers=-1)
    sd = np.sign(F) * d_true
    out = {}
    m = (F > 0) & (d_true > 0.02)
    out["over"] = float((F[m] / d_true[m]).max()) if m.any() else float("nan")
    for band, key in ((0.5 * r, "near50"), (0.1 * r, "near10")):
        mm = (np.abs(sd) < band) & (d_true > 1e-3)
        out[key] = float(np.abs(F[mm] - sd[mm]).max()) if mm.any() else float("nan")
    return out

def warp_twist_field(tau, hx=1.2, hy=0.4):
    """p' = R(tau*y)·p, dann Box(hx,hy) — bricht Lipschitz wie OP_WARP_TWIST."""
    def box(x, y):
        qx = np.abs(x) - hx
        qy = np.abs(y) - hy
        ax = np.maximum(qx, 0.0)
        ay = np.maximum(qy, 0.0)
        return np.sqrt(ax * ax + ay * ay) + np.minimum(np.maximum(qx, qy), 0.0)
    def f(x, y):
        a = tau * y
        c, s = np.cos(a), np.sin(a)
        xr = c * x - s * y
        yr = s * x + c * y
        return box(xr, yr)
    return f

def part_c():
    print("== C: Sampson f/|∇f| — near-band Fehler und Sicherheit ==")
    r = 1.0
    cases = []
    fg30 = make_config("union", 30, smin_circ_geom, r)
    cases.append(("circ_geom union30", fg30))
    from exp1_smin_variants import smin_r0
    fr0 = make_config("union", 60, smin_r0, r)
    cases.append(("rvachev_r0 union60", fr0))
    ftw = warp_twist_field(0.35)
    cases.append(("twist(0.35)+box", ftw))
    # Nichtlokaler Fehler: Interior-Seam der harten Union (Kreise überlappend)
    ca, cb = circle(-0.8, 0, 1.5), circle(0.8, 0, 1.5)
    fhard = lambda x, y: np.minimum(ca(x, y), cb(x, y))
    cases.append(("hard_min 2 Kreise", fhard))
    print(f"{'case':<20} {'roh over':>9} {'S over':>7} {'roh n50':>8} {'S n50':>7} {'roh n10':>8} {'S n10':>7}")
    for name, f in cases:
        contour = zero_contour(f, lo=-4.0, hi=4.0, n=1600)
        mr = measure(f, contour, r=r)
        ms = measure(sampson(f), contour, r=r)
        print(f"{name:<20} {mr['over']:>9.3f} {ms['over']:>7.3f} "
              f"{mr['near50']:>8.4f} {ms['near50']:>7.4f} {mr['near10']:>8.4f} {ms['near10']:>7.4f}")

if __name__ == "__main__":
    part_a()
    part_b()
    part_c()
