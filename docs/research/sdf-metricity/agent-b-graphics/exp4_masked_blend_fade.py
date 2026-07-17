#!/usr/bin/env python3
"""E4: 2D-Analogon von masked_blend_render_fade mit circ_geom- vs.
circ_cd-Blend: max |∇f| und Überschätzung. Entscheidet, ob nach dem
circ_cd-Tausch der ÷√2-Fade UND MASKED_BLEND_MARCH_RELAX 0.75 entfallen.

Aufbau wie kernel: hard = hartes Boolean, blended = smin, Maske = Disc um die
Kante (Radius 1.5*r), w = smoothstep über Band = r,
f = hard + (blended-hard)*w  [ohne ÷√2].
"""
import numpy as np
from scipy.spatial import cKDTree
from exp1_smin_variants import (
    smin_circ_geom, smin_circ_cd, halfplane, zero_contour,
)

R = 1.0
MASK_R = 1.5 * R
BAND = max(R, 1e-3)

def fade_field(kind, gamma_deg, smin):
    g = np.radians(gamma_deg)
    da = halfplane(np.sin(g / 2), np.cos(g / 2))
    db = halfplane(-np.sin(g / 2), np.cos(g / 2))
    def f(x, y):
        a, b = da(x, y), db(x, y)
        if kind == "union":
            hard = np.minimum(a, b)
            blended = smin(a, b, R)
        else:  # sdiff: A minus B
            hard = np.maximum(a, -b)
            blended = -smin(-a, b, R)
        mask_d = np.sqrt(x * x + y * y) - MASK_R
        t = np.clip(-mask_d / BAND, 0.0, 1.0)
        w = t * t * (3.0 - 2.0 * t)
        return hard + (blended - hard) * w
    return f

def metrics(f):
    contour = zero_contour(f, lo=-5, hi=5, n=2000)
    tree = cKDTree(contour)
    xs = np.linspace(-2.5, 2.5, 500)
    X, Y = np.meshgrid(xs, xs, indexing="ij")
    F = f(X, Y).ravel()
    P = np.column_stack([X.ravel(), Y.ravel()])
    d_true, _ = tree.query(P, workers=-1)
    h = 1e-4
    gx = (f(X + h, Y) - f(X - h, Y)) / (2 * h)
    gy = (f(X, Y + h) - f(X, Y - h)) / (2 * h)
    gn = np.sqrt(gx * gx + gy * gy).ravel()
    m = (F > 0) & (d_true > 0.02)
    return float(gn.max()), float((F[m] / d_true[m]).max())

print(f"{'config':<14} {'blend':<10} {'|∇f|max':>8} {'over_ratio':>10}")
for kind in ("union", "sdiff"):
    for gamma in (20, 30, 60, 90, 120, 150, 170):
        for bname, smin in (("circ_geom", smin_circ_geom), ("circ_cd", smin_circ_cd)):
            gmax, over = metrics(fade_field(kind, gamma, smin))
            print(f"{kind+'_'+str(gamma):<14} {bname:<10} {gmax:>8.3f} {over:>10.3f}")
