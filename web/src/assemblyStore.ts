/**
 * AssemblyStore — the assembly-domain controller (P1/P2).
 *
 * Owns everything about the ACTIVE assembly tab: the part instances,
 * mates (add/remove/angle/offset/joint limits), the assembly script,
 * IK, and the R3F export. The assembly document is written through
 * `updateAssemblyDocument` — the single write path that mirrors the
 * active tab (source of truth) into the derived `state.assembly`
 * read-view and schedules the autosave. The KernelStore facade
 * delegates its public assembly API 1:1 to this class.
 */

import type { Kernel } from "./kernel";
import {
  allocateAssemblyId,
  IDENTITY_TRANSFORM,
  solveAssembly,
  solveIK,
  type AssemblyDocument,
  type AssemblyPartInstance,
  type AssemblyMeshedBody,
  type Mate,
  type MateType,
  type MateConnectorDef,
  type IKTarget,
} from "./assembly";
import { executeScript } from "./fieldscript";
import { serializeProject as serializeProjectJson } from "./project";
import { readFdraftProject } from "./fdraftFile";
import { StoreCore, type StoreState } from "./storeCore";

/** Cross-domain pieces the assembly controller needs from the facade. */
export interface AssemblyStoreDeps {
  /** Tab switching lives in the project domain — `exitAssembly` jumps
   *  back to the first part tab. */
  setActiveTab(id: string): void;
}

export class AssemblyStore {
  constructor(
    private readonly core: StoreCore,
    private readonly deps: AssemblyStoreDeps,
  ) {}

  private get kernel(): Kernel {
    return this.core.kernel;
  }
  private get state(): StoreState {
    return this.core.state;
  }
  private set state(next: StoreState) {
    this.core.state = next;
  }
  private patch(partial: Partial<StoreState>): void {
    this.core.patch(partial);
  }
  private notify(): void {
    this.core.notify();
  }

  /** Single write path for assembly-document edits. Writes `next` into the
   *  active assembly tab (the source of truth) AND mirrors it into the
   *  derived `state.assembly` read-view (same object reference), then
   *  notifies + schedules autosave. No-op if the active tab isn't an
   *  assembly tab. */
  private updateAssemblyDocument(next: AssemblyDocument): void {
    const activeId = this.state.project.activeTabId;
    const tabs = this.state.project.tabs.map((t) =>
      t.id === activeId && t.kind === "assembly" ? { ...t, document: next } : t,
    );
    this.state = {
      ...this.state,
      project: { ...this.state.project, tabs },
      assembly: { ...this.state.assembly, document: next },
    };
    this.notify();
    this.core.scheduleAutosave();
  }

  /** MT-A: leave the active assembly tab by switching to the first
   *  part tab. Drives the Outliner's "exit assembly" button. */
  exitAssembly(): void {
    const partTab = this.state.project.tabs.find((t) => t.kind === "part");
    if (partTab) this.deps.setActiveTab(partTab.id);
  }

  /** Import a `.fdraft` part into the active assembly. Accepts a document
   *  JSON string or raw `.fdraft` bytes (Phase 3d: a content-addressed or
   *  legacy ZIP → its project is read back, mesh blobs re-inlined; legacy
   *  JSON bytes pass through). */
  async addPartToAssembly(
    source: string | Uint8Array | ArrayBuffer,
    name?: string,
  ): Promise<void> {
    const asm = this.state.assembly;
    if (!asm.active) return;

    let documentJson: string;
    if (typeof source === "string") {
      documentJson = source;
    } else {
      const u8 = source instanceof Uint8Array ? source : new Uint8Array(source);
      const isZip = u8.length >= 2 && u8[0] === 0x50 && u8[1] === 0x4b;
      if (isZip) {
        const project = await readFdraftProject(u8);
        if (!project) return;
        documentJson = serializeProjectJson(project);
      } else {
        documentJson = new TextDecoder().decode(u8);
      }
    }

    const id = allocateAssemblyId();
    // Derive the default name from the highest existing "Part N" so removing a
    // part and adding another doesn't reuse a name that's still in use.
    const maxPartNumber = asm.document.parts.reduce((max, p) => {
      const m = /^Part (\d+)$/.exec(p.name);
      return m ? Math.max(max, Number(m[1])) : max;
    }, 0);
    const partName = name ?? `Part ${maxPartNumber + 1}`;

    const meshBodies: AssemblyMeshedBody[] = [];
    const mateConnectors: MateConnectorDef[] = [];
    const savedJson = this.kernel.toJson();
    const syncChanged = this.kernel.setSyncMeshing(true);
    try {
      this.kernel.loadJson(documentJson);
      const bodyCount = this.kernel.bodyCount();
      for (let i = 0; i < bodyCount; i++) {
        meshBodies.push({
          positions: this.kernel.bodyPositionsView(i).slice(),
          normals: this.kernel.bodyNormalsView(i).slice(),
          indices: this.kernel.bodyIndicesView(i).slice(),
        });
      }
      // C2: mate-connector CSYS is resolved by the kernel from the
      // face query during the regeneration triggered by loadJson above.
      for (const c of this.kernel.resolvedMateConnectors()) {
        mateConnectors.push({
          featureId: c.id ?? 0,
          origin: [c.origin[0], c.origin[1], c.origin[2]],
          normal: [c.normal[0], c.normal[1], c.normal[2]],
        });
      }
    } catch {
      // Meshing failed — add the part with empty meshes
    } finally {
      // Restore the live document. If this ever throws the main kernel state
      // would be lost, so surface it loudly rather than swallowing it.
      try {
        this.kernel.loadJson(savedJson);
      } catch (err) {
        console.error(
          "fielddraft: failed to restore document after addPartToAssembly",
          err,
        );
      }
      if (syncChanged) this.kernel.setSyncMeshing(false);
    }

    const newPart: AssemblyPartInstance = {
      id,
      name: partName,
      documentJson,
      transform: IDENTITY_TRANSFORM,
      meshBodies,
      mateConnectors,
    };

    this.updateAssemblyDocument({
      ...asm.document,
      parts: [...asm.document.parts, newPart],
    });
  }

  removePartFromAssembly(partId: string): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.updateAssemblyDocument({
      ...asm.document,
      parts: asm.document.parts.filter((p) => p.id !== partId),
      mates: asm.document.mates.filter(
        (m) => m.connectorA.partId !== partId && m.connectorB.partId !== partId,
      ),
    });
    if (asm.selectedPartId === partId) this.selectAssemblyPart(null);
  }

  selectAssemblyPart(partId: string | null): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.state = {
      ...this.state,
      assembly: { ...asm, selectedPartId: partId },
    };
    this.notify();
  }

  setPartTransform(
    partId: string,
    translate: [number, number, number],
    rotate: [number, number, number],
  ): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.updateAssemblyDocument({
      ...asm.document,
      parts: asm.document.parts.map((p) =>
        p.id === partId ? { ...p, transform: { translate, rotate } } : p,
      ),
    });
  }

  addMate(
    type: MateType,
    partAId: string,
    connAFeatureId: number,
    partBId: string,
    connBFeatureId: number,
  ): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    const mate: Mate = {
      id: allocateAssemblyId(),
      name: `${type} mate`,
      type,
      connectorA: { partId: partAId, connectorFeatureId: connAFeatureId },
      connectorB: { partId: partBId, connectorFeatureId: connBFeatureId },
      angle: type === "revolute" ? 0 : undefined,
      offset: type === "prismatic" ? 0 : undefined,
    };
    const newDoc = {
      ...asm.document,
      mates: [...asm.document.mates, mate],
    };
    const solved = solveAssembly({ ...newDoc, parts: asm.document.parts });
    this.updateAssemblyDocument({ ...newDoc, parts: solved });
  }

  removeMate(mateId: string): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.updateAssemblyDocument({
      ...asm.document,
      mates: asm.document.mates.filter((m) => m.id !== mateId),
    });
  }

  setMateAngle(mateId: string, angleDeg: number): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    const newMates = asm.document.mates.map((m) =>
      m.id === mateId ? { ...m, angle: angleDeg } : m,
    );
    const newDoc = { ...asm.document, mates: newMates };
    const solved = solveAssembly({ ...newDoc, parts: asm.document.parts });
    this.updateAssemblyDocument({ ...newDoc, parts: solved });
  }

  setAssemblyScript(source: string): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.updateAssemblyDocument({ ...asm.document, script: source });
  }

  runFieldScript(
    params: Record<string, number | boolean>,
    time: number,
    dt: number,
  ): string | null {
    const asm = this.state.assembly;
    if (!asm.active || !asm.document.script) return null;

    const currentAngles = new Map<string, number>();
    for (const mate of asm.document.mates) {
      if (mate.type === "revolute") {
        currentAngles.set(mate.id, mate.angle ?? 0);
      }
    }

    const result = executeScript(
      asm.document.script,
      params,
      time,
      dt,
      currentAngles,
    );

    if (!result.success) return result.error ?? "script error";

    if (result.jointUpdates.size > 0) {
      let newMates = [...asm.document.mates];
      for (const [mateId, angle] of result.jointUpdates) {
        newMates = newMates.map((m) => (m.id === mateId ? { ...m, angle } : m));
      }
      const newDoc = { ...asm.document, mates: newMates };
      const solved = solveAssembly({ ...newDoc, parts: asm.document.parts });
      this.updateAssemblyDocument({ ...newDoc, parts: solved });
    }

    return null;
  }

  exportR3F(): void {
    if (!this.state.assembly.active) return;
    import("./exportR3F")
      .then(({ downloadR3FExport }) => {
        downloadR3FExport(this.state.assembly.document);
      })
      .catch((err) => {
        console.error("fielddraft: R3F export failed", err);
      });
  }

  toggleIKMode(): void {
    this.patch({ ikMode: !this.state.ikMode });
    this.notify();
  }

  runIK(target: IKTarget): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    const solved = solveIK(asm.document, target);
    if (!solved) return;
    const newDoc = { ...asm.document, mates: solved };
    const parts = solveAssembly({ ...newDoc, parts: asm.document.parts });
    this.updateAssemblyDocument({ ...newDoc, parts });
  }

  setMateOffset(mateId: string, offsetMm: number): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    const newMates = asm.document.mates.map((m) =>
      m.id === mateId ? { ...m, offset: offsetMm } : m,
    );
    const newDoc = { ...asm.document, mates: newMates };
    const solved = solveAssembly({ ...newDoc, parts: asm.document.parts });
    this.updateAssemblyDocument({ ...newDoc, parts: solved });
  }

  setMateJointLimits(mateId: string, minAngle: number, maxAngle: number): void {
    const asm = this.state.assembly;
    if (!asm.active) return;
    this.updateAssemblyDocument({
      ...asm.document,
      mates: asm.document.mates.map((m) =>
        m.id === mateId ? { ...m, minAngle, maxAngle } : m,
      ),
    });
  }
}
