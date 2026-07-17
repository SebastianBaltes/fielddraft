#!/usr/bin/env python3
"""E3: (A) Zweikanal-Eval (Wert + exakter Gradient, forward AD) auf einem
Mini-Tape mit Twist-Warp — AD-Sampson vs. FD-Sampson.
(B) Segment-Tracing (Galin 2020) mit pro-Opcode-Segment-Lipschitz-Schranke
vs. globale Schritt-Dämpfung — Schrittzahlen und Tunnelraten.
"""
import numpy as np
from scipy.spatial import cKDTree
from exp1_smin_variants import zero_contour
from exp2_trace_depth_sampson import measure, sampson, warp_twist_field, first_crossing

TAU = 0.35
HX, HY = 1.2, 0.4

# ── A: Forward-AD-Eval des Feldes twist(tau) ∘ box ────────────────────────

def box_val_grad(x, y):
    qx = np.abs(x) - HX
    qy = np.abs(y) - HY
    ax = np.maximum(qx, 0.0)
    ay = np.maximum(qy, 0.0)
    outer = np.sqrt(ax * ax + ay * ay)
    inner = np.minimum(np.maximum(qx, qy), 0.0)
    v = outer + inner
    # Gradient (Subgradienten an Knicken: wie GPU-Code eine Seite wählen)
    gx = np.zeros_like(x)
    gy = np.zeros_like(y)
    out_m = outer > 0
    with np.errstate(invalid="ignore", divide="ignore"):
        gx = np.where(out_m, ax / np.maximum(outer, 1e-12) * np.sign(x), gx)
        gy = np.where(out_m, ay / np.maximum(outer, 1e-12) * np.sign(y), gy)
    in_m = ~out_m
    take_x = qx >= qy
    gx = np.where(in_m & take_x, np.sign(x), gx)
    gy = np.where(in_m & ~take_x, np.sign(y), gy)
    return v, gx, gy

def twist_box_val_grad(x, y):
    """p' = R(tau*y)p; ∇_welt f = J^T ∇_lokal f mit J = ∂p'/∂p."""
    a = TAU * y
    c, s = np.cos(a), np.sin(a)
    xr = c * x - s * y
    yr = s * x + c * y
    v, gx, gy = box_val_grad(xr, yr)
    # J = [[c, -s + dc/dy*x... ]] — vollständige Ableitungen:
    # xr = c(y)x - s(y)y → ∂xr/∂x = c ; ∂xr/∂y = -tau*s*x - s - tau*c*y
    # yr = s(y)x + c(y)y → ∂yr/∂x = s ; ∂yr/∂y = tau*c*x + c - tau*s*y
    dxr_dx = c
    dxr_dy = -TAU * s * x - s - TAU * c * y
    dyr_dx = s
    dyr_dy = TAU * c * x + c - TAU * s * y
    wx = gx * dxr_dx + gy * dyr_dx
    wy = gx * dxr_dy + gy * dyr_dy
    return v, wx, wy

def f_twist(x, y):
    return twist_box_val_grad(x, y)[0]

def sampson_ad(x, y):
    v, wx, wy = twist_box_val_grad(x, y)
    return v / np.maximum(np.sqrt(wx * wx + wy * wy), 1e-9)

def part_a():
    print("== A: Twist(0.35)+Box — roh vs. FD-Sampson vs. AD-Sampson ==")
    contour = zero_contour(f_twist, lo=-4, hi=4, n=1600)
    mr = measure(f_twist, contour)
    mfd = measure(sampson(f_twist), contour)
    mad = measure(sampson_ad, contour)
    print(f"{'':<12} {'over':>7} {'near50':>8} {'near10':>8}")
    for name, m in (("roh", mr), ("FD-Sampson", mfd), ("AD-Sampson", mad)):
        print(f"{name:<12} {m['over']:>7.3f} {m['near50']:>8.4f} {m['near10']:>8.4f}")

# ── B: Segment-Tracing mit lokaler Lipschitz-Schranke ─────────────────────
# Schranke des Feldes twist∘box über ein Ray-Segment [p(t0), p(t1)]:
# L_box = 1; |∇(f∘warp)| ≤ ‖J‖ ≤ 1 + |tau|·‖p_lokal‖ ≈ 1 + |tau|·max‖p‖ über
# das Segment (‖p'‖=‖p‖, Rotation). Pro-Opcode-Metadatum: Twist liefert
# λ(seg) = 1 + |tau|·max(‖p(t0)‖, ‖p(t1)‖) (Segment konvex, Norm konvex).

def lam_segment(p0, p1):
    r0 = np.linalg.norm(p0, axis=-1)
    r1 = np.linalg.norm(p1, axis=-1)
    return 1.0 + np.abs(TAU) * np.maximum(r0, r1)

def segment_trace(f, o, d, eps=1e-4, t_max=10.0, max_steps=4000):
    n = len(o)
    t = np.zeros(n)
    steps = np.zeros(n, dtype=int)
    done = np.zeros(n, dtype=bool)
    hit = np.zeros(n, dtype=bool)
    for _ in range(max_steps):
        p = o + t[:, None] * d
        fv = f(p[:, 0], p[:, 1])
        newly = ~done & (fv < eps)
        hit |= newly
        done |= newly | (t > t_max)
        if done.all():
            break
        adv = ~done
        # Kandidaten-Segment: erst optimistisch fv, dann 2 Fixpunkt-Iterationen
        dt = np.maximum(fv, eps)
        for _ in range(2):
            p1 = p + dt[:, None] * d
            lam = lam_segment(p, p1)
            dt = np.maximum(fv / lam, eps)
        t[adv] += dt[adv]
        steps[adv] += 1
    return t, steps, hit

def global_relax_trace(f, o, d, relax, eps=1e-4, t_max=10.0, max_steps=6000):
    n = len(o)
    t = np.zeros(n)
    steps = np.zeros(n, dtype=int)
    done = np.zeros(n, dtype=bool)
    hit = np.zeros(n, dtype=bool)
    for _ in range(max_steps):
        p = o + t[:, None] * d
        fv = f(p[:, 0], p[:, 1])
        newly = ~done & (fv < eps)
        hit |= newly
        done |= newly | (t > t_max)
        if done.all():
            break
        adv = ~done
        t[adv] += np.maximum(fv[adv] * relax, eps)
        steps[adv] += 1
    return t, steps, hit

def part_b():
    print("== B: Segment-Tracing vs. globale Dämpfung (Twist(0.35)+Box) ==")
    rng = np.random.default_rng(11)
    n = 3000
    ang = rng.uniform(0, 2 * np.pi, n)
    o = np.column_stack([6 * np.cos(ang), 6 * np.sin(ang)])
    tgt = np.column_stack([rng.uniform(-1.5, 1.5, n), rng.uniform(-1.5, 1.5, n)])
    d = tgt - o
    d /= np.linalg.norm(d, axis=1, keepdims=True)
    t_true = first_crossing(f_twist, o, d, t_max=10.0)
    finite = np.isfinite(t_true)
    # globale Schranke im Sichtvolumen: r bis ~6+Marschweite ⇒ L_glob=1+0.35·7.5
    L_glob = 1.0 + TAU * 7.5
    for label, (tt, steps, hit) in (
        ("naiv volle Schritte", global_relax_trace(f_twist, o, d, 1.0)),
        (f"global 1/L={1/L_glob:.3f}", global_relax_trace(f_twist, o, d, 1.0 / L_glob)),
        ("segment-lokal (Galin)", segment_trace(f_twist, o, d)),
    ):
        tunnel = finite & (~hit | (tt > t_true + 0.01))
        ok = finite & hit & ~tunnel
        print(f"  {label:<24} tunnel: {tunnel.sum():4d}/{finite.sum()} "
              f"({100*tunnel[finite].mean():5.2f}%)  steps(mean/p95): "
              f"{steps[ok].mean():7.1f}/{np.quantile(steps[ok],0.95):6.0f}")

if __name__ == "__main__":
    part_a()
    part_b()
