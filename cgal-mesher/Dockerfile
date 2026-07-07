# fd-mesher — server-side CGAL meshing service (plan §P3 / M2).
# Build context is the REPO ROOT (fd-prepare needs the Rust workspace):
#   docker build -f cgal-mesher/Dockerfile .
#
# Three stages: Rust (fd-prepare), C++ (fd-cgal-mesh against distro
# CGAL/GMP/MPFR/TBB, Parallel_tag), Go (HTTP wrapper) → slim runtime.

# ── Stage 1: fd-prepare (Rust kernel binary) ──
FROM rust:1-trixie AS rust-build
WORKDIR /src
COPY Cargo.toml ./
COPY crates ./crates
RUN cargo build --release -p fielddraft-kernel --bin fd-prepare
# Golden vectors for the interpreter-sync check in the C++ stage.
RUN cargo run --release -p fielddraft-kernel --example gen_golden > /tmp/fd_golden.txt

# ── Stage 2: fd-cgal-mesh (CGAL Mesh_3, GMP/MPFR + TBB Parallel_tag) ──
FROM debian:trixie AS cpp-build
RUN apt-get update && apt-get install -y --no-install-recommends \
    g++ libcgal-dev libgmp-dev libmpfr-dev libtbb-dev libboost-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY cgal-mesher/src ./src
COPY cgal-mesher/test ./test
# Golden interpreter check is a MANDATORY build step: any Rust↔C++ SDF
# interpreter drift must fail the image build (plan §4).
COPY --from=rust-build /tmp/fd_golden.txt /tmp/fd_golden.txt
RUN g++ -O2 -std=c++17 -I src -o /tmp/check_interp test/check_interp.cpp \
    && /tmp/check_interp /tmp/fd_golden.txt
RUN g++ -O3 -DNDEBUG -std=c++17 -I src \
    -DCGAL_LINKED_WITH_TBB \
    -o /usr/local/bin/fd-cgal-mesh src/mesher.cpp src/cli.cpp \
    -ltbb -ltbbmalloc -lgmp -lmpfr -lpthread

# ── Stage 3: HTTP wrapper ──
FROM golang:1.26-trixie AS go-build
WORKDIR /src
COPY cgal-mesher/serve ./
RUN CGO_ENABLED=0 go build -o /fd-mesher-serve .

# ── Runtime ──
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    libgmp10 libmpfr6 libtbb12 python3-minimal \
    && rm -rf /var/lib/apt/lists/*
COPY --from=rust-build /src/target/release/fd-prepare /usr/local/bin/fd-prepare
COPY --from=cpp-build /usr/local/bin/fd-cgal-mesh /usr/local/bin/fd-cgal-mesh
COPY --from=go-build /fd-mesher-serve /usr/local/bin/fd-mesher-serve
COPY cgal-mesher/test/verify_stl.py /usr/local/share/fd/verify_stl.py
EXPOSE 9090
CMD ["fd-mesher-serve"]
