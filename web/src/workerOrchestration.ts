/**
 * WorkerOrchestrator — mesher- and weaver-worker orchestration.
 *
 * Owns the two latest-wins channels (see [`workerChannel.ts`]) to the
 * off-thread kernels: the mesher (Dual-Contouring render meshes,
 * coarse→refine ladder) and the B-Rep weaver (analytic edge/face
 * weaving; while it's up the main kernel stays permanently
 * B-Rep-suppressed). Also carries the render/drag toggles that gate
 * the scheduling: DC meshing on/off + algorithm, parametric weave,
 * drag-time B-Rep suppression, live extrude drag. Falls back to
 * synchronous main-thread meshing/weaving when the Worker constructor
 * throws (e.g. in Vitest).
 */

import type { Kernel } from "./kernel";
import type {
  MesherBodiesResponse,
  MesherBodyPayload,
  MesherRebuildRequest,
} from "./mesher.worker";
import type {
  WeaveBrepPayload,
  WeaveBrepsResponse,
  WeaveRequest,
} from "./weaver.worker";
import {
  CoalescingWorkerChannel,
  type ChannelEvent,
  type OutboundRequest,
} from "./workerChannel";
import {
  StoreCore,
  type MeshedBody,
  type StoreState,
} from "./storeCore";

/** Map mesher-worker messages onto channel events. Other traffic on the
 *  shared worker (`fd_result` responses for the Outer API) is ignored. */
function parseMesherResponse(
  data: unknown,
): ChannelEvent<MesherBodyPayload[]> | null {
  const msg = data as MesherBodiesResponse | null | undefined;
  if (!msg || typeof msg !== "object" || msg.type !== "bodies") return null;
  return msg.ok
    ? { kind: "result", requestId: msg.requestId, value: msg.bodies }
    : { kind: "error", requestId: msg.requestId, error: msg.error };
}

/** Map weaver-worker messages onto channel events. */
function parseWeaverResponse(
  data: unknown,
): ChannelEvent<WeaveBrepPayload[]> | null {
  const msg = data as WeaveBrepsResponse | null | undefined;
  if (!msg || typeof msg !== "object" || msg.type !== "breps") return null;
  return msg.ok
    ? { kind: "result", requestId: msg.requestId, value: msg.items }
    : { kind: "error", requestId: msg.requestId, error: msg.error };
}

export class WorkerOrchestrator {
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

  /** Latest-wins channel to the mesher worker: rapid-fire mutations
   *  coalesce into one follow-up rebuild built against the freshest
   *  document; stale responses are dropped; worker errors surface via
   *  `reportWorkerError` instead of masking as an empty mesh set. */
  private mesherChannel: CoalescingWorkerChannel<MesherBodyPayload[]> | null =
    null;
  /** Fallback mode: when the Worker constructor throws (e.g. the
   *  bundler didn't emit the worker chunk), we keep sync meshing on
   *  the main-thread kernel and drive the meshedBodies cache from
   *  its own `body*` views. Loses the off-thread benefit but keeps
   *  the app rendering. */
  private mesherAvailable = false;
  private dcMeshingEnabled = true;

  /** B-Rep weaver worker (third kernel instance). While it's up, the
   *  main kernel's B-Rep exports stay PERMANENTLY suppressed
   *  (cache-or-empty, never a synchronous main-thread weave); after
   *  every commit the stale bodies are farmed out here and the woven
   *  results come back as MessagePack buffers for `importBodyBrep`.
   *  Same latest-wins channel semantics as the mesher. */
  private weaverChannel: CoalescingWorkerChannel<WeaveBrepPayload[]> | null =
    null;
  private weaverAvailable = false;
  /** Mirror of the parametric-weave setting for weave requests. */
  private parametricWeaveEnabled = true;

  private meshAlgorithm: string = "mdc";

  constructor(private readonly core: StoreCore) {
      // Bring up the mesher worker BEFORE the initial document load: with the
      // worker available, sync meshing is disabled first, so `loadJson` below
      // neither runs the Dual-Contouring pass on the main thread nor enters
      // the kernel's mesh face-tagging loop — which would weave every body's
      // B-Rep synchronously and freeze the tab for minutes on weave-heavy
      // projects (spline-revolve + shell). On failure, sync meshing stays on
      // (functionally correct, just blocks the UI on every mesh rebuild).
      try {
        const mesher = new Worker(
          new URL("./mesher.worker.ts", import.meta.url),
          { type: "module" },
        );
        this.mesherChannel = new CoalescingWorkerChannel<MesherBodyPayload[]>({
          worker: mesher,
          parse: parseMesherResponse,
          buildRequest: (requestId, pass) =>
            this.buildMesherRequest(requestId, pass),
          onResult: (bodies) => this.applyMesherBodies(bodies),
          onError: (error) => this.reportWorkerError("Mesh rebuild", error),
          // M13: progressive refinement — the initial pass runs at coarse
          // resolution; its response auto-posts a full-resolution follow-up.
          followUpAfterInitial: true,
        });
        this.mesherAvailable = true;
        this.kernel.setSyncMeshing(false);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn(
          "fielddraft: could not start mesher worker, falling back to sync meshing on the main thread",
          err,
        );
        this.mesherAvailable = false;
      }

      // Bring up the B-Rep weaver worker. With it available, the main
      // kernel never weaves synchronously for an export again: exports are
      // permanently suppressed to cache-or-empty, and `scheduleWeave`
      // (called after every commit) keeps the caches fresh off-thread.
      // On failure the legacy behaviour remains: overlays weave
      // synchronously on first fetch.
      try {
        const weaver = new Worker(
          new URL("./weaver.worker.ts", import.meta.url),
          { type: "module" },
        );
        this.weaverChannel = new CoalescingWorkerChannel<WeaveBrepPayload[]>({
          worker: weaver,
          parse: parseWeaverResponse,
          buildRequest: (requestId) => this.buildWeaveRequest(requestId),
          onResult: (items) => this.applyWovenBreps(items),
          onError: (error) => this.reportWorkerError("B-Rep weave", error),
        });
        this.weaverAvailable = true;
        this.kernel.setBrepSuppressed(true);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn(
          "fielddraft: could not start weaver worker, falling back to synchronous B-Rep weaving",
          err,
        );
        this.weaverAvailable = false;
      }
  }

  /** False when the mesher worker could not start — the store then runs
   *  the sync-meshing fallback on the main-thread kernel. */
  get workerAvailable(): boolean {
    return this.mesherAvailable;
  }

  /**
   * Pull the latest meshes into `state.meshedBodies`. In worker mode
   * this posts a rebuild request and waits for the response
   * asynchronously; in fallback mode (no worker) it snapshots the
   * main-thread kernel's already-synced body buffers right now.
   * Callers are responsible for notifying listeners after — the
   * worker path notifies from inside `applyMesherBodies`, the
   * fallback path mutates state inline so the caller's post-call
   * listener sweep picks it up.
   */
  refreshMeshCache(): void {
    if (this.mesherAvailable) {
      this.scheduleWorkerRebuild();
      return;
    }
    if (!this.dcMeshingEnabled) return;
    this.state = {
      ...this.state,
      meshedBodies: this.snapshotMainThreadBodies(),
    };
  }

  setDcMeshingEnabled(enabled: boolean, algorithm?: string): void {
    const algoChanged =
      algorithm !== undefined && algorithm !== this.meshAlgorithm;
    if (algorithm !== undefined) this.meshAlgorithm = algorithm;
    if (this.dcMeshingEnabled === enabled && !algoChanged) return;
    this.dcMeshingEnabled = enabled;
    if (enabled) {
      this.scheduleWorkerRebuild();
    }
  }

  /** Milestone-1 parametric weaving toggle. Only the woven B-Rep changes (the
   * DC render mesh is identical), so we just flip the kernel flag and notify —
   * the kernel bumps each body's version, which refetches the B-Rep edge /
   * face overlays. In weaver mode the cleared caches re-weave off-thread. */
  setParametricWeave(enabled: boolean): void {
    this.parametricWeaveEnabled = enabled;
    if (this.kernel.setParametricWeave(enabled)) {
      this.scheduleWeave();
      this.notify();
    }
  }

  /** Setting "B-Rep rebuild while dragging" (default false): when off, a
   * gizmo drag suppresses every B-Rep weave for its whole duration — the
   * throttled drag commits only re-mesh, and the exact B-Rep weaves once
   * on release (`endGizmoDrag`). When on, drags keep the historical
   * live-weave behaviour. */
  private dragBrepRecompute = false;
  private gizmoDragActive = false;

  setDragBrepRecompute(enabled: boolean): void {
    this.dragBrepRecompute = enabled;
    // Flipping the setting ON mid-drag lifts an active suppression.
    if (enabled && this.gizmoDragActive && this.state.gizmoDragSuppressed) {
      if (this.weaverAvailable) {
        this.scheduleWeave();
      } else {
        this.kernel.setBrepSuppressed(false);
      }
      this.patch({ gizmoDragSuppressed: false });
      this.notify();
    }
  }

  /** Setting "Live extrude drag" (default true): the extrude depth gizmo
   * commits the depth throttled WHILE the pointer is down — the renderer
   * shows the real body live instead of an approximate preview prism.
   * The B-Rep weave stays governed by `dragBrepRecompute` via
   * begin/endGizmoDrag, exactly like the transform gizmo. */
  private liveExtrudeDrag = true;

  setLiveExtrudeDrag(enabled: boolean): void {
    this.liveExtrudeDrag = enabled;
  }

  isLiveExtrudeDrag(): boolean {
    return this.liveExtrudeDrag;
  }

  /** Called by the transform gizmo when a drag starts (dragging-changed).
   * `gizmoDragSuppressed` hides all B-Rep overlays — even cached
   * (pure-move) edges only refresh at the 50ms commit cadence and would
   * visibly trail the drag. With the weaver worker up the main kernel is
   * permanently suppressed anyway and `scheduleWeave` skips scheduling
   * for the drag's duration; the legacy (no-weaver) path additionally
   * toggles the kernel suppression flag. */
  beginGizmoDrag(): void {
    this.gizmoDragActive = true;
    if (!this.dragBrepRecompute) {
      if (!this.weaverAvailable) this.kernel.setBrepSuppressed(true);
      this.patch({ gizmoDragSuppressed: true });
      this.notify();
    }
  }

  /** Called when the drag ends (or the gizmo unmounts mid-drag). Weaver
   * mode schedules the off-thread catch-up weave; the legacy path
   * un-suppresses the kernel, which weaves each stale body synchronously
   * once and bumps its version so overlays refetch. Idempotent. */
  endGizmoDrag(): void {
    if (!this.gizmoDragActive) return;
    this.gizmoDragActive = false;
    if (this.weaverAvailable) {
      this.scheduleWeave();
      if (this.state.gizmoDragSuppressed) {
        this.patch({ gizmoDragSuppressed: false });
        this.notify();
      }
      return;
    }
    const kernelChanged = this.kernel.setBrepSuppressed(false);
    if (this.state.gizmoDragSuppressed) {
      this.patch({ gizmoDragSuppressed: false });
      this.notify();
    } else if (kernelChanged) {
      this.notify();
    }
  }

  /** Hand off the current document to the mesher worker. Coalescing,
   *  latest-wins and the coarse→refine ladder live in the channel;
   *  this is just the trigger. */
  scheduleWorkerRebuild(): void {
    if (!this.dcMeshingEnabled) return;
    this.mesherChannel?.schedule();
  }

  /** Channel callback: build the freshest rebuild request. The initial
   *  pass of a burst runs at coarse (half) resolution for a fast first
   *  paint (M13); the automatic follow-up refines at full resolution. */
  private buildMesherRequest(
    requestId: number,
    pass: "initial" | "followup",
  ): OutboundRequest | null {
    if (!this.dcMeshingEnabled) return null;
    const fullRes = this.kernel.meshResolution();
    const coarseRes = Math.max(8, Math.floor(fullRes / 2));
    const request: MesherRebuildRequest = {
      type: "rebuild",
      requestId,
      documentJson: this.kernel.toJson(),
      activeMeshResolution: pass === "initial" ? coarseRes : fullRes,
      contextMeshResolution: this.kernel.contextMeshResolution(),
      activeBodyId: this.kernel.activeBodyId(),
      meshAlgorithm: this.meshAlgorithm,
    };
    return { message: request };
  }

  /** Channel callback: publish the latest mesh set from the worker. */
  private applyMesherBodies(bodies: MesherBodyPayload[]): void {
    this.state = {
      ...this.state,
      meshedBodies: bodies,
      sceneSize: this.kernel.sceneAabb().size as [number, number, number],
    };
    this.core.updateSceneBounds();
    this.core.emit();
  }

  /** Channel callback: a worker job failed. Surface it — log + transient
   *  error banner — instead of masking it as an empty result (which used
   *  to silently blank the viewport / drop the weave). */
  private reportWorkerError(job: string, error: string): void {
    // eslint-disable-next-line no-console
    console.error(`fielddraft: ${job} failed in worker: ${error}`);
    this.patch({ transientError: `${job} failed: ${error}` });
    this.notify();
  }

  /**
   * Farm the stale B-Rep weaves out to the weaver worker. Rapid-fire
   * commits coalesce exactly like mesh rebuilds (latest-wins channel).
   * During a suppressed gizmo drag `buildWeaveRequest` returns null, so
   * nothing is posted — `endGizmoDrag` runs the catch-up schedule.
   */
  scheduleWeave(): void {
    this.weaverChannel?.schedule();
  }

  /** Channel callback: build the freshest weave request, or null when
   *  there is nothing to weave right now (no stale bodies, or a
   *  suppressed gizmo drag is running). */
  private buildWeaveRequest(requestId: number): OutboundRequest | null {
    if (this.gizmoDragActive && !this.dragBrepRecompute) return null;
    const stale = this.kernel.bodyBrepStaleIds();
    if (stale.length === 0) return null;
    const request: WeaveRequest = {
      type: "weave",
      requestId,
      documentJson: this.kernel.toJson(),
      parametricWeave: this.parametricWeaveEnabled,
      bodyIds: stale,
    };
    return { message: request };
  }

  /** Channel callback: install freshly woven B-Reps into the main kernel. */
  private applyWovenBreps(items: WeaveBrepPayload[]): void {
    let imported = false;
    for (const item of items) {
      // Validated against the CURRENT placement-invariant key — a
      // result that raced a newer edit misses here; the channel's
      // queued re-schedule covers it against the latest document.
      if (this.kernel.importBodyBrep(item.id, item.bytes)) imported = true;
    }
    if (imported) {
      // Publish a fresh snapshot so `useSyncExternalStore` actually re-renders
      // — `notify()` alone leaves the `getState()` reference unchanged, and the
      // viewport's edge overlay would keep its stale empty memo (the freshly
      // extruded body would render without its B-Rep edges until an unrelated
      // state change). The kernel already bumped each body's version on import;
      // a new state reference is all the overlays need to refetch.
      this.patch({ brepVersion: this.state.brepVersion + 1 });
      this.notify();
    }
  }

  /**
   * Fallback path for when the worker couldn't start: read per-body
   * meshes from the main-thread kernel's sync-meshing output
   * directly, copying each TypedArray out of WASM linear memory so
   * the store can retain it across future mutations.
   */
  snapshotMainThreadBodies(): MeshedBody[] {
    const count = this.kernel.bodyCount();
    const out: MeshedBody[] = [];
    for (let i = 0; i < count; i++) {
      let transformDelta: [number, number, number] | null = null;
      if (this.kernel.bodyHasTransformDelta(i)) {
        const d = this.kernel.bodyTransformDelta(i);
        transformDelta = [d[0], d[1], d[2]];
      }
      out.push({
        id: this.kernel.bodyId(i),
        seedId: this.kernel.bodySeedFeatureId(i),
        version: this.kernel.bodyVersion(i),
        positions: this.kernel.bodyPositionsView(i).slice(),
        normals: this.kernel.bodyNormalsView(i).slice(),
        indices: this.kernel.bodyIndicesView(i).slice(),
        classifications: this.kernel.bodyClassificationsView(i).slice(),
        contributors: this.kernel.bodyContributorsView(i).slice(),
        transformDelta,
      });
    }
    return out;
  }
}
