/**
 * Generic main-thread ⇄ worker request channels.
 *
 * Every worker conversation in the app follows one of two patterns, both
 * built on requestId correlation (the request carries a channel-unique id,
 * every response echoes it):
 *
 *   * {@link CorrelatedWorkerChannel} — promise per request, any number in
 *     flight, responses may arrive in any order. Errors reject the matching
 *     promise; progress messages stream to the matching caller. Used by the
 *     Outer API (`fd_call`, outerApi.ts) and the OCCT STEP worker — where the
 *     correlation is what stops a parallel export and import from capturing
 *     each other's answers.
 *
 *   * {@link CoalescingWorkerChannel} — latest-wins, single-flight job
 *     pipeline. `schedule()` posts the freshest request when idle; while a
 *     request is in flight further schedules coalesce into ONE follow-up
 *     that is built fresh (against the latest document) when the response
 *     lands. Stale responses (older requestId) are dropped. Used by the
 *     mesher and weaver workers.
 *
 * Wire contract: the channel does not dictate a message shape — a `parse`
 * function maps incoming worker messages onto {@link ChannelEvent}s (and
 * returns `null` for messages that belong to someone else, so multiple
 * channels can share one worker, e.g. mesher rebuilds + fd_calls).
 * Worker-side failures MUST be posted as error events, never masked as an
 * empty result — the channel propagates them to the caller.
 */

/**
 * Minimal view of a Web Worker that the channels actually touch. Narrower
 * than the DOM `Worker` type so tests can supply mocks without implementing
 * `terminate`, `onerror`, etc.
 */
export interface WorkerLike {
  postMessage(message: unknown, transfer?: Transferable[]): void;
  addEventListener(
    type: "message",
    listener: (event: MessageEvent<unknown>) => void,
  ): void;
  removeEventListener(
    type: "message",
    listener: (event: MessageEvent<unknown>) => void,
  ): void;
}

/** A parsed worker message, normalised for the channel. */
export type ChannelEvent<TValue, TProgress = never> =
  | { kind: "result"; requestId: number; value: TValue }
  | { kind: "error"; requestId: number; error: string }
  | { kind: "progress"; requestId: number; progress: TProgress };

/**
 * Maps a raw worker message onto a {@link ChannelEvent}, or `null` when the
 * message is not addressed to this channel.
 */
export type ChannelParser<TValue, TProgress = never> = (
  data: unknown,
) => ChannelEvent<TValue, TProgress> | null;

/** An outgoing request plus its postMessage transfer list. */
export interface OutboundRequest {
  message: unknown;
  transfer?: Transferable[];
}

interface PendingCall<TValue, TProgress> {
  resolve: (value: TValue) => void;
  reject: (reason: Error) => void;
  onProgress?: (progress: TProgress) => void;
}

/**
 * Promise-per-request channel. Multiple calls may be in flight at once;
 * each response resolves (or rejects) exactly the promise whose requestId
 * it echoes — never a sibling's.
 */
export class CorrelatedWorkerChannel<TValue, TProgress = never> {
  private nextRequestId = 1;
  private readonly pending = new Map<number, PendingCall<TValue, TProgress>>();
  private readonly listener = (event: MessageEvent<unknown>): void => {
    this.onMessage(event.data);
  };

  constructor(
    private readonly worker: WorkerLike,
    private readonly parse: ChannelParser<TValue, TProgress>,
    /** Diagnostic label used in the dispose-rejection message. */
    private readonly name = "worker channel",
  ) {
    worker.addEventListener("message", this.listener);
  }

  /**
   * Post one request and await its correlated response. `build` receives the
   * fresh requestId and must embed it in the message; progress events for
   * this id stream to `onProgress`.
   */
  call(
    build: (requestId: number) => OutboundRequest,
    onProgress?: (progress: TProgress) => void,
  ): Promise<TValue> {
    const requestId = this.nextRequestId++;
    const outbound = build(requestId);
    return new Promise<TValue>((resolve, reject) => {
      this.pending.set(requestId, { resolve, reject, onProgress });
      this.worker.postMessage(outbound.message, outbound.transfer ?? []);
    });
  }

  /**
   * Release the worker listener. Outstanding calls are rejected with an
   * explicit cancellation error.
   */
  dispose(): void {
    this.worker.removeEventListener("message", this.listener);
    for (const { reject } of this.pending.values()) {
      reject(new Error(`${this.name} disposed with in-flight calls`));
    }
    this.pending.clear();
  }

  private onMessage(data: unknown): void {
    const event = this.parse(data);
    if (!event) return;
    const entry = this.pending.get(event.requestId);
    if (!entry) {
      // Response for a request we don't know about — either a stale message
      // after dispose, or another channel's traffic that the parser matched.
      // Drop silently; throwing from a message listener is unobservable.
      return;
    }
    if (event.kind === "progress") {
      entry.onProgress?.(event.progress);
      return;
    }
    this.pending.delete(event.requestId);
    if (event.kind === "result") {
      entry.resolve(event.value);
    } else {
      entry.reject(new Error(event.error));
    }
  }
}

export interface CoalescingChannelOptions<TValue> {
  worker: WorkerLike;
  parse: ChannelParser<TValue>;
  /**
   * Build the freshest request for `requestId` — called at POST time, never
   * earlier, so a coalesced follow-up always ships the latest document.
   * Return `null` when there is nothing to do right now (the channel stays
   * idle; the next `schedule()` re-asks).
   *
   * `pass` is `"initial"` for the burst-opening post and `"followup"` for
   * the automatic re-post after a response — the mesher uses this for its
   * coarse-first / full-res-after ladder.
   */
  buildRequest: (
    requestId: number,
    pass: "initial" | "followup",
  ) => OutboundRequest | null;
  /** Deliver the latest (non-stale) successful payload. */
  onResult: (value: TValue) => void;
  /** A worker-side failure for the latest request. The pipeline keeps
   *  moving (queued schedules still post); the error must be surfaced,
   *  not swallowed. */
  onError: (error: string) => void;
  /**
   * Post one automatic follow-up after the initial pass' response even
   * without queued schedules (the mesher's refine pass). Default off.
   */
  followUpAfterInitial?: boolean;
}

/**
 * Latest-wins job pipeline over one worker: at most one request in flight,
 * rapid-fire `schedule()`s coalesce into a single follow-up built fresh when
 * the in-flight response lands, stale responses are dropped.
 */
export class CoalescingWorkerChannel<TValue> {
  private nextRequestId = 1;
  /** requestId of the newest posted request; older responses are stale. */
  private latestRequestId = 0;
  private inflight = false;
  private queued = false;
  private followUpPending = false;
  private readonly listener = (event: MessageEvent<unknown>): void => {
    this.onMessage(event.data);
  };

  constructor(private readonly opts: CoalescingChannelOptions<TValue>) {
    opts.worker.addEventListener("message", this.listener);
  }

  /** Ask for a (re)run against the latest state. Coalesces while busy. */
  schedule(): void {
    if (this.inflight) {
      this.queued = true;
      return;
    }
    this.post("initial");
  }

  /** Release the worker listener. */
  dispose(): void {
    this.opts.worker.removeEventListener("message", this.listener);
  }

  private post(pass: "initial" | "followup"): void {
    const requestId = this.nextRequestId++;
    const outbound = this.opts.buildRequest(requestId, pass);
    this.queued = false;
    if (!outbound) return; // nothing to do right now — stay idle
    this.latestRequestId = requestId;
    this.inflight = true;
    this.followUpPending =
      pass === "initial" && this.opts.followUpAfterInitial === true;
    this.opts.worker.postMessage(outbound.message, outbound.transfer ?? []);
  }

  private onMessage(data: unknown): void {
    const event = this.opts.parse(data);
    if (!event || event.kind === "progress") return;
    if (event.requestId < this.latestRequestId) {
      // Stale response — a newer request is already in flight or queued.
      // Drop the payload (we'll deliver the fresh one shortly) but keep the
      // pipeline moving.
      this.followUpPending = false;
      if (this.queued) this.post("followup");
      return;
    }
    this.inflight = false;
    if (event.kind === "result") {
      this.opts.onResult(event.value);
    } else {
      this.opts.onError(event.error);
    }
    if (this.queued || this.followUpPending) {
      this.followUpPending = false;
      this.post("followup");
    }
  }
}
