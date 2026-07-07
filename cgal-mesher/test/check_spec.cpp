// check_spec — value-exactness harness for the cell-local tape
// specialization (tape_spec.h). Reads a .fdmesh interchange file, builds
// the spec octree over the (padded) AABB of every body and compares the
// specialized evaluation against the full tape on a dense random point
// set. Any deviation beyond float slop is a pruning bug.
//
//   g++ -O2 -std=c++17 -I src -o check_spec test/check_spec.cpp
//   ./check_spec job.fdmesh

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <sstream>
#include <string>
#include <vector>

#include "../src/sdf_interp.h"
#include "../src/tape_spec.h"

namespace {

float hex_f32(const std::string& tok) {
  uint32_t bits = (uint32_t)std::stoul(tok, nullptr, 16);
  float v;
  std::memcpy(&v, &bits, 4);
  return v;
}

// xorshift — deterministic point set.
uint32_t rng_state = 0x12345678u;
float frand() {
  rng_state ^= rng_state << 13;
  rng_state ^= rng_state >> 17;
  rng_state ^= rng_state << 5;
  return (float)(rng_state & 0xFFFFFF) / (float)0xFFFFFF;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 2) {
    std::fprintf(stderr, "usage: check_spec <job.fdmesh>\n");
    return 2;
  }
  std::ifstream in(argv[1]);
  if (!in) {
    std::fprintf(stderr, "cannot open %s\n", argv[1]);
    return 2;
  }

  std::string line;
  int body = -1;
  float aabb[6] = {0, 0, 0, 0, 0, 0};
  std::vector<float> ops;
  float worst = 0.0f;
  long checked = 0;
  int bodies = 0;

  auto flush_body = [&]() {
    if (ops.empty()) return;
    ++bodies;
    const float ex = aabb[3] - aabb[0], ey = aabb[4] - aabb[1],
                ez = aabb[5] - aabb[2];
    const float diag = std::sqrt(ex * ex + ey * ey + ez * ez);
    const float pad = 0.05f * diag + 1.0f;
    float mn[3] = {aabb[0] - pad, aabb[1] - pad, aabb[2] - pad};
    float mx[3] = {aabb[3] + pad, aabb[4] + pad, aabb[5] + pad};
    fd_spec::SpecOctree oct = fd_spec::build_spec_octree(
        ops.data(), (int)ops.size(), mn, mx, 6);
    if (!oct.root) {
      std::printf("body %d: tape not specializable (ok, full-tape fallback)\n",
                  body);
      return;
    }
    for (int i = 0; i < 20000; ++i) {
      float p[3] = {mn[0] + (mx[0] - mn[0]) * frand(),
                    mn[1] + (mx[1] - mn[1]) * frand(),
                    mn[2] + (mx[2] - mn[2]) * frand()};
      float full = fd::eval_opcodes(ops.data(), (int)ops.size(), p[0], p[1],
                                    p[2]);
      const std::vector<float>* tape = oct.lookup(p);
      if (!tape) continue;
      float spec = fd::eval_opcodes(tape->data(), (int)tape->size(), p[0],
                                    p[1], p[2]);
      float err = std::fabs(full - spec);
      if (err > worst) worst = err;
      ++checked;
    }
    std::printf("body %d: %ld cells, leaf avg %.0f records (full %ld)\n",
                body, oct.cells,
                oct.cells ? (double)oct.leaf_records / (double)oct.cells : 0.0,
                oct.full_records);
  };

  while (std::getline(in, line)) {
    std::istringstream ls(line);
    std::string tag;
    ls >> tag;
    if (tag == "BODY") {
      flush_body();
      ops.clear();
      ls >> body;
    } else if (tag == "AABB") {
      std::string t;
      for (int i = 0; i < 6 && ls >> t; ++i) aabb[i] = hex_f32(t);
    } else if (tag == "OP") {
      std::string t;
      ls >> t;
      ops.push_back(hex_f32(t));
    } else if (tag == "END") {
      break;
    }
  }
  flush_body();

  std::printf("%d bodies, %ld points, worst |full-spec| = %.9f\n", bodies,
              checked, worst);
  if (worst > 1e-4f) {
    std::printf("FAIL: specialization is not value-exact\n");
    return 1;
  }
  std::printf("PASS\n");
  return 0;
}
