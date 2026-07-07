// THROWAWAY cross-check: verify the C++ SDF interpreter port
// (sdf_interp.h) reproduces the authoritative Rust `eval_opcodes`
// bit-for-bit-equivalently, using golden triples emitted by the Rust
// example `gen_golden`.
//
// Build & run:
//   c++ -std=c++17 -O2 -I ../src check_interp.cpp -o /tmp/check_interp
//   cargo run -p fielddraft-kernel --example gen_golden > /tmp/fd_golden.txt
//   /tmp/check_interp /tmp/fd_golden.txt
//
// Delete once the port is validated.

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <string>
#include <vector>

#include "sdf_interp.h"

int main(int argc, char** argv) {
  const char* path = argc > 1 ? argv[1] : "/tmp/fd_golden.txt";
  std::ifstream in(path);
  if (!in) {
    std::fprintf(stderr, "cannot open %s\n", path);
    return 2;
  }

  std::string tok;
  int scenes = 0;
  long checks = 0;
  double worst = 0.0;
  std::string worst_scene;
  int failures = 0;

  while (in >> tok) {
    if (tok != "SCENE") continue;
    std::string name;
    in >> name;
    std::string kw;
    // AABB values are unused here and may be "inf" (gyroid), which
    // operator>>(double) rejects and wedges the stream — read as tokens.
    std::string ab[6];
    in >> kw >> ab[0] >> ab[1] >> ab[2] >> ab[3] >> ab[4] >> ab[5];  // AABB ...
    long nops = 0;
    in >> kw >> nops;  // NOPS k
    std::vector<float> ops(nops);
    for (long i = 0; i < nops; ++i) {
      std::string op;
      uint32_t bits = 0;
      in >> op >> std::hex >> bits >> std::dec;  // OP <hex>
      float f;
      std::memcpy(&f, &bits, 4);
      ops[i] = f;
    }
    long npts = 0;
    in >> kw >> npts;  // NPTS m
    double scene_worst = 0.0;
    for (long i = 0; i < npts; ++i) {
      std::string pt;
      double x, y, z, expected;
      in >> pt >> x >> y >> z >> expected;  // PT x y z sdf
      if (std::fabs(expected) > 1e30) continue;  // skip empty-region sentinels
      float got = fd::eval_opcodes(ops.data(), (int)nops, (float)x, (float)y,
                                   (float)z);
      double err = std::fabs((double)got - expected);
      double tol = 1e-2 + 1e-3 * std::fabs(expected);
      checks++;
      if (err > scene_worst) scene_worst = err;
      if (err > worst) {
        worst = err;
        worst_scene = name;
      }
      if (err > tol) {
        if (failures < 12) {
          std::printf("  MISMATCH [%s] at (%.3f,%.3f,%.3f): got=%.5f exp=%.5f err=%.5f tol=%.5f\n",
                      name.c_str(), x, y, z, got, expected, err, tol);
        }
        failures++;
      }
    }
    std::printf("scene %-24s  worst_err=%.6f\n", name.c_str(), scene_worst);
    scenes++;
    in >> kw;  // END
  }

  std::printf("\n%d scenes, %ld checks, worst_err=%.6f (%s), %d failures\n",
              scenes, checks, worst, worst_scene.c_str(), failures);
  return failures == 0 ? 0 : 1;
}
