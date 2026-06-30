/**
 * Mate-connector gizmo — the Onshape-style CSYS widget rendered at a placed or
 * inferred mate connector: a colour-coded axis triad (X red, Y green, Z blue),
 * a circle in the X/Y rotation plane, and a translucent filled quarter sector
 * spanning the +X→+Y quadrant of that plane.
 *
 * The builder is renderer-agnostic on purpose: it uses classic THREE materials
 * (`LineBasicMaterial` / `MeshBasicMaterial`), which render under BOTH the
 * WebGPU and the classic WebGL backend in this app (the same way the datum
 * labels and the legacy connector triad already do). No node materials, no
 * per-frame `.resolution`, so the same `THREE.Group` works in every viewport.
 *
 * A connector's CSYS is handed to us as a raw `(origin, z, x)` triple (z = the
 * primary/face-normal axis, x = an in-plane reference). `orthonormalMateFrame`
 * re-orthonormalises that into a right-handed basis before drawing, so a
 * slightly non-orthogonal or non-unit input still yields a clean gizmo.
 */

import * as THREE from "three";

/** Axis + accent colours of the gizmo. Kept here (not in `tokens.css`) because
 *  these drive 3D material colours, not DOM styling. */
export const MATE_GIZMO_COLORS = {
  x: 0xe74c3c,
  y: 0x2ecc71,
  z: 0x3498db,
  /** The rotation-plane circle. */
  ring: 0x4a4a3a,
  /** The filled quarter sector between +X and +Y. */
  sector: 0xf5c542,
} as const;

/** A raw connector CSYS as the kernel hands it back: origin + primary Z axis +
 *  an in-plane X reference (need not be exactly unit or exactly ⟂ Z). */
export interface MateFrameRaw {
  origin: [number, number, number];
  /** CSYS Z-axis (primary; the face normal). */
  z: [number, number, number];
  /** CSYS X-axis (secondary, in-plane reference). */
  x: [number, number, number];
}

/** A clean right-handed frame ready to drive the gizmo's group transform. */
export interface MateGizmoFrame {
  origin: THREE.Vector3;
  x: THREE.Vector3;
  y: THREE.Vector3;
  z: THREE.Vector3;
}

const EPS = 1e-6;

/** A world axis the least aligned with `z`, to seed a fallback in-plane X when
 *  the supplied X is degenerate or parallel to Z. */
function fallbackInPlane(z: THREE.Vector3): THREE.Vector3 {
  const ax = Math.abs(z.x);
  const ay = Math.abs(z.y);
  const az = Math.abs(z.z);
  const seed =
    ax <= ay && ax <= az
      ? new THREE.Vector3(1, 0, 0)
      : ay <= az
        ? new THREE.Vector3(0, 1, 0)
        : new THREE.Vector3(0, 0, 1);
  return seed.projectOnPlane(z).normalize();
}

/**
 * Re-orthonormalise a raw `(origin, z, x)` into a right-handed `(x, y, z)`
 * frame. Z is kept as the primary axis; X is re-projected ⟂ Z; Y = Z × X.
 * Returns `null` only if Z itself is degenerate (a connector with no normal).
 */
export function orthonormalMateFrame(f: MateFrameRaw): MateGizmoFrame | null {
  const z = new THREE.Vector3(f.z[0], f.z[1], f.z[2]);
  if (z.lengthSq() < EPS) return null;
  z.normalize();

  let x = new THREE.Vector3(f.x[0], f.x[1], f.x[2]).projectOnPlane(z);
  if (x.lengthSq() < EPS) x = fallbackInPlane(z);
  else x.normalize();

  const y = new THREE.Vector3().crossVectors(z, x).normalize();
  // Re-derive X from Y×Z so the basis is exactly orthonormal.
  x.crossVectors(y, z).normalize();

  return {
    origin: new THREE.Vector3(f.origin[0], f.origin[1], f.origin[2]),
    x,
    y,
    z,
  };
}

export interface MateGizmoOptions {
  /** World-space length of the axis arms (the circle/sector use a fraction of
   *  this). Defaults to 12 mm. */
  size?: number;
  /** Preview ("ghost") look while placing — slightly more translucent. */
  ghost?: boolean;
  /** Render order base; the gizmo always draws over geometry (depthTest off). */
  renderOrder?: number;
  /** Extra Object3D layers to enable (e.g. the overlay pass). */
  layer?: number;
}

/** Tag set on every object the builder creates, so a caller can dispose the
 *  whole gizmo generically. */
const GIZMO_TAG = "__mateGizmo";

function tag(o: THREE.Object3D, layer?: number, order = 0): void {
  o.frustumCulled = false;
  o.renderOrder = order;
  (o as unknown as Record<string, unknown>)[GIZMO_TAG] = true;
  if (layer !== undefined) o.layers.enable(layer);
}

/**
 * Build the mate-connector gizmo as a `THREE.Group` already transformed to the
 * frame: the children are authored in the frame's LOCAL basis (X=+x, Y=+y,
 * Z=+z) and the group carries the world placement, so the geometry is trivial
 * and reusable. Dispose with {@link disposeMateGizmo}.
 */
export function buildMateGizmo(
  frame: MateGizmoFrame,
  opts: MateGizmoOptions = {},
): THREE.Group {
  const size = opts.size ?? 12;
  const r = size * 0.7;
  const ghost = opts.ghost ?? false;
  const baseOrder = opts.renderOrder ?? 1000;
  const layer = opts.layer;

  const group = new THREE.Group();
  group.position.copy(frame.origin);
  group.quaternion.setFromRotationMatrix(
    new THREE.Matrix4().makeBasis(frame.x, frame.y, frame.z),
  );
  tag(group, layer, baseOrder);

  const lineMat = (color: number, opacity = 1) =>
    new THREE.LineBasicMaterial({
      color,
      transparent: true,
      opacity: ghost ? opacity * 0.85 : opacity,
      depthTest: false,
      depthWrite: false,
    });

  // ── Axis triad (positive arms only, Onshape style) ──────────────
  const axes: [number, THREE.Vector3][] = [
    [MATE_GIZMO_COLORS.x, new THREE.Vector3(size, 0, 0)],
    [MATE_GIZMO_COLORS.y, new THREE.Vector3(0, size, 0)],
    [MATE_GIZMO_COLORS.z, new THREE.Vector3(0, 0, size)],
  ];
  for (const [color, end] of axes) {
    const g = new THREE.BufferGeometry();
    g.setAttribute(
      "position",
      new THREE.Float32BufferAttribute([0, 0, 0, end.x, end.y, end.z], 3),
    );
    const line = new THREE.LineSegments(g, lineMat(color));
    tag(line, layer, baseOrder + 2);
    group.add(line);
  }

  // ── Rotation-plane circle (local X/Y plane, radius r) ───────────
  {
    const seg = 48;
    const pts: number[] = [];
    for (let i = 0; i <= seg; i++) {
      const a = (i / seg) * Math.PI * 2;
      pts.push(Math.cos(a) * r, Math.sin(a) * r, 0);
    }
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", new THREE.Float32BufferAttribute(pts, 3));
    const loop = new THREE.Line(g, lineMat(MATE_GIZMO_COLORS.ring, 0.9));
    tag(loop, layer, baseOrder + 1);
    group.add(loop);
  }

  // ── Filled quarter sector spanning +X → +Y of the rotation plane ─
  {
    // CircleGeometry lies in the local X/Y plane; thetaStart=0,
    // thetaLength=π/2 fills exactly the +X→+Y quadrant.
    const g = new THREE.CircleGeometry(r, 24, 0, Math.PI / 2);
    const mat = new THREE.MeshBasicMaterial({
      color: MATE_GIZMO_COLORS.sector,
      transparent: true,
      opacity: ghost ? 0.3 : 0.45,
      depthTest: false,
      depthWrite: false,
      side: THREE.DoubleSide,
    });
    const sector = new THREE.Mesh(g, mat);
    tag(sector, layer, baseOrder);
    group.add(sector);
  }

  return group;
}

/** Dispose every geometry + material the gizmo allocated and clear the group. */
export function disposeMateGizmo(group: THREE.Group): void {
  group.traverse((o) => {
    const mesh = o as THREE.Mesh | THREE.Line | THREE.LineSegments;
    if ("geometry" in mesh && mesh.geometry) mesh.geometry.dispose();
    const mat = (mesh as { material?: THREE.Material | THREE.Material[] })
      .material;
    if (Array.isArray(mat)) mat.forEach((m) => m.dispose());
    else if (mat) mat.dispose();
  });
  group.clear();
}
