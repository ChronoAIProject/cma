import { frameKey } from '../wire.js';
import {
  ApiError,
  CmaError,
  ProtocolError,
  RequestAborted,
  TransportError,
  ValidationError,
} from '../errors.js';
import type { OperationOptions } from '../http.js';
import { toEvent, type AguiEvent } from './events.js';
import { ProjectionAssembler } from './projection.js';

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
  /** Fraction of each backoff window eligible for jitter; one is full jitter. */
  readonly jitterRatio?: number;
  readonly random?: () => number;
}

const DEFAULT_BACKOFF = 500;
const DEFAULT_MAX_BACKOFF = 15_000;
const DEFAULT_JITTER_RATIO = 1;
const TERMINAL_HTTP_STATUSES = new Set([401, 403, 404, 410, 422]);

interface PendingProjection {
  readonly seq: number;
  readonly finalKey: string;
  readonly fingerprint: string;
  acknowledged: number;
}

class EventCallbackFailure extends Error {
  constructor(cause: unknown) {
    super('the AG-UI event consumer did not acknowledge a durable frame', { cause });
    this.name = 'EventCallbackFailure';
  }
}

/** Where the link is, for a surface that has to say so. */
export type LinkStatus = 'live' | 'reconnecting' | 'failed' | 'closed';

/** A permanent link failure, retaining the service status when there was one. */
export interface LinkFailure {
  readonly status?: number;
  readonly message: string;
}

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
   * Stamped frames already delivered at the current cursor, keyed by the
   * durable event they came from AND the kind of frame it projected to.
   * Advancing the cursor clears older keys, keeping this set bounded for a
   * connection that lives as long as its session.
   *
   * The kind remains part of the key for compatibility with control planes
   * that predate final-frame acknowledgement and stamp every frame of a
   * projection with the same sequence. No projection emits the same kind
   * twice for one event, so this pair is exact.
   *
   * The kind is the one that arrived on the wire, not the one this build
   * normalised it to: everything unfamiliar normalises to UNKNOWN, so two
   * different unfamiliar kinds at one sequence would collide precisely where
   * a client can least afford to drop something it does not understand.
   */
  #seen = new Set<string>();
  /** A complete projection whose consumer callback has not acknowledged every frame. */
  #pendingProjection: PendingProjection | undefined;
  #closed = false;
  /** The one terminal notification this one-shot connection has emitted. */
  #terminal: 'failed' | 'closed' | undefined;
  #stopStream: (() => void) | undefined;
  /** Ends the current wait, so closing does not have to wait for a drop. */
  #wake: (() => void) | undefined;

  private readonly options: ConnectionOptions;

  constructor(
    private readonly transport: AguiTransport,
    private readonly session: string,
    options: ConnectionOptions = {},
  ) {
    // A connection is one logical operation. Its private options object lets
    // the built-in transport retain one correlation id across every live and
    // history reconnect even when a caller reuses their input object elsewhere.
    this.options = { ...options };
  }

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
    onStatus?: (status: LinkStatus, failure?: LinkFailure) => void,
    onError?: (error: CmaError) => void,
  ): () => void {
    void this.#run(onEvent, onStatus, onError).catch((cause: unknown) => {
      this.#stopStream?.();
      this.#stopStream = undefined;
      this.#wake?.();
      const error =
        cause instanceof CmaError
          ? cause
          : new ProtocolError('the AG-UI connection failed while delivering an event', { cause });
      this.#fail(error, onStatus, onError);
    });
    return () => {
      if (this.#terminal !== undefined) {
        return;
      }
      this.#closed = true;
      this.#stopStream?.();
      this.#stopStream = undefined;
      this.#wake?.();
      this.#close(onStatus);
    };
  }

  async #run(
    onEvent: (event: AguiEvent) => void,
    onStatus?: (status: LinkStatus, failure?: LinkFailure) => void,
    onError?: (error: CmaError) => void,
  ) {
    let backoff = this.options.backoffMs ?? DEFAULT_BACKOFF;
    const ceiling = this.options.maxBackoffMs ?? DEFAULT_MAX_BACKOFF;
    const jitterRatio = this.options.jitterRatio ?? DEFAULT_JITTER_RATIO;
    const random = this.options.random ?? Math.random;
    const sleep = this.options.sleep ?? ((ms: number) => new Promise((r) => setTimeout(r, ms)));
    const invalid = validateOptions(backoff, ceiling, jitterRatio);
    if (invalid !== undefined) {
      this.#fail(invalid, onStatus, onError);
      return;
    }
    if (this.options.signal?.aborted === true) {
      this.#close(
        onStatus,
        onError,
        new RequestAborted(undefined, { cause: this.options.signal.reason }),
      );
      return;
    }
    const onAbort = () => {
      this.#closed = true;
      this.#stopStream?.();
      this.#stopStream = undefined;
      this.#wake?.();
      this.#close(
        onStatus,
        onError,
        new RequestAborted(undefined, { cause: this.options.signal?.reason }),
      );
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
        const streamProjection = new ProjectionAssembler();
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
              if (this.#closed || dropped) {
                return;
              }
              if (live) {
                this.#accept(raw, onEvent, streamProjection);
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

        // Then history, from the cursor. Cursorless durable frames are held
        // until their numbered final frame makes the projection an exact,
        // occurrence-aware deduplication unit.
        try {
          const history = await this.transport.history(this.session, this.#cursor, this.options);
          if (this.#closed) {
            return;
          }
          const historyProjection = new ProjectionAssembler();
          for (const raw of history) {
            if (this.#closed) {
              return;
            }
            this.#accept(raw, onEvent, historyProjection);
          }
          historyProjection.assertComplete('AG-UI history');
          if (this.#pendingProjection !== undefined) {
            throw new ProtocolError(
              `AG-UI history did not replay the unacknowledged projection at sequence ${String(this.#pendingProjection.seq)}`,
            );
          }
        } catch (cause) {
          this.#stopStream?.();
          this.#stopStream = undefined;
          if (this.#wake === settle) {
            this.#wake = undefined;
          }
          if (this.#closed) {
            return;
          }
          if (cause instanceof RequestAborted) {
            this.#close(onStatus, onError, cause);
            return;
          }
          const permanent = permanentFailure(cause);
          if (permanent !== undefined) {
            this.#fail(permanent, onStatus, onError);
            return;
          }
          // A transient history failure reconnects from the same cursor. A
          // permanent service answer above ends the loop instead of retrying
          // an operation whose result cannot change.
          onStatus?.('reconnecting');
          if (this.#closed) {
            return;
          }
          await this.#pause(sleep(jittered(backoff, jitterRatio, random)));
          if (this.#closed) {
            return;
          }
          backoff = Math.min(backoff * 2, ceiling);
          continue;
        }

        try {
          while (pending.length > 0) {
            if (this.#closed) {
              return;
            }
            const raw = pending.shift();
            this.#accept(raw, onEvent, streamProjection);
          }
        } catch (cause) {
          drop(cause);
        }
        live = true;
        if (!dropped) {
          onStatus?.('live');
          backoff = this.options.backoffMs ?? DEFAULT_BACKOFF;
        }

        await ended;
        if (this.#wake === settle) {
          this.#wake = undefined;
        }
        this.#stopStream?.();
        this.#stopStream = undefined;
        if (this.#closed) {
          return;
        }
        if (dropCause instanceof RequestAborted) {
          this.#close(onStatus, onError, dropCause);
          return;
        }
        const permanent = permanentFailure(dropCause);
        if (permanent !== undefined) {
          this.#fail(permanent, onStatus, onError);
          return;
        }
        onStatus?.('reconnecting');
        if (this.#closed) {
          return;
        }
        await this.#pause(sleep(jittered(backoff, jitterRatio, random)));
        if (this.#closed) {
          return;
        }
        backoff = Math.min(backoff * 2, ceiling);
      }
    } finally {
      this.options.signal?.removeEventListener('abort', onAbort);
    }
  }

  async #pause(delay: Promise<void>): Promise<void> {
    if (this.#closed) {
      return;
    }
    let wake: () => void = () => {};
    const cancelled = new Promise<void>((resolve) => {
      wake = resolve;
    });
    this.#wake = wake;
    try {
      await Promise.race([delay, cancelled]);
    } finally {
      if (this.#wake === wake) {
        this.#wake = undefined;
      }
    }
  }

  #fail(
    error: CmaError,
    onStatus: ((status: LinkStatus, failure?: LinkFailure) => void) | undefined,
    onError: ((error: CmaError) => void) | undefined,
  ): void {
    this.#closed = true;
    if (this.#terminal !== undefined) {
      return;
    }
    this.#terminal = 'failed';
    onError?.(error);
    onStatus?.('failed', {
      ...(error instanceof ApiError ? { status: error.status } : {}),
      message: error.message,
    });
  }

  #close(
    onStatus: ((status: LinkStatus, failure?: LinkFailure) => void) | undefined,
    onError?: ((error: CmaError) => void) | undefined,
    error?: RequestAborted,
  ): void {
    this.#closed = true;
    if (this.#terminal !== undefined) {
      return;
    }
    this.#terminal = 'closed';
    if (error !== undefined) {
      onError?.(error);
    }
    onStatus?.('closed');
  }

  /** Parse one wire frame and deliver complete durable projections once. */
  #accept(raw: unknown, onEvent: (event: AguiEvent) => void, assembler: ProjectionAssembler): void {
    if (this.#closed) {
      return;
    }
    const events = assembler.accept(toEvent(raw));
    if (events.length === 0) {
      return;
    }
    const first = events[0] as AguiEvent;
    /*
     * Liveness passes straight through, touching neither the cursor nor the
     * seen-set. It has no sequence — what it arrives with is whatever id the
     * last durable frame carried — so recording it would mark a durable
     * event as already delivered and drop the real one when it comes, and
     * every piece of a message after the first would collide with the piece
     * before it and be dropped in silence.
     */
    if (first.ephemeral === true) {
      onEvent(first);
      return;
    }
    this.#deliverProjection(events, onEvent);
  }

  /** Deliver a projection and acknowledge its cursor only after every callback returns. */
  #deliverProjection(events: readonly AguiEvent[], onEvent: (event: AguiEvent) => void): void {
    const final = events.at(-1);
    if (final === undefined || final.ephemeral === true || final.seq <= 0) {
      throw new ProtocolError('AG-UI durable projection had no numbered final frame');
    }
    const wireKind = final.type === 'UNKNOWN' ? final.kind : final.type;
    const finalKey = frameKey(final.seq, wireKind);
    if (final.seq < this.#cursor) {
      return;
    }
    if (final.seq === this.#cursor && this.#seen.has(finalKey)) {
      return;
    }

    const fingerprint = canonicalJson(events);
    let pending = this.#pendingProjection;
    if (pending === undefined) {
      pending = { seq: final.seq, finalKey, fingerprint, acknowledged: 0 };
      this.#pendingProjection = pending;
    } else if (
      pending.seq !== final.seq ||
      pending.finalKey !== finalKey ||
      pending.fingerprint !== fingerprint
    ) {
      throw new ProtocolError(
        `AG-UI replay changed the unacknowledged projection at sequence ${String(pending.seq)}`,
      );
    }

    for (let index = pending.acknowledged; index < events.length; index += 1) {
      try {
        onEvent(events[index] as AguiEvent);
      } catch (cause) {
        throw new EventCallbackFailure(cause);
      }
      pending.acknowledged = index + 1;
      if (this.#closed && pending.acknowledged < events.length) {
        return;
      }
    }

    if (final.seq > this.#cursor) {
      this.#cursor = final.seq;
      // A reconnect asks strictly after the cursor. Only legacy siblings at
      // this sequence can still overlap, so older keys need not survive.
      this.#seen.clear();
    }
    this.#seen.add(finalKey);
    this.#pendingProjection = undefined;
  }
}

/** Stable JSON identity for retrying a callback without accepting changed history. */
function canonicalJson(value: unknown, ancestors = new Set<object>()): string {
  if (value === null || typeof value === 'boolean' || typeof value === 'string') {
    return JSON.stringify(value);
  }
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) {
      throw new ProtocolError('AG-UI projection contained a non-JSON number');
    }
    return JSON.stringify(value);
  }
  if (value === undefined) {
    return 'undefined';
  }
  if (typeof value !== 'object') {
    throw new ProtocolError('AG-UI projection contained a non-JSON value');
  }
  if (ancestors.has(value)) {
    throw new ProtocolError('AG-UI projection contained a cycle');
  }
  ancestors.add(value);
  try {
    if (Array.isArray(value)) {
      return `[${value.map((entry) => canonicalJson(entry, ancestors)).join(',')}]`;
    }
    const record = value as Record<string, unknown>;
    return `{${Object.keys(record)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(record[key], ancestors)}`)
      .join(',')}}`;
  } finally {
    ancestors.delete(value);
  }
}

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

function jittered(delay: number, ratio: number, random: () => number): number {
  const sample = random();
  if (!Number.isFinite(sample) || sample < 0 || sample >= 1) {
    throw new ValidationError(
      'connection random source must return a finite number from zero (inclusive) to one (exclusive)',
    );
  }
  const minimum = delay * (1 - ratio);
  return Math.floor(minimum + sample * (delay - minimum));
}

function permanentFailure(cause: unknown): CmaError | undefined {
  if (!(cause instanceof CmaError) || cause instanceof RequestAborted) {
    return undefined;
  }
  if (cause instanceof ApiError) {
    return TERMINAL_HTTP_STATUSES.has(cause.status) ? cause : undefined;
  }
  if (cause instanceof TransportError) {
    return undefined;
  }
  return cause;
}
