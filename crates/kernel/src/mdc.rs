//! Manifold Dual Contouring (MDC).
//!
//! Guarantees 2-manifold output by using Nielson's Dual Marching Cubes
//! approach: each cell may contain multiple contour components (cycles),
//! each receiving its own vertex placed via QEF minimization for sharp
//! feature preservation.
//!
//! Reference: Schaefer, Ju, Warren — "Manifold Dual Contouring" (TVCG).

use crate::bvh::BoundedCsg;
use crate::csg::{EvalMode, ProxyElement, MAX_CONTRIBUTORS};
use crate::dc::{sdf_normal, Mesh, CONTRIBUTOR_SENTINEL};
use std::collections::{HashMap, HashSet};

// ── MC vertex / edge conventions ────────────────────────────────────
//
//   MC vertex layout (Paul Bourke / Lorensen & Cline):
//
//       7-----------6
//      /|          /|
//     / |         / |
//    4-----------5  |
//    |  3--------|--2
//    | /         | /
//    |/          |/
//    0-----------1
//
//   Vertex positions:
//     0=(0,0,0)  1=(1,0,0)  2=(1,1,0)  3=(0,1,0)
//     4=(0,0,1)  5=(1,0,1)  6=(1,1,1)  7=(0,1,1)

const MC_CORNER_POS: [[f32; 3]; 8] = [
    [0.0, 0.0, 0.0], // 0
    [1.0, 0.0, 0.0], // 1
    [1.0, 1.0, 0.0], // 2
    [0.0, 1.0, 0.0], // 3
    [0.0, 0.0, 1.0], // 4
    [1.0, 0.0, 1.0], // 5
    [1.0, 1.0, 1.0], // 6
    [0.0, 1.0, 1.0], // 7
];

const MC_EDGE_VERTS: [[usize; 2]; 12] = [
    [0, 1], [1, 2], [2, 3], [3, 0], // bottom ring
    [4, 5], [5, 6], [6, 7], [7, 4], // top ring
    [0, 4], [1, 5], [2, 6], [3, 7], // vertical pillars
];

/// Maps each MC edge of a cell to the grid edge it corresponds to.
/// (axis, di, dj, dk) — grid edge at (ci+di, cj+dj, ck+dk) along axis.
#[allow(dead_code)]
const MC_EDGE_TO_GRID: [(usize, usize, usize, usize); 12] = [
    (0, 0, 0, 0), // MC 0:  X-edge at (ci,   cj,   ck)
    (1, 1, 0, 0), // MC 1:  Y-edge at (ci+1, cj,   ck)
    (0, 0, 1, 0), // MC 2:  X-edge at (ci,   cj+1, ck)
    (1, 0, 0, 0), // MC 3:  Y-edge at (ci,   cj,   ck)
    (0, 0, 0, 1), // MC 4:  X-edge at (ci,   cj,   ck+1)
    (1, 1, 0, 1), // MC 5:  Y-edge at (ci+1, cj,   ck+1)
    (0, 0, 1, 1), // MC 6:  X-edge at (ci,   cj+1, ck+1)
    (1, 0, 0, 1), // MC 7:  Y-edge at (ci,   cj,   ck+1)
    (2, 0, 0, 0), // MC 8:  Z-edge at (ci,   cj,   ck)
    (2, 1, 0, 0), // MC 9:  Z-edge at (ci+1, cj,   ck)
    (2, 1, 1, 0), // MC 10: Z-edge at (ci+1, cj+1, ck)
    (2, 0, 1, 0), // MC 11: Z-edge at (ci,   cj+1, ck)
];

/// For each grid-edge axis, the 4 (cell-offset, mc-edge) pairs that
/// share that edge, in quad winding order (v00, v10, v11, v01).
///
/// For axis A grid-edge at (ei, ej, ek), cell is at
/// (ei + off[0], ej + off[1], ek + off[2]).
const QUAD_CELLS: [[(i32, i32, i32, usize); 4]; 3] = [
    // X-edges: 4 cells in the YZ plane
    [
        (0, -1, -1, 6), // v00
        (0,  0, -1, 4), // v10
        (0,  0,  0, 0), // v11
        (0, -1,  0, 2), // v01
    ],
    // Y-edges: 4 cells in the XZ plane
    [
        (-1, 0, -1, 5), // v00
        ( 0, 0, -1, 7), // v10
        ( 0, 0,  0, 3), // v11
        (-1, 0,  0, 1), // v01
    ],
    // Z-edges: 4 cells in the XY plane
    [
        (-1, -1, 0, 10), // v00
        ( 0, -1, 0, 11), // v10
        ( 0,  0, 0,  8), // v11
        (-1,  0, 0,  9), // v01
    ],
];

// ── Marching Cubes lookup tables ────────────────────────────────────

/// Bitmask of intersected edges for each of 256 MC cases.
#[rustfmt::skip]
const EDGE_TABLE: [u16; 256] = [
    0x000, 0x109, 0x203, 0x30a, 0x406, 0x50f, 0x605, 0x70c,
    0x80c, 0x905, 0xa0f, 0xb06, 0xc0a, 0xd03, 0xe09, 0xf00,
    0x190, 0x099, 0x393, 0x29a, 0x596, 0x49f, 0x795, 0x69c,
    0x99c, 0x895, 0xb9f, 0xa96, 0xd9a, 0xc93, 0xf99, 0xe90,
    0x230, 0x339, 0x033, 0x13a, 0x636, 0x73f, 0x435, 0x53c,
    0xa3c, 0xb35, 0x83f, 0x936, 0xe3a, 0xf33, 0xc39, 0xd30,
    0x3a0, 0x2a9, 0x1a3, 0x0aa, 0x7a6, 0x6af, 0x5a5, 0x4ac,
    0xbac, 0xaa5, 0x9af, 0x8a6, 0xfaa, 0xea3, 0xda9, 0xca0,
    0x460, 0x569, 0x663, 0x76a, 0x066, 0x16f, 0x265, 0x36c,
    0xc6c, 0xd65, 0xe6f, 0xf66, 0x86a, 0x963, 0xa69, 0xb60,
    0x5f0, 0x4f9, 0x7f3, 0x6fa, 0x1f6, 0x0ff, 0x3f5, 0x2fc,
    0xdfc, 0xcf5, 0xfff, 0xef6, 0x9fa, 0x8f3, 0xbf9, 0xaf0,
    0x650, 0x759, 0x453, 0x55a, 0x256, 0x35f, 0x055, 0x15c,
    0xe5c, 0xf55, 0xc5f, 0xd56, 0xa5a, 0xb53, 0x859, 0x950,
    0x7c0, 0x6c9, 0x5c3, 0x4ca, 0x3c6, 0x2cf, 0x1c5, 0x0cc,
    0xfcc, 0xec5, 0xdcf, 0xcc6, 0xbca, 0xac3, 0x9c9, 0x8c0,
    0x8c0, 0x9c9, 0xac3, 0xbca, 0xcc6, 0xdcf, 0xec5, 0xfcc,
    0x0cc, 0x1c5, 0x2cf, 0x3c6, 0x4ca, 0x5c3, 0x6c9, 0x7c0,
    0x950, 0x859, 0xb53, 0xa5a, 0xd56, 0xc5f, 0xf55, 0xe5c,
    0x15c, 0x055, 0x35f, 0x256, 0x55a, 0x453, 0x759, 0x650,
    0xaf0, 0xbf9, 0x8f3, 0x9fa, 0xef6, 0xfff, 0xcf5, 0xdfc,
    0x2fc, 0x3f5, 0x0ff, 0x1f6, 0x6fa, 0x7f3, 0x4f9, 0x5f0,
    0xb60, 0xa69, 0x963, 0x86a, 0xf66, 0xe6f, 0xd65, 0xc6c,
    0x36c, 0x265, 0x16f, 0x066, 0x76a, 0x663, 0x569, 0x460,
    0xca0, 0xda9, 0xea3, 0xfaa, 0x8a6, 0x9af, 0xaa5, 0xbac,
    0x4ac, 0x5a5, 0x6af, 0x7a6, 0x0aa, 0x1a3, 0x2a9, 0x3a0,
    0xd30, 0xc39, 0xf33, 0xe3a, 0x936, 0x83f, 0xb35, 0xa3c,
    0x53c, 0x435, 0x73f, 0x636, 0x13a, 0x033, 0x339, 0x230,
    0xe90, 0xf99, 0xc93, 0xd9a, 0xa96, 0xb9f, 0x895, 0x99c,
    0x69c, 0x795, 0x49f, 0x596, 0x29a, 0x393, 0x099, 0x190,
    0xf00, 0xe09, 0xd03, 0xc0a, 0xb06, 0xa0f, 0x905, 0x80c,
    0x70c, 0x605, 0x50f, 0x406, 0x30a, 0x203, 0x109, 0x000,
];

/// Triangle table: up to 5 triangles (15 edge indices) per case, -1 terminated.
#[rustfmt::skip]
const TRI_TABLE: [[i8; 16]; 256] = [
    [-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 8, 3,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 1, 9,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 8, 3, 9, 8, 1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 2,10,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 8, 3, 1, 2,10,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 2,10, 0, 2, 9,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 2, 8, 3, 2,10, 8,10, 9, 8,-1,-1,-1,-1,-1,-1,-1],
    [ 3,11, 2,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 0,11, 2, 8,11, 0,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 9, 0, 2, 3,11,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 1,11, 2, 1, 9,11, 9, 8,11,-1,-1,-1,-1,-1,-1,-1],
    [ 3,10, 1,11,10, 3,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 0,10, 1, 0, 8,10, 8,11,10,-1,-1,-1,-1,-1,-1,-1],
    [ 3, 9, 0, 3,11, 9,11,10, 9,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 8,10,10, 8,11,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 4, 7, 8,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 4, 3, 0, 7, 3, 4,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 1, 9, 8, 4, 7,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 4, 1, 9, 4, 7, 1, 7, 3, 1,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 2,10, 8, 4, 7,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 3, 4, 7, 3, 0, 4, 1, 2,10,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 2,10, 9, 0, 2, 8, 4, 7,-1,-1,-1,-1,-1,-1,-1],
    [ 2,10, 9, 2, 9, 7, 2, 7, 3, 7, 9, 4,-1,-1,-1,-1],
    [ 8, 4, 7, 3,11, 2,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [11, 4, 7,11, 2, 4, 2, 0, 4,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 0, 1, 8, 4, 7, 2, 3,11,-1,-1,-1,-1,-1,-1,-1],
    [ 4, 7,11, 9, 4,11, 9,11, 2, 9, 2, 1,-1,-1,-1,-1],
    [ 3,10, 1, 3,11,10, 7, 8, 4,-1,-1,-1,-1,-1,-1,-1],
    [ 1,11,10, 1, 4,11, 1, 0, 4, 7,11, 4,-1,-1,-1,-1],
    [ 4, 7, 8, 9, 0,11, 9,11,10,11, 0, 3,-1,-1,-1,-1],
    [ 4, 7,11, 4,11, 9, 9,11,10,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 5, 4,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 5, 4, 0, 8, 3,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 5, 4, 1, 5, 0,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 8, 5, 4, 8, 3, 5, 3, 1, 5,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 2,10, 9, 5, 4,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 3, 0, 8, 1, 2,10, 4, 9, 5,-1,-1,-1,-1,-1,-1,-1],
    [ 5, 2,10, 5, 4, 2, 4, 0, 2,-1,-1,-1,-1,-1,-1,-1],
    [ 2,10, 5, 3, 2, 5, 3, 5, 4, 3, 4, 8,-1,-1,-1,-1],
    [ 9, 5, 4, 2, 3,11,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 0,11, 2, 0, 8,11, 4, 9, 5,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 5, 4, 0, 1, 5, 2, 3,11,-1,-1,-1,-1,-1,-1,-1],
    [ 2, 1, 5, 2, 5, 8, 2, 8,11, 4, 8, 5,-1,-1,-1,-1],
    [10, 3,11,10, 1, 3, 9, 5, 4,-1,-1,-1,-1,-1,-1,-1],
    [ 4, 9, 5, 0, 8, 1, 8,10, 1, 8,11,10,-1,-1,-1,-1],
    [ 5, 4, 0, 5, 0,11, 5,11,10,11, 0, 3,-1,-1,-1,-1],
    [ 5, 4, 8, 5, 8,10,10, 8,11,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 7, 8, 5, 7, 9,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 3, 0, 9, 5, 3, 5, 7, 3,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 7, 8, 0, 1, 7, 1, 5, 7,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 5, 3, 3, 5, 7,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 7, 8, 9, 5, 7,10, 1, 2,-1,-1,-1,-1,-1,-1,-1],
    [10, 1, 2, 9, 5, 0, 5, 3, 0, 5, 7, 3,-1,-1,-1,-1],
    [ 8, 0, 2, 8, 2, 5, 8, 5, 7,10, 5, 2,-1,-1,-1,-1],
    [ 2,10, 5, 2, 5, 3, 3, 5, 7,-1,-1,-1,-1,-1,-1,-1],
    [ 7, 9, 5, 7, 8, 9, 3,11, 2,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 5, 7, 9, 7, 2, 9, 2, 0, 2, 7,11,-1,-1,-1,-1],
    [ 2, 3,11, 0, 1, 8, 1, 7, 8, 1, 5, 7,-1,-1,-1,-1],
    [11, 2, 1,11, 1, 7, 7, 1, 5,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 5, 8, 8, 5, 7,10, 1, 3,10, 3,11,-1,-1,-1,-1],
    [ 5, 7, 0, 5, 0, 9, 7,11, 0, 1, 0,10,11,10, 0,-1],
    [11,10, 0,11, 0, 3,10, 5, 0, 8, 0, 7, 5, 7, 0,-1],
    [11,10, 5, 7,11, 5,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [10, 6, 5,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 8, 3, 5,10, 6,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 0, 1, 5,10, 6,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 8, 3, 1, 9, 8, 5,10, 6,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 6, 5, 2, 6, 1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 6, 5, 1, 2, 6, 3, 0, 8,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 6, 5, 9, 0, 6, 0, 2, 6,-1,-1,-1,-1,-1,-1,-1],
    [ 5, 9, 8, 5, 8, 2, 5, 2, 6, 3, 2, 8,-1,-1,-1,-1],
    [ 2, 3,11,10, 6, 5,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [11, 0, 8,11, 2, 0,10, 6, 5,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 1, 9, 2, 3,11, 5,10, 6,-1,-1,-1,-1,-1,-1,-1],
    [ 5,10, 6, 1, 9, 2, 9,11, 2, 9, 8,11,-1,-1,-1,-1],
    [ 6, 3,11, 6, 5, 3, 5, 1, 3,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 8,11, 0,11, 5, 0, 5, 1, 5,11, 6,-1,-1,-1,-1],
    [ 3,11, 6, 0, 3, 6, 0, 6, 5, 0, 5, 9,-1,-1,-1,-1],
    [ 6, 5, 9, 6, 9,11,11, 9, 8,-1,-1,-1,-1,-1,-1,-1],
    [ 5,10, 6, 4, 7, 8,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 4, 3, 0, 4, 7, 3, 6, 5,10,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 9, 0, 5,10, 6, 8, 4, 7,-1,-1,-1,-1,-1,-1,-1],
    [10, 6, 5, 1, 9, 7, 1, 7, 3, 7, 9, 4,-1,-1,-1,-1],
    [ 6, 1, 2, 6, 5, 1, 4, 7, 8,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 2, 5, 5, 2, 6, 3, 0, 4, 3, 4, 7,-1,-1,-1,-1],
    [ 8, 4, 7, 9, 0, 5, 0, 6, 5, 0, 2, 6,-1,-1,-1,-1],
    [ 7, 3, 9, 7, 9, 4, 3, 2, 9, 5, 9, 6, 2, 6, 9,-1],
    [ 3,11, 2, 7, 8, 4,10, 6, 5,-1,-1,-1,-1,-1,-1,-1],
    [ 5,10, 6, 4, 7, 2, 4, 2, 0, 2, 7,11,-1,-1,-1,-1],
    [ 0, 1, 9, 4, 7, 8, 2, 3,11, 5,10, 6,-1,-1,-1,-1],
    [ 9, 2, 1, 9,11, 2, 9, 4,11, 7,11, 4, 5,10, 6,-1],
    [ 8, 4, 7, 3,11, 5, 3, 5, 1, 5,11, 6,-1,-1,-1,-1],
    [ 5, 1,11, 5,11, 6, 1, 0,11, 7,11, 4, 0, 4,11,-1],
    [ 0, 5, 9, 0, 6, 5, 0, 3, 6,11, 6, 3, 8, 4, 7,-1],
    [ 6, 5, 9, 6, 9,11, 4, 7, 9, 7,11, 9,-1,-1,-1,-1],
    [10, 4, 9, 6, 4,10,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 4,10, 6, 4, 9,10, 0, 8, 3,-1,-1,-1,-1,-1,-1,-1],
    [10, 0, 1,10, 6, 0, 6, 4, 0,-1,-1,-1,-1,-1,-1,-1],
    [ 8, 3, 1, 8, 1, 6, 8, 6, 4, 6, 1,10,-1,-1,-1,-1],
    [ 1, 4, 9, 1, 2, 4, 2, 6, 4,-1,-1,-1,-1,-1,-1,-1],
    [ 3, 0, 8, 1, 2, 9, 2, 4, 9, 2, 6, 4,-1,-1,-1,-1],
    [ 0, 2, 4, 4, 2, 6,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 8, 3, 2, 8, 2, 4, 4, 2, 6,-1,-1,-1,-1,-1,-1,-1],
    [10, 4, 9,10, 6, 4,11, 2, 3,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 8, 2, 2, 8,11, 4, 9,10, 4,10, 6,-1,-1,-1,-1],
    [ 3,11, 2, 0, 1, 6, 0, 6, 4, 6, 1,10,-1,-1,-1,-1],
    [ 6, 4, 1, 6, 1,10, 4, 8, 1, 2, 1,11, 8,11, 1,-1],
    [ 9, 6, 4, 9, 3, 6, 9, 1, 3,11, 6, 3,-1,-1,-1,-1],
    [ 8,11, 1, 8, 1, 0,11, 6, 1, 9, 1, 4, 6, 4, 1,-1],
    [ 3,11, 6, 3, 6, 0, 0, 6, 4,-1,-1,-1,-1,-1,-1,-1],
    [ 6, 4, 8,11, 6, 8,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 7,10, 6, 7, 8,10, 8, 9,10,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 7, 3, 0,10, 7, 0, 9,10, 6, 7,10,-1,-1,-1,-1],
    [10, 6, 7, 1,10, 7, 1, 7, 8, 1, 8, 0,-1,-1,-1,-1],
    [10, 6, 7,10, 7, 1, 1, 7, 3,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 2, 6, 1, 6, 8, 1, 8, 9, 8, 6, 7,-1,-1,-1,-1],
    [ 2, 6, 9, 2, 9, 1, 6, 7, 9, 0, 9, 3, 7, 3, 9,-1],
    [ 7, 8, 0, 7, 0, 6, 6, 0, 2,-1,-1,-1,-1,-1,-1,-1],
    [ 7, 3, 2, 6, 7, 2,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 2, 3,11,10, 6, 8,10, 8, 9, 8, 6, 7,-1,-1,-1,-1],
    [ 2, 0, 7, 2, 7,11, 0, 9, 7, 6, 7,10, 9,10, 7,-1],
    [ 1, 8, 0, 1, 7, 8, 1,10, 7, 6, 7,10, 2, 3,11,-1],
    [11, 2, 1,11, 1, 7,10, 6, 1, 6, 7, 1,-1,-1,-1,-1],
    [ 8, 9, 6, 8, 6, 7, 9, 1, 6,11, 6, 3, 1, 3, 6,-1],
    [ 0, 9, 1,11, 6, 7,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 7, 8, 0, 7, 0, 6, 3,11, 0,11, 6, 0,-1,-1,-1,-1],
    [ 7,11, 6,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 7, 6,11,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 3, 0, 8,11, 7, 6,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 1, 9,11, 7, 6,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 8, 1, 9, 8, 3, 1,11, 7, 6,-1,-1,-1,-1,-1,-1,-1],
    [10, 1, 2, 6,11, 7,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 2,10, 3, 0, 8, 6,11, 7,-1,-1,-1,-1,-1,-1,-1],
    [ 2, 9, 0, 2,10, 9, 6,11, 7,-1,-1,-1,-1,-1,-1,-1],
    [ 6,11, 7, 2,10, 3,10, 8, 3,10, 9, 8,-1,-1,-1,-1],
    [ 7, 2, 3, 6, 2, 7,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 7, 0, 8, 7, 6, 0, 6, 2, 0,-1,-1,-1,-1,-1,-1,-1],
    [ 2, 7, 6, 2, 3, 7, 0, 1, 9,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 6, 2, 1, 8, 6, 1, 9, 8, 8, 7, 6,-1,-1,-1,-1],
    [10, 7, 6,10, 1, 7, 1, 3, 7,-1,-1,-1,-1,-1,-1,-1],
    [10, 7, 6, 1, 7,10, 1, 8, 7, 1, 0, 8,-1,-1,-1,-1],
    [ 0, 3, 7, 0, 7,10, 0,10, 9, 6,10, 7,-1,-1,-1,-1],
    [ 7, 6,10, 7,10, 8, 8,10, 9,-1,-1,-1,-1,-1,-1,-1],
    [ 6, 8, 4,11, 8, 6,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 3, 6,11, 3, 0, 6, 0, 4, 6,-1,-1,-1,-1,-1,-1,-1],
    [ 8, 6,11, 8, 4, 6, 9, 0, 1,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 4, 6, 9, 6, 3, 9, 3, 1,11, 3, 6,-1,-1,-1,-1],
    [ 6, 8, 4, 6,11, 8, 2,10, 1,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 2,10, 3, 0,11, 0, 6,11, 0, 4, 6,-1,-1,-1,-1],
    [ 4,11, 8, 4, 6,11, 0, 2, 9, 2,10, 9,-1,-1,-1,-1],
    [10, 9, 3,10, 3, 2, 9, 4, 3,11, 3, 6, 4, 6, 3,-1],
    [ 8, 2, 3, 8, 4, 2, 4, 6, 2,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 4, 2, 4, 6, 2,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 9, 0, 2, 3, 4, 2, 4, 6, 4, 3, 8,-1,-1,-1,-1],
    [ 1, 9, 4, 1, 4, 2, 2, 4, 6,-1,-1,-1,-1,-1,-1,-1],
    [ 8, 1, 3, 8, 6, 1, 8, 4, 6, 6,10, 1,-1,-1,-1,-1],
    [10, 1, 0,10, 0, 6, 6, 0, 4,-1,-1,-1,-1,-1,-1,-1],
    [ 4, 6, 3, 4, 3, 8, 6,10, 3, 0, 3, 9,10, 9, 3,-1],
    [10, 9, 4, 6,10, 4,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 4, 9, 5, 7, 6,11,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 8, 3, 4, 9, 5,11, 7, 6,-1,-1,-1,-1,-1,-1,-1],
    [ 5, 0, 1, 5, 4, 0, 7, 6,11,-1,-1,-1,-1,-1,-1,-1],
    [11, 7, 6, 8, 3, 4, 3, 5, 4, 3, 1, 5,-1,-1,-1,-1],
    [ 9, 5, 4,10, 1, 2, 7, 6,11,-1,-1,-1,-1,-1,-1,-1],
    [ 6,11, 7, 1, 2,10, 0, 8, 3, 4, 9, 5,-1,-1,-1,-1],
    [ 7, 6,11, 5, 4,10, 4, 2,10, 4, 0, 2,-1,-1,-1,-1],
    [ 3, 4, 8, 3, 5, 4, 3, 2, 5,10, 5, 2,11, 7, 6,-1],
    [ 7, 2, 3, 7, 6, 2, 5, 4, 9,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 5, 4, 0, 8, 6, 0, 6, 2, 6, 8, 7,-1,-1,-1,-1],
    [ 3, 6, 2, 3, 7, 6, 1, 5, 0, 5, 4, 0,-1,-1,-1,-1],
    [ 6, 2, 8, 6, 8, 7, 2, 1, 8, 4, 8, 5, 1, 5, 8,-1],
    [ 9, 5, 4,10, 1, 6, 1, 7, 6, 1, 3, 7,-1,-1,-1,-1],
    [ 1, 6,10, 1, 7, 6, 1, 0, 7, 8, 7, 0, 9, 5, 4,-1],
    [ 4, 0,10, 4,10, 5, 0, 3,10, 6,10, 7, 3, 7,10,-1],
    [ 7, 6,10, 7,10, 8, 5, 4,10, 4, 8,10,-1,-1,-1,-1],
    [ 6, 9, 5, 6,11, 9,11, 8, 9,-1,-1,-1,-1,-1,-1,-1],
    [ 3, 6,11, 0, 6, 3, 0, 5, 6, 0, 9, 5,-1,-1,-1,-1],
    [ 0,11, 8, 0, 5,11, 0, 1, 5, 5, 6,11,-1,-1,-1,-1],
    [ 6,11, 3, 6, 3, 5, 5, 3, 1,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 2,10, 9, 5,11, 9,11, 8,11, 5, 6,-1,-1,-1,-1],
    [ 0,11, 3, 0, 6,11, 0, 9, 6, 5, 6, 9, 1, 2,10,-1],
    [11, 8, 5,11, 5, 6, 8, 0, 5,10, 5, 2, 0, 2, 5,-1],
    [ 6,11, 3, 6, 3, 5, 2,10, 3,10, 5, 3,-1,-1,-1,-1],
    [ 5, 8, 9, 5, 2, 8, 5, 6, 2, 3, 8, 2,-1,-1,-1,-1],
    [ 9, 5, 6, 9, 6, 0, 0, 6, 2,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 5, 8, 1, 8, 0, 5, 6, 8, 3, 8, 2, 6, 2, 8,-1],
    [ 1, 5, 6, 2, 1, 6,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 3, 6, 1, 6,10, 3, 8, 6, 5, 6, 9, 8, 9, 6,-1],
    [10, 1, 0,10, 0, 6, 9, 5, 0, 5, 6, 0,-1,-1,-1,-1],
    [ 0, 3, 8, 5, 6,10,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [10, 5, 6,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [11, 5,10, 7, 5,11,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [11, 5,10,11, 7, 5, 8, 3, 0,-1,-1,-1,-1,-1,-1,-1],
    [ 5,11, 7, 5,10,11, 1, 9, 0,-1,-1,-1,-1,-1,-1,-1],
    [10, 7, 5,10,11, 7, 9, 8, 1, 8, 3, 1,-1,-1,-1,-1],
    [11, 1, 2,11, 7, 1, 7, 5, 1,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 8, 3, 1, 2, 7, 1, 7, 5, 7, 2,11,-1,-1,-1,-1],
    [ 9, 7, 5, 9, 2, 7, 9, 0, 2, 2,11, 7,-1,-1,-1,-1],
    [ 7, 5, 2, 7, 2,11, 5, 9, 2, 3, 2, 8, 9, 8, 2,-1],
    [ 2, 5,10, 2, 3, 5, 3, 7, 5,-1,-1,-1,-1,-1,-1,-1],
    [ 8, 2, 0, 8, 5, 2, 8, 7, 5,10, 2, 5,-1,-1,-1,-1],
    [ 9, 0, 1, 5,10, 3, 5, 3, 7, 3,10, 2,-1,-1,-1,-1],
    [ 9, 8, 2, 9, 2, 1, 8, 7, 2,10, 2, 5, 7, 5, 2,-1],
    [ 1, 3, 5, 3, 7, 5,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 8, 7, 0, 7, 1, 1, 7, 5,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 0, 3, 9, 3, 5, 5, 3, 7,-1,-1,-1,-1,-1,-1,-1],
    [ 9, 8, 7, 5, 9, 7,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 5, 8, 4, 5,10, 8,10,11, 8,-1,-1,-1,-1,-1,-1,-1],
    [ 5, 0, 4, 5,11, 0, 5,10,11,11, 3, 0,-1,-1,-1,-1],
    [ 0, 1, 9, 8, 4,10, 8,10,11,10, 4, 5,-1,-1,-1,-1],
    [10,11, 4,10, 4, 5,11, 3, 4, 9, 4, 1, 3, 1, 4,-1],
    [ 2, 5, 1, 2, 8, 5, 2,11, 8, 4, 5, 8,-1,-1,-1,-1],
    [ 0, 4,11, 0,11, 3, 4, 5,11, 2,11, 1, 5, 1,11,-1],
    [ 0, 2, 5, 0, 5, 9, 2,11, 5, 4, 5, 8,11, 8, 5,-1],
    [ 9, 4, 5, 2,11, 3,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 2, 5,10, 3, 5, 2, 3, 4, 5, 3, 8, 4,-1,-1,-1,-1],
    [ 5,10, 2, 5, 2, 4, 4, 2, 0,-1,-1,-1,-1,-1,-1,-1],
    [ 3,10, 2, 3, 5,10, 3, 8, 5, 4, 5, 8, 0, 1, 9,-1],
    [ 5,10, 2, 5, 2, 4, 1, 9, 2, 9, 4, 2,-1,-1,-1,-1],
    [ 8, 4, 5, 8, 5, 3, 3, 5, 1,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 4, 5, 1, 0, 5,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 8, 4, 5, 8, 5, 3, 9, 0, 5, 0, 3, 5,-1,-1,-1,-1],
    [ 9, 4, 5,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 4,11, 7, 4, 9,11, 9,10,11,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 8, 3, 4, 9, 7, 9,11, 7, 9,10,11,-1,-1,-1,-1],
    [ 1,10,11, 1,11, 4, 1, 4, 0, 7, 4,11,-1,-1,-1,-1],
    [ 3, 1, 4, 3, 4, 8, 1,10, 4, 7, 4,11,10,11, 4,-1],
    [ 4,11, 7, 9,11, 4, 9, 2,11, 9, 1, 2,-1,-1,-1,-1],
    [ 9, 7, 4, 9,11, 7, 9, 1,11, 2,11, 1, 0, 8, 3,-1],
    [11, 7, 4, 11, 4, 2, 2, 4, 0,-1,-1,-1,-1,-1,-1,-1],
    [11, 7, 4,11, 4, 2, 8, 3, 4, 3, 2, 4,-1,-1,-1,-1],
    [ 2, 9,10, 2, 7, 9, 2, 3, 7, 7, 4, 9,-1,-1,-1,-1],
    [ 9,10, 7, 9, 7, 4,10, 2, 7, 8, 7, 0, 2, 0, 7,-1],
    [ 3, 7,10, 3,10, 2, 7, 4,10, 1,10, 0, 4, 0,10,-1],
    [ 1,10, 2, 8, 7, 4,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 4, 9, 1, 4, 1, 7, 7, 1, 3,-1,-1,-1,-1,-1,-1,-1],
    [ 4, 9, 1, 4, 1, 7, 0, 8, 1, 8, 7, 1,-1,-1,-1,-1],
    [ 4, 0, 3, 7, 4, 3,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 4, 8, 7,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 9,10, 8,10,11, 8,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 3, 0, 9, 3, 9,11,11, 9,10,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 1,10, 0,10, 8, 8,10,11,-1,-1,-1,-1,-1,-1,-1],
    [ 3, 1,10,11, 3,10,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 2,11, 1,11, 9, 9,11, 8,-1,-1,-1,-1,-1,-1,-1],
    [ 3, 0, 9, 3, 9,11, 1, 2, 9, 2,11, 9,-1,-1,-1,-1],
    [ 0, 2,11, 8, 0,11,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 3, 2,11,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 2, 3, 8, 2, 8,10,10, 8, 9,-1,-1,-1,-1,-1,-1,-1],
    [ 9,10, 2, 0, 9, 2,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 2, 3, 8, 2, 8,10, 0, 1, 8, 1,10, 8,-1,-1,-1,-1],
    [ 1,10, 2,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 1, 3, 8, 9, 1, 8,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 9, 1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [ 0, 3, 8,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
    [-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1],
];

// ── QEF Solver ──────────────────────────────────────────────────────

struct QefData {
    ata: [f64; 6],
    atb: [f64; 3],
    btb: f64,
    mass_point: [f64; 3],
    num_points: u32,
}

impl QefData {
    fn new() -> Self {
        Self {
            ata: [0.0; 6],
            atb: [0.0; 3],
            btb: 0.0,
            mass_point: [0.0; 3],
            num_points: 0,
        }
    }

    fn add(&mut self, p: [f32; 3], n: [f32; 3]) {
        let (px, py, pz) = (p[0] as f64, p[1] as f64, p[2] as f64);
        let (nx, ny, nz) = (n[0] as f64, n[1] as f64, n[2] as f64);

        self.ata[0] += nx * nx;
        self.ata[1] += nx * ny;
        self.ata[2] += nx * nz;
        self.ata[3] += ny * ny;
        self.ata[4] += ny * nz;
        self.ata[5] += nz * nz;

        let dot = nx * px + ny * py + nz * pz;
        self.atb[0] += nx * dot;
        self.atb[1] += ny * dot;
        self.atb[2] += nz * dot;
        self.btb += dot * dot;

        self.mass_point[0] += px;
        self.mass_point[1] += py;
        self.mass_point[2] += pz;
        self.num_points += 1;
    }

    fn solve(&self) -> [f32; 3] {
        if self.num_points == 0 {
            return [0.0; 3];
        }
        let mp = [
            self.mass_point[0] / self.num_points as f64,
            self.mass_point[1] / self.num_points as f64,
            self.mass_point[2] / self.num_points as f64,
        ];
        let solved = svd_solve_3x3(&self.ata, &self.atb, &mp);
        [solved[0] as f32, solved[1] as f32, solved[2] as f32]
    }
}

fn svd_solve_3x3(ata: &[f64; 6], atb: &[f64; 3], mass_point: &[f64; 3]) -> [f64; 3] {
    let mut a = [
        [ata[0], ata[1], ata[2]],
        [ata[1], ata[3], ata[4]],
        [ata[2], ata[4], ata[5]],
    ];

    let b = [
        atb[0] - (a[0][0] * mass_point[0] + a[0][1] * mass_point[1] + a[0][2] * mass_point[2]),
        atb[1] - (a[1][0] * mass_point[0] + a[1][1] * mass_point[1] + a[1][2] * mass_point[2]),
        atb[2] - (a[2][0] * mass_point[0] + a[2][1] * mass_point[1] + a[2][2] * mass_point[2]),
    ];

    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..10 {
        for p in 0..3 {
            for q in (p + 1)..3 {
                jacobi_rotation(&mut a, &mut v, p, q);
            }
        }
    }

    let threshold = 0.1;
    let mut x = [0.0f64; 3];
    for i in 0..3 {
        let sigma = a[i][i];
        if sigma.abs() > threshold {
            let vb = v[0][i] * b[0] + v[1][i] * b[1] + v[2][i] * b[2];
            let contrib = vb / sigma;
            x[0] += v[0][i] * contrib;
            x[1] += v[1][i] * contrib;
            x[2] += v[2][i] * contrib;
        }
    }

    [
        x[0] + mass_point[0],
        x[1] + mass_point[1],
        x[2] + mass_point[2],
    ]
}

fn jacobi_rotation(a: &mut [[f64; 3]; 3], v: &mut [[f64; 3]; 3], p: usize, q: usize) {
    if a[p][q].abs() < 1e-15 {
        return;
    }
    let tau = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
    let t = if tau >= 0.0 {
        1.0 / (tau + (1.0 + tau * tau).sqrt())
    } else {
        -1.0 / (-tau + (1.0 + tau * tau).sqrt())
    };
    let c = 1.0 / (1.0 + t * t).sqrt();
    let s = t * c;

    let app = a[p][p];
    let aqq = a[q][q];
    let apq = a[p][q];
    a[p][p] = c * c * app - 2.0 * s * c * apq + s * s * aqq;
    a[q][q] = s * s * app + 2.0 * s * c * apq + c * c * aqq;
    a[p][q] = 0.0;
    a[q][p] = 0.0;
    for r in 0..3 {
        if r != p && r != q {
            let arp = a[r][p];
            let arq = a[r][q];
            a[r][p] = c * arp - s * arq;
            a[p][r] = a[r][p];
            a[r][q] = s * arp + c * arq;
            a[q][r] = a[r][q];
        }
    }

    for r in 0..3 {
        let vrp = v[r][p];
        let vrq = v[r][q];
        v[r][p] = c * vrp - s * vrq;
        v[r][q] = s * vrp + c * vrq;
    }
}

// ── Cycle detection ─────────────────────────────────────────────────

/// For a given MC case, determine which edges belong to which contour
/// cycle using union-find on the MC triangle table.
///
/// Returns (edge_to_cycle, num_cycles) where edge_to_cycle[e] is the
/// cycle index for MC edge e, or -1 if the edge has no sign change.
fn compute_edge_cycles(case: u8) -> ([i8; 12], usize) {
    let mut parent = [0u8; 12];
    for i in 0..12 {
        parent[i] = i as u8;
    }

    fn find(parent: &mut [u8; 12], mut x: u8) -> u8 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }

    fn union(parent: &mut [u8; 12], a: u8, b: u8) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra as usize] = rb;
        }
    }

    let tris = &TRI_TABLE[case as usize];
    let mut active = [false; 12];
    let mut i = 0;
    while i + 2 < 16 && tris[i] >= 0 {
        let e0 = tris[i] as u8;
        let e1 = tris[i + 1] as u8;
        let e2 = tris[i + 2] as u8;
        active[e0 as usize] = true;
        active[e1 as usize] = true;
        active[e2 as usize] = true;
        union(&mut parent, e0, e1);
        union(&mut parent, e1, e2);
        i += 3;
    }

    let mut cycle_of_root = [-1i8; 12];
    let mut next_cycle = 0i8;
    let mut edge_cycle = [-1i8; 12];

    for e in 0..12 {
        if !active[e] {
            continue;
        }
        let root = find(&mut parent, e as u8) as usize;
        if cycle_of_root[root] < 0 {
            cycle_of_root[root] = next_cycle;
            next_cycle += 1;
        }
        edge_cycle[e] = cycle_of_root[root];
    }

    (edge_cycle, next_cycle as usize)
}

// ── Main algorithm ──────────────────────────────────────────────────

struct MdcVertex {
    pos: [f32; 3],
    normal: [f32; 3],
    classification: u8,
    contributors: [u32; MAX_CONTRIBUTORS],
}

/// Per-cell data: maps each MC edge to the vertex index assigned to it.
struct CellInfo {
    edge_vertex: [u32; 12],
}

/// Process a single MC cell: detect cycles via MDC, place QEF vertices,
/// return edge→vertex mapping and whether any feature vertex was created.
fn process_mc_cell(
    node: &BoundedCsg,
    mc_corners: &[f32; 8],
    cell_size: f32,
    cell_min: [f32; 3],
    normal_eps: f32,
    contributor_eps: f32,
    vertices: &mut Vec<MdcVertex>,
) -> ([u32; 12], bool) {
    let mut case = 0u8;
    for v in 0..8 {
        if mc_corners[v] < 0.0 {
            case |= 1 << v;
        }
    }
    if EDGE_TABLE[case as usize] == 0 {
        return ([u32::MAX; 12], false);
    }

    let (edge_cycle, num_cycles) = compute_edge_cycles(case);
    let mut edge_vertex = [u32::MAX; 12];
    let mut has_feature = false;

    for cycle in 0..num_cycles {
        let cycle_id = cycle as i8;
        let mut qef = QefData::new();

        for e in 0..12 {
            if edge_cycle[e] != cycle_id {
                continue;
            }
            let [v0, v1] = MC_EDGE_VERTS[e];
            let d0 = mc_corners[v0];
            let d1 = mc_corners[v1];
            if (d0 < 0.0) == (d1 < 0.0) {
                continue;
            }
            let p0 = [
                cell_min[0] + MC_CORNER_POS[v0][0] * cell_size,
                cell_min[1] + MC_CORNER_POS[v0][1] * cell_size,
                cell_min[2] + MC_CORNER_POS[v0][2] * cell_size,
            ];
            let p1 = [
                cell_min[0] + MC_CORNER_POS[v1][0] * cell_size,
                cell_min[1] + MC_CORNER_POS[v1][1] * cell_size,
                cell_min[2] + MC_CORNER_POS[v1][2] * cell_size,
            ];
            let sign_lo = d0 < 0.0;
            let mut lo = 0.0f32;
            let mut hi = 1.0f32;
            let mut t = d0 / (d0 - d1);
            for _ in 0..6 {
                let mp = [
                    p0[0] + t * (p1[0] - p0[0]),
                    p0[1] + t * (p1[1] - p0[1]),
                    p0[2] + t * (p1[2] - p0[2]),
                ];
                let d = node.sdf(mp, EvalMode::Visual);
                if (d < 0.0) == sign_lo {
                    lo = t;
                } else {
                    hi = t;
                }
                t = (lo + hi) * 0.5;
            }
            let p = [
                p0[0] + t * (p1[0] - p0[0]),
                p0[1] + t * (p1[1] - p0[1]),
                p0[2] + t * (p1[2] - p0[2]),
            ];
            let n = sdf_normal(node, p, normal_eps);
            qef.add(p, n);
        }

        if qef.num_points == 0 {
            continue;
        }

        let mut pos = qef.solve();
        let cell_max = [
            cell_min[0] + cell_size,
            cell_min[1] + cell_size,
            cell_min[2] + cell_size,
        ];
        pos[0] = pos[0].clamp(cell_min[0], cell_max[0]);
        pos[1] = pos[1].clamp(cell_min[1], cell_max[1]);
        pos[2] = pos[2].clamp(cell_min[2], cell_max[2]);

        let normal = sdf_normal(node, pos, normal_eps);
        let (_, cs) = node.evaluate(pos, contributor_eps, EvalMode::Visual);
        let mut element = cs.element();
        if element == ProxyElement::None {
            element = ProxyElement::Face;
        }
        let slice = cs.as_slice();
        let mut contribs = [CONTRIBUTOR_SENTINEL; MAX_CONTRIBUTORS];
        for slot in 0..MAX_CONTRIBUTORS {
            if slot < slice.len() {
                contribs[slot] = slice[slot]
                    .as_u32_or_root()
                    .unwrap_or(CONTRIBUTOR_SENTINEL);
            }
        }

        if element.tag() >= 2 {
            has_feature = true;
        }

        let idx = vertices.len() as u32;
        vertices.push(MdcVertex {
            pos,
            normal,
            classification: element.tag(),
            contributors: contribs,
        });

        for e in 0..12 {
            if edge_cycle[e] == cycle_id {
                edge_vertex[e] = idx;
            }
        }
    }

    (edge_vertex, has_feature)
}

/// Emit quads for a grid level. Pushes triangle indices into `indices`.
/// `skip_cell` is called with (ci, cj, ck) — return true to exclude
/// edges touching that cell.
fn emit_quads<'a>(
    sdf_at: &dyn Fn(usize, usize, usize) -> f32,
    cell_at: &dyn Fn(usize, usize, usize) -> Option<&'a CellInfo>,
    nx: usize,
    ny: usize,
    nz: usize,
    skip_cell: &dyn Fn(usize, usize, usize) -> bool,
    require_any: &dyn Fn(usize, usize, usize) -> bool,
    indices: &mut Vec<u32>,
) {
    for axis in 0..3usize {
        let ei_max = match axis {
            0 => nx,
            1 => nx + 1,
            _ => nx + 1,
        };
        let (ej_start, ek_start) = match axis {
            0 => (1usize, 1usize),
            1 => (0, 1),
            _ => (0, 0),
        };
        let (ej_end, ek_end) = (ny, nz);

        for ek in ek_start..ek_end {
            for ej in ej_start..ej_end {
                for ei in 0..ei_max {
                    let (ca, cb) = match axis {
                        0 => ((ei, ej, ek), (ei + 1, ej, ek)),
                        1 => ((ei, ej, ek), (ei, ej + 1, ek)),
                        _ => ((ei, ej, ek), (ei, ej, ek + 1)),
                    };
                    let da = sdf_at(ca.0, ca.1, ca.2);
                    let db = sdf_at(cb.0, cb.1, cb.2);
                    if (da < 0.0) == (db < 0.0) {
                        continue;
                    }

                    let quad_spec = &QUAD_CELLS[axis];
                    let mut vi = [u32::MAX; 4];
                    let mut all_valid = true;
                    let mut any_skip = false;
                    let mut any_required = false;

                    for q in 0..4 {
                        let (di, dj, dk, mc_edge) = quad_spec[q];
                        let ci = ei as i32 + di;
                        let cj = ej as i32 + dj;
                        let ck = ek as i32 + dk;
                        if ci < 0
                            || cj < 0
                            || ck < 0
                            || ci >= nx as i32
                            || cj >= ny as i32
                            || ck >= nz as i32
                        {
                            all_valid = false;
                            break;
                        }
                        let (cu, cv, cw) = (ci as usize, cj as usize, ck as usize);
                        if skip_cell(cu, cv, cw) {
                            any_skip = true;
                            break;
                        }
                        if require_any(cu, cv, cw) {
                            any_required = true;
                        }
                        if let Some(info) = cell_at(cu, cv, cw) {
                            vi[q] = info.edge_vertex[mc_edge];
                        } else {
                            all_valid = false;
                            break;
                        }
                    }

                    if any_skip || !all_valid || !any_required {
                        continue;
                    }
                    if vi[0] == u32::MAX
                        || vi[1] == u32::MAX
                        || vi[2] == u32::MAX
                        || vi[3] == u32::MAX
                    {
                        continue;
                    }
                    if vi[0] == vi[1]
                        || vi[0] == vi[2]
                        || vi[0] == vi[3]
                        || vi[1] == vi[2]
                        || vi[1] == vi[3]
                        || vi[2] == vi[3]
                    {
                        continue;
                    }

                    let (v00, v10, v11, v01) = (vi[0], vi[1], vi[2], vi[3]);
                    let flip = axis == 1;
                    if (da > 0.0) ^ flip {
                        indices.extend_from_slice(&[v00, v01, v11]);
                        indices.extend_from_slice(&[v00, v11, v10]);
                    } else {
                        indices.extend_from_slice(&[v00, v10, v11]);
                        indices.extend_from_slice(&[v00, v11, v01]);
                    }
                }
            }
        }
    }
}

pub fn manifold_dc(
    node: &BoundedCsg,
    grid_min: [f32; 3],
    cell: f32,
    nx: usize,
    ny: usize,
    nz: usize,
) -> Mesh {
    let cnx = nx + 1;
    let cny = ny + 1;
    let cnz = nz + 1;

    let corner_index =
        |i: usize, j: usize, k: usize| -> usize { i + j * cnx + k * cnx * cny };
    let corner_point = |i: usize, j: usize, k: usize| -> [f32; 3] {
        [
            grid_min[0] + i as f32 * cell,
            grid_min[1] + j as f32 * cell,
            grid_min[2] + k as f32 * cell,
        ]
    };
    let cell_index = |i: usize, j: usize, k: usize| -> usize { i + j * nx + k * nx * ny };

    // ── 1. Evaluate SDF at all coarse grid corners ──────────────────

    let mut sdf_grid = vec![0.0_f32; cnx * cny * cnz];
    for k in 0..cnz {
        for j in 0..cny {
            for i in 0..cnx {
                sdf_grid[corner_index(i, j, k)] =
                    node.sdf(corner_point(i, j, k), EvalMode::Visual);
            }
        }
    }

    let normal_eps = cell * 0.25;
    let contributor_eps = cell * 0.5;

    // ── 2. Process coarse cells ─────────────────────────────────────

    let mut vertices: Vec<MdcVertex> = Vec::new();
    let mut coarse_data: HashMap<usize, CellInfo> = HashMap::new();
    let mut feature_cells: Vec<(usize, usize, usize)> = Vec::new();

    for ck in 0..nz {
        for cj in 0..ny {
            for ci in 0..nx {
                let mc_corners: [f32; 8] = std::array::from_fn(|v| {
                    let gi = ci + MC_CORNER_POS[v][0] as usize;
                    let gj = cj + MC_CORNER_POS[v][1] as usize;
                    let gk = ck + MC_CORNER_POS[v][2] as usize;
                    sdf_grid[corner_index(gi, gj, gk)]
                });

                let cm = corner_point(ci, cj, ck);
                let (edge_vertex, has_feature) = process_mc_cell(
                    node,
                    &mc_corners,
                    cell,
                    cm,
                    normal_eps,
                    contributor_eps,
                    &mut vertices,
                );

                if edge_vertex.iter().any(|&v| v != u32::MAX) {
                    coarse_data.insert(cell_index(ci, cj, ck), CellInfo { edge_vertex });
                    if has_feature {
                        feature_cells.push((ci, cj, ck));
                    }
                }
            }
        }
    }

    // ── 3. Build refinement sets ────────────────────────────────────

    let mut refine_set: HashSet<(usize, usize, usize)> = HashSet::new();
    for &(ci, cj, ck) in &feature_cells {
        for di in -1i32..=1 {
            for dj in -1i32..=1 {
                for dk in -1i32..=1 {
                    let ri = ci as i32 + di;
                    let rj = cj as i32 + dj;
                    let rk = ck as i32 + dk;
                    if ri >= 0
                        && ri < nx as i32
                        && rj >= 0
                        && rj < ny as i32
                        && rk >= 0
                        && rk < nz as i32
                    {
                        refine_set.insert((ri as usize, rj as usize, rk as usize));
                    }
                }
            }
        }
    }

    if refine_set.is_empty() {
        // No features — emit coarse mesh directly.
        let mut indices = Vec::new();
        let always_true = |_: usize, _: usize, _: usize| true;
        let never = |_: usize, _: usize, _: usize| false;
        emit_quads(
            &|i, j, k| sdf_grid[corner_index(i, j, k)],
            &|i, j, k| coarse_data.get(&cell_index(i, j, k)),
            nx,
            ny,
            nz,
            &never,
            &always_true,
            &mut indices,
        );
        return pack_mesh(vertices, indices);
    }

    // Transition set: non-refined cells adjacent to refine_set.
    let mut transition_set: HashSet<(usize, usize, usize)> = HashSet::new();
    for &(ci, cj, ck) in &refine_set {
        for di in -1i32..=1 {
            for dj in -1i32..=1 {
                for dk in -1i32..=1 {
                    let ti = ci as i32 + di;
                    let tj = cj as i32 + dj;
                    let tk = ck as i32 + dk;
                    if ti >= 0
                        && ti < nx as i32
                        && tj >= 0
                        && tj < ny as i32
                        && tk >= 0
                        && tk < nz as i32
                    {
                        let tc = (ti as usize, tj as usize, tk as usize);
                        if !refine_set.contains(&tc) {
                            transition_set.insert(tc);
                        }
                    }
                }
            }
        }
    }

    // Promote multi-cycle transition cells into refine_set.
    let mut promoted: Vec<(usize, usize, usize)> = Vec::new();
    for &(ci, cj, ck) in &transition_set {
        if let Some(info) = coarse_data.get(&cell_index(ci, cj, ck)) {
            let mut unique = 0u32;
            let mut first = u32::MAX;
            for &v in &info.edge_vertex {
                if v != u32::MAX {
                    if first == u32::MAX {
                        first = v;
                        unique = 1;
                    } else if v != first {
                        unique = 2;
                        break;
                    }
                }
            }
            if unique > 1 {
                promoted.push((ci, cj, ck));
            }
        }
    }
    for &c in &promoted {
        transition_set.remove(&c);
        refine_set.insert(c);
    }
    if !promoted.is_empty() {
        for &(ci, cj, ck) in &promoted {
            for di in -1i32..=1 {
                for dj in -1i32..=1 {
                    for dk in -1i32..=1 {
                        let ti = ci as i32 + di;
                        let tj = cj as i32 + dj;
                        let tk = ck as i32 + dk;
                        if ti >= 0
                            && ti < nx as i32
                            && tj >= 0
                            && tj < ny as i32
                            && tk >= 0
                            && tk < nz as i32
                        {
                            let tc = (ti as usize, tj as usize, tk as usize);
                            if !refine_set.contains(&tc) {
                                transition_set.insert(tc);
                            }
                        }
                    }
                }
            }
        }
    }

    // ── 4. Fine SDF sampling ────────────────────────────────────────

    let fine_cell = cell * 0.5;
    let fnx = 2 * nx;
    let fny = 2 * ny;
    let fnz = 2 * nz;
    let fcnx = fnx + 1;
    let fcny = fny + 1;

    let fine_corner_index =
        |fi: usize, fj: usize, fk: usize| -> usize { fi + fj * fcnx + fk * fcnx * fcny };
    let fine_corner_point = |fi: usize, fj: usize, fk: usize| -> [f32; 3] {
        [
            grid_min[0] + fi as f32 * fine_cell,
            grid_min[1] + fj as f32 * fine_cell,
            grid_min[2] + fk as f32 * fine_cell,
        ]
    };
    let fine_cell_index =
        |fi: usize, fj: usize, fk: usize| -> usize { fi + fj * fnx + fk * fnx * fny };

    let mut fine_sdf: HashMap<usize, f32> = HashMap::new();
    for ci in 0..nx {
        for cj in 0..ny {
            for ck in 0..nz {
                if !refine_set.contains(&(ci, cj, ck))
                    && !transition_set.contains(&(ci, cj, ck))
                {
                    continue;
                }
                for di in 0..=2usize {
                    for dj in 0..=2usize {
                        for dk in 0..=2usize {
                            let fi = 2 * ci + di;
                            let fj = 2 * cj + dj;
                            let fk = 2 * ck + dk;
                            let fidx = fine_corner_index(fi, fj, fk);
                            if fine_sdf.contains_key(&fidx) {
                                continue;
                            }
                            let val = if fi % 2 == 0 && fj % 2 == 0 && fk % 2 == 0 {
                                sdf_grid[corner_index(fi / 2, fj / 2, fk / 2)]
                            } else {
                                node.sdf(fine_corner_point(fi, fj, fk), EvalMode::Visual)
                            };
                            fine_sdf.insert(fidx, val);
                        }
                    }
                }
            }
        }
    }

    // ── 5. Fine-level QEF for refined cells ─────────────────────────

    let fine_normal_eps = fine_cell * 0.25;
    let fine_contributor_eps = fine_cell * 0.5;
    let mut fine_data: HashMap<usize, CellInfo> = HashMap::new();

    for &(ci, cj, ck) in &refine_set {
        for si in 0..2usize {
            for sj in 0..2usize {
                for sk in 0..2usize {
                    let fi = 2 * ci + si;
                    let fj = 2 * cj + sj;
                    let fk = 2 * ck + sk;

                    let mc_corners: [f32; 8] = std::array::from_fn(|v| {
                        let gi = fi + MC_CORNER_POS[v][0] as usize;
                        let gj = fj + MC_CORNER_POS[v][1] as usize;
                        let gk = fk + MC_CORNER_POS[v][2] as usize;
                        fine_sdf[&fine_corner_index(gi, gj, gk)]
                    });

                    let fcm = fine_corner_point(fi, fj, fk);
                    let (edge_vertex, _) = process_mc_cell(
                        node,
                        &mc_corners,
                        fine_cell,
                        fcm,
                        fine_normal_eps,
                        fine_contributor_eps,
                        &mut vertices,
                    );

                    if edge_vertex.iter().any(|&v| v != u32::MAX) {
                        fine_data
                            .insert(fine_cell_index(fi, fj, fk), CellInfo { edge_vertex });
                    }
                }
            }
        }
    }

    // ── 6. Virtual fine sub-cells for transition cells ───────────────

    for &(ci, cj, ck) in &transition_set {
        let coarse_info = match coarse_data.get(&cell_index(ci, cj, ck)) {
            Some(info) => info,
            None => continue,
        };

        let mut coarse_vertex = u32::MAX;
        for &v in &coarse_info.edge_vertex {
            if v != u32::MAX {
                coarse_vertex = v;
                break;
            }
        }
        if coarse_vertex == u32::MAX {
            continue;
        }

        for si in 0..2usize {
            for sj in 0..2usize {
                for sk in 0..2usize {
                    let fi = 2 * ci + si;
                    let fj = 2 * cj + sj;
                    let fk = 2 * ck + sk;

                    let mut mc_corners = [0.0f32; 8];
                    let mut all_ok = true;
                    for v in 0..8 {
                        let gi = fi + MC_CORNER_POS[v][0] as usize;
                        let gj = fj + MC_CORNER_POS[v][1] as usize;
                        let gk = fk + MC_CORNER_POS[v][2] as usize;
                        match fine_sdf.get(&fine_corner_index(gi, gj, gk)) {
                            Some(&val) => mc_corners[v] = val,
                            None => {
                                all_ok = false;
                                break;
                            }
                        }
                    }
                    if !all_ok {
                        continue;
                    }

                    let mut case = 0u8;
                    for v in 0..8 {
                        if mc_corners[v] < 0.0 {
                            case |= 1 << v;
                        }
                    }
                    if EDGE_TABLE[case as usize] == 0 {
                        continue;
                    }

                    let (edge_cycle, _) = compute_edge_cycles(case);
                    let mut edge_vertex = [u32::MAX; 12];
                    for e in 0..12 {
                        if edge_cycle[e] >= 0 {
                            edge_vertex[e] = coarse_vertex;
                        }
                    }

                    fine_data
                        .insert(fine_cell_index(fi, fj, fk), CellInfo { edge_vertex });
                }
            }
        }
    }

    // ── 7. Emit quads ───────────────────────────────────────────────

    let mut indices: Vec<u32> = Vec::new();

    // 7a. Coarse quads: skip edges touching refine_set.
    let always_true = |_: usize, _: usize, _: usize| true;
    emit_quads(
        &|i, j, k| sdf_grid[corner_index(i, j, k)],
        &|i, j, k| coarse_data.get(&cell_index(i, j, k)),
        nx,
        ny,
        nz,
        &|ci, cj, ck| refine_set.contains(&(ci, cj, ck)),
        &always_true,
        &mut indices,
    );

    // 7b. Fine quads: emit where all 4 fine cells have data and at
    //     least one lies in a refined coarse cell.
    emit_quads(
        &|fi, fj, fk| {
            fine_sdf
                .get(&fine_corner_index(fi, fj, fk))
                .copied()
                .unwrap_or(0.0)
        },
        &|fi, fj, fk| fine_data.get(&fine_cell_index(fi, fj, fk)),
        fnx,
        fny,
        fnz,
        &|_, _, _| false,
        &|fi, fj, fk| refine_set.contains(&(fi / 2, fj / 2, fk / 2)),
        &mut indices,
    );

    // ── 8. Pack into Mesh ───────────────────────────────────────────

    pack_mesh(vertices, indices)
}

fn pack_mesh(vertices: Vec<MdcVertex>, indices: Vec<u32>) -> Mesh {
    let vert_count = vertices.len();
    let mut positions = Vec::with_capacity(vert_count * 3);
    let mut normals = Vec::with_capacity(vert_count * 3);
    let mut classifications = Vec::with_capacity(vert_count);
    let mut contributors = Vec::with_capacity(vert_count * MAX_CONTRIBUTORS);

    for v in &vertices {
        positions.extend_from_slice(&v.pos);
        normals.extend_from_slice(&v.normal);
        classifications.push(v.classification);
        contributors.extend_from_slice(&v.contributors);
    }

    Mesh {
        positions,
        normals,
        indices,
        classifications,
        contributors,
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::CsgNode;
    use fielddraft_protocol::{BoolOp, Feature, FeatureId, Primitive, Transform};

    fn make_feature(id: u32, prim: Primitive) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: String::new(),
            op: BoolOp::Union,
            primitive: prim,
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
            target_ids: vec![],
        }
    }

    fn bounded_sphere(center: [f32; 3], radius: f32) -> crate::bvh::BoundedCsg {
        let f = make_feature(1, Primitive::Sphere { center, radius });
        let csg = CsgNode::from_features(&[f]);
        crate::bvh::BoundedCsg::from_csg(&csg)
    }

    fn bounded_box(center: [f32; 3], half: [f32; 3]) -> crate::bvh::BoundedCsg {
        let f = make_feature(
            1,
            Primitive::Box {
                center,
                half_extents: half,
            },
        );
        let csg = CsgNode::from_features(&[f]);
        crate::bvh::BoundedCsg::from_csg(&csg)
    }

    fn run_manifold_dc(bounded: &crate::bvh::BoundedCsg, resolution: u32) -> Mesh {
        let aabb = bounded.aabb();
        let raw_size = aabb.size();
        let max_dim = raw_size[0].max(raw_size[1]).max(raw_size[2]);
        let cell = max_dim / resolution as f32;
        let grid = aabb.expanded(cell * crate::dc::DC_MARGIN_CELLS);
        let size = grid.size();
        let nx = ((size[0] / cell).ceil() as usize).max(1);
        let ny = ((size[1] / cell).ceil() as usize).max(1);
        let nz = ((size[2] / cell).ceil() as usize).max(1);
        manifold_dc(bounded, grid.min, cell, nx, ny, nz)
    }

    #[test]
    fn cycle_detection_single_triangle() {
        // Case 1: only MC vertex 0 inside → edges 0, 3, 8 → 1 cycle
        let (cycles, num) = compute_edge_cycles(1);
        assert_eq!(num, 1);
        assert!(cycles[0] >= 0);
        assert!(cycles[3] >= 0);
        assert!(cycles[8] >= 0);
        assert_eq!(cycles[0], cycles[3]);
        assert_eq!(cycles[3], cycles[8]);
    }

    #[test]
    fn cycle_detection_empty() {
        let (cycles, num) = compute_edge_cycles(0);
        assert_eq!(num, 0);
        assert!(cycles.iter().all(|&c| c < 0));
    }

    #[test]
    fn cycle_detection_full() {
        let (cycles, num) = compute_edge_cycles(255);
        assert_eq!(num, 0);
        assert!(cycles.iter().all(|&c| c < 0));
    }

    #[test]
    fn sphere_produces_manifold_mesh() {
        let bounded = bounded_sphere([0.0, 0.0, 0.0], 10.0);
        let mesh = run_manifold_dc(&bounded, 32);

        assert!(mesh.vertex_count() > 0, "mesh should have vertices");
        assert!(mesh.triangle_count() > 0, "mesh should have triangles");
        assert_eq!(mesh.indices.len() % 3, 0);

        for &idx in &mesh.indices {
            assert!(
                (idx as usize) < mesh.vertex_count(),
                "index {idx} out of bounds"
            );
        }

        // Check manifold: every edge should be shared by exactly 2 triangles
        let mut edge_count: HashMap<(u32, u32), usize> = HashMap::new();
        for t in 0..mesh.triangle_count() {
            let tri = [
                mesh.indices[t * 3],
                mesh.indices[t * 3 + 1],
                mesh.indices[t * 3 + 2],
            ];
            for e in 0..3 {
                let a = tri[e];
                let b = tri[(e + 1) % 3];
                let key = if a < b { (a, b) } else { (b, a) };
                *edge_count.entry(key).or_default() += 1;
            }
        }
        let non_manifold = edge_count.values().filter(|&&c| c != 2).count();
        assert_eq!(
            non_manifold, 0,
            "{non_manifold} non-manifold edges (expected all edges shared by exactly 2 triangles)"
        );
    }

    #[test]
    fn sphere_vertices_hug_surface() {
        let radius = 10.0;
        let bounded = bounded_sphere([0.0, 0.0, 0.0], radius);
        let mesh = run_manifold_dc(&bounded, 32);

        let mut max_dist = 0.0f32;
        for i in 0..mesh.vertex_count() {
            let x = mesh.positions[i * 3];
            let y = mesh.positions[i * 3 + 1];
            let z = mesh.positions[i * 3 + 2];
            let dist = (x * x + y * y + z * z).sqrt() - radius;
            max_dist = max_dist.max(dist.abs());
        }
        let cell = 20.0 / 32.0;
        assert!(
            max_dist < cell * 2.0,
            "sphere vertices should be near surface, max dist = {max_dist}"
        );
    }

    #[test]
    fn box_preserves_sharp_edges() {
        let half = [5.0, 5.0, 5.0];
        let bounded = bounded_box([0.0, 0.0, 0.0], half);
        let mesh = run_manifold_dc(&bounded, 32);

        assert!(mesh.vertex_count() > 0);
        assert!(mesh.triangle_count() > 0);

        let corners = [
            [-5.0, -5.0, -5.0],
            [5.0, -5.0, -5.0],
            [-5.0, 5.0, -5.0],
            [5.0, 5.0, -5.0],
            [-5.0, -5.0, 5.0],
            [5.0, -5.0, 5.0],
            [-5.0, 5.0, 5.0],
            [5.0, 5.0, 5.0],
        ];
        let mut corners_found = 0;
        for corner in &corners {
            let mut best = f32::MAX;
            for i in 0..mesh.vertex_count() {
                let dx = mesh.positions[i * 3] - corner[0];
                let dy = mesh.positions[i * 3 + 1] - corner[1];
                let dz = mesh.positions[i * 3 + 2] - corner[2];
                let d = (dx * dx + dy * dy + dz * dz).sqrt();
                best = best.min(d);
            }
            if best < 1.0 {
                corners_found += 1;
            }
        }
        assert!(
            corners_found >= 6,
            "QEF should place vertices near box corners, found {corners_found}/8"
        );
    }

    #[test]
    fn box_manifold_check() {
        let bounded = bounded_box([0.0, 0.0, 0.0], [5.0, 5.0, 5.0]);
        let mesh = run_manifold_dc(&bounded, 16);

        assert!(mesh.vertex_count() > 0);

        let mut edge_count: HashMap<(u32, u32), usize> = HashMap::new();
        for t in 0..mesh.triangle_count() {
            let tri = [
                mesh.indices[t * 3],
                mesh.indices[t * 3 + 1],
                mesh.indices[t * 3 + 2],
            ];
            for e in 0..3 {
                let a = tri[e];
                let b = tri[(e + 1) % 3];
                let key = if a < b { (a, b) } else { (b, a) };
                *edge_count.entry(key).or_default() += 1;
            }
        }
        let non_manifold = edge_count.values().filter(|&&c| c != 2).count();
        assert_eq!(non_manifold, 0, "{non_manifold} non-manifold edges in box mesh");
    }

    #[test]
    fn classification_arrays_parallel() {
        let bounded = bounded_box([0.0, 0.0, 0.0], [5.0, 5.0, 5.0]);
        let mesh = run_manifold_dc(&bounded, 16);

        assert_eq!(mesh.classifications.len(), mesh.vertex_count());
        assert_eq!(
            mesh.contributors.len(),
            mesh.vertex_count() * MAX_CONTRIBUTORS
        );
        assert_eq!(mesh.positions.len(), mesh.vertex_count() * 3);
        assert_eq!(mesh.normals.len(), mesh.vertex_count() * 3);
    }

    #[test]
    fn qef_places_vertex_on_plane() {
        let mut qef = QefData::new();
        qef.add([5.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        qef.add([5.0, 1.0, 0.0], [1.0, 0.0, 0.0]);
        qef.add([5.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
        let pos = qef.solve();
        assert!(
            (pos[0] - 5.0).abs() < 0.1,
            "x should be ~5, got {}",
            pos[0]
        );
    }
}
