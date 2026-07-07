#!/usr/bin/env bash
# Native build of fd-cgal-mesh (server-side mesher CLI, plan §P1/M1).
#
# Header-only CGAL/Boost stack (Boost.MP instead of GMP — no dev packages
# needed), but a REAL release build: -O3 -march=native -DNDEBUG (CGAL
# assertions off) and TBB Parallel_tag (headers vendored to match the
# system libtbb12; linked via -l: against the versioned .so since
# libtbb-dev is not installed). The Docker image (M2) switches to distro
# CGAL/GMP/MPFR + TBB proper.
#
# Env overrides: CGAL_DIR, BOOST_DIR, TBB_INC, OUT.
set -euo pipefail
cd "$(dirname "$0")"

CGAL_DIR="${CGAL_DIR:-$HOME/cgal-wasm-spike/CGAL-6.0.1}"
BOOST_DIR="${BOOST_DIR:-$HOME/cgal-wasm-spike/boost_1_86_0}"
TBB_INC="${TBB_INC:-$HOME/cgal-wasm-spike/oneTBB-2021.11.0/include}"
OUT="${OUT:-build/fd-cgal-mesh}"
mkdir -p "$(dirname "$OUT")"

g++ -O3 -march=native -DNDEBUG -std=c++17 \
  -I "$CGAL_DIR/include" -I "$BOOST_DIR" -I "$TBB_INC" -I src \
  -DCGAL_DISABLE_GMP -DCGAL_USE_BOOST_MP -DCGAL_LINKED_WITH_TBB \
  -o "$OUT" src/mesher.cpp src/cli.cpp \
  -l:libtbb.so.12 -l:libtbbmalloc.so.2 -lpthread

echo "built $OUT"
