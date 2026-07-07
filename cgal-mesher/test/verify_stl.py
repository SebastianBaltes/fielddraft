#!/usr/bin/env python3
"""M1-Prüfskript des Server-Meshers (docs/CGAL-SERVER-MESHING-PLAN.md).

    verify_stl.py <job.fdmesh> <mesh.stl> [--edge-tol-mult 1.5]

Prüft das binäre STL gegen den Interchange-Job:
  1. Wasserdicht/2-manifold: nach bitweisem Vertex-Weld hat jede
     ungerichtete Kante GENAU zwei inzidente Dreiecke, und jede gerichtete
     Kante kommt genau einmal vor (konsistente Orientierung).
  2. Signiertes Volumen > 0 (Außen-Orientierung).
  3. Kanten-Treue: jeder Punkt jeder geschützten EDGE-Polyline hat einen
     Mesh-Vertex in < edge_tol (default 1.5 × cellSizeMm) — die
     CGAL-Schutzkugeln setzen Kurven-Vertices im edge_size-Abstand.
Exit 0 = alles bestanden, 1 = Verletzung, 2 = Usage/IO.
"""
import json
import struct
import sys


def read_job(path):
    cell = 0.5
    edges = []  # list of [(x,y,z), ...]
    with open(path) as f:
        for line in f:
            parts = line.split()
            if not parts:
                continue
            if parts[0] == "PARAMS":
                try:
                    cell = float(json.loads(line[7:]).get("cellSizeMm", cell))
                except (ValueError, json.JSONDecodeError):
                    pass
            elif parts[0] == "EDGE":
                n = int(parts[1])
                c = [struct.unpack("<f", struct.pack("<I", int(h, 16)))[0]
                     for h in parts[2:2 + 3 * n]]
                edges.append([(c[3 * i], c[3 * i + 1], c[3 * i + 2])
                              for i in range(n)])
    return cell, edges


def read_stl(path):
    with open(path, "rb") as f:
        f.read(80)
        (ntri,) = struct.unpack("<I", f.read(4))
        tris = []
        for _ in range(ntri):
            rec = f.read(50)
            v = struct.unpack("<12f", rec[:48])
            tris.append(((v[3], v[4], v[5]), (v[6], v[7], v[8]),
                         (v[9], v[10], v[11])))
    return tris


def main():
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    tol_mult = 1.5
    if "--edge-tol-mult" in sys.argv:
        tol_mult = float(sys.argv[sys.argv.index("--edge-tol-mult") + 1])
    cell, edges = read_job(sys.argv[1])
    tris = read_stl(sys.argv[2])
    if not tris:
        print("FAIL: empty STL")
        return 1

    # Bitwise weld (fd-cgal-mesh writes each welded vertex identically).
    vid = {}
    faces = []
    for t in tris:
        idx = []
        for p in t:
            if p not in vid:
                vid[p] = len(vid)
            idx.append(vid[p])
        faces.append(tuple(idx))

    ok = True
    undirected = {}
    directed = {}
    for f3 in faces:
        if len(set(f3)) < 3:
            print(f"FAIL: degenerate triangle {f3}")
            ok = False
        for a, b in ((f3[0], f3[1]), (f3[1], f3[2]), (f3[2], f3[0])):
            undirected[(min(a, b), max(a, b))] = undirected.get(
                (min(a, b), max(a, b)), 0) + 1
            directed[(a, b)] = directed.get((a, b), 0) + 1
    bad_u = {e: n for e, n in undirected.items() if n != 2}
    bad_d = {e: n for e, n in directed.items() if n != 1}
    if bad_u:
        print(f"FAIL: {len(bad_u)} edges not shared by exactly 2 triangles "
              f"(examples: {list(bad_u.items())[:5]})")
        ok = False
    if bad_d:
        print(f"FAIL: {len(bad_d)} directed edges duplicated — inconsistent "
              f"orientation (examples: {list(bad_d.items())[:5]})")
        ok = False

    verts = list(vid)
    vol6 = 0.0
    for f3 in faces:
        a, b, c = (verts[f3[0]], verts[f3[1]], verts[f3[2]])
        vol6 += (a[0] * (b[1] * c[2] - b[2] * c[1])
                 - a[1] * (b[0] * c[2] - b[2] * c[0])
                 + a[2] * (b[0] * c[1] - b[1] * c[0]))
    if vol6 <= 0:
        print(f"FAIL: signed volume {vol6 / 6.0:.3f} ≤ 0 (inward orientation)")
        ok = False

    # Edge fidelity: nearest mesh vertex per polyline point.
    tol = tol_mult * cell
    worst = 0.0
    misses = 0
    for poly in edges:
        for p in poly:
            best = min((p[0] - q[0]) ** 2 + (p[1] - q[1]) ** 2
                       + (p[2] - q[2]) ** 2 for q in verts)
            d = best ** 0.5
            worst = max(worst, d)
            if d > tol:
                misses += 1
    if misses:
        print(f"FAIL: {misses} protected-edge points farther than "
              f"{tol:.3f}mm from any mesh vertex (worst {worst:.3f}mm)")
        ok = False

    v, e, f = len(verts), len(undirected), len(faces)
    print(f"{'PASS' if ok else 'FAIL'}: V={v} E={e} F={f} "
          f"euler={v - e + f} volume={vol6 / 6.0:.1f}mm3 "
          f"edge_worst={worst:.3f}mm (tol {tol:.3f})")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
