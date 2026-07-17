"""E3: Wie exakt ist smin_poly als Distanz zu seiner EIGENEN Nullflaeche —
und was heisst das fuer Offsets {f=delta}?

csg.rs behauptet: im Arc-Bereich exakte Distanz zum Fillet-Kreis; max.
Undershoot unter min ist (sqrt(2)-1)*r bei da=db=0. Hier: vollstaendige
Fehlerkarte (aussen/innen), dann Offset-Isokonturen roh vs. Wurzel-Sampson.
"""

import numpy as np

from lib import GroundTruth, boundary_by_rays, circle, grad_fd, smin_poly

R = 1.0
r_blend = 0.25
cx = 0.6
c1, c2 = np.array([-cx, 0.0]), np.array([cx, 0.0])
f1, f2 = circle(c1, R), circle(c2, R)
rng = np.random.default_rng(5)


def field(p):
    return smin_poly(f1(p), f2(p), r_blend)


bpts = boundary_by_rays(field, np.zeros(2), n_rays=400_000, r_max=4.0)
truth = GroundTruth(bpts, field)

# ── Fehlerkarte ────────────────────────────────────────────────────
pts = rng.uniform(-2.2, 2.2, (300_000, 2))
d = truth(pts)
f = field(pts)
err = f - d
out = d > 0
inn = d < 0
near = np.abs(d) < 0.5
print("smin-Union cx=0.6, r=0.25 — Fehler f - d_true (zu EIGENER Nullflaeche):")
print(f"  aussen:        min={err[out].min():+.4e} max={err[out].max():+.4e}")
print(f"  aussen nah:    min={err[out & near].min():+.4e} max={err[out & near].max():+.4e}")
print(f"  innen:         min={err[inn].min():+.4e} max={err[inn].max():+.4e}")
print(f"  Ueberschaetzung (f>d, Sphere-Trace-Gefahr): Anteil {(err > 1e-6).mean():.2%}, "
      f"max Faktor f/d = {np.nanmax(np.where(out & (d > 1e-3), f / d, np.nan)):.4f}")

# wo liegt die Ueberschaetzung?
ov = err > 1e-4
if ov.any():
    po = pts[ov]
    print(f"  Ueberschaetzungs-Zone: |x| in [{np.abs(po[:,0]).min():.2f},"
          f"{np.abs(po[:,0]).max():.2f}], d_true in "
          f"[{d[ov].min():+.3f},{d[ov].max():+.3f}]")

# ── Offsets: Isokontur {F=delta} vs wahre Distanz ─────────────────


def iso_by_rays(F, delta, n=100_000):
    def g(p):
        return F(p) - delta

    return boundary_by_rays(g, np.zeros(2), n_rays=n, r_max=4.0)


def sampson_field(p, h=1e-5):
    fv = field(p)
    g = grad_fd(field, p, h)
    return fv / np.maximum(np.linalg.norm(g, axis=-1), 1e-12)


print("\nOffset-Isokonturen: wahre Distanz d_true auf {F=delta} (soll = delta):")
print(f"  {'delta':>7} {'roh min':>10} {'roh max':>10} {'sampson min':>12} {'sampson max':>12}")
for delta in [0.05, 0.1, 0.2, 0.4]:
    iso_raw = iso_by_rays(field, delta)
    d_raw = truth(iso_raw)
    iso_s = iso_by_rays(sampson_field, delta)
    d_s = truth(iso_s)
    print(f"  {delta:>7} {d_raw.min():>10.4f} {d_raw.max():>10.4f} "
          f"{d_s.min():>12.4f} {d_s.max():>12.4f}")

print("\nInward-Offset (delta<0), roh:")
for delta in [-0.05, -0.1, -0.2]:
    iso_raw = iso_by_rays(field, delta)
    d_raw = truth(iso_raw)
    iso_s = iso_by_rays(sampson_field, delta)
    d_s = truth(iso_s)
    print(f"  {delta:>7} {d_raw.min():>10.4f} {d_raw.max():>10.4f} "
          f"{d_s.min():>12.4f} {d_s.max():>12.4f}")
