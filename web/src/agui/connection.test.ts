import { describe, expect, it, vi } from 'vitest';
import { AguiConnection, type AguiTransport } from './connection';
import type { AguiEvent } from './events';

/**
 * The AG-UI connection — `docs/API_Overview.md` §3.
 *
 * No DOM. This is a protocol client, and the properties it has to hold are
 * about ordering and identity rather than about rendering.
 *
 * @vitest-environment node
 */

/** A transport whose stream and history a test drives frame by frame. */
class Fake implements AguiTransport {
  frames: ((raw: unknown) => void) | undefined;
  drop: ((why: string) => void) | undefined;
  opens = 0;
  historyReads: number[] = [];
  /** Reads are answered in order, so a test can fail the first and pass the next. */
  responses: (readonly unknown[] | Error)[] = [];
  /** Frames pushed the moment a stream opens, before history is read. */
  onOpen: readonly unknown[] = [];

  open(_session: string, onFrame: (raw: unknown) => void, onDrop: (why: string) => void) {
    this.opens += 1;
    this.frames = onFrame;
    this.drop = onDrop;
    for (const frame of this.onOpen) {
      onFrame(frame);
    }
    return () => {
      this.frames = undefined;
    };
  }

  history(_session: string, cursor: number): Promise<readonly unknown[]> {
    this.historyReads.push(cursor);
    const next = this.responses.shift() ?? [];
    return next instanceof Error ? Promise.reject(next) : Promise.resolve(next);
  }
}

function text(seq: number, body: string) {
  return {
    type: 'TEXT_MESSAGE_CONTENT',
    threadId: 'sesn_1',
    runId: 'run_1',
    seq,
    messageId: `m_${String(seq)}`,
    delta: body,
  };
}

/** Let the connection's own loop advance. */
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function connect(fake: Fake) {
  const seen: AguiEvent[] = [];
  const statuses: string[] = [];
  const connection = new AguiConnection(fake, 'sesn_1', {
    backoffMs: 0,
    maxBackoffMs: 0,
    sleep: () => Promise.resolve(),
  });
  const stop = connection.subscribe(
    (event) => seen.push(event),
    (status) => statuses.push(status),
  );
  return { connection, seen, statuses, stop };
}

describe('the reconnect order', () => {
  it('opens the stream before it reads history', async () => {
    // §3: "open the stream first, then read history, then deduplicate on event
    // id — never by assuming the stream replays." Read history first and
    // anything landing in between is gone, with nothing afterwards to notice.
    const fake = new Fake();
    const order: string[] = [];
    fake.open = ((session, onFrame, onDrop) => {
      order.push('open');
      return Fake.prototype.open.call(fake, session, onFrame, onDrop);
    }) as Fake['open'];
    fake.history = ((session: string, cursor: number) => {
      order.push('history');
      return Fake.prototype.history.call(fake, session, cursor);
    }) as Fake['history'];

    const { stop } = connect(fake);
    await tick();
    stop();

    expect(order).toEqual(['open', 'history']);
  });

  it('loses nothing that arrives during the history read', async () => {
    // The window nobody tests. A frame that lands after the stream opened and
    // before history came back must be delivered, and delivered after the
    // history it is newer than.
    const fake = new Fake();
    fake.onOpen = [text(3, 'live')];
    fake.responses = [[text(1, 'past'), text(2, 'past')]];

    const { seen, stop } = connect(fake);
    await tick();
    await tick();
    stop();

    expect(seen.map((e) => e.seq)).toEqual([1, 2, 3]);
  });

  it('delivers a frame seen twice exactly once', async () => {
    // Opening before reading guarantees an overlap. Deduplication is what
    // makes the overlap harmless.
    const fake = new Fake();
    fake.onOpen = [text(2, 'both')];
    fake.responses = [[text(1, 'past'), text(2, 'both')]];

    const { seen, stop } = connect(fake);
    await tick();
    await tick();
    stop();

    expect(seen.map((e) => e.seq)).toEqual([1, 2]);
  });

  it('resumes from the cursor and never from zero', async () => {
    // The cursor is the only resumption state there is, and it is per session:
    // durable events carry a per-session `seq`, not a global one.
    const fake = new Fake();
    fake.responses = [[text(1, 'a'), text(7, 'b')], []];

    const { connection, stop } = connect(fake);
    await tick();
    await tick();
    expect(connection.cursor).toBe(7);

    fake.drop?.('network');
    await tick();
    await tick();
    stop();

    expect(fake.historyReads).toEqual([0, 7]);
  });

  it('loses nothing across a mid-turn disconnect', async () => {
    // Proven rather than inspected, which is what #116 asks for.
    const fake = new Fake();
    fake.responses = [[text(1, 'first half')], [text(2, 'second half')]];

    const { seen, stop } = connect(fake);
    await tick();
    await tick();

    fake.drop?.('the laptop slept');
    await tick();
    await tick();
    stop();

    expect(seen.map((e) => e.seq)).toEqual([1, 2]);
    expect(fake.opens).toBe(2);
  });

  it('retries a history read that failed without moving the cursor', async () => {
    const fake = new Fake();
    fake.responses = [new Error('502'), [text(1, 'a')]];

    const { seen, stop } = connect(fake);
    await tick();
    await tick();
    await tick();
    stop();

    expect(fake.historyReads).toEqual([0, 0]);
    expect(seen.map((e) => e.seq)).toEqual([1]);
  });

  it('reports where the link is, so a surface can say so', async () => {
    const fake = new Fake();
    fake.responses = [[], []];
    const { statuses, stop } = connect(fake);
    await tick();
    await tick();
    expect(statuses).toContain('live');

    fake.drop?.('gone');
    await tick();
    expect(statuses).toContain('reconnecting');

    stop();
    expect(statuses.at(-1)).toBe('closed');
  });

  it('stops for good when it is closed', async () => {
    const fake = new Fake();
    fake.responses = [[]];
    const { stop } = connect(fake);
    await tick();
    stop();
    const opened = fake.opens;

    fake.drop?.('gone');
    await tick();
    await tick();

    expect(fake.opens).toBe(opened);
  });

  it('does not tear the stream down over one unreadable frame', async () => {
    const fake = new Fake();
    fake.responses = [[text(1, 'a'), 'not an object', text(2, 'b')]];
    const { seen, stop } = connect(fake);
    await tick();
    await tick();
    stop();

    // The unreadable one becomes an unknown event rather than an exception.
    expect(seen.map((e) => e.type)).toContain('UNKNOWN');
    expect(seen.filter((e) => e.type === 'TEXT_MESSAGE_CONTENT')).toHaveLength(2);
  });
});

describe('history and stream agree', () => {
  it('produces the same events replayed as watched', async () => {
    // Web_Style.md §18.1's equivalence rule, at the layer that decides it. A
    // component drawing from history must produce what one watching produced,
    // and that starts here.
    const frames = [text(1, 'a'), text(2, 'b'), text(3, 'c')];

    const streamed = new Fake();
    streamed.onOpen = frames;
    streamed.responses = [[]];
    const watching = connect(streamed);
    await tick();
    await tick();
    watching.stop();

    const stored = new Fake();
    stored.responses = [frames];
    const replaying = connect(stored);
    await tick();
    await tick();
    replaying.stop();

    expect(watching.seen).toEqual(replaying.seen);
  });
});

describe('closing', () => {
  it('is safe to call twice', async () => {
    const fake = new Fake();
    fake.responses = [[]];
    const { stop } = connect(fake);
    await tick();
    stop();
    expect(() => {
      stop();
    }).not.toThrow();
  });

  it('does not need a DOM', () => {
    // §116: the suite runs without jsdom. This file declares the node
    // environment, and this asserts the assumption rather than trusting it.
    expect(typeof globalThis.document).toBe('undefined');
    expect(vi.isMockFunction(() => {})).toBe(false);
  });
});
