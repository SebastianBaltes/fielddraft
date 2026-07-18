// Cell-local opcode-tape specialization — the C++ mirror of the kernel's
// `cell_csg.rs` (fidget-style tape shortening, ported to the flat opcode
// stream). CGAL Mesh_3 makes O(10⁸) point oracle calls; on an N-part
// assembly every call walks all N parts. This module builds an octree over
// the meshing AABB and, per cell, prunes boolean branches that provably
// cannot influence the field inside the cell (ONE center eval per subtree
// + a 1-Lipschitz ball bound), inheriting the shortened tape to child
// cells. The oracle then evaluates the cell's short tape instead of the
// full one.
//
// Pruning is VALUE-EXACT under the ball bound: min(a,b) collapses to `a`
// in a cell only when a_hi < b_lo there (resp. difference/intersection;
// smooth ops widen the gap by the blend radius). The compiled tape is
// conservative-1-Lipschitz by construction (the kernel bakes all Lipschitz
// normalisation into the stream — sphere tracing runs it with relax 1.0),
// so |d(x) − d(c)| ≤ |x − c| holds per subtree value.
//
// Scope: specialization looks at the boolean spine OUTSIDE warp brackets
// (WARP_PUSH…WARP_POP) — warped subtrees (extrude/revolve/helix/sweep)
// stay opaque leaf spans, exactly like assemblies compose them. Everything
// unknown collapses conservatively into leaf spans, so any tape shape is
// safe; only recognized boolean nodes are ever pruned.

#ifndef FD_TAPE_SPEC_H
#define FD_TAPE_SPEC_H

#include <cstdint>
#include <cstdio>
#include <memory>
#include <vector>

#include "sdf_interp.h"

namespace fd_spec {

// ── Tape walking ───────────────────────────────────────────────────

// Payload length (floats AFTER the tag) of one op at `pc` (tag already
// consumed). Count-prefixed ops read their count from the stream.
// Returns -1 for unknown tags (caller falls back to no specialization).
inline int payload_len(const float* buf, int n, uint32_t tag, int pc) {
  using namespace fd;
  switch (tag) {
    case OP_PRIM_BOX: return 13;
    case OP_PRIM_SPHERE: return 11;
    case OP_PRIM_CYLINDER: return 12;
    case OP_PRIM_TORUS: return 12;
    case OP_PRIM_CONE: return 13;
    case OP_PRIM_CAPSULE: return 12;
    case OP_PRIM_HEX_PRISM: return 12;
    case OP_PRIM_ELLIPSOID: return 13;
    case OP_PRIM_CONST: return 1;
    case OP_PRIM_GYROID: return 12;
    case OP_PRIM_THREAD: return 14;
    case OP_PRIM_TEX3D: return 17;
    case OP_PRIM_HALFSPACE: return 15;
    case OP_PRIM_CUSTOM: {
      if (pc + 14 >= n) return -1;
      int nrec = (int)buf[pc + 14];
      return 15 + 2 * nrec;
    }
    case OP_PRIM2D_CIRCLE: return 3;
    case OP_PRIM2D_RECT: return 4;
    case OP_PRIM2D_POLY: {
      if (pc >= n) return -1;
      int np = (int)buf[pc];
      return 1 + 2 * np;
    }
    case OP_PRIM2D_SECTOR: return 5;
    case OP_PRIM2D_PATH: {
      if (pc + 1 >= n) return -1;
      int nrec = (int)buf[pc];
      return 2 + 7 * nrec;
    }
    case OP_UNION:
    case OP_DIFFERENCE:
    case OP_INTERSECT: return 0;
    case OP_SMOOTH_UNION:
    case OP_SMOOTH_DIFF: return 2;
    case OP_SHELL: return 1;
    case OP_MORPH: return 1;
    case OP_DISPLACE_VAL: return 2;
    case OP_EXTRUDE: return 3;
    case OP_REVOLVE_SCALE: return 1;
    case OP_OFFSET_2D: return 1;
    case OP_NEG: return 0;
    case OP_WARP_PUSH: return 0;
    case OP_WARP_POP: return 0;
    case OP_WARP_TWIST: return 1;
    case OP_WARP_BEND: return 1;
    case OP_WARP_TRANSFORM: return 10;
    case OP_WARP_REVOLVE: return 0;
    case OP_WARP_HELIX: return 7;
    case OP_HELIX_SCALE: return 1;
    case OP_WARP_SWEEP: {
      if (pc >= n) return -1;
      int np = (int)buf[pc];
      return 4 + 10 * np;
    }
    case OP_MASKED_BLEND: {
      if (pc >= n) return -1;
      int np = (int)buf[pc];
      return 1 + 3 * np + 4;
    }
    case OP_ARC_CHANNEL: {
      if (pc >= n) return -1;
      int np = (int)buf[pc];
      return 9 + 27 * np;
    }
    case OP_END: return 0;
    default: return -1;
  }
}

// Value-stack pops of one op (pushes are pops+delta; every value op nets
// exactly one push except pure warps, which net zero).
inline int value_pops(uint32_t tag) {
  using namespace fd;
  switch (tag) {
    case OP_UNION:
    case OP_DIFFERENCE:
    case OP_INTERSECT:
    case OP_SMOOTH_UNION:
    case OP_SMOOTH_DIFF:
    case OP_MORPH:
    case OP_MASKED_BLEND: return 2;
    case OP_SHELL:
    case OP_ARC_CHANNEL:
    case OP_DISPLACE_VAL:
    case OP_EXTRUDE:
    case OP_REVOLVE_SCALE:
    case OP_OFFSET_2D:
    case OP_NEG:
    case OP_HELIX_SCALE: return 1;
    default: return 0;  // leaf primitives push, warps neither
  }
}

inline bool is_warp(uint32_t tag) {
  using namespace fd;
  return tag == OP_WARP_PUSH || tag == OP_WARP_POP || tag == OP_WARP_TWIST ||
         tag == OP_WARP_BEND || tag == OP_WARP_TRANSFORM ||
         tag == OP_WARP_REVOLVE || tag == OP_WARP_HELIX ||
         tag == OP_WARP_SWEEP;
}

inline bool pushes_value(uint32_t tag) {
  return !is_warp(tag) && tag != fd::OP_END;
}

// ── Node tree (boolean spine over opaque leaf spans) ───────────────

struct Node {
  enum Kind { LEAF, UNION, DIFFERENCE, INTERSECT, SUNION, SDIFF } kind = LEAF;
  int start = 0, end = 0;     // record span in the ORIGINAL buffer
  float r = 0.0f;             // blend radius (smooth ops)
  int left = -1, right = -1;  // child node indices (boolean kinds)
};

struct Tree {
  std::vector<Node> nodes;
  int root = -1;
  bool ok = false;
};

inline int subtree_start(const struct Tree& t, int i);

// Parse the tape into a boolean tree. Anything that is not a plain
// boolean at warp depth 0 collapses the affected operands into one LEAF
// span (conservative — never pruned, always evaluated verbatim).
inline Tree parse_tape(const float* buf, int n) {
  Tree t;
  std::vector<int> stack;  // node indices
  int pc = 0;
  int warp_depth = 0;
  int warp_start = -1;
  int warp_stack_base = 0;
  while (pc < n) {
    uint32_t tag = fd::bits_of(buf[pc]);
    int body = payload_len(buf, n, tag, pc + 1);
    if (body < 0) return t;  // unknown op → no specialization
    int op_start = pc;
    pc += 1 + body;
    if (tag == fd::OP_END) break;

    if (tag == fd::OP_WARP_PUSH) {
      if (warp_depth == 0) {
        warp_start = op_start;
        warp_stack_base = (int)stack.size();
      }
      ++warp_depth;
      continue;
    }
    if (tag == fd::OP_WARP_POP) {
      --warp_depth;
      if (warp_depth < 0) return t;
      if (warp_depth == 0) {
        // The whole bracket becomes one leaf producing ONE value; any
        // values produced inside belong to it.
        stack.resize(warp_stack_base);
        Node leaf;
        leaf.kind = Node::LEAF;
        leaf.start = warp_start;
        leaf.end = pc;
        t.nodes.push_back(leaf);
        stack.push_back((int)t.nodes.size() - 1);
      }
      continue;
    }
    if (warp_depth > 0) continue;  // inside a bracket — swallowed above

    int pops = value_pops(tag);
    if ((int)stack.size() < pops) return t;

    Node::Kind bk;
    bool boolean = true;
    switch (tag) {
      case fd::OP_UNION: bk = Node::UNION; break;
      case fd::OP_DIFFERENCE: bk = Node::DIFFERENCE; break;
      case fd::OP_INTERSECT: bk = Node::INTERSECT; break;
      case fd::OP_SMOOTH_UNION: bk = Node::SUNION; break;
      case fd::OP_SMOOTH_DIFF: bk = Node::SDIFF; break;
      default: boolean = false; bk = Node::LEAF; break;
    }

    if (boolean) {
      Node nn;
      nn.kind = bk;
      nn.right = stack.back();
      stack.pop_back();
      nn.left = stack.back();
      stack.pop_back();
      nn.start = op_start;  // span of the OP RECORD itself
      nn.end = pc;
      nn.r = (bk == Node::SUNION || bk == Node::SDIFF) ? buf[op_start + 1] : 0.0f;
      t.nodes.push_back(nn);
      stack.push_back((int)t.nodes.size() - 1);
    } else if (pops == 0) {
      Node leaf;
      leaf.kind = Node::LEAF;
      leaf.start = op_start;
      leaf.end = pc;
      t.nodes.push_back(leaf);
      stack.push_back((int)t.nodes.size() - 1);
    } else {
      // Non-boolean consumer (shell/morph/offset/…): merge the consumed
      // operands + this record into one opaque leaf. Operand spans are
      // contiguous by construction (postfix stream).
      int first = (int)stack.size() - pops;
      int span_start = t.nodes[stack[first]].start;
      // A leaf's span must start at the FIRST contributing record; child
      // node spans of merged booleans already cover their subtrees.
      span_start = std::min(span_start, subtree_start(t, stack[first]));
      stack.resize(first);
      Node leaf;
      leaf.kind = Node::LEAF;
      leaf.start = span_start;
      leaf.end = pc;
      t.nodes.push_back(leaf);
      stack.push_back((int)t.nodes.size() - 1);
    }
  }
  if (stack.size() != 1) return t;
  t.root = stack.back();
  t.ok = true;
  return t;
}

// Earliest record index contributing to node i (leaf: its own start;
// boolean: the leftmost descendant's start — operands precede the op).
inline int subtree_start(const Tree& t, int i) {
  const Node& nd = t.nodes[i];
  if (nd.kind == Node::LEAF) return nd.start;
  return subtree_start(t, nd.left);
}

// ── Specialization ─────────────────────────────────────────────────

// Emit node i's records (in postfix order) into `out`.
inline void emit_subtree(const Tree& t, int i, const float* buf,
                         std::vector<float>& out) {
  const Node& nd = t.nodes[i];
  if (nd.kind == Node::LEAF) {
    out.insert(out.end(), buf + nd.start, buf + nd.end);
    return;
  }
  emit_subtree(t, nd.left, buf, out);
  emit_subtree(t, nd.right, buf, out);
  out.insert(out.end(), buf + nd.start, buf + nd.end);
}

// Evaluate node i of the tree at a point by materializing is too slow —
// instead evaluate the subtree via a scratch tape reused across calls.
struct SpecCtx {
  const float* buf = nullptr;
  int n = 0;
  Tree tree;
  std::vector<float> scratch;
  long evals = 0;

  float eval_node(int i, const float p[3]) {
    ++evals;
    const Node& nd = tree.nodes[i];
    if (nd.kind == Node::LEAF) {
      return fd::eval_opcodes(buf + nd.start, nd.end - nd.start, p[0], p[1],
                              p[2]);
    }
    scratch.clear();
    emit_subtree(tree, i, buf, scratch);
    return fd::eval_opcodes(scratch.data(), (int)scratch.size(), p[0], p[1],
                            p[2]);
  }
};

// Specialized form of a node inside a cell: either the node itself, one
// of its children, or (after recursion) a rebuilt subtree. Represented as
// a small tree of "kept" entries referencing original node ids.
struct Kept {
  int node;                  // original node id
  int left = -1, right = -1; // Kept indices (boolean) or -1 (leaf/opaque)
  bool as_leaf = false;      // true → emit verbatim, no recursion happened
};

struct SpecResult {
  std::vector<Kept> kept;
  int root = -1;
  int leaf_count = 0;
};

// Ball-bound specialization of node i for a cell with center c and
// radius rad (half-diagonal). SAFETY: 1-Lipschitz per subtree value +
// 5% slack on the radius.
inline int specialize_node(SpecCtx& cx, int i, const float c[3], float rad,
                           SpecResult& out) {
  const Node& nd = cx.tree.nodes[i];
  if (nd.kind == Node::LEAF) {
    out.kept.push_back({i, -1, -1, true});
    ++out.leaf_count;
    return (int)out.kept.size() - 1;
  }
  const float R = rad * 1.05f;
  float dl = cx.eval_node(nd.left, c);
  float dr = cx.eval_node(nd.right, c);
  float margin = (nd.kind == Node::SUNION || nd.kind == Node::SDIFF)
                     ? nd.r
                     : 0.0f;
  bool drop_right = false, drop_left = false;
  switch (nd.kind) {
    case Node::UNION:
    case Node::SUNION:
      // min: the loser is provably above the winner everywhere in cell.
      drop_right = dr - R > dl + R + margin;
      drop_left = dl - R > dr + R + margin;
      break;
    case Node::INTERSECT:
      drop_right = dr + R < dl - R - margin;
      drop_left = dl + R < dr - R - margin;
      break;
    case Node::DIFFERENCE:
    case Node::SDIFF:
      // max(a, −b): drop b when −b is provably below a (b far outside).
      drop_right = -(dr - R) < dl - R - margin;
      // never drop the left operand of a difference (result would be −b).
      break;
    default: break;
  }
  if (drop_right) return specialize_node(cx, nd.left, c, rad, out);
  if (drop_left && (nd.kind == Node::UNION || nd.kind == Node::SUNION ||
                    nd.kind == Node::INTERSECT)) {
    return specialize_node(cx, nd.right, c, rad, out);
  }
  int kl = specialize_node(cx, nd.left, c, rad, out);
  int kr = specialize_node(cx, nd.right, c, rad, out);
  out.kept.push_back({i, kl, kr, false});
  return (int)out.kept.size() - 1;
}

inline void emit_kept(const SpecCtx& cx, const SpecResult& sr, int ki,
                      std::vector<float>& out) {
  const Kept& k = sr.kept[ki];
  const Node& nd = cx.tree.nodes[k.node];
  if (k.as_leaf || nd.kind == Node::LEAF) {
    emit_subtree(cx.tree, k.node, cx.buf, out);
    return;
  }
  emit_kept(cx, sr, k.left, out);
  emit_kept(cx, sr, k.right, out);
  out.insert(out.end(), cx.buf + nd.start, cx.buf + nd.end);
}

// ── Octree over the meshing AABB ───────────────────────────────────

struct Cell {
  float min[3], max[3];
  std::vector<float> tape;     // specialized tape (leaf cells)
  std::unique_ptr<Cell> kids[8];
  bool is_leaf = true;
};

struct SpecOctree {
  std::unique_ptr<Cell> root;
  long cells = 0;
  long leaf_records = 0;
  long full_records = 0;

  const std::vector<float>* lookup(const float p[3]) const {
    const Cell* c = root.get();
    if (!c) return nullptr;
    for (int k = 0; k < 3; ++k) {
      if (p[k] < c->min[k] || p[k] > c->max[k]) return nullptr;
    }
    while (!c->is_leaf) {
      float mid[3] = {0.5f * (c->min[0] + c->max[0]),
                      0.5f * (c->min[1] + c->max[1]),
                      0.5f * (c->min[2] + c->max[2])};
      int idx = (p[0] > mid[0] ? 1 : 0) | (p[1] > mid[1] ? 2 : 0) |
                (p[2] > mid[2] ? 4 : 0);
      c = c->kids[idx].get();
    }
    return &c->tape;
  }
};

inline void build_cell(SpecCtx& cx, Cell* cell, const float* tape, int n,
                       int depth, int max_depth, SpecOctree& oct) {
  ++oct.cells;
  // Parse THIS cell's inherited tape and specialize it for the cell ball.
  SpecCtx local;
  local.buf = tape;
  local.n = n;
  local.tree = parse_tape(tape, n);
  if (!local.tree.ok) {
    cell->tape.assign(tape, tape + n);
    oct.leaf_records += n;
    return;
  }
  float c[3] = {0.5f * (cell->min[0] + cell->max[0]),
                0.5f * (cell->min[1] + cell->max[1]),
                0.5f * (cell->min[2] + cell->max[2])};
  float ex = cell->max[0] - cell->min[0], ey = cell->max[1] - cell->min[1],
        ez = cell->max[2] - cell->min[2];
  float rad = 0.5f * std::sqrt(ex * ex + ey * ey + ez * ez);
  SpecResult sr;
  sr.root = specialize_node(local, sr.root >= 0 ? sr.root : local.tree.root, c,
                            rad, sr);
  cx.evals += local.evals;
  std::vector<float> spec;
  emit_kept(local, sr, sr.root, spec);

  // Recurse while the tape is still long and depth remains.
  if (depth < max_depth && sr.leaf_count > 2) {
    cell->is_leaf = false;
    float mid[3] = {c[0], c[1], c[2]};
    for (int i = 0; i < 8; ++i) {
      auto kid = std::make_unique<Cell>();
      for (int k = 0; k < 3; ++k) {
        bool hi = (i >> k) & 1;
        kid->min[k] = hi ? mid[k] : cell->min[k];
        kid->max[k] = hi ? cell->max[k] : mid[k];
      }
      build_cell(cx, kid.get(), spec.data(), (int)spec.size(), depth + 1,
                 max_depth, oct);
      cell->kids[i] = std::move(kid);
    }
    return;
  }
  oct.leaf_records += (long)spec.size();
  cell->tape = std::move(spec);
}

// Build the specialization octree over the (padded) meshing AABB.
inline SpecOctree build_spec_octree(const float* opcodes, int n,
                                    const float mn[3], const float mx[3],
                                    int max_depth) {
  SpecOctree oct;
  oct.full_records = n;
  Tree probe = parse_tape(opcodes, n);
  if (!probe.ok) return oct;  // unknown shape → stay on the full tape
  SpecCtx cx;
  oct.root = std::make_unique<Cell>();
  for (int k = 0; k < 3; ++k) {
    oct.root->min[k] = mn[k];
    oct.root->max[k] = mx[k];
  }
  build_cell(cx, oct.root.get(), opcodes, n, 0, max_depth, oct);
  return oct;
}

}  // namespace fd_spec

#endif  // FD_TAPE_SPEC_H
