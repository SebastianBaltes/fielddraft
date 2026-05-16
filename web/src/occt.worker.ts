/// <reference lib="webworker" />
/**
 * M14c/M14i: OCCT STEP export + import worker.
 *
 * Runs OpenCASCADE Technology (via opencascade.js WASM) in a dedicated
 * worker thread. The main thread sends either:
 *   - a StepCommand[] pipeline for export, or
 *   - a STEP file (Uint8Array) for import/tessellation.
 *
 * The ~25 MB OCCT WASM module is loaded lazily on the first request
 * — it is NOT part of the initial app bundle. This keeps the dev
 * build fast and the initial page load unaffected.
 *
 * Wire format:
 *
 *   main → worker : { type: "export-step", commands: StepCommand[] }
 *                  | { type: "import-step", data: Uint8Array }
 *   worker → main : { type: "step-result", data: Uint8Array }
 *                  | { type: "step-error", message: string }
 *                  | { type: "step-progress", phase: string }
 *                  | { type: "import-result", positions: Float32Array, indices: Uint32Array }
 */

// ─── Command protocol ──────────────────────────────────────────
//
// The command list is a linearized CSG build sequence. The kernel's
// step_classifier produces it from the feature tree; each command
// references shapes by string handles.

export type StepCommand =
  | { op: "box"; handle: string; cx: number; cy: number; cz: number; hx: number; hy: number; hz: number }
  | { op: "sphere"; handle: string; cx: number; cy: number; cz: number; radius: number }
  | { op: "cylinder"; handle: string; cx: number; cy: number; cz: number; radius: number; halfHeight: number }
  | { op: "cone"; handle: string; cx: number; cy: number; cz: number; rBottom: number; rTop: number; halfHeight: number }
  | { op: "torus"; handle: string; cx: number; cy: number; cz: number; majorRadius: number; minorRadius: number }
  | { op: "hexprism"; handle: string; apothem: number; halfHeight: number }
  | { op: "extrude"; handle: string; wirePts: number[]; depth: number; cx: number; cy: number; cz: number }
  | { op: "revolve"; handle: string; wirePts: number[]; angle: number; cx: number; cy: number; cz: number }
  | { op: "union"; handle: string; a: string; b: string }
  | { op: "difference"; handle: string; a: string; b: string }
  | { op: "intersect"; handle: string; a: string; b: string }
  | { op: "transform"; handle: string; source: string; tx: number; ty: number; tz: number; qx: number; qy: number; qz: number; qw: number; scale: number };

export interface StepExportRequest {
  type: "export-step";
  commands: StepCommand[];
}

export interface StepImportRequest {
  type: "import-step";
  data: Uint8Array;
}

export type StepWorkerRequest = StepExportRequest | StepImportRequest;

export type StepWorkerMessage =
  | { type: "step-result"; data: Uint8Array }
  | { type: "step-error"; message: string }
  | { type: "step-progress"; phase: string }
  | { type: "import-result"; positions: Float32Array; indices: Uint32Array };

// ─── Worker implementation ─────────────────────────────────────

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let oc: any = null;

async function ensureOcct(): Promise<void> {
  if (oc) return;
  post({ type: "step-progress", phase: "Loading OpenCASCADE (~25 MB)..." });
  try {
    // Dynamic import — opencascade.js is an optional peer dependency
    // that's only loaded when the user clicks "Export STEP". The
    // @ts-ignore suppresses the missing-module error at type-check
    // time; the runtime import will fail gracefully if the package
    // isn't installed.
    // @ts-ignore — optional dynamic dependency
    const initOpenCascade = (await import("opencascade.js")).default;
    oc = await initOpenCascade();
  } catch (e) {
    throw new Error(
      `Failed to load OpenCASCADE WASM. Install it with: npm install opencascade.js@beta\n${e}`,
    );
  }
}

function post(msg: StepWorkerMessage, transfer?: Transferable[]): void {
  self.postMessage(msg, { transfer: transfer ?? [] });
}

self.onmessage = async (e: MessageEvent<StepWorkerRequest>) => {
  if (e.data.type === "export-step") {
    try {
      await ensureOcct();
      post({ type: "step-progress", phase: "Building B-Rep geometry..." });

      const shapes = new Map<string, unknown>();
      for (const cmd of e.data.commands) {
        const shape = executeCommand(cmd, shapes);
        if (shape) shapes.set(cmd.handle, shape);
      }

      post({ type: "step-progress", phase: "Writing STEP file..." });
      const stepBytes = writeStep(shapes);

      // Clean up OCCT shapes
      for (const s of shapes.values()) {
        try { (s as { delete(): void }).delete(); } catch { /* already freed */ }
      }

      post(
        { type: "step-result", data: stepBytes },
        [stepBytes.buffer],
      );
    } catch (err) {
      post({
        type: "step-error",
        message: err instanceof Error ? err.message : String(err),
      });
    }
  } else if (e.data.type === "import-step") {
    try {
      // Decode text before OCCT consumes the buffer
      const stepText = new TextDecoder().decode(e.data.data);

      await ensureOcct();
      const brep = readAndTessellateStep(e.data.data);

      // Parse AP242 tessellated shells (mesh-only bodies like ConvergentBody)
      post({ type: "step-progress", phase: "Parsing tessellated shells..." });
      const tess = parseTessellatedShells(stepText);

      let positions: Float32Array;
      let indices: Uint32Array;

      if (tess) {
        // Merge B-Rep tessellation with parsed tessellated shells
        const brepVerts = brep.positions.length / 3;
        positions = new Float32Array(brep.positions.length + tess.positions.length);
        positions.set(brep.positions);
        positions.set(tess.positions, brep.positions.length);

        const mergedIndices = new Uint32Array(brep.indices.length + tess.indices.length);
        mergedIndices.set(brep.indices);
        for (let i = 0; i < tess.indices.length; i++) {
          mergedIndices[brep.indices.length + i] = tess.indices[i] + brepVerts;
        }
        indices = mergedIndices;
      } else {
        positions = brep.positions;
        indices = brep.indices;
      }

      post(
        { type: "import-result", positions, indices },
        [positions.buffer, indices.buffer],
      );
    } catch (err) {
      post({
        type: "step-error",
        message: err instanceof Error ? err.message : String(err),
      });
    }
  }
};

// ─── Command execution ─────────────────────────────────────────

function executeCommand(
  cmd: StepCommand,
  shapes: Map<string, unknown>,
): unknown {
  switch (cmd.op) {
    case "box": return makeBox(cmd);
    case "sphere": return makeSphere(cmd);
    case "cylinder": return makeCylinder(cmd);
    case "cone": return makeCone(cmd);
    case "torus": return makeTorus(cmd);
    case "hexprism": return makeHexPrism(cmd);
    case "extrude": return makeExtrude(cmd);
    case "revolve": return makeRevolve(cmd);
    case "union": return boolOp("fuse", shapes.get(cmd.a), shapes.get(cmd.b));
    case "difference": return boolOp("cut", shapes.get(cmd.a), shapes.get(cmd.b));
    case "intersect": return boolOp("common", shapes.get(cmd.a), shapes.get(cmd.b));
    case "transform": return applyTransform(cmd, shapes.get(cmd.source));
    default: return null;
  }
}

function makeBox(cmd: { cx: number; cy: number; cz: number; hx: number; hy: number; hz: number }): unknown {
  const ax = new oc.gp_Ax2_3(
    new oc.gp_Pnt_3(cmd.cx - cmd.hx, cmd.cy - cmd.hy, cmd.cz - cmd.hz),
    new oc.gp_Dir_4(0, 0, 1),
  );
  const maker = new oc.BRepPrimAPI_MakeBox_5(ax, cmd.hx * 2, cmd.hy * 2, cmd.hz * 2);
  const shape = maker.Shape();
  ax.delete();
  maker.delete();
  return shape;
}

function makeSphere(cmd: { cx: number; cy: number; cz: number; radius: number }): unknown {
  const maker = new oc.BRepPrimAPI_MakeSphere_5(
    new oc.gp_Pnt_3(cmd.cx, cmd.cy, cmd.cz),
    cmd.radius,
  );
  const shape = maker.Shape();
  maker.delete();
  return shape;
}

function makeCylinder(cmd: { cx: number; cy: number; cz: number; radius: number; halfHeight: number }): unknown {
  const ax = new oc.gp_Ax2_3(
    new oc.gp_Pnt_3(cmd.cx, cmd.cy - cmd.halfHeight, cmd.cz),
    new oc.gp_Dir_4(0, 1, 0),
  );
  const maker = new oc.BRepPrimAPI_MakeCylinder_3(ax, cmd.radius, cmd.halfHeight * 2);
  const shape = maker.Shape();
  ax.delete();
  maker.delete();
  return shape;
}

function makeCone(cmd: { cx: number; cy: number; cz: number; rBottom: number; rTop: number; halfHeight: number }): unknown {
  const ax = new oc.gp_Ax2_3(
    new oc.gp_Pnt_3(cmd.cx, cmd.cy - cmd.halfHeight, cmd.cz),
    new oc.gp_Dir_4(0, 1, 0),
  );
  const maker = new oc.BRepPrimAPI_MakeCone_3(ax, cmd.rBottom, cmd.rTop, cmd.halfHeight * 2);
  const shape = maker.Shape();
  ax.delete();
  maker.delete();
  return shape;
}

function makeTorus(cmd: { cx: number; cy: number; cz: number; majorRadius: number; minorRadius: number }): unknown {
  const ax = new oc.gp_Ax2_3(
    new oc.gp_Pnt_3(cmd.cx, cmd.cy, cmd.cz),
    new oc.gp_Dir_4(0, 1, 0),
  );
  const maker = new oc.BRepPrimAPI_MakeTorus_5(ax, cmd.majorRadius, cmd.minorRadius);
  const shape = maker.Shape();
  ax.delete();
  maker.delete();
  return shape;
}

function makeHexPrism(cmd: { apothem: number; halfHeight: number }): unknown {
  // Build a hexagonal wire from 6 vertices, extrude along Y
  const r = cmd.apothem / Math.cos(Math.PI / 6); // circumradius
  const wire = new oc.BRepBuilderAPI_MakeWire_1();
  for (let i = 0; i < 6; i++) {
    const a1 = (Math.PI / 3) * i;
    const a2 = (Math.PI / 3) * ((i + 1) % 6);
    const p1 = new oc.gp_Pnt_3(r * Math.cos(a1), 0, r * Math.sin(a1));
    const p2 = new oc.gp_Pnt_3(r * Math.cos(a2), 0, r * Math.sin(a2));
    const edge = new oc.BRepBuilderAPI_MakeEdge_3(p1, p2);
    wire.Add_1(edge.Edge());
    p1.delete(); p2.delete(); edge.delete();
  }
  const face = new oc.BRepBuilderAPI_MakeFace_15(wire.Wire(), true);
  const prism = new oc.BRepPrimAPI_MakePrism_1(
    face.Face(),
    new oc.gp_Vec_4(0, cmd.halfHeight * 2, 0),
    false, true,
  );
  const shape = prism.Shape();
  // Move center to origin
  const trsf = new oc.gp_Trsf_1();
  trsf.SetTranslation_1(new oc.gp_Vec_4(0, -cmd.halfHeight, 0));
  const moved = new oc.BRepBuilderAPI_Transform_2(shape, trsf, true);
  const result = moved.Shape();
  wire.delete(); face.delete(); prism.delete(); trsf.delete(); moved.delete();
  return result;
}

function makeExtrude(cmd: { wirePts: number[]; depth: number; cx: number; cy: number; cz: number }): unknown {
  const wire = wireFromPoints(cmd.wirePts);
  const face = new oc.BRepBuilderAPI_MakeFace_15(wire, true);
  const prism = new oc.BRepPrimAPI_MakePrism_1(
    face.Face(),
    new oc.gp_Vec_4(0, 0, cmd.depth),
    false, true,
  );
  const shape = prism.Shape();
  // Translate to world position
  const trsf = new oc.gp_Trsf_1();
  trsf.SetTranslation_1(new oc.gp_Vec_4(cmd.cx, cmd.cy, cmd.cz));
  const moved = new oc.BRepBuilderAPI_Transform_2(shape, trsf, true);
  const result = moved.Shape();
  face.delete(); prism.delete(); trsf.delete(); moved.delete();
  return result;
}

function makeRevolve(cmd: { wirePts: number[]; angle: number; cx: number; cy: number; cz: number }): unknown {
  const wire = wireFromPoints(cmd.wirePts);
  const face = new oc.BRepBuilderAPI_MakeFace_15(wire, true);
  const axis = new oc.gp_Ax1_2(
    new oc.gp_Pnt_3(cmd.cx, cmd.cy, cmd.cz),
    new oc.gp_Dir_4(0, 1, 0),
  );
  const revol = new oc.BRepPrimAPI_MakeRevol_1(
    face.Face(), axis, cmd.angle, true,
  );
  const shape = revol.Shape();
  face.delete(); axis.delete(); revol.delete();
  return shape;
}

function wireFromPoints(pts: number[]): unknown {
  const wire = new oc.BRepBuilderAPI_MakeWire_1();
  const count = pts.length / 2;
  for (let i = 0; i < count; i++) {
    const j = (i + 1) % count;
    const p1 = new oc.gp_Pnt_3(pts[i * 2], pts[i * 2 + 1], 0);
    const p2 = new oc.gp_Pnt_3(pts[j * 2], pts[j * 2 + 1], 0);
    const edge = new oc.BRepBuilderAPI_MakeEdge_3(p1, p2);
    wire.Add_1(edge.Edge());
    p1.delete(); p2.delete(); edge.delete();
  }
  return wire.Wire();
}

function boolOp(kind: "fuse" | "cut" | "common", a: unknown, b: unknown): unknown {
  if (!a || !b) return a ?? b;
  const progress = new oc.Message_ProgressRange_1();
  let maker;
  switch (kind) {
    case "fuse":
      maker = new oc.BRepAlgoAPI_Fuse_3(a, b, progress);
      break;
    case "cut":
      maker = new oc.BRepAlgoAPI_Cut_3(a, b, progress);
      break;
    case "common":
      maker = new oc.BRepAlgoAPI_Common_3(a, b, progress);
      break;
  }
  maker.Build(new oc.Message_ProgressRange_1());
  const shape = maker.Shape();
  progress.delete();
  maker.delete();
  return shape;
}

function applyTransform(
  cmd: { tx: number; ty: number; tz: number; qx: number; qy: number; qz: number; qw: number; scale: number },
  source: unknown,
): unknown {
  if (!source) return null;
  const trsf = new oc.gp_Trsf_1();

  // Quaternion → rotation matrix via gp_Quaternion
  const quat = new oc.gp_Quaternion_2(cmd.qx, cmd.qy, cmd.qz, cmd.qw);
  trsf.SetRotation_2(quat);
  quat.delete();

  // Apply translation
  trsf.SetTranslationPart(new oc.gp_Vec_4(cmd.tx, cmd.ty, cmd.tz));

  if (Math.abs(cmd.scale - 1.0) > 1e-6) {
    trsf.SetScaleFactor(cmd.scale);
  }

  const moved = new oc.BRepBuilderAPI_Transform_2(source, trsf, true);
  const shape = moved.Shape();
  trsf.delete();
  moved.delete();
  return shape;
}

// ─── STEP writer ───────────────────────────────────────────────

function writeStep(shapes: Map<string, unknown>): Uint8Array {
  const writer = new oc.STEPControl_Writer_1();
  const progress = new oc.Message_ProgressRange_1();

  for (const [, shape] of shapes) {
    writer.Transfer(
      shape,
      oc.STEPControl_StepModelType.STEPControl_AsIs,
      true,
      progress,
    );
  }

  const filename = "/tmp/fielddraft_export.step";
  writer.Write(filename);
  const stepData: Uint8Array = oc.FS.readFile(filename, { encoding: "binary" });

  try { oc.FS.unlink(filename); } catch { /* cleanup best-effort */ }
  writer.delete();
  progress.delete();

  return stepData;
}

// ─── M14i: STEP reader + tessellation ─────────────────────────

function readAndTessellateStep(
  fileBytes: Uint8Array,
): { positions: Float32Array; indices: Uint32Array } {
  post({ type: "step-progress", phase: "Reading STEP file..." });

  const tmpFile = "/tmp/fielddraft_import.step";
  oc.FS.writeFile(tmpFile, fileBytes);

  const reader = new oc.STEPControl_Reader_1();
  const status = reader.ReadFile(tmpFile);
  try { oc.FS.unlink(tmpFile); } catch { /* best-effort */ }

  if (status !== oc.IFSelect_ReturnStatus.IFSelect_RetDone) {
    reader.delete();
    throw new Error("Failed to parse STEP file (invalid format or empty).");
  }

  const progress = new oc.Message_ProgressRange_1();
  reader.TransferRoots(progress);
  progress.delete();

  const shape = reader.OneShape();
  reader.delete();

  post({ type: "step-progress", phase: "Tessellating B-Rep..." });

  const linDeflection = 0.1;
  const angDeflection = 0.5;
  const mesher = new oc.BRepMesh_IncrementalMesh_2(
    shape, linDeflection, false, angDeflection, false,
  );
  mesher.Perform_1(new oc.Message_ProgressRange_1());
  mesher.delete();

  post({ type: "step-progress", phase: "Extracting triangles..." });

  const allPositions: number[] = [];
  const allIndices: number[] = [];
  let vertexOffset = 0;

  const explorer = new oc.TopExp_Explorer_2(
    shape,
    oc.TopAbs_ShapeEnum.TopAbs_FACE,
    oc.TopAbs_ShapeEnum.TopAbs_SHAPE,
  );

  while (explorer.More()) {
    const face = oc.TopoDS.Face_1(explorer.Current());
    const loc = new oc.TopLoc_Location_1();
    const handleTri = oc.BRep_Tool.Triangulation(face, loc, 0);

    if (!handleTri.IsNull()) {
      const tri = handleTri.get();
      const trsf = loc.Transformation();
      const nbNodes = tri.NbNodes();
      const nbTris = tri.NbTriangles();

      for (let i = 1; i <= nbNodes; i++) {
        const p = tri.Node(i).Transformed(trsf);
        allPositions.push(p.X(), p.Y(), p.Z());
        p.delete();
      }

      for (let i = 1; i <= nbTris; i++) {
        const t = tri.Triangle(i);
        allIndices.push(
          t.Value(1) - 1 + vertexOffset,
          t.Value(2) - 1 + vertexOffset,
          t.Value(3) - 1 + vertexOffset,
        );
        t.delete();
      }

      vertexOffset += nbNodes;
    }

    loc.delete();
    explorer.Next();
  }

  explorer.delete();
  try { shape.delete(); } catch { /* already freed */ }

  if (allPositions.length === 0) {
    throw new Error("STEP file contains no geometry (0 triangles after tessellation).");
  }

  const positions = new Float32Array(allPositions);
  const indices = new Uint32Array(allIndices);
  return { positions, indices };
}

// ─── AP242 tessellated geometry parser ────────────────────────────
//
// Parses COORDINATES_LIST and COMPLEX_TRIANGULATED_FACE entities
// directly from STEP text. These represent mesh-only bodies (e.g.
// Onshape ConvergentBody) that OCCT's STEPControl_Reader cannot
// tessellate via BRep_Tool.Triangulation.

function parseTessellatedShells(
  stepText: string,
): { positions: Float32Array; indices: Uint32Array } | null {
  // Flatten multi-line entities into single lines keyed by entity ID.
  // STEP entities start with #<id>= and end with ;
  const entityMap = new Map<number, string>();
  const entityRegex = /#(\d+)\s*=\s*/g;
  let match: RegExpExecArray | null;

  // Find all entity starts and extract their full content up to ;
  const lines = stepText;
  while ((match = entityRegex.exec(lines)) !== null) {
    const id = parseInt(match[1], 10);
    const startIdx = match.index + match[0].length;
    const endIdx = lines.indexOf(";", startIdx);
    if (endIdx === -1) continue;
    const content = lines.substring(startIdx, endIdx).replace(/\s+/g, " ");
    entityMap.set(id, content);
  }

  // Extract COORDINATES_LIST: entity content looks like:
  // COORDINATES_LIST('',<count>,((<x>,<y>,<z>),(<x>,<y>,<z>),...))
  const coordsMap = new Map<number, Float64Array>();
  for (const [id, content] of entityMap) {
    if (!content.startsWith("COORDINATES_LIST")) continue;
    const dataStart = content.indexOf(",((");
    if (dataStart === -1) continue;
    const dataStr = content.substring(dataStart + 2, content.lastIndexOf(")") + 1);
    const nums = dataStr.replace(/[()]/g, "").split(",").map(Number);
    coordsMap.set(id, new Float64Array(nums));
  }

  if (coordsMap.size === 0) return null;

  // Determine length unit scale factor (STEP → mm, OCCT's internal unit).
  // Look for SI_UNIT($,.METRE.) inside a LENGTH_UNIT() complex entity.
  let unitScale = 1000.0; // default: meters → mm
  const milliMatch = stepText.match(
    /LENGTH_UNIT\(\)\s*NAMED_UNIT\(\*\)\s*SI_UNIT\(\.(MILLI)\.,\.METRE\.\)/,
  );
  if (milliMatch) unitScale = 1.0; // already mm

  // Apply scale to all coordinate lists
  for (const [id, coords] of coordsMap) {
    if (unitScale !== 1.0) {
      const scaled = new Float64Array(coords.length);
      for (let i = 0; i < coords.length; i++) scaled[i] = coords[i] * unitScale;
      coordsMap.set(id, scaled);
    }
  }

  // Extract COMPLEX_TRIANGULATED_FACE entities
  const allPositions: number[] = [];
  const allIndices: number[] = [];
  let vertexOffset = 0;

  for (const [, content] of entityMap) {
    if (!content.startsWith("COMPLEX_TRIANGULATED_FACE")) continue;

    // Parse reference to COORDINATES_LIST: second argument is #<id>
    const refMatch = content.match(/COMPLEX_TRIANGULATED_FACE\('[^']*',#(\d+)/);
    if (!refMatch) continue;
    const coordsId = parseInt(refMatch[1], 10);
    const coords = coordsMap.get(coordsId);
    if (!coords) continue;

    const nbVerts = coords.length / 3;

    // Find the triangle list — the last non-empty parenthesized group
    // before the final ");". Format: ((v1,v2,v3),(v1,v2,v3),...)
    // The pnindex field (before triangles) may be $ or a parenthesized list.
    // Triangle strips come after pnindex, triangle fans after that.
    //
    // Strategy: find the section that contains triangle tuples.
    // After the normals array and pnindex, there's ((i,i,i),...) for strips/triangles.
    // We look for all occurrences of ((<digits>,<digits>,<digits>),...)
    // that come after the normals.

    // Find end of normals array — the normals are the large ((...)) block
    // after the count. Skip past: name, coordsRef, normalsCount, normalsArray.
    // Then we have: pnindex, triangle_strips, triangle_fans
    const normCountMatch = content.match(
      /COMPLEX_TRIANGULATED_FACE\('[^']*',#\d+,(\d+),/,
    );
    if (!normCountMatch) continue;

    // Find the position after the normals array ends.
    // Normals array starts with (( after the count.
    const normCountEnd = content.indexOf(
      normCountMatch[0],
    ) + normCountMatch[0].length;
    // Find the matching closing )) of the normals array
    let depth = 0;
    let normEnd = normCountEnd;
    for (let i = normCountEnd; i < content.length; i++) {
      if (content[i] === "(") depth++;
      else if (content[i] === ")") {
        depth--;
        if (depth === 0) {
          normEnd = i + 1;
          break;
        }
      }
    }

    // After normals: ,<pnindex>,<triangle_strips>,<triangle_fans>
    // pnindex can be $ or (...)
    const afterNormals = content.substring(normEnd);

    // Skip comma and pnindex
    const pnMatch = afterNormals.match(/^\s*,\s*(\$|\([^)]*\))\s*,\s*/);
    if (!pnMatch) continue;
    const afterPnindex = afterNormals.substring(pnMatch[0].length);

    // Triangle strips/list: ((v1,v2,v3),(v4,v5,v6),...)
    // Extract all triangle tuples from this section
    const triRegex = /\((\d+),(\d+),(\d+)\)/g;
    let triMatch: RegExpExecArray | null;
    const localIndices: number[] = [];

    while ((triMatch = triRegex.exec(afterPnindex)) !== null) {
      const a = parseInt(triMatch[1], 10) - 1; // 1-based → 0-based
      const b = parseInt(triMatch[2], 10) - 1;
      const c = parseInt(triMatch[3], 10) - 1;
      localIndices.push(
        a + vertexOffset,
        b + vertexOffset,
        c + vertexOffset,
      );
    }

    if (localIndices.length === 0) continue;

    // Add vertices
    for (let i = 0; i < coords.length; i++) {
      allPositions.push(coords[i]);
    }

    for (const idx of localIndices) {
      allIndices.push(idx);
    }

    vertexOffset += nbVerts;
  }

  if (allPositions.length === 0) return null;

  return {
    positions: new Float32Array(allPositions),
    indices: new Uint32Array(allIndices),
  };
}
