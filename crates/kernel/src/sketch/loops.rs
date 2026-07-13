//! Loop-Inferenz über dem gelösten Sketch: T-Junction-Splitting
//! (`split_line_tjunctions`) und Ketten-Schluss (`infer_closed_loops`).

use std::collections::HashMap;

use crate::sketch2d::{SketchSegment};
/// One chainable element for the loop-inference pass: the lowered
/// segments of a free `sketch_line` or open `sketch_spline`, plus the
/// index of the sub-call it came from.
#[derive(Clone)]
pub(crate) struct ChainElement {
    pub(crate) call_index: usize,
    pub(crate) segments: Vec<SketchSegment>,
    /// The solver curve entity id (line/arc/spline) this element lowered
    /// from — the stable provenance the wall-sub table is built on.
    /// `None` when the replay could not attribute one; a loop containing
    /// such an element gets no stable wall ids (positional fallback).
    pub(crate) source: Option<u32>,
}

/// Endpoint-coincidence tolerance for loop inference (mm). Snapped
/// geometry is exactly coincident (the solver equalises coordinates),
/// so this only needs to absorb f32 rounding — a visibly open contour
/// must NOT silently close.
pub(crate) const LOOP_JOIN_TOL: f32 = 1e-3;

/// K7.1 loop inference: chain free open elements (lines, open splines)
/// into closed contours by endpoint coincidence. Greedy walk per start
/// element; at a junction of ≥ 3 elements the first match wins (no
/// backtracking — V1). Returns the closed loops (keyed by the call
/// index of their first member, so the caller can splice each loop
/// into the accumulator at the position the user drew it) plus the
/// set of consumed call indices.
/// Split lone straight edges at T-junctions (ticket mdye): where another
/// element's endpoint lands on a `Line`'s INTERIOR — an overshooting edge whose
/// corner is only touched partway along — cut the line there so both halves
/// become endpoint-joinable elements. The overshoot half then peels off as a
/// dangling stub in the 2-core prune inside [`infer_closed_loops`], leaving the
/// real loop closed and extrudable — consistent with the sketch-view fill
/// (`sketchRegions.ts` planarizes the same way). Only single-line elements are
/// split: an overshoot is always a straight edge, so arcs/paths stay intact.
pub(crate) fn split_line_tjunctions(elems: &[ChainElement]) -> Vec<ChainElement> {
    let close = |a: [f32; 2], b: [f32; 2]| {
        (a[0] - b[0]).abs() <= LOOP_JOIN_TOL && (a[1] - b[1]).abs() <= LOOP_JOIN_TOL
    };
    let mut junctions: Vec<[f32; 2]> = Vec::new();
    for e in elems {
        if let (Some(f), Some(l)) = (e.segments.first(), e.segments.last()) {
            junctions.push(f.start_point());
            junctions.push(l.end_point());
        }
    }
    let mut out: Vec<ChainElement> = Vec::new();
    for e in elems {
        if let [SketchSegment::Line { start, end }] = e.segments.as_slice() {
            let (start, end) = (*start, *end);
            let d = [end[0] - start[0], end[1] - start[1]];
            let len2 = d[0] * d[0] + d[1] * d[1];
            if len2 > LOOP_JOIN_TOL * LOOP_JOIN_TOL {
                let len = len2.sqrt();
                let mut cuts: Vec<(f32, [f32; 2])> = Vec::new();
                for &j in &junctions {
                    let t = ((j[0] - start[0]) * d[0] + (j[1] - start[1]) * d[1]) / len2;
                    if t <= 0.0 || t >= 1.0 {
                        continue;
                    }
                    let proj = [start[0] + t * d[0], start[1] + t * d[1]];
                    if !close(j, proj) {
                        continue; // off the line
                    }
                    if t * len <= LOOP_JOIN_TOL || (1.0 - t) * len <= LOOP_JOIN_TOL {
                        continue; // effectively at an endpoint
                    }
                    cuts.push((t, proj));
                }
                if !cuts.is_empty() {
                    cuts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                    let mut prev = start;
                    for (_, c) in cuts {
                        if close(prev, c) {
                            continue;
                        }
                        out.push(ChainElement {
                            call_index: e.call_index,
                            segments: vec![SketchSegment::Line { start: prev, end: c }],
                            source: e.source,
                        });
                        prev = c;
                    }
                    if !close(prev, end) {
                        out.push(ChainElement {
                            call_index: e.call_index,
                            segments: vec![SketchSegment::Line { start: prev, end }],
                            source: e.source,
                        });
                    }
                    continue;
                }
            }
        }
        out.push(e.clone());
    }
    out
}

/// One inferred closed loop: the stitched segments plus, per segment, the
/// solver curve entity id it came from (`None` = unattributed).
pub(crate) type TracedLoop = (Vec<SketchSegment>, Vec<Option<u32>>);

pub(crate) fn infer_closed_loops(
    elems: &[ChainElement],
) -> (HashMap<usize, Vec<TracedLoop>>, std::collections::HashSet<usize>) {
    // Ticket mdye: split overshooting edges at T-junctions first, so a loop that
    // is only touched partway along another line still closes.
    let elems_owned = split_line_tjunctions(elems);
    let elems = &elems_owned[..];
    let close = |a: [f32; 2], b: [f32; 2]| {
        (a[0] - b[0]).abs() <= LOOP_JOIN_TOL && (a[1] - b[1]).abs() <= LOOP_JOIN_TOL
    };
    let mut loops: HashMap<usize, Vec<TracedLoop>> = HashMap::new();
    let mut consumed = std::collections::HashSet::new();
    let mut used = vec![false; elems.len()];

    // Ticket mdye: prune DANGLING stubs before loop-walking. An overhanging
    // line — a "flag pole" hanging off a closed rectangle's corner — leaves one
    // endpoint touched by no other element. Such an edge can never be part of a
    // closed loop, yet the greedy walk below could follow it off the branch
    // vertex into a dead end and then fail to close the real loop. Iteratively
    // retire every element that has a FREE endpoint (the 2-core): a whole chain
    // of stubs peels off end-first, leaving clean loops. A self-closed element
    // (start == end, e.g. a closed spline) is a loop on its own — never pruned.
    let endpoints: Vec<([f32; 2], [f32; 2])> = elems
        .iter()
        .map(|e| match (e.segments.first(), e.segments.last()) {
            (Some(f), Some(l)) => (f.start_point(), l.end_point()),
            _ => ([0.0, 0.0], [0.0, 0.0]),
        })
        .collect();
    loop {
        let mut changed = false;
        for i in 0..elems.len() {
            if used[i] || elems[i].segments.is_empty() {
                continue;
            }
            let (a, b) = endpoints[i];
            if close(a, b) {
                continue; // self-closed element — a loop on its own
            }
            let touches = |p: [f32; 2]| {
                (0..elems.len()).any(|j| {
                    j != i
                        && !used[j]
                        && !elems[j].segments.is_empty()
                        && (close(p, endpoints[j].0) || close(p, endpoints[j].1))
                })
            };
            if !touches(a) || !touches(b) {
                used[i] = true; // retire the stub; never a loop member
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for start in 0..elems.len() {
        if used[start] || elems[start].segments.is_empty() {
            continue;
        }
        let mut chain: Vec<SketchSegment> = elems[start].segments.clone();
        let mut sources: Vec<Option<u32>> = vec![elems[start].source; chain.len()];
        let mut members = vec![start];
        used[start] = true;
        let chain_start = chain.first().unwrap().start_point();
        let mut chain_end = chain.last().unwrap().end_point();
        while !close(chain_end, chain_start) {
            let mut found = false;
            for (j, elem) in elems.iter().enumerate() {
                if used[j] || elem.segments.is_empty() {
                    continue;
                }
                let s = elem.segments.first().unwrap().start_point();
                let e = elem.segments.last().unwrap().end_point();
                if close(chain_end, s) {
                    chain.extend(elem.segments.iter().cloned());
                    sources.extend(std::iter::repeat(elem.source).take(elem.segments.len()));
                    chain_end = e;
                } else if close(chain_end, e) {
                    chain.extend(elem.segments.iter().rev().map(|sg| sg.reversed()));
                    sources.extend(std::iter::repeat(elem.source).take(elem.segments.len()));
                    chain_end = s;
                } else {
                    continue;
                }
                used[j] = true;
                members.push(j);
                found = true;
                break;
            }
            if !found {
                break;
            }
        }
        if close(chain_end, chain_start) && chain.len() >= 2 {
            for &m in &members {
                consumed.insert(elems[m].call_index);
            }
            let anchor = members.iter().map(|&m| elems[m].call_index).min().unwrap();
            loops.entry(anchor).or_default().push((chain, sources));
        } else {
            // Open chain: release the members so they can join a loop
            // seeded from a different start element. `start` itself
            // stays used — re-seeding from it would just rebuild the
            // same open chain.
            for &m in &members[1..] {
                used[m] = false;
            }
        }
    }
    (loops, consumed)
}
