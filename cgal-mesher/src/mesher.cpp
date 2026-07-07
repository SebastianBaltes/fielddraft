// FieldDraft CGAL mesher — Emscripten/WebAssembly module.
//
// Replaces the low-quality MDC mesher with CGAL `Mesh_3` (Restricted
// Delaunay refinement, angle/sliver guarantees, 1D sharp-feature
// protection). The Rust kernel ships the body's SDF as a flat opcode
// buffer (the "Data Shipping" model) plus optional sharp B-Rep edge
// polylines; this module evaluates the SDF autonomously via the ported
// interpreter (sdf_interp.h) — no per-sample FFI callback into Rust —
// meshes the implicit domain, and returns flat vertex/normal/index
// buffers that JS slices straight into Three.js BufferGeometry.
//
// Build: see ../build.sh (Emscripten + CGAL 6 + Boost 1.86 headers).

#include <CGAL/Exact_predicates_inexact_constructions_kernel.h>

#include <CGAL/Mesh_triangulation_3.h>
#include <CGAL/Mesh_complex_3_in_triangulation_3.h>
#include <CGAL/Mesh_criteria_3.h>

#include <CGAL/Labeled_mesh_domain_3.h>
#include <CGAL/Mesh_domain_with_polyline_features_3.h>
#include <CGAL/make_mesh_3.h>

#include <CGAL/facets_in_complex_3_to_triangle_mesh.h>
#include <CGAL/Surface_mesh.h>

#include <atomic>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <cstdio>
#include <list>
#include <unordered_map>
#include <vector>

#include "sdf_interp.h"

#if defined(__EMSCRIPTEN__)
#include <emscripten/emscripten.h>
#define FD_EXPORT extern "C" EMSCRIPTEN_KEEPALIVE
#else
#define FD_EXPORT extern "C"
#endif

typedef CGAL::Exact_predicates_inexact_constructions_kernel K;
typedef K::FT FT;
typedef K::Point_3 Point;

typedef CGAL::Mesh_domain_with_polyline_features_3<
    CGAL::Labeled_mesh_domain_3<K> > Mesh_domain;

typedef std::vector<Point> Polyline_3;
typedef std::list<Polyline_3> Polylines;

// Parallel refinement when built against TBB (native server build);
// the wasm build stays sequential.
#ifdef CGAL_LINKED_WITH_TBB
typedef CGAL::Parallel_tag Concurrency_tag;
#else
typedef CGAL::Sequential_tag Concurrency_tag;
#endif
typedef CGAL::Mesh_triangulation_3<Mesh_domain, CGAL::Default,
                                   Concurrency_tag>::type Tr;
typedef CGAL::Mesh_complex_3_in_triangulation_3<
    Tr, Mesh_domain::Corner_index, Mesh_domain::Curve_index> C3t3;
typedef CGAL::Mesh_criteria_3<Tr> Mesh_criteria;
typedef CGAL::Surface_mesh<Point> SMesh;

namespace params = CGAL::parameters;

// ── Module state ───────────────────────────────────────────────────
//
// CGAL's implicit-domain function is a plain callable `FT(const Point&)`,
// so the active opcode buffer is held in globals the function reads.

static const float* g_ops = nullptr;
static int g_nops = 0;

static std::vector<float> g_positions;   // 3 per vertex
static std::vector<float> g_normals;     // 3 per vertex
static std::vector<uint32_t> g_indices;  // 3 per triangle
static Polylines g_edges;                // protected sharp features

// SDF value at a world point — sign defines the label (negative = inside).
// The eval counter is the cheapest progress signal Mesh_3 offers: a run
// that keeps printing is eval-bound, a silent one is stuck elsewhere.
static std::atomic<long> g_eval_count{0};

static FT sdf_fn(const Point& p) {
  long n = ++g_eval_count;
  if (n % 1000000 == 0)
    std::fprintf(stderr, "[cgal] sdf evals: %ldM\n", n / 1000000);
  float v = fd::eval_opcodes(g_ops, g_nops, (float)CGAL::to_double(p.x()),
                             (float)CGAL::to_double(p.y()),
                             (float)CGAL::to_double(p.z()));
  return FT(v);
}

// Analytic-ish surface normal via central differences of the SDF.
static void sdf_normal(float x, float y, float z, float eps, float out[3]) {
  float dx = fd::eval_opcodes(g_ops, g_nops, x + eps, y, z) -
             fd::eval_opcodes(g_ops, g_nops, x - eps, y, z);
  float dy = fd::eval_opcodes(g_ops, g_nops, x, y + eps, z) -
             fd::eval_opcodes(g_ops, g_nops, x, y - eps, z);
  float dz = fd::eval_opcodes(g_ops, g_nops, x, y, z + eps) -
             fd::eval_opcodes(g_ops, g_nops, x, y, z - eps);
  float len = std::sqrt(dx * dx + dy * dy + dz * dz);
  if (len > 1e-12f) {
    out[0] = dx / len;
    out[1] = dy / len;
    out[2] = dz / len;
  } else {
    out[0] = 0.0f;
    out[1] = 1.0f;
    out[2] = 0.0f;
  }
}

// ── Exported API ───────────────────────────────────────────────────

FD_EXPORT void fd_reset() {
  g_edges.clear();
  g_positions.clear();
  g_normals.clear();
  g_indices.clear();
}

// Append a protected sharp-edge polyline: `pts` holds `n` points,
// 3 floats each (x,y,z). Call once per B-Rep edge before fd_mesh.
FD_EXPORT void fd_add_edge(const float* pts, int n) {
  if (n < 2) return;
  Polyline_3 poly;
  poly.reserve(n);
  for (int i = 0; i < n; ++i) {
    poly.push_back(Point(pts[3 * i], pts[3 * i + 1], pts[3 * i + 2]));
  }
  g_edges.push_back(std::move(poly));
}

// Mesh the implicit SDF over the given AABB. `resolution` is the
// approximate number of cells along the AABB diagonal (drives facet/cell
// sizing). Returns the triangle count, or 0 on empty/failure.
FD_EXPORT int fd_mesh(const float* opcodes, int n_ops, float minx, float miny,
                      float minz, float maxx, float maxy, float maxz,
                      int resolution, float facet_dist_mult) {
  g_ops = opcodes;
  g_nops = n_ops;
  g_eval_count = 0;
  g_positions.clear();
  g_normals.clear();
  g_indices.clear();

  const float ex = maxx - minx, ey = maxy - miny, ez = maxz - minz;
  float diag = std::sqrt(ex * ex + ey * ey + ez * ez);
  if (!(diag > 0.0f) || resolution < 1) return 0;

  const float cell = diag / (float)resolution;

  // Marker 6 (entry): proves the C++ module is actually invoked and with
  // which parameters. Emscripten routes stdout to the JS console.
  std::fprintf(stderr,
      "[cgal] fd_mesh: n_ops=%d aabb=[%.3f,%.3f,%.3f .. %.3f,%.3f,%.3f] "
      "res=%d cell=%.4f facet_dist_mult=%.3f edges=%zu\n",
      n_ops, minx, miny, minz, maxx, maxy, maxz, resolution, cell,
      facet_dist_mult, g_edges.size());

  // Bounding sphere must enclose the whole surface (with margin); CGAL's
  // Sphere_3 constructor takes the SQUARED radius.
  const float cx = 0.5f * (minx + maxx);
  const float cy = 0.5f * (miny + maxy);
  const float cz = 0.5f * (minz + maxz);
  const float bsr = 0.5f * diag * 1.05f + 2.0f * cell;

  // Fail fast on a field with no surface: CGAL's initial-point search
  // never terminates when the SDF has no sign change inside the bounding
  // sphere (billions of evals — exactly what a broken/desynced opcode
  // stream produces). A coarse 24³ probe costs ~14k evals and catches it.
  // Probe over a padded box, not the tight AABB: a body that fills its
  // AABB exactly (plain box) has no positive sample inside it, but the
  // mesh domain sphere extends beyond — so must the probe.
  {
    fd::unknown_opcode_seen() = false;
    const int N = 24;
    const float pad = 0.025f * diag + 2.0f * cell;
    const float pminx = minx - pad, pminy = miny - pad, pminz = minz - pad;
    const float pex = ex + 2.0f * pad, pey = ey + 2.0f * pad,
                pez = ez + 2.0f * pad;
    bool any_neg = false, any_pos = false;
    for (int i = 0; i < N && !(any_neg && any_pos); ++i)
      for (int j = 0; j < N; ++j)
        for (int k = 0; k < N; ++k) {
          float x = pminx + pex * (i + 0.5f) / N;
          float y = pminy + pey * (j + 0.5f) / N;
          float z = pminz + pez * (k + 0.5f) / N;
          float v = fd::eval_opcodes(g_ops, g_nops, x, y, z);
          if (v < 0.0f) any_neg = true;
          else if (v > 0.0f) any_pos = true;
        }
    if (fd::unknown_opcode_seen()) {
      std::fprintf(stderr, "[cgal] fd_mesh: ABORT — opcode stream contains "
                           "unsupported opcodes, SDF is garbage\n");
      return 0;
    }
    if (!any_neg || !any_pos) {
      std::fprintf(stderr, "[cgal] fd_mesh: ABORT — SDF has no sign change "
                           "in AABB (neg=%d pos=%d), no surface to mesh\n",
                   (int)any_neg, (int)any_pos);
      return 0;
    }
  }

  Mesh_domain domain = Mesh_domain::create_implicit_mesh_domain(
      sdf_fn, K::Sphere_3(Point(cx, cy, cz), FT(bsr * bsr)));

  // We only want the boundary SURFACE, but Mesh_3 always builds a full
  // tetrahedral volume. The perf lever is to drive surface refinement by
  // CURVATURE (facet_distance), not by a uniform size cap: a flat region
  // has zero approximation error, so facet_distance never forces a split
  // and a box face collapses to a handful of triangles instead of the
  // ~22k a uniform facet_size(cell) produced. facet_size therefore stays a
  // loose upper bound (diag = effectively unbounded) so flat faces are not
  // subdivided needlessly; facet_distance = cell * facet_dist_mult sets the
  // chord error on curved regions.
  //
  // cell_size stays a bounded multiple of cell (NOT diag): the interior tets
  // are discarded, but their density seeds CGAL's surface discovery. A loose
  // cell_size(diag) under-seeds complex multi-sheet surfaces (e.g. a gyroid
  // meshes to nothing). cell*3 keeps tet count bounded by surface area while
  // guaranteeing every surface component is found.
  const double fdist = cell * (facet_dist_mult > 0.0f ? facet_dist_mult : 0.1f);
  Mesh_criteria criteria(params::edge_size(cell)
                             .facet_angle(25.0)
                             .facet_size(diag)
                             .facet_distance(fdist)
                             .cell_radius_edge_ratio(3.0)
                             .cell_size(cell * 3.0));

  bool has_features = !g_edges.empty();
  if (has_features) {
    std::size_t npts = 0;
    for (const auto& e : g_edges) npts += e.size();
    std::fprintf(stderr, "[cgal] add_features: %zu polylines, %zu points\n",
                 g_edges.size(), npts);
    domain.add_features(g_edges.begin(), g_edges.end());
    std::fprintf(stderr, "[cgal] add_features done\n");
  }

  // params::manifold(): refine until the surface is a 2-manifold — the
  // watertightness contract of the server mesher (plan §P1). manifold
  // refinement can degenerate into a runaway loop on some SDFs, so
  // FD_NO_MANIFOLD=1 disables it for diagnosis.
  const bool want_manifold = std::getenv("FD_NO_MANIFOLD") == nullptr;
  std::fprintf(stderr, "[cgal] make_mesh_3 (manifold=%d)...\n",
               (int)want_manifold);
  C3t3 c3t3;
  if (want_manifold) {
    c3t3 = has_features
               ? CGAL::make_mesh_3<C3t3>(domain, criteria, params::manifold())
               : CGAL::make_mesh_3<C3t3>(domain, criteria,
                                         params::no_features(),
                                         params::manifold());
  } else {
    c3t3 = has_features
               ? CGAL::make_mesh_3<C3t3>(domain, criteria)
               : CGAL::make_mesh_3<C3t3>(domain, criteria,
                                         params::no_features());
  }

  SMesh sm;
  CGAL::facets_in_complex_3_to_triangle_mesh(c3t3, sm);

  const std::size_t nv = sm.number_of_vertices();
  const std::size_t nf = sm.number_of_faces();
  // Marker 6 (exit): what Mesh_3 actually produced. nv==0/nf==0 here is the
  // "CGAL ran but yielded nothing" case (→ empty body, not an MDC fallback).
  std::fprintf(stderr, "[cgal] fd_mesh: result verts=%zu faces=%zu has_features=%d\n",
              nv, nf, (int)has_features);
  if (nv == 0 || nf == 0) return 0;

  g_positions.reserve(nv * 3);
  g_normals.reserve(nv * 3);
  g_indices.reserve(nf * 3);

  const float neps = cell * 0.5f;
  std::unordered_map<uint32_t, uint32_t> remap;
  remap.reserve(nv * 2);

  for (SMesh::Vertex_index v : sm.vertices()) {
    const Point& p = sm.point(v);
    float x = (float)CGAL::to_double(p.x());
    float y = (float)CGAL::to_double(p.y());
    float z = (float)CGAL::to_double(p.z());
    uint32_t compact = (uint32_t)(g_positions.size() / 3);
    remap[(uint32_t)v] = compact;
    g_positions.push_back(x);
    g_positions.push_back(y);
    g_positions.push_back(z);
    float nrm[3];
    sdf_normal(x, y, z, neps, nrm);
    g_normals.push_back(nrm[0]);
    g_normals.push_back(nrm[1]);
    g_normals.push_back(nrm[2]);
  }

  for (SMesh::Face_index f : sm.faces()) {
    uint32_t tri[3];
    int k = 0;
    bool ok = true;
    for (SMesh::Vertex_index v : CGAL::vertices_around_face(sm.halfedge(f), sm)) {
      if (k >= 3) {
        ok = false;
        break;
      }
      tri[k++] = remap[(uint32_t)v];
    }
    if (ok && k == 3) {
      g_indices.push_back(tri[0]);
      g_indices.push_back(tri[1]);
      g_indices.push_back(tri[2]);
    }
  }

  return (int)(g_indices.size() / 3);
}

FD_EXPORT int fd_vertex_count() { return (int)(g_positions.size() / 3); }
FD_EXPORT int fd_index_count() { return (int)g_indices.size(); }
FD_EXPORT const float* fd_positions() { return g_positions.data(); }
FD_EXPORT const float* fd_normals() { return g_normals.data(); }
FD_EXPORT const uint32_t* fd_indices() { return g_indices.data(); }
