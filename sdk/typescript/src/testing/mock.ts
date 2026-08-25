import type { AguiTransport } from '../agui/connection.js';
import { toEvent } from '../agui/events.js';
import type { OperationTransport } from '../agui/operations.js';
import { ProjectionAssembler } from '../agui/projection.js';
import type { Transcript } from './types.js';

/**
 * Drives the AG-UI client from a recorded transcript.
 *
 * **A transport for the real client, never a parallel implementation.** It
 * supplies frames and history; the cursor, the reconnect order, the
 * deduplication and the event parsing are all `AguiConnection`'s, exactly as
 * they are against the wire. If a component can tell the difference between
 * this and the wire, the mock is wrong.
 *
 * The reference client's terminal drivers carry this pattern — a mock and a
 * transcript driving the terminal from recorded output, which is why the
 * terminal's components are testable and why the signed-out introduction can
 * be the same engine driven by a different source (`Web_Style.md` §1, "one
 * surface, many drivers"). The conversation surface needs it for a sharper
 * reason: its backend arrives later, and without this every component in the
 * milestone would wait on it and land at the end all at once.
 *
 * Timing is honoured, so streaming looks like streaming rather than appearing
 * whole. A surface that has only ever seen a finished conversation is a
 * surface whose streaming behaviour nobody has looked at.
 */

export interface MockOptions {
  /**
   * Multiplier on every recorded delay.
   *
   * `0` replays instantly, which is what a test wants; `1` replays at the
   * pace it was recorded at, which is what somebody looking at the surface
   * wants. Both are the same code path.
   */
  readonly speed?: number;
  /**
   * Cut the live stream after this many frames, then reconnect.
   *
   * The one thing a mock can do that the wire cannot do on demand: reproduce
   * the mid-turn disconnect that `docs/API_Overview.md` §3's reconnect order
   * exists to survive.
   */
  readonly dropAfter?: number;
  /** Injected so tests do not wait in real time. */
  readonly sleep?: (ms: number) => Promise<void>;
}

interface DurableProjection {
  readonly seq: number;
  readonly frames: readonly Readonly<Record<string, unknown>>[];
}

function validateProjection(
  frames: readonly Readonly<Record<string, unknown>>[],
  expectedThreadId: string,
  source: string,
): DurableProjection {
  if (frames.length === 0) {
    throw new TypeError(`${source} was empty`);
  }

  const assembler = new ProjectionAssembler(expectedThreadId);
  let completed: ReturnType<ProjectionAssembler['accept']> = [];
  frames.forEach((frame, index) => {
    const accepted = assembler.accept(toEvent(frame));
    if (index < frames.length - 1 && accepted.length > 0) {
      throw new TypeError(`${source} completed before its final frame`);
    }
    if (accepted.length > 0) {
      completed = accepted;
    }
  });
  assembler.assertComplete(source);

  const final = completed.at(-1);
  if (
    completed.length !== frames.length ||
    final === undefined ||
    final.ephemeral === true ||
    final.seq <= 0
  ) {
    throw new TypeError(`${source} must end in one positive-sequence durable frame`);
  }
  return { seq: final.seq, frames };
}

/** Associate every durable transcript frame with the event projection it came from. */
function durableProjections(
  transcript: Transcript,
  expectedThreadId: string,
): readonly (DurableProjection | undefined)[] {
  const byFrame: (DurableProjection | undefined)[] = new Array(transcript.frames.length);
  let prefixes: { readonly index: number; readonly event: Readonly<Record<string, unknown>> }[] =
    [];

  transcript.frames.forEach((frame, index) => {
    if (frame.event['cma:live'] === true) {
      const live = toEvent(frame.event);
      new ProjectionAssembler(expectedThreadId).accept(live);
      if (live.seq !== 0) {
        throw new TypeError(`mock transcript ${transcript.name} gave liveness a durable sequence`);
      }
      return;
    }
    if (Number(frame.event.seq ?? 0) === 0) {
      prefixes.push({ index, event: frame.event });
      return;
    }

    const members = [...prefixes, { index, event: frame.event }];
    const projection = validateProjection(
      members.map((member) => member.event),
      expectedThreadId,
      `mock transcript ${transcript.name}`,
    );
    for (const member of members) {
      byFrame[member.index] = projection;
    }
    prefixes = [];
  });

  if (prefixes.length > 0) {
    throw new TypeError(`mock transcript ${transcript.name} ended inside a durable projection`);
  }

  return byFrame;
}

/**
 * A mock AG-UI session.
 *
 * Holds the transcript, replays it, and remembers what history has already
 * happened — so a reconnect reads the past rather than starting again, which
 * is the property the surface is being tested against.
 */
export class MockSession {
  /** Durable events whose projection has begun streaming, in sequence order. */
  readonly #available = new Set<DurableProjection>();
  readonly #projectionByFrame: readonly (DurableProjection | undefined)[];
  #cursorAtDrop = 0;
  #dropsRemaining: number;

  /** The session/thread identity recorded by this transcript. */
  readonly id: string;

  constructor(
    private readonly transcript: Transcript,
    private readonly options: MockOptions = {},
  ) {
    const threadId = transcript.frames[0]?.event.threadId;
    if (typeof threadId !== 'string' || threadId.length === 0) {
      throw new TypeError(`mock transcript ${transcript.name} has no threadId`);
    }
    this.id = threadId;
    this.#projectionByFrame = durableProjections(transcript, threadId);
    this.#dropsRemaining = options.dropAfter === undefined ? 0 : 1;
  }

  /** What the transcript is and where it came from, for a surface that says so. */
  get about(): Transcript {
    return this.transcript;
  }

  /** Everything the operations recorded, so a test can assert what was sent. */
  readonly sent: { path: string; body: unknown }[] = [];

  /** The transport `AguiConnection` runs against. */
  get transport(): AguiTransport {
    const speed = this.options.speed ?? 0;
    const sleep = this.options.sleep ?? ((ms: number) => new Promise((r) => setTimeout(r, ms)));

    return {
      open: (_session, _cursor, onFrame, onDrop) => {
        let stopped = false;
        void (async () => {
          // Let the connection take its initial history snapshot before a
          // zero-speed replay fills the live queue. Without this, durable
          // frames after a liveness frame appeared in history first and the
          // liveness frame was delivered at the end instead of where it
          // happened.
          await Promise.resolve();
          // Replay from where the last connection got to, which is what a
          // real stream does: it carries what happens next, and the past is
          // read from the log.
          const from = this.#cursorAtDrop;
          let index = 0;
          for (const frame of this.transcript.frames) {
            index += 1;
            if (index <= from) {
              continue;
            }
            if (speed > 0) {
              await sleep(frame.afterMs * speed);
            }
            if (stopped) {
              return;
            }
            if (this.#dropsRemaining > 0 && index > (this.options.dropAfter ?? 0)) {
              // The disconnect. What has been delivered stays in history, so
              // the reconnect finds it there rather than losing it.
              this.#dropsRemaining -= 1;
              this.#cursorAtDrop = index - 1;
              onDrop('the mock dropped the stream');
              return;
            }
            // The durable event exists before the server starts projecting it
            // onto SSE. Expose the whole projection to history before its first
            // frame is sent, so a disconnect after a seq: 0 prefix replays the
            // complete event. Liveness has no projection and never enters
            // history.
            const projection = this.#projectionByFrame[index - 1];
            if (projection !== undefined) {
              this.#available.add(projection);
            }
            onFrame(frame.event);
          }
        })();
        return () => {
          stopped = true;
        };
      },

      history: (_session, cursor) =>
        // Every complete durable projection already available after the
        // caller's cursor, oldest first — the same contract the wire's
        // `starting_after` honours.
        Promise.resolve(
          [...this.#available]
            .filter((projection) => projection.seq > cursor)
            .sort((left, right) => left.seq - right.seq)
            .flatMap((projection) => projection.frames),
        ),
    };
  }

  /**
   * The transport the operations run against.
   *
   * Accepts everything and records it. A mock that refused would be asserting
   * something about the control plane, which it is in no position to know.
   */
  get operations(): OperationTransport {
    return {
      send: (path, body) => {
        this.sent.push({ path, body });
        return Promise.resolve(
          path === '/api/v1/agui/run'
            ? new Response('', {
                status: 200,
                headers: { 'content-type': 'text/event-stream' },
              })
            : new Response(null, { status: 202 }),
        );
      },
    };
  }

  /** Emit liveness or one numbered durable frame that is not in the transcript. */
  emit(event: Readonly<Record<string, unknown>>, onFrame: (raw: unknown) => void): void {
    if (event['cma:live'] === true) {
      const live = toEvent(event);
      new ProjectionAssembler(this.id).accept(live);
      if (live.seq !== 0) {
        throw new TypeError('mock liveness must have sequence zero');
      }
      onFrame(event);
      return;
    }
    this.emitProjection([event], onFrame);
  }

  /**
   * Emit one complete durable event projection that is not in the transcript.
   *
   * The full projection enters history before its first cursorless prefix is
   * delivered. A reconnect at any point therefore reads the same complete
   * durable event that the production history endpoint would return.
   */
  emitProjection(
    frames: readonly Readonly<Record<string, unknown>>[],
    onFrame: (raw: unknown) => void,
  ): void {
    const projection = validateProjection(frames, this.id, 'mock emitted projection');
    this.#available.add(projection);
    for (const frame of frames) {
      onFrame(frame);
    }
  }
}
