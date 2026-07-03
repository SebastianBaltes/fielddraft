/**
 * IoStore — the import/export domain controller.
 *
 * Owns every mesh-format boundary of the app: reference-mesh import
 * (STL / OBJ / GLB → SDF feature via `addImportedMesh`), the retained
 * per-feature mesh-overlay cache, and the exporters (STL ASCII/binary,
 * 3MF incl. slicer-metadata roundtrip, STEP via the OCCT worker, GLB,
 * OBJ). The KernelStore facade delegates its public I/O API 1:1 here.
 */

import { BoolOpTag, type Kernel } from "./kernel";
import {
  type MeshOverlayData,
  canTriggerDownload,
  triggerDownload,
  parseStlBinary,
  parseObjText,
  buildObjExportText,
} from "./meshIO";
import {
  import3mf,
  export3mfRoundtrip,
  extractModelXml,
} from "./threemf";
import { parseGlbToMesh } from "./glbImport";
import { StoreCore, type StoreState } from "./storeCore";

export class IoStore {
  constructor(private readonly core: StoreCore) {}

  private get kernel(): Kernel {
    return this.core.kernel;
  }
  private get state(): StoreState {
    return this.core.state;
  }
  private patch(partial: Partial<StoreState>): void {
    this.core.patch(partial);
  }
  private notify(): void {
    this.core.notify();
  }
  private setLoading(phase: string | null): void {
    this.core.setLoading(phase);
  }

  /** Retained TypedArrays of imported meshes, keyed by feature id —
   *  the suppressed `imported_mesh` overlay renders from this cache. */
  private meshDataCache = new Map<number, MeshOverlayData>();

  /** Drop the retained overlay data of a removed feature (called by the
   *  feature domain when an `imported_mesh` feature is deleted). */
  dropMeshData(id: number): void {
    this.meshDataCache.delete(id);
  }

  // ── K9.0: STL reference import ──────────────────────────────

  async loadReferenceMesh(buffer: ArrayBuffer): Promise<boolean> {
    this.setLoading("Parsing STL…");
    await new Promise((r) => setTimeout(r, 0));
    const mesh = parseStlBinary(buffer);
    if (!mesh) {
      this.setLoading(null);
      return false;
    }
    this.setLoading("Building SDF from mesh…");
    await new Promise((r) => setTimeout(r, 0));
    const id = this.importMeshAsFeature(
      mesh.positions,
      mesh.normals,
      mesh.indices,
    );
    this.setLoading(null);
    return id !== null;
  }

  /** K9.2: load a GLB/glTF file as a visual reference mesh. The
   *  parse + merge lives in [`glbImport.ts`](./glbImport); the merged
   *  mesh is imported as an SDF feature like the STL/OBJ paths. */
  async loadReferenceGlb(buffer: ArrayBuffer): Promise<boolean> {
    this.setLoading("Loading GLB file…");
    try {
      const mesh = await parseGlbToMesh(buffer);
      if (!mesh) return false;
      this.setLoading("Building SDF from mesh…");
      await new Promise((r) => setTimeout(r, 0));
      const id = this.importMeshAsFeature(
        mesh.positions,
        mesh.normals,
        mesh.indices,
      );
      this.setLoading(null);
      return id !== null;
    } catch (err) {
      this.setLoading(null);
      console.error("fielddraft: GLB import failed", err);
      return false;
    }
  }

  async loadReferenceObj(text: string): Promise<boolean> {
    const mesh = parseObjText(text);
    if (!mesh) return false;
    this.setLoading("Building SDF from mesh…");
    await new Promise((r) => setTimeout(r, 0));
    const id = this.importMeshAsFeature(
      mesh.positions,
      mesh.normals,
      mesh.indices,
    );
    this.setLoading(null);
    return id !== null;
  }

  /** K13.1: convert the current reference mesh into a CSG solid
   *  that participates in Booleans. The reference mesh is
   *  consumed (cleared) and replaced by a feature. */
  convertReferenceToSolid(): number | null {
    const ref_ = this.state.referenceMesh;
    if (!ref_ || ref_.positions.length === 0) return null;
    const t0 = performance.now();
    const triCount = ref_.indices.length / 3;
    console.log(
      `fielddraft: converting reference mesh to solid — ${triCount} triangles, building BVH…`,
    );
    let id: number;
    try {
      id = this.kernel.addImportedMesh(
        ref_.positions,
        ref_.indices,
        BoolOpTag.New,
      );
    } catch (err) {
      console.error("fielddraft: convertReferenceToSolid failed", err);
      return null;
    }
    this.patch({ referenceMesh: null });
    this.core.commit(new Set([id]), id);
    console.log(
      `fielddraft: reference → solid complete — feature #${id} in ${(performance.now() - t0).toFixed(0)} ms`,
    );
    return id;
  }

  clearReferenceMesh(): void {
    this.patch({ referenceMesh: null });
    this.notify();
  }

  importMeshAsFeature(
    positions: Float32Array,
    normals: Float32Array,
    indices: Uint32Array,
  ): number | null {
    const t0 = performance.now();
    const triCount = indices.length / 3;
    console.log(
      `fielddraft: importing mesh as SDF feature — ${triCount} triangles, building BVH…`,
    );
    let id: number;
    try {
      id = this.kernel.addImportedMesh(positions, indices, BoolOpTag.New);
    } catch (err) {
      console.error("fielddraft: importMeshAsFeature failed", err);
      return null;
    }
    this.meshDataCache.set(id, { positions, normals, indices });
    this.core.commit(new Set([id]), id);
    console.log(
      `fielddraft: mesh imported as feature #${id} (solid) in ${(performance.now() - t0).toFixed(0)} ms`,
    );
    return id;
  }

  getMeshOverlayData(id: number): MeshOverlayData | undefined {
    return this.meshDataCache.get(id);
  }

  getMeshOverlayFeatures(): { id: number; data: MeshOverlayData }[] {
    const result: { id: number; data: MeshOverlayData }[] = [];
    for (const f of this.state.features) {
      if (f.kind === "imported_mesh" && f.suppressed) {
        const data = this.meshDataCache.get(f.id);
        if (data) result.push({ id: f.id, data });
      }
    }
    return result;
  }

  // ── K7.2.1: STL export ────────────────────────────────────────

  /** Serialise the current scene as ASCII STL. Mostly useful
   *  for debug / diffing — the Download button uses the binary
   *  path below. */
  exportStlAscii(): string {
    return this.kernel.exportStlAscii();
  }

  /** Serialise the current scene as binary STL bytes. */
  exportStlBinary(): Uint8Array {
    return this.kernel.exportStlBinary();
  }

  /** Fire a browser download of the current scene as a binary
   *  STL file. No-op outside the browser (the store is also
   *  used in Vitest where `document`/`URL` may not exist). */
  downloadStl(filename = "fielddraft.stl"): void {
    if (!canTriggerDownload()) return;
    const bytes = this.kernel.exportStlBinary();
    const copy = new Uint8Array(bytes.length);
    copy.set(bytes);
    triggerDownload(copy, "model/stl", filename);
  }

  /** M14a: export the current scene as a 3MF archive with
   *  per-body colors. Same WASM→Blob→download pattern as STL. */
  download3mf(filename = "fielddraft.3mf"): void {
    if (!canTriggerDownload()) return;
    const bytes = this.kernel.export3mf();
    const copy = new Uint8Array(bytes.length);
    copy.set(bytes);
    triggerDownload(
      copy,
      "application/vnd.ms-package.3dmanufacturing-3dmodel+xml",
      filename,
    );
  }

  /** Import a .3mf file: extract meshes as CSG solids, stash
   *  slicer metadata for roundtrip re-export. */
  import3mfFile(buffer: ArrayBuffer): number[] {
    const t0 = performance.now();
    console.log(
      `fielddraft: importing 3MF — ${(buffer.byteLength / 1024).toFixed(0)} KB`,
    );
    this.setLoading("Extracting 3MF archive…");
    try {
      const result = import3mf(buffer);
      console.log(
        `fielddraft: 3MF extracted — ${result.meshes.length} mesh(es)`,
      );
      const ids: number[] = [];
      for (let i = 0; i < result.meshes.length; i++) {
        const mesh = result.meshes[i];
        this.setLoading(`Building SDF ${i + 1}/${result.meshes.length}…`);
        console.log(
          `fielddraft: 3MF mesh ${i + 1} — ${mesh.indices.length / 3} triangles, building BVH…`,
        );
        const id = this.kernel.addImportedMesh(
          mesh.positions,
          mesh.indices,
          BoolOpTag.New,
        );
        ids.push(id);
      }
      this.patch({ threemfStash: result.stash, loading: null });
      if (ids.length > 0) {
        this.core.commit(new Set(ids), ids[0]);
      } else {
        this.notify();
      }
      console.log(
        `fielddraft: 3MF import complete — ${ids.length} body(ies) in ${(performance.now() - t0).toFixed(0)} ms`,
      );
      return ids;
    } catch (e) {
      this.setLoading(null);
      console.error("fielddraft: 3MF import failed", e);
      alert(`3MF import failed: ${e instanceof Error ? e.message : e}`);
      return [];
    }
  }

  /** Re-export as 3MF, preserving stashed slicer metadata from the
   *  last import. Falls back to normal 3MF export if no stash exists. */
  download3mfRoundtrip(filename = "fielddraft.3mf"): void {
    if (!canTriggerDownload()) return;
    const stash = this.state.threemfStash;
    if (!stash) {
      this.download3mf(filename);
      return;
    }
    const kernelZip = this.kernel.export3mf();
    const copy = new Uint8Array(kernelZip.length);
    copy.set(kernelZip);
    const modelXmlOnly = extractModelXml(copy);
    const bytes = export3mfRoundtrip(modelXmlOnly, stash);
    const blobData = new Uint8Array(bytes.length);
    blobData.set(bytes);
    triggerDownload(
      blobData,
      "application/vnd.ms-package.3dmanufacturing-3dmodel+xml",
      filename,
    );
  }

  /** M14c: export the mechanical core of the scene as STEP via
   *  the OCCT Web Worker. Lazy-loads opencascade.js (~25 MB)
   *  on first call. */
  async downloadStep(filename = "fielddraft.step"): Promise<void> {
    if (!canTriggerDownload()) return;
    try {
      const classJson = this.kernel.classifyForStep();
      const report = JSON.parse(classJson) as {
        hard: number;
        degraded: number;
        excluded: number;
        total_geometric: number;
        features: Array<{ id: string; name: string; status: string }>;
      };

      if (report.total_geometric === 0) {
        alert("No geometric features to export.");
        return;
      }

      if (report.degraded > 0 || report.excluded > 0) {
        const lines: string[] = ["STEP Export — Feature Report\n"];
        if (report.excluded > 0) {
          lines.push(
            `${report.excluded} feature(s) will be EXCLUDED (not exportable):`,
          );
          for (const f of report.features) {
            if (f.status.startsWith("excluded:")) {
              lines.push(`  • ${f.name}: ${f.status.slice(10)}`);
            }
          }
          lines.push("");
        }
        if (report.degraded > 0) {
          lines.push(
            `${report.degraded} feature(s) will be DEGRADED (sharp edges instead of fillets/chamfers):`,
          );
          for (const f of report.features) {
            if (f.status.startsWith("degraded:")) {
              lines.push(`  • ${f.name}: ${f.status.slice(10)}`);
            }
          }
          lines.push("");
        }
        lines.push(`${report.hard} feature(s) will export exactly.`);
        lines.push("\nProceed with export?");

        if (!confirm(lines.join("\n"))) return;
      }

      const { exportStep } = await import("./stepExport");
      const commandsJson = this.kernel.stepCommands();
      const commands = JSON.parse(commandsJson);
      if (!Array.isArray(commands) || commands.length === 0) {
        alert("No STEP-exportable features found.");
        return;
      }
      const stepBytes = await exportStep(commands);
      const copy = new Uint8Array(stepBytes.length);
      copy.set(stepBytes);
      triggerDownload(copy, "application/step", filename);
    } catch (e) {
      console.error("STEP export failed:", e);
      alert(`STEP export failed: ${e instanceof Error ? e.message : e}`);
    }
  }

  /** M14i: import a STEP file via the OCCT worker. Tessellates
   *  the B-Rep geometry and adds it as an ImportedMesh feature. */
  async importStepFile(buffer: ArrayBuffer): Promise<number | null> {
    if (typeof window === "undefined") return null;
    const t0 = performance.now();
    console.log(
      `fielddraft: importing STEP — ${(buffer.byteLength / 1024).toFixed(0)} KB`,
    );
    this.setLoading("Loading STEP importer…");
    try {
      const { importStep } = await import("./stepImport");
      const fileBytes = new Uint8Array(buffer);
      const { positions, indices } = await importStep(fileBytes, (phase) => {
        console.log(`fielddraft: STEP phase — ${phase}`);
        this.setLoading(phase);
      });

      const posF32 =
        positions instanceof Float32Array
          ? positions
          : new Float32Array(positions);
      const idxU32 =
        indices instanceof Uint32Array ? indices : new Uint32Array(indices);

      console.log(
        `fielddraft: STEP tessellated — ${idxU32.length / 3} triangles, building BVH…`,
      );
      this.setLoading("Building SDF from mesh…");
      const id = this.kernel.addImportedMesh(posF32, idxU32, BoolOpTag.New);
      this.setLoading(null);
      this.core.commit(new Set([id]), id);
      console.log(
        `fielddraft: STEP import complete — feature #${id} in ${(performance.now() - t0).toFixed(0)} ms`,
      );
      return id;
    } catch (e) {
      this.setLoading(null);
      console.error("fielddraft: STEP import failed", e);
      alert(`STEP import failed: ${e instanceof Error ? e.message : e}`);
      return null;
    }
  }

  /** K12.1: export the current scene as a binary GLB file
   *  (glTF 2.0). Builds a temporary Three.js scene from the
   *  body mesh cache and runs the GLTFExporter. */
  async downloadGlb(filename = "fielddraft.glb"): Promise<void> {
    if (!canTriggerDownload()) return;
    const THREE = await import("three");
    const { GLTFExporter } =
      await import("three/examples/jsm/exporters/GLTFExporter.js");

    const scene = new THREE.Scene();
    const mat = new THREE.MeshStandardMaterial({ color: 0x8ca6d9 });
    for (const body of this.state.meshedBodies) {
      if (body.positions.length === 0) continue;
      const geo = new THREE.BufferGeometry();
      geo.setAttribute(
        "position",
        new THREE.BufferAttribute(body.positions, 3),
      );
      geo.setAttribute("normal", new THREE.BufferAttribute(body.normals, 3));
      geo.setIndex(new THREE.BufferAttribute(body.indices, 1));
      scene.add(new THREE.Mesh(geo, mat));
    }

    const exporter = new GLTFExporter();
    const glb = (await exporter.parseAsync(scene, {
      binary: true,
    })) as ArrayBuffer;
    triggerDownload(glb, "model/gltf-binary", filename);
  }

  downloadObj(filename = "fielddraft.obj"): void {
    if (!canTriggerDownload()) return;
    triggerDownload(
      buildObjExportText(this.state.meshedBodies),
      "text/plain",
      filename,
    );
  }
}
