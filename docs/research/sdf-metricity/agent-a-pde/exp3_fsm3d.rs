//! E3: Kosten/Genauigkeit von 3D-Fast-Sweeping-Redistancing auf einem
//! voxel.rs-artigen Gitter (200^3, f32), Feld = Union zweier Kugeln r=1 um
//! (+-0.55, 0, 0) via min (hard_combine-Union). Ground Truth analytisch
//! (Rotationssymmetrie -> 2D-Arc/Corner-Distanz).
//!
//! Misst: Zeit fuer (a) Feld-Sampling (Analog voxelizeOccupancy, aber f32),
//! (b) Band-Init, (c) Gauss-Seidel-Sweeps bis Konvergenz; Fehler im
//! Innen-/Aussenband vor/nach Redistancing.
//!
//! Build: rustc -O exp3_fsm3d.rs -o /tmp/exp3 && /tmp/exp3

use std::time::Instant;

const N: usize = 200;
const DOM: f32 = 2.4;
const R: f32 = 1.0;
const CX: f32 = 0.55;
const BIG: f32 = 1e10;

#[inline]
fn idx(i: usize, j: usize, k: usize) -> usize { (k * N + j) * N + i }

#[inline]
fn field(x: f32, y: f32, z: f32) -> f32 {
    let d1 = ((x + CX) * (x + CX) + y * y + z * z).sqrt() - R;
    let d2 = ((x - CX) * (x - CX) + y * y + z * z).sqrt() - R;
    d1.min(d2)
}

/// Wahre signierte Distanz zur Union-Randflaeche (rotationssymmetrisch um x).
fn d_true(x: f32, y: f32, z: f32) -> f32 {
    let rr = (y * y + z * z).sqrt();
    let b = (1.0f32 - CX * CX).sqrt(); // Nahtkreis bei x=0, r=b
    let mut best = f32::INFINITY;
    for c in [-CX, CX] {
        let ux = x - c;
        let l = (ux * ux + rr * rr).sqrt().max(1e-30);
        // Closest Point auf der Vollkugel, in (x, rr)-Ebene:
        let qx = c + R * ux / l;
        let qr = R * rr / l;
        let oc = if c < 0.0 { CX } else { -CX };
        let qo = ((qx - oc) * (qx - oc) + qr * qr).sqrt();
        let retained = qo >= R;
        if retained {
            best = best.min((l - R).abs());
        } else {
            // Distanz zum Nahtkreis (x=0, radius b) im 3D:
            let dx = x;
            let dr = rr - b;
            best = best.min((dx * dx + dr * dr).sqrt());
        }
    }
    if field(x, y, z) < 0.0 { -best } else { best }
}

#[inline]
fn coord(i: usize) -> f32 { -DOM + (i as f32) * (2.0 * DOM / (N as f32 - 1.0)) }

fn godunov3(a: f32, b: f32, c: f32, h: f32) -> f32 {
    // sortiere
    let (mut a1, mut a2, mut a3) = (a, b, c);
    if a1 > a2 { std::mem::swap(&mut a1, &mut a2); }
    if a2 > a3 { std::mem::swap(&mut a2, &mut a3); }
    if a1 > a2 { std::mem::swap(&mut a1, &mut a2); }
    let mut t = a1 + h;
    if t > a2 {
        t = 0.5 * (a1 + a2 + (2.0 * h * h - (a1 - a2) * (a1 - a2)).max(0.0).sqrt());
        if t > a3 {
            let s = a1 + a2 + a3;
            let q = a1 * a1 + a2 * a2 + a3 * a3;
            let disc = (s * s - 3.0 * (q - h * h)).max(0.0);
            t = (s + disc.sqrt()) / 3.0;
        }
    }
    t
}

fn main() {
    let h = 2.0 * DOM / (N as f32 - 1.0);
    let total = N * N * N;

    // (a) Feld-Sampling
    let t0 = Instant::now();
    let mut f = vec![0f32; total];
    for k in 0..N { for j in 0..N { for i in 0..N {
        f[idx(i, j, k)] = field(coord(i), coord(j), coord(k));
    }}}
    let t_sample = t0.elapsed().as_secs_f64();

    // (b) Band-Init: Zellen mit Vorzeichenwechsel zu einem 6-Nachbarn
    let t0 = Instant::now();
    let mut u = vec![BIG; total];
    let mut band = vec![false; total];
    for k in 0..N { for j in 0..N { for i in 0..N {
        let v = f[idx(i, j, k)];
        let mut cut = false;
        if i + 1 < N && (v < 0.0) != (f[idx(i + 1, j, k)] < 0.0) { cut = true; }
        if i > 0 && (v < 0.0) != (f[idx(i - 1, j, k)] < 0.0) { cut = true; }
        if j + 1 < N && (v < 0.0) != (f[idx(i, j + 1, k)] < 0.0) { cut = true; }
        if j > 0 && (v < 0.0) != (f[idx(i, j - 1, k)] < 0.0) { cut = true; }
        if k + 1 < N && (v < 0.0) != (f[idx(i, j, k + 1)] < 0.0) { cut = true; }
        if k > 0 && (v < 0.0) != (f[idx(i, j, k - 1)] < 0.0) { cut = true; }
        if cut {
            band[idx(i, j, k)] = true;
            u[idx(i, j, k)] = v.abs();
        }
    }}}
    let t_band = t0.elapsed().as_secs_f64();

    // (c) Gauss-Seidel-Sweeps (8 Orderings) bis Konvergenz
    let t0 = Instant::now();
    let mut sweep_sets = 0usize;
    loop {
        sweep_sets += 1;
        let mut maxch = 0f32;
        for order in 0..8u8 {
            let (si, sj, sk) = (order & 1 != 0, order & 2 != 0, order & 4 != 0);
            for kk in 0..N { let k = if sk { N - 1 - kk } else { kk };
            for jj in 0..N { let j = if sj { N - 1 - jj } else { jj };
            for ii in 0..N { let i = if si { N - 1 - ii } else { ii };
                let id = idx(i, j, k);
                if band[id] { continue; }
                let ax = {
                    let l = if i > 0 { u[idx(i - 1, j, k)] } else { BIG };
                    let r = if i + 1 < N { u[idx(i + 1, j, k)] } else { BIG };
                    l.min(r)
                };
                let ay = {
                    let l = if j > 0 { u[idx(i, j - 1, k)] } else { BIG };
                    let r = if j + 1 < N { u[idx(i, j + 1, k)] } else { BIG };
                    l.min(r)
                };
                let az = {
                    let l = if k > 0 { u[idx(i, j, k - 1)] } else { BIG };
                    let r = if k + 1 < N { u[idx(i, j, k + 1)] } else { BIG };
                    l.min(r)
                };
                let t = godunov3(ax, ay, az, h);
                if t < u[id] {
                    maxch = maxch.max(u[id] - t);
                    u[id] = t;
                }
            }}}
        }
        eprintln!("sweep_set {} maxch={:.3e}", sweep_sets, maxch);
        if maxch < 1e-6 || sweep_sets >= 8 { break; }
    }
    let t_sweep = t0.elapsed().as_secs_f64();

    // Fehlerauswertung in Baendern
    let (mut raw_int_max, mut raw_ext_max) = (0f32, 0f32);
    let (mut fsm_int_max, mut fsm_ext_max) = (0f32, 0f32);
    let (mut fsm_int_sum, mut fsm_int_cnt) = (0f64, 0u64);
    for k in 0..N { for j in 0..N { for i in 0..N {
        let (x, y, z) = (coord(i), coord(j), coord(k));
        let dt = d_true(x, y, z);
        let id = idx(i, j, k);
        let g = if f[id] < 0.0 { -u[id] } else { u[id] };
        if dt > 0.0 && dt <= 0.5 {
            raw_ext_max = raw_ext_max.max((f[id] - dt).abs());
            fsm_ext_max = fsm_ext_max.max((g - dt).abs());
        } else if dt < 0.0 && dt >= -0.5 {
            raw_int_max = raw_int_max.max((f[id] - dt).abs());
            fsm_int_max = fsm_int_max.max((g - dt).abs());
            fsm_int_sum += (g - dt).abs() as f64;
            fsm_int_cnt += 1;
        }
    }}}

    println!("N={} h={:.5} cells={}", N, h, total);
    println!("t_sample={:.3}s t_band={:.3}s t_sweeps={:.3}s (sweep_sets={})",
             t_sample, t_band, t_sweep, sweep_sets);
    println!("raw:  ext_max={:.6} int_max={:.6}", raw_ext_max, raw_int_max);
    println!("fsm:  ext_max={:.6} int_max={:.6} int_mean={:.6}",
             fsm_ext_max, fsm_int_max, fsm_int_sum / fsm_int_cnt as f64);
}
