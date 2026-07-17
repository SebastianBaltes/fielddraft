//! Agent C (Hybrid) — Experiment-Harness: Nahfeld-Distanz zur exakten
//! Randrepräsentation (Proxy-Mesh + BVH) vs. min/max-Schranken-Feld.
//!
//! Experimente:
//!   E1  Box-minus-Box (L-Prisma): min/max-Feld vs. analytische Ground-Truth
//!       an der konkaven Naht; Proxy-Mesh-Distanz als Validierung (planar ⇒ exakt).
//!   E1b Offset-Konsequenz: wahre Distanz der Isofläche f=δ.
//!   E2  Zylinder: Tesselationsfehler von d_mesh über Segmentzahl (Sehnenfehler).
//!   E3  BVH-Query-Kosten über Meshgröße (Build + Queries/s).
//!   E4  Hybrid-Blend d_near↔f: Sprung/Gradienten-Spike im Blendband.
//!
//! Alles f64, dependency-frei. Ausgabe: Klartext-Tabellen.

use std::time::Instant;

// ---------- Mini-Vec3 ----------
type V3 = [f64; 3];
fn sub(a: V3, b: V3) -> V3 { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
fn add(a: V3, b: V3) -> V3 { [a[0]+b[0], a[1]+b[1], a[2]+b[2]] }
fn scale(a: V3, s: f64) -> V3 { [a[0]*s, a[1]*s, a[2]*s] }
fn dot(a: V3, b: V3) -> f64 { a[0]*b[0]+a[1]*b[1]+a[2]*b[2] }
fn cross(a: V3, b: V3) -> V3 { [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]] }
fn len(a: V3) -> f64 { dot(a,a).sqrt() }

// ---------- Exakte Primitiv-SDFs ----------
fn sd_box(p: V3, c: V3, he: V3) -> f64 {
    let q = [ (p[0]-c[0]).abs()-he[0], (p[1]-c[1]).abs()-he[1], (p[2]-c[2]).abs()-he[2] ];
    let outside = [ q[0].max(0.0), q[1].max(0.0), q[2].max(0.0) ];
    len(outside) + q[0].max(q[1]).max(q[2]).min(0.0)
}

/// Exakter Zylinder (Achse z, Radius r, |z|<=hz), zentriert.
fn sd_cyl(p: V3, r: f64, hz: f64) -> f64 {
    let dr = (p[0]*p[0]+p[1]*p[1]).sqrt() - r;
    let dz = p[2].abs() - hz;
    let ox = dr.max(0.0); let oy = dz.max(0.0);
    (ox*ox+oy*oy).sqrt() + dr.max(dz).min(0.0)
}

// ---------- Exakte SDF eines extrudierten Polygons (Ground Truth L-Prisma) ----------
/// 2D-Polygon-SDF (exakt, Kanten + Parität), Quickhull-frei.
fn sd_polygon2(p: [f64;2], v: &[[f64;2]]) -> f64 {
    let n = v.len();
    let mut d = f64::MAX;
    let mut sign = 1.0f64;
    let mut j = n - 1;
    for i in 0..n {
        let e = [v[j][0]-v[i][0], v[j][1]-v[i][1]];
        let w = [p[0]-v[i][0], p[1]-v[i][1]];
        let t = ((w[0]*e[0]+w[1]*e[1]) / (e[0]*e[0]+e[1]*e[1])).clamp(0.0, 1.0);
        let b = [w[0]-e[0]*t, w[1]-e[1]*t];
        d = d.min(b[0]*b[0]+b[1]*b[1]);
        // Parität (Crossing-Test)
        let cond = [p[1] >= v[i][1], p[1] < v[j][1], e[0]*w[1] > e[1]*w[0]];
        if cond.iter().all(|&c| c) || cond.iter().all(|&c| !c) { sign = -sign; }
        j = i;
    }
    sign * d.sqrt()
}

/// Exakte SDF der Extrusion des Polygons entlang z, |z| <= hz.
fn sd_extrude(p: V3, poly: &[[f64;2]], hz: f64) -> f64 {
    let d2 = sd_polygon2([p[0], p[1]], poly);
    let dz = p[2].abs() - hz;
    let ox = d2.max(0.0); let oy = dz.max(0.0);
    (ox*ox+oy*oy).sqrt() + d2.max(dz).min(0.0)
}

// ---------- Punkt-Dreieck-Distanz (Ericson, RTCD Kap. 5.1.5) ----------
fn closest_on_tri(p: V3, a: V3, b: V3, c: V3) -> V3 {
    let ab = sub(b,a); let ac = sub(c,a); let ap = sub(p,a);
    let d1 = dot(ab,ap); let d2 = dot(ac,ap);
    if d1 <= 0.0 && d2 <= 0.0 { return a; }
    let bp = sub(p,b);
    let d3 = dot(ab,bp); let d4 = dot(ac,bp);
    if d3 >= 0.0 && d4 <= d3 { return b; }
    let vc = d1*d4 - d3*d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1/(d1-d3); return add(a, scale(ab,v));
    }
    let cp = sub(p,c);
    let d5 = dot(ab,cp); let d6 = dot(ac,cp);
    if d6 >= 0.0 && d5 <= d6 { return c; }
    let vb = d5*d2 - d1*d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2/(d2-d6); return add(a, scale(ac,w));
    }
    let va = d3*d6 - d5*d4;
    if va <= 0.0 && (d4-d3) >= 0.0 && (d5-d6) >= 0.0 {
        let w = (d4-d3)/((d4-d3)+(d5-d6));
        return add(b, scale(sub(c,b), w));
    }
    let denom = 1.0/(va+vb+vc);
    let v = vb*denom; let w = vc*denom;
    add(a, add(scale(ab,v), scale(ac,w)))
}

// ---------- Mesh + BVH ----------
struct Mesh { v: Vec<V3>, t: Vec<[u32;3]> }

#[derive(Clone, Copy)]
struct Aabb { min: V3, max: V3 }
impl Aabb {
    const EMPTY: Aabb = Aabb { min: [f64::MAX;3], max: [f64::MIN;3] };
    fn grow(&mut self, p: V3) {
        for k in 0..3 { self.min[k] = self.min[k].min(p[k]); self.max[k] = self.max[k].max(p[k]); }
    }
    fn merge(&mut self, o: &Aabb) {
        for k in 0..3 { self.min[k] = self.min[k].min(o.min[k]); self.max[k] = self.max[k].max(o.max[k]); }
    }
    fn dist2(&self, p: V3) -> f64 {
        let mut d = 0.0;
        for k in 0..3 {
            let v = if p[k] < self.min[k] { self.min[k]-p[k] }
                    else if p[k] > self.max[k] { p[k]-self.max[k] } else { 0.0 };
            d += v*v;
        }
        d
    }
}

struct BvhNode { bb: Aabb, left: u32, right: u32, start: u32, count: u32 }

struct Bvh { nodes: Vec<BvhNode>, order: Vec<u32> }

impl Bvh {
    fn build(mesh: &Mesh) -> Bvh {
        let n = mesh.t.len();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let cents: Vec<V3> = mesh.t.iter().map(|t| {
            let (a,b,c) = (mesh.v[t[0] as usize], mesh.v[t[1] as usize], mesh.v[t[2] as usize]);
            scale(add(add(a,b),c), 1.0/3.0)
        }).collect();
        let mut nodes = Vec::with_capacity(2*n);
        Self::build_rec(mesh, &cents, &mut order, 0, n, &mut nodes);
        Bvh { nodes, order }
    }
    fn build_rec(mesh: &Mesh, cents: &[V3], order: &mut [u32], start: usize, count: usize, nodes: &mut Vec<BvhNode>) -> u32 {
        let mut bb = Aabb::EMPTY;
        for &ti in &order[start..start+count] {
            let t = mesh.t[ti as usize];
            for &vi in &t { bb.grow(mesh.v[vi as usize]); }
        }
        let idx = nodes.len() as u32;
        nodes.push(BvhNode { bb, left: 0, right: 0, start: start as u32, count: count as u32 });
        if count <= 4 { return idx; }
        // Median-Split auf längster Centroid-Achse
        let mut cb = Aabb::EMPTY;
        for &ti in &order[start..start+count] { cb.grow(cents[ti as usize]); }
        let ext = sub(cb.max, cb.min);
        let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] { 0 } else if ext[1] >= ext[2] { 1 } else { 2 };
        let seg = &mut order[start..start+count];
        let mid = count/2;
        seg.select_nth_unstable_by(mid, |&a, &b| cents[a as usize][axis].partial_cmp(&cents[b as usize][axis]).unwrap());
        let l = Self::build_rec(mesh, cents, order, start, mid, nodes);
        let r = Self::build_rec(mesh, cents, order, start+mid, count-mid, nodes);
        nodes[idx as usize].left = l;
        nodes[idx as usize].right = r;
        nodes[idx as usize].count = 0;
        idx
    }
    /// Unsignierte Distanz Punkt→Mesh (exakter Nearest über BVH-Pruning).
    fn dist(&self, mesh: &Mesh, p: V3) -> f64 {
        let mut best = f64::MAX; // Distanz²
        let mut stack = [0u32; 64];
        let mut sp = 0usize;
        stack[sp] = 0; sp += 1;
        while sp > 0 {
            sp -= 1;
            let node = &self.nodes[stack[sp] as usize];
            if node.bb.dist2(p) >= best { continue; }
            if node.count > 0 {
                for i in node.start..node.start+node.count {
                    let t = mesh.t[self.order[i as usize] as usize];
                    let q = closest_on_tri(p, mesh.v[t[0] as usize], mesh.v[t[1] as usize], mesh.v[t[2] as usize]);
                    let d2 = { let d = sub(p,q); dot(d,d) };
                    if d2 < best { best = d2; }
                }
            } else {
                // Näheres Kind zuerst (bessere Pruning-Reihenfolge)
                let (l, r) = (node.left, node.right);
                let dl = self.nodes[l as usize].bb.dist2(p);
                let dr = self.nodes[r as usize].bb.dist2(p);
                let (first, second, dfirst, dsecond) = if dl <= dr { (l, r, dl, dr) } else { (r, l, dr, dl) };
                if dsecond < best { stack[sp] = second; sp += 1; }
                if dfirst < best { stack[sp] = first; sp += 1; }
            }
        }
        best.sqrt()
    }
}

// ---------- Szenen ----------
// Szene E1: A = Box he(20,10,10) @ Origin; B = Box he(12.5,10,12.5) @ (12.5,10,0).
// A\B = L-Prisma: Polygon (CCW) (−20,−10),(20,−10),(20,0),(0,0),(0,10),(−20,10), z∈[−10,10].
// Konkave Kante: x=0, y=0, z∈[−10,10].
const L_POLY: [[f64;2];6] = [[-20.0,-10.0],[20.0,-10.0],[20.0,0.0],[0.0,0.0],[0.0,10.0],[-20.0,10.0]];

fn f_minmax_e1(p: V3) -> f64 {
    let da = sd_box(p, [0.0,0.0,0.0], [20.0,10.0,10.0]);
    let db = sd_box(p, [12.5,10.0,0.0], [12.5,10.0,12.5]);
    da.max(-db)
}
fn d_true_e1(p: V3) -> f64 { sd_extrude(p, &L_POLY, 10.0) }

/// Exaktes Proxy-Mesh des L-Prismas (20 Dreiecke; planar ⇒ tesselationsfehlerfrei).
fn l_prism_mesh() -> Mesh {
    let hz = 10.0;
    let poly = L_POLY;
    let mut v: Vec<V3> = Vec::new();
    for &q in &poly { v.push([q[0], q[1], -hz]); }
    for &q in &poly { v.push([q[0], q[1],  hz]); }
    let mut t: Vec<[u32;3]> = Vec::new();
    // Deckel: L = zwei Rechtecke: (−20,−10)-(20,0) und (−20,0)-(0,10)
    // unterer Deckel (z=-hz), Indizes 0..5 (Orientierung hier egal — nur Distanz)
    // Rechteck 1: v0(−20,−10) v1(20,−10) v2(20,0) + Punkt (−20,0) — nicht im Ring!
    // Einfacher: trianguliere das L als Fan-frei per fixer Zerlegung mit Extra-Punkt (−20,0).
    let extra_lo = v.len() as u32; v.push([-20.0, 0.0, -hz]);
    let extra_hi = v.len() as u32; v.push([-20.0, 0.0,  hz]);
    // unten: (0,1,2),(0,2,3)? (0,0) ist Index 3. Rechteck1 = 0,1,2,3? Punkte:
    // 0=(−20,−10) 1=(20,−10) 2=(20,0) 3=(0,0) 4=(0,10) 5=(−20,10)
    // Rechteck A: 0,1,2 + (−20,0)=extra_lo → Quad 0,1,2,extra_lo? Nein: Quad (−20,−10),(20,−10),(20,0),(−20,0) = 0,1,2,extra.
    t.push([0,1,2]); t.push([0,2,extra_lo]);
    // Rechteck B (oben links): (−20,0),(0,0),(0,10),(−20,10) = extra,3,4,5
    t.push([extra_lo,3,4]); t.push([extra_lo,4,5]);
    // oberer Deckel analog (Offset 6)
    t.push([6,7,8]); t.push([6,8,extra_hi]);
    t.push([extra_hi,9,10]); t.push([extra_hi,10,11]);
    // Seitenwände: Ringkanten (i, j=i+1 mod 6)
    for i in 0..6u32 {
        let j = (i+1)%6;
        t.push([i, j, j+6]);
        t.push([i, j+6, i+6]);
    }
    Mesh { v, t }
}

/// Zylinder-Mesh (Achse z), n_seg Segmente, n_ring Höhenringe, inkl. Deckel-Fans.
fn cylinder_mesh(r: f64, hz: f64, n_seg: usize, n_ring: usize) -> Mesh {
    let mut v: Vec<V3> = Vec::new();
    let mut t: Vec<[u32;3]> = Vec::new();
    for iz in 0..=n_ring {
        let z = -hz + 2.0*hz*(iz as f64)/(n_ring as f64);
        for i in 0..n_seg {
            let a = std::f64::consts::TAU * (i as f64)/(n_seg as f64);
            v.push([r*a.cos(), r*a.sin(), z]);
        }
    }
    for iz in 0..n_ring {
        for i in 0..n_seg {
            let j = (i+1)%n_seg;
            let (a,b,c,d) = ((iz*n_seg+i) as u32, (iz*n_seg+j) as u32, ((iz+1)*n_seg+j) as u32, ((iz+1)*n_seg+i) as u32);
            t.push([a,b,c]); t.push([a,c,d]);
        }
    }
    let c_lo = v.len() as u32; v.push([0.0,0.0,-hz]);
    let c_hi = v.len() as u32; v.push([0.0,0.0, hz]);
    for i in 0..n_seg {
        let j = (i+1)%n_seg;
        t.push([c_lo, i as u32, j as u32]);
        let base = (n_ring*n_seg) as u32;
        t.push([c_hi, base + j as u32, base + i as u32]);
    }
    Mesh { v, t }
}

// ---------- RNG (xorshift64*) ----------
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x >> 12; x ^= x << 25; x ^= x >> 27;
        self.0 = x;
        ((x.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f64) / ((1u64<<53) as f64)
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 { lo + (hi-lo)*self.next() }
}

fn main() {
    e1();
    e1b();
    e1c();
    e1d();
    e2();
    e3();
    e4();
    e5();
}

// ---------- E1c: DERSELBE L-Körper als Union zweier Boxen — klassischer √2-Fall innen ----------
fn e1c() {
    println!("== E1c: L als UNION (Box y∈[−10,0] ∪ Box x∈[−20,0]) — Fehler INNEN an der konkaven Ecke ==");
    let f_union = |p: V3| -> f64 {
        let d1 = sd_box(p, [0.0,-5.0,0.0], [20.0,5.0,10.0]);   // y∈[−10,0]
        let d2 = sd_box(p, [-10.0,0.0,0.0], [10.0,10.0,10.0]); // x∈[−20,0], y∈[−10,10]
        d1.min(d2)
    };
    let mut rng = Rng(0xC0FFEE0123456789);
    let mut max_ratio = 1.0f64; let mut max_abs = 0.0f64; let mut sum = 0.0; let mut n = 0usize;
    while n < 100_000 {
        let p = [rng.range(-8.0,-0.01), rng.range(-8.0,-0.01), rng.range(-9.0,9.0)];
        let dt = d_true_e1(p);
        if dt >= 0.0 || dt < -5.0 { continue; }
        let f = f_union(p);
        let err = (f - dt).abs();
        sum += err; n += 1;
        if err > max_abs { max_abs = err; }
        if f.abs() > 1e-9 { max_ratio = max_ratio.max(dt.abs()/f.abs()); }
    }
    println!("  innen nahe konkaver Ecke: max|f−d|={:.4} mm  mean={:.4} mm  max |d|/|f| = {:.4}  (√2 = {:.4})",
        max_abs, sum/(n as f64), max_ratio, std::f64::consts::SQRT_2);
}

// ---------- E1d: globaler Iso-Survey f=δ (nicht nur Notch) ----------
fn e1d() {
    println!("== E1d: globaler Survey der Isofläche f_minmax=δ=2: wahre Distanz dieser Punkte ==");
    let delta = 2.0;
    let mut rng = Rng(0x0DDBA11FEEDC0DE5);
    let (mut min_d, mut max_d) = (f64::MAX, f64::MIN);
    let mut worst = [0.0;3];
    let mut n = 0usize;
    while n < 50_000 {
        let p0 = [rng.range(-26.0,26.0), rng.range(-16.0,16.0), rng.range(-16.0,16.0)];
        let p1 = [rng.range(-26.0,26.0), rng.range(-16.0,16.0), rng.range(-16.0,16.0)];
        let (f0, f1) = (f_minmax_e1(p0), f_minmax_e1(p1));
        if !((f0-delta) * (f1-delta) < 0.0) { continue; }
        let (mut a, mut b) = if f0 < delta { (p0, p1) } else { (p1, p0) };
        for _ in 0..60 {
            let m = scale(add(a,b), 0.5);
            if f_minmax_e1(m) < delta { a = m; } else { b = m; }
        }
        let iso = scale(add(a,b), 0.5);
        let dt = d_true_e1(iso);
        if dt < min_d { min_d = dt; }
        if dt > max_d { max_d = dt; worst = iso; }
        n += 1;
    }
    println!("  f=2-Punkte: wahre Distanz ∈ [{:.4}, {:.4}] mm — Offset-Flächenfehler bis {:.1}%, worst p=({:.2},{:.2},{:.2})",
        min_d, max_d, (max_d/delta - 1.0)*100.0, worst[0], worst[1], worst[2]);
}

// ---------- E5: Tiefe-2-Differenz (zweiter Notch) — Fehler über Verschachtelung ----------
fn e5() {
    println!("== E5: (A−B)−B2 (zweiter Notch oben links) — Schranken-Güte bei Tiefe 2 ==");
    // B2: x∈[−15,−5], y∈[5,15], z∈[−12.5,12.5]
    let f2 = |p: V3| -> f64 {
        let db2 = sd_box(p, [-10.0,10.0,0.0], [5.0,5.0,12.5]);
        f_minmax_e1(p).max(-db2)
    };
    // Ground truth: 10-Eck-Prisma
    let poly2: [[f64;2];10] = [[-20.0,-10.0],[20.0,-10.0],[20.0,0.0],[0.0,0.0],[0.0,10.0],
                               [-5.0,10.0],[-5.0,5.0],[-15.0,5.0],[-15.0,10.0],[-20.0,10.0]];
    let d_true2 = |p: V3| -> f64 { sd_extrude(p, &poly2, 10.0) };
    let mut rng = Rng(0xBADC0FFEE0DDF00D);
    let mut max_ratio = 1.0f64; let mut max_abs = 0.0f64; let mut sum = 0.0; let mut n = 0usize;
    let mut worst = [0.0;3];
    while n < 200_000 {
        let p = [rng.range(-26.0,26.0), rng.range(-16.0,16.0), rng.range(-16.0,16.0)];
        let dt = d_true2(p);
        if dt.abs() > 5.0 { continue; }
        let f = f2(p);
        let err = (f - dt).abs();
        sum += err; n += 1;
        if err > max_abs { max_abs = err; worst = p; }
        if f.abs() > 1e-9 { max_ratio = max_ratio.max(dt.abs()/f.abs()); }
    }
    println!("  Nahfeld |d|<5: max|f−d|={:.4} mm  mean={:.4} mm  max |d|/|f| = {:.4}  worst p=({:.2},{:.2},{:.2})",
        max_abs, sum/(n as f64), max_ratio, worst[0], worst[1], worst[2]);
}

// ---------- E1: min/max-Feld vs. Ground Truth an der konkaven Naht ----------
fn e1() {
    println!("== E1: Box−Box (L-Prisma): f_minmax vs. d_true; d_mesh-Validierung ==");
    let mesh = l_prism_mesh();
    let bvh = Bvh::build(&mesh);
    let mut rng = Rng(0x9E3779B97F4A7C15);

    // (a) Validierung: |d_mesh| ≡ |d_true| (planar ⇒ maschinengenau)
    let mut max_val_err = 0.0f64;
    for _ in 0..200_000 {
        let p = [rng.range(-25.0,25.0), rng.range(-15.0,15.0), rng.range(-15.0,15.0)];
        let dm = bvh.dist(&mesh, p);
        let dt = d_true_e1(p).abs();
        max_val_err = max_val_err.max((dm-dt).abs());
    }
    println!("  d_mesh-Validierung: max |d_mesh − |d_true||   = {:.3e} mm (200k Zufallspunkte)", max_val_err);

    // (b) Nahfeld-Fehler des Schranken-Felds, global + Notch-Region
    let mut stats = |region: &str, filter: &dyn Fn(V3,f64)->bool| {
        let mut n = 0usize; let mut max_abs = 0.0f64; let mut sum_abs = 0.0f64;
        let mut max_ratio = 1.0f64; let mut worst = [0.0;3];
        let mut tries = 0usize;
        let mut rng = Rng(0xDEADBEEFCAFE1234);
        while n < 100_000 && tries < 20_000_000 {
            tries += 1;
            let p = [rng.range(-25.0,25.0), rng.range(-15.0,15.0), rng.range(-15.0,15.0)];
            let dt = d_true_e1(p);
            if !filter(p, dt) { continue; }
            let f = f_minmax_e1(p);
            let err = (f - dt).abs();
            sum_abs += err; n += 1;
            if err > max_abs { max_abs = err; worst = p; }
            if f.abs() > 1e-9 { max_ratio = max_ratio.max(dt.abs()/f.abs()); }
        }
        println!("  [{}] n={}  max|f−d|={:.4} mm  mean|f−d|={:.4} mm  max d/f={:.4}  worst p=({:.2},{:.2},{:.2})",
            region, n, max_abs, sum_abs/(n as f64), max_ratio, worst[0], worst[1], worst[2]);
    };
    stats("Nahfeld |d|<5, überall     ", &|_p, dt| dt.abs() < 5.0);
    stats("Notch außen (x,y>0, |d|<5) ", &|p, dt| dt > 0.0 && dt < 5.0 && p[0] > 0.0 && p[1] > 0.0);
    stats("innen nahe konkaver Kante  ", &|p, dt| dt < 0.0 && dt > -5.0 && p[0] < 0.0 && p[1] < 0.0 && p[0] > -8.0 && p[1] > -8.0);
}

// ---------- E1b: Offset-Konsequenz — wahre Distanz der Isofläche f=δ ----------
fn e1b() {
    println!("== E1b: Isofläche f_minmax=δ (δ=2mm): wahre Distanz dieser Punkte ==");
    let delta = 2.0;
    let mut rng = Rng(0x1234567890ABCDEF);
    let mut min_d = f64::MAX; let mut max_d = f64::MIN;
    let mut n = 0;
    // Isoflächen-Punkte per Bisektion entlang zufälliger Radialstrahlen in der Notch-Region
    while n < 20_000 {
        let x = rng.range(0.1, 15.0); let y = rng.range(0.1, 8.0); let z = rng.range(-9.0, 9.0);
        let p0 = [x, y, z];
        if f_minmax_e1(p0) <= delta { continue; } // außen genug starten
        // Bisektion Richtung konkave Kante (0,0,z)
        let p1 = [1e-4, 1e-4, z];
        if f_minmax_e1(p1) >= delta { continue; }
        let (mut a, mut b) = (p1, p0); // f(a)<δ, f(b)>δ
        for _ in 0..60 {
            let m = scale(add(a,b), 0.5);
            if f_minmax_e1(m) < delta { a = m; } else { b = m; }
        }
        let iso = scale(add(a,b), 0.5);
        let dt = d_true_e1(iso);
        min_d = min_d.min(dt); max_d = max_d.max(dt);
        n += 1;
    }
    println!("  Punkte mit f=δ=2: wahre Distanz ∈ [{:.4}, {:.4}] mm (Soll für echten Offset: exakt 2.0; √2·δ = {:.4})",
        min_d, max_d, delta*std::f64::consts::SQRT_2);
}

// ---------- E2: Tesselationsfehler über Segmentzahl (Zylinder) ----------
fn e2() {
    println!("== E2: Zylinder r=10, hz=10 — d_mesh-Fehler vs. Segmentzahl (Sehnenfehler-Skalierung) ==");
    println!("  {:>6} {:>10} {:>14} {:>14} {:>14}", "n_seg", "tris", "max|err| mm", "mean|err| mm", "sag-Prognose");
    for &n_seg in &[16usize, 32, 64, 128, 256, 512] {
        let mesh = cylinder_mesh(10.0, 10.0, n_seg, 8);
        let bvh = Bvh::build(&mesh);
        let mut rng = Rng(0xABCDEF0123456789);
        let mut max_err = 0.0f64; let mut sum = 0.0f64; let mut n = 0usize;
        while n < 50_000 {
            let p = [rng.range(-14.0,14.0), rng.range(-14.0,14.0), rng.range(-13.0,13.0)];
            let dt = sd_cyl(p, 10.0, 10.0);
            if dt.abs() > 4.0 { continue; } // Nahfeld
            let dm = bvh.dist(&mesh, p);
            let err = (dm - dt.abs()).abs();
            sum += err; n += 1;
            if err > max_err { max_err = err; }
        }
        let sag = 10.0*(1.0 - (std::f64::consts::PI/(n_seg as f64)).cos());
        println!("  {:>6} {:>10} {:>14.5} {:>14.5} {:>14.5}", n_seg, mesh.t.len(), max_err, sum/(n as f64), sag);
    }
}

// ---------- E3: BVH-Kosten ----------
fn e3() {
    println!("== E3: BVH Build- und Query-Kosten (1 Thread, f64, Median-Split, Leaf=4) ==");
    println!("  {:>10} {:>12} {:>14} {:>16}", "tris", "build ms", "queries/s", "ns/query");
    for &(n_seg, n_ring) in &[(64usize, 16usize), (256, 40), (1024, 100), (2048, 250)] {
        let mesh = cylinder_mesh(10.0, 10.0, n_seg, n_ring);
        let t0 = Instant::now();
        let bvh = Bvh::build(&mesh);
        let build_ms = t0.elapsed().as_secs_f64()*1e3;
        let mut rng = Rng(0x5DEECE66D);
        let pts: Vec<V3> = (0..200_000).map(|_| [rng.range(-14.0,14.0), rng.range(-14.0,14.0), rng.range(-13.0,13.0)]).collect();
        let t1 = Instant::now();
        let mut acc = 0.0;
        for &p in &pts { acc += bvh.dist(&mesh, p); }
        let dt = t1.elapsed().as_secs_f64();
        let qps = pts.len() as f64 / dt;
        println!("  {:>10} {:>12.1} {:>14.0} {:>16.0}   (checksum {:.1})", mesh.t.len(), build_ms, qps, 1e9/qps, acc);
    }
}

// ---------- E4: Blend Nahfeld↔Fernfeld ----------
fn e4() {
    println!("== E4: Hybrid-Blend h = mix(d_mesh_signed, f_minmax, w(|f|)) — Sprung & Gradienten-Spike ==");
    let mesh = l_prism_mesh();
    let bvh = Bvh::build(&mesh);
    // Vorzeichen aus dem Feld (bei harten Booleans exakt), Betrag aus dem Mesh.
    let d_near = |p: V3| -> f64 {
        let s = if f_minmax_e1(p) < 0.0 { -1.0 } else { 1.0 };
        s * bvh.dist(&mesh, p)
    };
    let smoothstep = |e0: f64, e1: f64, x: f64| {
        let t = ((x-e0)/(e1-e0)).clamp(0.0,1.0);
        t*t*(3.0-2.0*t)
    };
    // Blendband [τ_in, τ_out] auf |f|
    for &(tau_in, tau_out) in &[(2.0f64, 3.0f64), (2.0, 4.0), (2.0, 6.0), (2.0, 10.0)] {
        let h = |p: V3| -> f64 {
            let f = f_minmax_e1(p);
            let w = smoothstep(tau_in, tau_out, f.abs());
            if w <= 0.0 { d_near(p) } else if w >= 1.0 { f } else { (1.0-w)*d_near(p) + w*f }
        };
        // Max-Gradientennorm (zentrale Differenzen) auf Zufallspunkten im Band + Notch
        let mut rng = Rng(0xFEEDFACE12345678);
        let mut max_grad = 0.0f64; let mut max_over = 0.0f64; // Überschätzung ggü. d_true (Marsch-Sicherheit)
        let eps = 1e-4;
        for _ in 0..100_000 {
            let p = [rng.range(-24.0,24.0), rng.range(-14.0,14.0), rng.range(-14.0,14.0)];
            let f = f_minmax_e1(p);
            if f.abs() < tau_in*0.5 || f.abs() > tau_out*1.5 { continue; }
            let mut g = [0.0;3];
            for k in 0..3 {
                let mut pp = p; pp[k] += eps;
                let mut pm = p; pm[k] -= eps;
                g[k] = (h(pp)-h(pm))/(2.0*eps);
            }
            max_grad = max_grad.max(len(g));
            let dt = d_true_e1(p);
            if dt > 0.0 { max_over = max_over.max(h(p) - dt); }
        }
        println!("  Band [{:>4.1},{:>4.1}]: max|∇h| = {:.3}   max Überschätzung h−d_true = {:.4} mm",
            tau_in, tau_out, max_grad, max_over);
    }
}
