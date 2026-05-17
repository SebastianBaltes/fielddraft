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
 *                  | { type: "import-result", positions: Float32Array, normals: Float32Array, indices: Uint32Array }
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
  | { type: "import-result"; positions: Float32Array; normals: Float32Array; indices: Uint32Array };

// ─── Worker implementation ─────────────────────────────────────

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let oc: any = null;

async function ensureOcct(): Promise<void> {
  if (oc) return;
  post({ type: "step-progress", phase: "Loading OpenCASCADE (~25 MB)..." });
  try {
    // @ts-ignore — optional dynamic dependency
    const initOpenCascade = (await import("opencascade.js")).default;
    oc = await initOpenCascade();
  } catch (e) {
    throw new Error(
      `Failed to load OpenCASCADE WASM. Install it with: npm install opencascade.js@beta\n${e}`,
    );
  }
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let occtImporter: any = null;

async function ensureOcctImporter(): Promise<any> {
  if (occtImporter) return occtImporter;
  post({ type: "step-progress", phase: "Loading occt-import-js (~12 MB)..." });
  try {
    // @ts-ignore — optional dynamic dependency
    const init = (await import("occt-import-js")).default;
    occtImporter = await init({
      locateFile: (name: string) =>
        name.endsWith(".wasm") ? "/occt-import-js.wasm" : name,
    });
    return occtImporter;
  } catch (e) {
    throw new Error(
      `Failed to load occt-import-js WASM. Install it with: npm install occt-import-js\n${e}`,
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
      const result = await importStepViaOcctImportJs(e.data.data);
      post(
        { type: "import-result", ...result },
        [result.positions.buffer, result.normals.buffer, result.indices.buffer],
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

// ─── M14i: STEP import via occt-import-js ─────────────────────

async function importStepViaOcctImportJs(
  fileBytes: Uint8Array,
): Promise<{ positions: Float32Array; normals: Float32Array; indices: Uint32Array }> {
  post({ type: "step-progress", phase: "Reading STEP file..." });
  const occtImp = await ensureOcctImporter();

  post({ type: "step-progress", phase: "Tessellating B-Rep..." });
  const result = occtImp.ReadStepFile(fileBytes, {
    linearUnit: "millimeter",
    linearDeflectionType: "absolute_value",
    linearDeflection: 0.02,
    angularDeflection: 0.1,
  });

  if (!result.success) {
    throw new Error("Failed to parse STEP file (invalid format or empty).");
  }

  post({ type: "step-progress", phase: "Extracting triangles..." });

  const allPositions: number[] = [];
  const allNormals: number[] = [];
  const allIndices: number[] = [];
  let vertexOffset = 0;

  for (const mesh of result.meshes) {
    const pos = mesh.attributes.position.array;
    const nrm = mesh.attributes.normal?.array;
    const idx = mesh.index.array;

    for (let i = 0; i < pos.length; i++) allPositions.push(pos[i]);

    if (nrm && nrm.length === pos.length) {
      for (let i = 0; i < nrm.length; i++) allNormals.push(nrm[i]);
    } else {
      for (let i = 0; i < pos.length; i++) allNormals.push(0);
    }

    for (let i = 0; i < idx.length; i++) allIndices.push(idx[i] + vertexOffset);
    vertexOffset += pos.length / 3;
  }

  // AP242 STEP files may contain COMPLEX_TRIANGULATED_FACE entities
  // (convergent bodies / embedded meshes) that occt-import-js skips.
  const stepText = new TextDecoder().decode(fileBytes);
  if (stepText.includes("COMPLEX_TRIANGULATED_FACE")) {
    post({ type: "step-progress", phase: "Extracting embedded meshes..." });
    const tess = extractTessellatedGeometry(stepText);
    if (tess) {
      for (let i = 0; i < tess.positions.length; i++) allPositions.push(tess.positions[i]);
      for (let i = 0; i < tess.normals.length; i++) allNormals.push(tess.normals[i]);
      for (let i = 0; i < tess.indices.length; i++) allIndices.push(tess.indices[i] + vertexOffset);
      vertexOffset += tess.positions.length / 3;
    }
  }

  if (allPositions.length === 0) {
    throw new Error("STEP file contains no geometry (0 triangles after tessellation).");
  }

  return {
    positions: new Float32Array(allPositions),
    normals: new Float32Array(allNormals),
    indices: new Uint32Array(allIndices),
  };
}

// ─── AP242 tessellated geometry parser ────────────────────────

function extractTessellatedGeometry(stepText: string): {
  positions: number[];
  normals: number[];
  indices: number[];
} | null {
  const coordsMap = new Map<number, Float64Array>();

  const coordsRe = /#(\d+)\s*=\s*COORDINATES_LIST\s*\(\s*'[^']*'\s*,\s*(\d+)\s*,/g;
  let cm;
  while ((cm = coordsRe.exec(stepText)) !== null) {
    const entityId = parseInt(cm[1]);
    const count = parseInt(cm[2]);
    const dataStart = cm.index + cm[0].length;

    const nums = parseNumbersFromParenBlock(stepText, dataStart, count * 3);
    if (nums.length === count * 3) {
      coordsMap.set(entityId, new Float64Array(nums));
    }
  }

  if (coordsMap.size === 0) return null;

  const allPositions: number[] = [];
  const allNormals: number[] = [];
  const allIndices: number[] = [];
  let vertexOffset = 0;

  const faceRe = /#(\d+)\s*=\s*COMPLEX_TRIANGULATED_FACE\s*\(\s*'[^']*'\s*,\s*#(\d+)\s*,\s*(\d+)\s*,/g;
  let fm;
  while ((fm = faceRe.exec(stepText)) !== null) {
    const coordRef = parseInt(fm[2]);
    const pnmax = parseInt(fm[3]);
    const coords = coordsMap.get(coordRef);
    if (!coords || pnmax === 0) continue;

    const afterHeader = fm.index + fm[0].length;
    const face = parseTessellatedFace(stepText, afterHeader, pnmax, coords);
    if (!face) continue;

    for (let i = 0; i < face.positions.length; i++) allPositions.push(face.positions[i]);
    for (let i = 0; i < face.normals.length; i++) allNormals.push(face.normals[i]);
    for (let i = 0; i < face.indices.length; i++) allIndices.push(face.indices[i] + vertexOffset);
    vertexOffset += face.positions.length / 3;
  }

  if (allPositions.length === 0) return null;
  return { positions: allPositions, normals: allNormals, indices: allIndices };
}

function parseTessellatedFace(
  text: string,
  offset: number,
  pnmax: number,
  coords: Float64Array,
): { positions: number[]; normals: number[]; indices: number[] } | null {
  // Parse normals list: ((nx,ny,nz),(nx,ny,nz),...)
  const normalsStart = text.indexOf("((", offset);
  if (normalsStart === -1) return null;
  const normNums = parseNumbersFromParenBlock(text, normalsStart, pnmax * 3);
  if (normNums.length < pnmax * 3) return null;

  // Skip past normals block
  let pos = normalsStart;
  pos = skipParenBlock(text, pos);
  if (pos === -1) return null;

  // Skip comma and '$' (derived attribute)
  pos = skipWhitespaceAndComma(text, pos);
  if (text[pos] === "$") {
    pos++;
    pos = skipWhitespaceAndComma(text, pos);
  }

  // Parse pnindex: (i1,i2,i3,...) — maps point index → coordinate index (1-based)
  let pnindex: number[] | null = null;
  if (text[pos] === "(") {
    const nextChar = text[pos + 1];
    if (nextChar !== "(" && nextChar !== ")") {
      pnindex = parseIntList(text, pos);
      pos = skipParenBlock(text, pos);
      if (pos === -1) return null;
      pos = skipWhitespaceAndComma(text, pos);
    }
  }

  // Parse triangle strips: ((a,b,c),(d,e,f),...) or ()
  const triangleIndices: number[] = [];
  if (text[pos] === "(") {
    if (text[pos + 1] === "(") {
      parseTriangleStrips(text, pos, triangleIndices);
    }
    pos = skipParenBlock(text, pos);
    if (pos === -1) return null;
    pos = skipWhitespaceAndComma(text, pos);
  }

  // Parse triangle fans: ((center,a,b,c,...),(...),...) or ()
  if (text[pos] === "(") {
    if (text[pos + 1] === "(") {
      parseTriangleFans(text, pos, triangleIndices);
    }
  }

  if (triangleIndices.length === 0) return null;

  // Build output: expand each point into position + normal
  // Triangle indices reference "points" (1-based into pnindex/normals)
  const positions: number[] = [];
  const normals: number[] = [];
  const indices: number[] = [];
  const pointMap = new Map<number, number>(); // point_1based → output vertex index
  let outIdx = 0;

  for (let t = 0; t < triangleIndices.length; t += 3) {
    for (let k = 0; k < 3; k++) {
      const ptIdx = triangleIndices[t + k]; // 1-based point index
      let vi = pointMap.get(ptIdx);
      if (vi === undefined) {
        vi = outIdx++;
        pointMap.set(ptIdx, vi);

        const coordIdx = pnindex ? pnindex[ptIdx - 1] - 1 : ptIdx - 1;
        const ni = ptIdx - 1;
        const nx = normNums[ni * 3], ny = normNums[ni * 3 + 1], nz = normNums[ni * 3 + 2];
        const bias = 0.1; // mm — nudge along normal to win depth test over B-Rep
        positions.push(coords[coordIdx * 3] * 1000 + nx * bias);
        positions.push(coords[coordIdx * 3 + 1] * 1000 + ny * bias);
        positions.push(coords[coordIdx * 3 + 2] * 1000 + nz * bias);

        normals.push(nx);
        normals.push(ny);
        normals.push(nz);
      }
      indices.push(vi);
    }
  }

  return { positions, normals, indices };
}

function parseNumbersFromParenBlock(text: string, start: number, maxCount: number): number[] {
  const nums: number[] = [];
  let i = start;
  while (i < text.length && nums.length < maxCount) {
    const c = text[i];
    if (c === "-" || c === "." || (c >= "0" && c <= "9")) {
      let end = i + 1;
      while (end < text.length && "0123456789.eE+-".includes(text[end])) end++;
      nums.push(parseFloat(text.substring(i, end)));
      i = end;
    } else if (c === ";" || (c === ")" && text[i + 1] === ";")) {
      break;
    } else {
      i++;
    }
  }
  return nums;
}

function parseIntList(text: string, start: number): number[] {
  const end = text.indexOf(")", start);
  if (end === -1) return [];
  const inner = text.substring(start + 1, end);
  return inner.split(",").map((s) => parseInt(s.trim())).filter((n) => !isNaN(n));
}

function parseTriangleStrips(text: string, start: number, out: number[]): void {
  // ((a,b,c),(d,e,f),...) — each inner list is a triangle strip
  let depth = 0;
  let i = start;
  while (i < text.length) {
    if (text[i] === "(") {
      depth++;
      if (depth === 2) {
        // Start of a strip
        const end = text.indexOf(")", i);
        if (end === -1) return;
        const inner = text.substring(i + 1, end);
        const vals = inner.split(",").map((s) => parseInt(s.trim()));
        // Convert strip to triangles
        for (let j = 2; j < vals.length; j++) {
          if (j % 2 === 0) {
            out.push(vals[j - 2], vals[j - 1], vals[j]);
          } else {
            out.push(vals[j - 1], vals[j - 2], vals[j]);
          }
        }
        i = end + 1;
        depth--;
      } else {
        i++;
      }
    } else if (text[i] === ")") {
      depth--;
      if (depth <= 0) return;
      i++;
    } else {
      i++;
    }
  }
}

function parseTriangleFans(text: string, start: number, out: number[]): void {
  let depth = 0;
  let i = start;
  while (i < text.length) {
    if (text[i] === "(") {
      depth++;
      if (depth === 2) {
        const end = text.indexOf(")", i);
        if (end === -1) return;
        const inner = text.substring(i + 1, end);
        const vals = inner.split(",").map((s) => parseInt(s.trim()));
        // Fan: first vertex is center, triangles are (center, v[j], v[j+1])
        for (let j = 1; j + 1 < vals.length; j++) {
          out.push(vals[0], vals[j], vals[j + 1]);
        }
        i = end + 1;
        depth--;
      } else {
        i++;
      }
    } else if (text[i] === ")") {
      depth--;
      if (depth <= 0) return;
      i++;
    } else {
      i++;
    }
  }
}

function skipParenBlock(text: string, start: number): number {
  let depth = 0;
  for (let i = start; i < text.length; i++) {
    if (text[i] === "(") depth++;
    else if (text[i] === ")") {
      depth--;
      if (depth === 0) return i + 1;
    }
  }
  return -1;
}

function skipWhitespaceAndComma(text: string, pos: number): number {
  while (pos < text.length && " \t\n\r,".includes(text[pos])) pos++;
  return pos;
}
