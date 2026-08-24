import { frameKey } from '../wire.js';
import { CmaError, ProtocolError, RequestAborted, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { toEvent, type AguiEvent } from './events.js';

/**
 * The AG-UI connection: its cursor, its reconnect order, and its deduplication.
 *
 * `docs/API_Overview.md` §3 states the contract precisely:
 *
 * > Clients reconnect by opening the stream first, then reading history, then
 * > deduplicating on event id — never by assuming the stream replays.
 *
 * **The order is enforced here rather than left to a caller to remember**,
 * because getting it wrong loses events in exactly the window nobody tests:
 * read history first and anything that lands between the read and the open is
 * gone, and there is nothing afterwards that would notice.
 *
 * Durable events carry a **per-session** monotonic `seq`, not a global one, so
 * the cursor is per session and is the only resumption state there is. The
 * transport projects that cursor into the protocol's resume query and, where
 * supported, `Last-Event-ID`; callers do not have a second cursor to maintain.
 *
 * No React here. This is a protocol client and it is testable without a DOM.
 */

/** Where a driver gets its frames. Implemented by the wire and by the mock. */
export interface AguiTransport {
  /**
   * Open the live stream. Frames arrive on `onFrame` until `close` is called.
   *
   * Opened **before** history is read. A transport that buffers is welcome to;
   * one that drops what arrives before the first read is what this order
   * exists to survive.
   */
  open(
    session: string,
    cursor: number,
    onFrame: (raw: unknown) => void,
    onDrop: (cause: unknown) => void,
    options?: OperationOptions,
  ): () => void;
  /** Durable events after `cursor`, oldest first. */
  history(session: string, cursor: number, options?: OperationOptions): Promise<readonly unknown[]>;
}

export interface ConnectionOptions extends OperationOptions {
  /** How long to wait before the first reconnect attempt. */
  readonly backoffMs?: number;
  /** The ceiling the backoff climbs to. */
  readonly maxBackoffMs?: number;
  /** Injected so tests do not wait in real time. */
  readonly sleep?: (ms: number) => Promise<void>;
  /** Fractional random spread around backoff, from zero through one. */
  readonly jitterRatio?: number;
  readonly random?: () => number;
}

const DEFAULT_BACKOFF = 500;
const DEFAULT_MAX_BACKOFF = 15_000;
const DEFAULT_JITTER_RATIO = 0.2;

/**
 * One session's stream, resumable.
 *
 * `subscribe` yields every durable event exactly once, in sequence order,
 * whether it was watched arriving or read from history — which is the property
 * §18.1 of `Web_Style.md` turns on, and the reason deduplication lives here
 * rather than in whatever draws the conversation.
 */
export class AguiConnection {
  #cursor = 0;
  /*
   * Frames already delivered, keyed by the durable event they came from AND
   * the kind of frame it projected to.
   *
   * Not by sequence alone. One durable event legitimately projects to several
   * frames — a committed message becomes TEXT_MESSAGE_START, then CONTENT,
   * then END — and every one of them carries the sequence of the event it was
   * projected from, because that sequence is the cursor a reconnect resumes
   * on. Keyed by sequence alone, the first frame was delivered and its
   * siblings were dropped as duplicates: every agent message opened and then
   * received none of its own text. No projection emits the same kind twice
   * for one event, so this pair is exact.
   *
   * The kind is the one that arrived on the wire, not the one this build
   * normalised it to: everything unfamiliar normalises to UNKNOWN, so two
   * different unfamiliar kinds at one sequence would collide precisely where
   * a client can least afford to drop something it does not understand.
   */
  #seen = new Set<string>();
  #closed = false;
  #stopStream: (() => void) | undefined;
  /** Ends the current wait, so closing does not have to wait for a drop. */
  #wake: (() => void) | undefined;

  constructor(
    private readonly transport: AguiTransport,
    private readonly session: string,
    private readonly options: ConnectionOptions = {},
  ) {}

  /** The highest sequence delivered. Resumption state, and nothing else. */
  get cursor(): number {
    return this.#cursor;
  }

  /**
   * Start delivering events, reconnecting on its own until closed.
   *
   * Returns a function that stops it. Calling it twice is safe.
   */
  subscribe(
    onEvent: (event: AguiEvent) => void,
    onStatus?: (status: LinkStatus) => void,
    onError?: (error: CmaError) => void,
  ): () => void {
    void this.#run(onEvent, onStatus, onError).catch((cause: unknown) => {
      this.#closed = true;
      this.#stopStream?.();
      this.#stopStream = undefined;
      this.#wake?.();
      const error =
        cause instanceof CmaError
          ? cause
          : new ProtocolError('the AG-UI connection failed while delivering an event', { cause });
      onError?.(error);
      onStatus?.('closed');
    });
    return () => {
      this.#closed = true;
      this.#stopStream?.();
      this.#stopStream = undefined;
      this.#wake?.();
      onStatus?.('closed');
    };
  }

  async #run(
    onEvent: (event: AguiEvent) => void,
    onStatus?: (status: LinkStatus) => void,
    onError?: (error: CmaError) => void,
  ) {
    let backoff = this.options.backoffMs ?? DEFAULT_BACKOFF;
    const ceiling = this.options.maxBackoffMs ?? DEFAULT_MAX_BACKOFF;
    const jitterRatio = this.options.jitterRatio ?? DEFAULT_JITTER_RATIO;
    const random = this.options.random ?? Math.random;
    const sleep = this.options.sleep ?? ((ms: number) => new Promise((r) => setTimeout(r, ms)));
    const invalid = validateOptions(backoff, ceiling, jitterRatio);
    if (invalid !== undefined) {
      onError?.(invalid);
      onStatus?.('closed');
      return;
    }
    if (this.options.signal?.aborted === true) {
      onError?.(new RequestAborted(undefined, { cause: this.options.signal.reason }));
      onStatus?.('closed');
      return;
    }
    const onAbort = () => {
      this.#closed = true;
      this.#stopStream?.();
      this.#wake?.();
    };
    this.options.signal?.addEventListener('abort', onAbort, { once: true });

    try {
      while (!this.#closed) {
        /*
         * Resolved when the stream ends — because the server closed it, the
         * network dropped it, or the caller closed us. Awaited rather than
         * polled: a loop that sleeps and re-checks is a busy-wait that starves
         * whatever else is on the queue, and it makes the connection's lifetime
         * depend on a timer rather than on the event it is actually waiting for.
         */
        let settle: () => void = () => {};
        const ended = new Promise<void>((resolve) => {
          settle = resolve;
        });
        this.#wake = settle;

        /*
         * The stream first. Anything arriving from here on is held by the
         * deduplication below rather than lost to the gap between reading
         * history and opening a connection.
         */
        const pending: unknown[] = [];
        let live = false;
        let dropped = false;
        let dropCause: unknown;
        const drop = (cause: unknown) => {
          if (!dropped) {
            dropped = true;
            dropCause = cause;
          }
          settle();
        };
        this.#stopStream = this.transport.open(
          this.session,
          this.#cursor,
          (raw) => {
            try {
              if (live) {
                this.#deliver(raw, onEvent);
              } else {
                // Held until history has been replayed, so the caller sees one
                // ordered sequence rather than the future before the past.
                pending.push(raw);
              }
            } catch (cause) {
              drop(cause);
            }
          },
          drop,
          this.options,
        );

        // Then history, from the cursor. Then deduplicate — which the delivery
        // below does on `seq`, so a frame seen twice is delivered once.
        try {
          for (const raw of await this.transport.history(
            this.session,
            this.#cursor,
            this.options,
          )) {
            this.#deliver(raw, onEvent);
          }
        } catch (cause) {
          // A history read that failed is a reconnect, not a lost session. The
          // cursor has not moved, so the next attempt asks for the same range.
          this.#stopStream?.();
          this.#stopStream = undefined;
          if (cause instanceof ProtocolError) {
            onError?.(cause);
            onStatus?.('closed');
            return;
          }
          onStatus?.('reconnecting');
          if (this.#closed) {
            return;
          }
          await sleep(jittered(backoff, jitterRatio, random, onError));
          backoff = Math.min(backoff * 2, ceiling);
          continue;
        }

        live = true;
        try {
          for (const raw of pending.splice(0)) {
            this.#deliver(raw, onEvent);
          }
        } catch (cause) {
          drop(cause);
        }
        if (!dropped) {
          onStatus?.('live');
          backoff = this.options.backoffMs ?? DEFAULT_BACKOFF;
        }

        await ended;
        this.#wake = undefined;
        this.#stopStream?.();
        this.#stopStream = undefined;
        if (this.#closed) {
          return;
        }
        if (dropCause instanceof ProtocolError) {
          onError?.(dropCause);
          onStatus?.('closed');
          return;
        }
        if (dropCause instanceof RequestAborted) {
          onError?.(dropCause);
          onStatus?.('closed');
          return;
        }
        onStatus?.('reconnecting');
        await sleep(jittered(backoff, jitterRatio, random, onError));
        backoff = Math.min(backoff * 2, ceiling);
      }
    } finally {
      this.options.signal?.removeEventListener('abort', onAbort);
      if (isAborted(this.options.signal)) {
        onError?.(new RequestAborted(undefined, { cause: this.options.signal?.reason }));
        onStatus?.('closed');
      }
    }
  }

  /**
   * Deliver one frame, once.
   *
   * Deduplicated on the durable event a frame was projected from together
   * with the kind of frame it is, which is what the reconnect order above
   * depends on: opening the stream before reading history guarantees an
   * overlap, and this is what makes the overlap harmless without discarding
   * the siblings of a frame already seen.
   */
  #deliver(raw: unknown, onEvent: (event: AguiEvent) => void) {
    const event = toEvent(raw);
    /*
     * Liveness passes straight through, touching neither the cursor nor the
     * seen-set. It has no sequence — what it arrives with is whatever id the
     * last durable frame carried — so recording it would mark a durable
     * event as already delivered and drop the real one when it comes, and
     * every piece of a message after the first would collide with the piece
     * before it and be dropped in silence.
     */
    if (event.ephemeral === true) {
      onEvent(event);
      return;
    }
    const wireKind = event.type === 'UNKNOWN' ? event.kind : event.type;
    const delivered = frameKey(event.seq, wireKind);
    if (this.#seen.has(delivered)) {
      return;
    }
    this.#seen.add(delivered);
    if (event.seq > this.#cursor) {
      this.#cursor = event.seq;
    }
    onEvent(event);
  }
}

/** Where the link is, for a surface that has to say so. */
export type LinkStatus = 'live' | 'reconnecting' | 'closed';

function validateOptions(
  backoff: number,
  ceiling: number,
  jitterRatio: number,
): ValidationError | undefined {
  if (!Number.isSafeInteger(backoff) || backoff < 0) {
    return new ValidationError('backoffMs must be a non-negative safe integer');
  }
  if (!Number.isSafeInteger(ceiling) || ceiling < backoff) {
    return new ValidationError('maxBackoffMs must be a safe integer no smaller than backoffMs');
  }
  if (!Number.isFinite(jitterRatio) || jitterRatio < 0 || jitterRatio > 1) {
    return new ValidationError('jitterRatio must be a finite number from zero through one');
  }
  return undefined;
}

function jittered(
  delay: number,
  ratio: number,
  random: () => number,
  onError: ((error: CmaError) => void) | undefined,
): number {
  const sample = random();
  if (!Number.isFinite(sample) || sample < 0 || sample > 1) {
    const error = new ValidationError(
      'connection random source must return a finite number from zero through one',
    );
    onError?.(error);
    return delay;
  }
  return Math.max(0, Math.round(delay * (1 + (sample * 2 - 1) * ratio)));
}

function isAborted(signal: AbortSignal | undefined): boolean {
  return signal?.aborted === true;
}
