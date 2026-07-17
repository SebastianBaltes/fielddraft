#!/usr/bin/env python3
"""E2d: Größe des strukturellen Defekts von Innen-Offset (Erosion) über
harter Union: {min(a,b) = -δ} (= Union der Erosionen) vs. wahre Erosion
{sd_true = -δ}. Zwei Kreise R=1.5, Zentren (±0.8, 0)."""
import numpy as np
from scipy.spatial import cKDTree
from exp1_smin_variants import zero_contour, circle

ca, cb = circle(-0.8, 0, 1.5), circle(0.8, 0, 1.5)

def f_min(x, y):
    return np.minimum(ca(x, y), cb(x, y))

# Wahre signierte Distanz zur Union-Oberfläche
union_contour = zero_contour(f_min, lo=-4, hi=4, n=2400)
tree = cKDTree(union_contour)

def sd_true(x, y):
    x = np.asarray(x, dtype=float)
    y = np.asarray(y, dtype=float)
    shp = np.broadcast(x, y).shape
    P = np.column_stack([np.broadcast_to(x, shp).ravel(),
                         np.broadcast_to(y, shp).ravel()])
    d, _ = tree.query(P, workers=-1)
    return (np.sign(f_min(x, y)) * d.reshape(shp))

print(f"{'δ':>5} {'Hausdorff':>10} {'max Feld→wahr':>14} {'max wahr→Feld':>14}")
for delta in (0.2, 0.4, 0.6, 0.8):
    c_field = zero_contour(lambda x, y: f_min(x, y) + delta, lo=-4, hi=4, n=2000)
    c_truth = zero_contour(lambda x, y: sd_true(x, y) + delta, lo=-4, hi=4, n=900)
    tf, tt = cKDTree(c_field), cKDTree(c_truth)
    d1 = tt.query(c_field, workers=-1)[0]   # Feld-Kontur → wahre Kontur
    d2 = tf.query(c_truth, workers=-1)[0]
    j = np.argmax(d2)
    print(f"{delta:>5.1f} {max(d1.max(), d2.max()):>10.4f} {d1.max():>14.4f} {d2.max():>14.4f}"
          f"   (größter Defekt bei p=({c_truth[j,0]:+.2f},{c_truth[j,1]:+.2f}))")
