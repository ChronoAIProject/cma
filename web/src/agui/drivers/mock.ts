import type { AguiTransport } from '../connection';
import type { OperationTransport } from '../operations';
import type { Transcript } from '../fixtures/types';

/**
 * Drives the AG-UI client from a recorded transcript.
 *
 * **A transport for the real client, never a parallel implementation.** It
 * supplies frames and history; the cursor, the reconnect order, the
 * deduplication and the event parsing are all `AguiConnection`'s, exactly as
 * they are against the wire. If a component can tell the difference between
 * this and the wire, the mock is wrong.
 *
 * `web/src/terminal/drivers/` already carries this pattern — a mock and a
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

/**
 * A mock AG-UI session.
 *
 * Holds the transcript, replays it, and remembers what history has already
 * happened — so a reconnect reads the past rather than starting again, which
 * is the property the surface is being tested against.
 */
export class MockSession {
  /** Durable frames the session has already emitted, in sequence order. */
  #delivered: Record<string, unknown>[] = [];
  #cursorAtDrop = 0;
  #dropsRemaining: number;

  constructor(
    private readonly transcript: Transcript,
    private readonly options: MockOptions = {},
  ) {
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
      open: (_session, onFrame, onDrop) => {
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
            // Liveness is never history. Recording it here makes the
            // connection deliver it once from history and once from the live
            // stream, and makes a reload show reasoning the protocol never
            // persisted.
            if (frame.event['cma:live'] !== true) {
              this.#delivered.push(frame.event);
            }
            onFrame(frame.event);
          }
        })();
        return () => {
          stopped = true;
        };
      },

      history: (_session, cursor) =>
        // Everything already emitted after the caller's cursor, oldest first —
        // the same contract the wire's `starting_after` honours.
        Promise.resolve(this.#delivered.filter((frame) => Number(frame.seq ?? 0) > cursor)),
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
        return Promise.resolve(new Response(null, { status: 202 }));
      },
    };
  }

  /**
   * Emit a frame that is not in the transcript.
   *
   * For the cases a recording cannot express — an approval answered *now*, a
   * turn steered mid-flight. A durable frame goes into history so a reconnect
   * finds it; liveness remains live-only, like it does on the wire.
   */
  emit(event: Record<string, unknown>, onFrame: (raw: unknown) => void): void {
    if (event['cma:live'] !== true) {
      this.#delivered.push(event);
    }
    onFrame(event);
  }
}
