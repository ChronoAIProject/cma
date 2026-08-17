import { toEvent, type AguiEvent } from './events';

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
 * the cursor is per session and is the only resumption state there is. There is
 * no `Last-Event-ID` plumbing and there is nothing else to hold.
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
  open(session: string, onFrame: (raw: unknown) => void, onDrop: (why: string) => void): () => void;
  /** Durable events after `cursor`, oldest first. */
  history(session: string, cursor: number): Promise<readonly unknown[]>;
}

export interface ConnectionOptions {
  /** How long to wait before the first reconnect attempt. */
  readonly backoffMs?: number;
  /** The ceiling the backoff climbs to. */
  readonly maxBackoffMs?: number;
  /** Injected so tests do not wait in real time. */
  readonly sleep?: (ms: number) => Promise<void>;
}

const DEFAULT_BACKOFF = 500;
const DEFAULT_MAX_BACKOFF = 15_000;

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
  #seen = new Set<number>();
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
  ): () => void {
    void this.#run(onEvent, onStatus);
    return () => {
      this.#closed = true;
      this.#stopStream?.();
      this.#stopStream = undefined;
      this.#wake?.();
      onStatus?.('closed');
    };
  }

  async #run(onEvent: (event: AguiEvent) => void, onStatus?: (status: LinkStatus) => void) {
    let backoff = this.options.backoffMs ?? DEFAULT_BACKOFF;
    const ceiling = this.options.maxBackoffMs ?? DEFAULT_MAX_BACKOFF;
    const sleep = this.options.sleep ?? ((ms: number) => new Promise((r) => setTimeout(r, ms)));

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
      this.#stopStream = this.transport.open(
        this.session,
        (raw) => {
          if (live) {
            this.#deliver(raw, onEvent);
          } else {
            // Held until history has been replayed, so the caller sees one
            // ordered sequence rather than the future before the past.
            pending.push(raw);
          }
        },
        settle,
      );

      // Then history, from the cursor. Then deduplicate — which the delivery
      // below does on `seq`, so a frame seen twice is delivered once.
      try {
        for (const raw of await this.transport.history(this.session, this.#cursor)) {
          this.#deliver(raw, onEvent);
        }
      } catch (cause) {
        // A history read that failed is a reconnect, not a lost session. The
        // cursor has not moved, so the next attempt asks for the same range.
        this.#stopStream?.();
        this.#stopStream = undefined;
        onStatus?.('reconnecting');
        if (this.#closed) {
          return;
        }
        await sleep(backoff);
        backoff = Math.min(backoff * 2, ceiling);
        void cause;
        continue;
      }

      live = true;
      for (const raw of pending.splice(0)) {
        this.#deliver(raw, onEvent);
      }
      onStatus?.('live');
      backoff = this.options.backoffMs ?? DEFAULT_BACKOFF;

      await ended;
      this.#wake = undefined;
      this.#stopStream?.();
      this.#stopStream = undefined;
      if (this.#closed) {
        return;
      }
      onStatus?.('reconnecting');
      await sleep(backoff);
      backoff = Math.min(backoff * 2, ceiling);
    }
  }

  /**
   * Deliver one frame, once.
   *
   * Deduplicated on `seq`, which is what the reconnect order above depends on:
   * opening the stream before reading history guarantees an overlap, and this
   * is what makes the overlap harmless.
   */
  #deliver(raw: unknown, onEvent: (event: AguiEvent) => void) {
    const event = toEvent(raw);
    if (this.#seen.has(event.seq)) {
      return;
    }
    this.#seen.add(event.seq);
    if (event.seq > this.#cursor) {
      this.#cursor = event.seq;
    }
    onEvent(event);
  }
}

/** Where the link is, for a surface that has to say so. */
export type LinkStatus = 'live' | 'reconnecting' | 'closed';
