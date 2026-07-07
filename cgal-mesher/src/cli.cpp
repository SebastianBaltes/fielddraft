// fd-cgal-mesh — native CLI of the server-side CGAL mesher
// (docs/CGAL-SERVER-MESHING-PLAN.md, P1).
//
// Usage:  fd-cgal-mesh <job.fdmesh> <out.stl>
//
// Reads the interchange file emitted by `fd-prepare` (FDMESH 1: PARAMS
// json, then per body the SDF opcode program as bit-exact f32 hex, the
// world AABB and the protected B-Rep edge polylines), meshes every body
// via the fd_* API in mesher.cpp (CGAL Mesh_3, implicit domain + polyline
// feature protection), accumulates all bodies into one binary STL and
// prints a small JSON report to stderr.
//
// Exit codes: 0 ok, 1 mesh failure (a body yielded no triangles),
// 2 usage/parse error.

#include <cctype>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <sstream>
#include <string>
#include <vector>

// mesher.cpp API (linked in the same binary).
extern "C" {
void fd_reset();
void fd_add_edge(const float* pts, int n);
int fd_mesh(const float* opcodes, int n_ops, float minx, float miny,
            float minz, float maxx, float maxy, float maxz, int resolution,
            float facet_dist_mult);
int fd_vertex_count();
int fd_index_count();
const float* fd_positions();
const uint32_t* fd_indices();
}

namespace {

float hex_f32(const std::string& tok) {
  uint32_t bits = (uint32_t)std::stoul(tok, nullptr, 16);
  float v;
  std::memcpy(&v, &bits, 4);
  return v;
}

// Minimal numeric extractor for the flat PARAMS json — full JSON parsing
// is not needed for "key": number pairs.
bool json_number(const std::string& json, const char* key, double* out) {
  std::string pat = std::string("\"") + key + "\"";
  size_t k = json.find(pat);
  if (k == std::string::npos) return false;
  k = json.find(':', k + pat.size());
  if (k == std::string::npos) return false;
  ++k;
  while (k < json.size() && std::isspace((unsigned char)json[k])) ++k;
  try {
    *out = std::stod(json.substr(k));
  } catch (...) {
    return false;
  }
  return true;
}

struct Body {
  float aabb[6] = {0, 0, 0, 0, 0, 0};
  std::vector<float> opcodes;
  std::vector<std::vector<float>> edges;  // flat xyz per polyline
};

}  // namespace

int main(int argc, char** argv) {
  if (argc != 3) {
    std::fprintf(stderr, "usage: fd-cgal-mesh <job.fdmesh> <out.stl>\n");
    return 2;
  }
  std::ifstream in(argv[1]);
  if (!in) {
    std::fprintf(stderr, "fd-cgal-mesh: cannot open %s\n", argv[1]);
    return 2;
  }

  std::string params_json = "{}";
  std::vector<Body> bodies;
  Body* cur = nullptr;

  std::string line;
  while (std::getline(in, line)) {
    std::istringstream ls(line);
    std::string tag;
    ls >> tag;
    if (tag == "FDMESH") {
      int ver = 0;
      ls >> ver;
      if (ver != 1) {
        std::fprintf(stderr, "fd-cgal-mesh: unsupported FDMESH version %d\n", ver);
        return 2;
      }
    } else if (tag == "PARAMS") {
      std::getline(ls, params_json);
    } else if (tag == "BODY") {
      bodies.emplace_back();
      cur = &bodies.back();
    } else if (tag == "AABB" && cur) {
      std::string t;
      for (int i = 0; i < 6 && ls >> t; ++i) cur->aabb[i] = hex_f32(t);
    } else if (tag == "NOPS" && cur) {
      size_t n = 0;
      ls >> n;
      cur->opcodes.reserve(n);
    } else if (tag == "OP" && cur) {
      std::string t;
      ls >> t;
      cur->opcodes.push_back(hex_f32(t));
    } else if (tag == "EDGE" && cur) {
      size_t n = 0;
      ls >> n;
      std::vector<float> pts;
      pts.reserve(n * 3);
      std::string t;
      while (ls >> t) pts.push_back(hex_f32(t));
      if (pts.size() == n * 3 && n >= 2) cur->edges.push_back(std::move(pts));
    } else if (tag == "END") {
      break;
    }  // NBODIES / NEDGES are informational — counts are implicit.
  }
  if (bodies.empty()) {
    std::fprintf(stderr, "fd-cgal-mesh: no bodies in %s\n", argv[1]);
    return 2;
  }

  double cell_mm = 0.5, facet_dist_mult = 0.15;
  json_number(params_json, "cellSizeMm", &cell_mm);
  json_number(params_json, "facetDistMult", &facet_dist_mult);
  if (!(cell_mm > 0.0)) cell_mm = 0.5;

  // Accumulate all bodies into one soup (STL is index-free anyway).
  std::vector<float> all_pos;      // 3 per vertex
  std::vector<uint32_t> all_idx;   // 3 per triangle
  long total_edges = 0;

  for (size_t bi = 0; bi < bodies.size(); ++bi) {
    const Body& b = bodies[bi];
    const float ex = b.aabb[3] - b.aabb[0];
    const float ey = b.aabb[4] - b.aabb[1];
    const float ez = b.aabb[5] - b.aabb[2];
    const float diag = std::sqrt(ex * ex + ey * ey + ez * ez);
    if (!(diag > 0.0f) || b.opcodes.empty()) continue;
    int resolution = (int)std::ceil(diag / (float)cell_mm);
    if (resolution < 8) resolution = 8;
    if (resolution > 4096) resolution = 4096;

    fd_reset();
    for (const auto& e : b.edges) {
      fd_add_edge(e.data(), (int)(e.size() / 3));
      ++total_edges;
    }
    int tris = fd_mesh(b.opcodes.data(), (int)b.opcodes.size(), b.aabb[0],
                       b.aabb[1], b.aabb[2], b.aabb[3], b.aabb[4], b.aabb[5],
                       resolution, (float)facet_dist_mult);
    if (tris <= 0) {
      std::fprintf(stderr,
                   "{\"error\":\"body %zu meshed to zero triangles\"}\n", bi);
      return 1;
    }
    const uint32_t base = (uint32_t)(all_pos.size() / 3);
    const int nv = fd_vertex_count();
    const int ni = fd_index_count();
    all_pos.insert(all_pos.end(), fd_positions(), fd_positions() + nv * 3);
    // CGAL's extracted surface is consistently oriented but not
    // deterministically OUTWARD — flip the whole body if its signed
    // volume is negative.
    const float* bp = fd_positions();
    const uint32_t* bidx = fd_indices();
    double vol6 = 0.0;
    for (int i = 0; i < ni; i += 3) {
      const float* a = bp + bidx[i] * 3;
      const float* b2 = bp + bidx[i + 1] * 3;
      const float* c = bp + bidx[i + 2] * 3;
      vol6 += (double)a[0] * ((double)b2[1] * c[2] - (double)b2[2] * c[1]) -
              (double)a[1] * ((double)b2[0] * c[2] - (double)b2[2] * c[0]) +
              (double)a[2] * ((double)b2[0] * c[1] - (double)b2[1] * c[0]);
    }
    const bool flip = vol6 < 0.0;
    for (int i = 0; i < ni; i += 3) {
      all_idx.push_back(base + bidx[i]);
      all_idx.push_back(base + bidx[i + (flip ? 2 : 1)]);
      all_idx.push_back(base + bidx[i + (flip ? 1 : 2)]);
    }
  }
  if (all_idx.empty()) {
    std::fprintf(stderr, "{\"error\":\"no geometry produced\"}\n");
    return 1;
  }

  // ── Binary STL ──
  std::ofstream out(argv[2], std::ios::binary);
  if (!out) {
    std::fprintf(stderr, "fd-cgal-mesh: cannot write %s\n", argv[2]);
    return 2;
  }
  char header[80] = "FieldDraft CGAL Mesh_3";
  out.write(header, 80);
  uint32_t ntri = (uint32_t)(all_idx.size() / 3);
  out.write((const char*)&ntri, 4);
  for (uint32_t t = 0; t < ntri; ++t) {
    const float* a = &all_pos[all_idx[t * 3 + 0] * 3];
    const float* b = &all_pos[all_idx[t * 3 + 1] * 3];
    const float* c = &all_pos[all_idx[t * 3 + 2] * 3];
    float u[3] = {b[0] - a[0], b[1] - a[1], b[2] - a[2]};
    float v[3] = {c[0] - a[0], c[1] - a[1], c[2] - a[2]};
    float n[3] = {u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2],
                  u[0] * v[1] - u[1] * v[0]};
    float len = std::sqrt(n[0] * n[0] + n[1] * n[1] + n[2] * n[2]);
    if (len > 1e-20f) {
      n[0] /= len;
      n[1] /= len;
      n[2] /= len;
    }
    out.write((const char*)n, 12);
    out.write((const char*)a, 12);
    out.write((const char*)b, 12);
    out.write((const char*)c, 12);
    uint16_t attr = 0;
    out.write((const char*)&attr, 2);
  }
  out.close();

  std::fprintf(stderr,
               "{\"bodies\":%zu,\"edges\":%ld,\"vertices\":%zu,"
               "\"triangles\":%u,\"cellSizeMm\":%g,\"facetDistMult\":%g}\n",
               bodies.size(), total_edges, all_pos.size() / 3, ntri, cell_mm,
               facet_dist_mult);
  return 0;
}
