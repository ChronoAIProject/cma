import { describe, expect, it, vi } from 'vitest';
import { AguiConnection, type AguiTransport, type LinkFailure } from '../../src/agui/connection.js';
import type { AguiEvent } from '../../src/agui/events.js';
import { fold } from '../../src/conversation/fold.js';
import { ApiError, ProtocolError, RequestAborted, ValidationError } from '../../src/errors.js';

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
  drop: ((cause: unknown) => void) | undefined;
  opens = 0;
  openCursors: number[] = [];
  historyReads: number[] = [];
  /** Reads are answered in order, so a test can fail the first and pass the next. */
  responses: (readonly unknown[] | Error)[] = [];
  /** Frames pushed the moment a stream opens, before history is read. */
  onOpen: readonly unknown[] = [];

  open(
    _session: string,
    cursor: number,
    onFrame: (raw: unknown) => void,
    onDrop: (cause: unknown) => void,
  ) {
    this.opens += 1;
    this.openCursors.push(cursor);
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

function committedMessage(seq: number, messageId: string, body: string, runId = 'run_1') {
  return [
    {
      type: 'TEXT_MESSAGE_START',
      threadId: 'sesn_1',
      runId,
      seq: 0,
      messageId,
      role: 'assistant',
    },
    {
      type: 'TEXT_MESSAGE_CONTENT',
      threadId: 'sesn_1',
      runId,
      seq: 0,
      messageId,
      delta: body,
    },
    { type: 'TEXT_MESSAGE_END', threadId: 'sesn_1', runId, seq, messageId },
  ] as const;
}

function toolUpdate(seq: number, toolCallId: string, status: string) {
  return [
    {
      type: 'TOOL_CALL_END',
      threadId: 'sesn_1',
      runId: 'run_1',
      seq: 0,
      toolCallId,
      'cma:status': status,
    },
    {
      type: 'TOOL_CALL_RESULT',
      threadId: 'sesn_1',
      runId: 'run_1',
      seq,
      toolCallId,
      content: `${status} result`,
      'cma:truncated': false,
    },
  ] as const;
}

/** Let the connection's own loop advance. */
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function connect(fake: Fake, session = 'sesn_1') {
  const seen: AguiEvent[] = [];
  const statuses: string[] = [];
  const failures: LinkFailure[] = [];
  const errors: Error[] = [];
  const connection = new AguiConnection(fake, session, {
    backoffMs: 0,
    maxBackoffMs: 0,
    sleep: () => Promise.resolve(),
  });
  const stop = connection.subscribe(
    (event) => seen.push(event),
    (status, failure) => {
      statuses.push(status);
      if (failure !== undefined) {
        failures.push(failure);
      }
    },
    (error) => errors.push(error),
  );
  return { connection, errors, failures, seen, statuses, stop };
}

describe('the reconnect order', () => {
  it('opens the stream before it reads history', async () => {
    // §3: "open the stream first, then read history, then deduplicate on event
    // id — never by assuming the stream replays." Read history first and
    // anything landing in between is gone, with nothing afterwards to notice.
    const fake = new Fake();
    const order: string[] = [];
    fake.open = ((session, cursor, onFrame, onDrop) => {
      order.push('open');
      return Fake.prototype.open.call(fake, session, cursor, onFrame, onDrop);
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
    expect(fake.openCursors).toEqual([0, 7]);
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

  it.each([401, 403, 404, 410, 422])(
    'ends on permanent HTTP %i and retains its status',
    async (status) => {
      const fake = new Fake();
      fake.responses = [new ApiError(status, 'this session is unavailable', 'session_unavailable')];

      const { errors, failures, statuses, stop } = connect(fake);
      await vi.waitFor(() => expect(statuses).toContain('failed'));

      expect(fake.opens).toBe(1);
      expect(fake.historyReads).toEqual([0]);
      expect(errors[0]).toBeInstanceOf(ApiError);
      expect(failures).toEqual([{ status, message: 'this session is unavailable' }]);
      stop();
      expect(statuses.filter((entry) => entry === 'failed')).toHaveLength(1);
      expect(statuses.at(-1)).toBe('failed');
    },
  );

  it.each([408, 429, 503])('still retries nonterminal HTTP %i', async (status) => {
    const fake = new Fake();
    fake.responses = [new ApiError(status, 'temporarily unavailable'), [text(1, 'back')]];

    const { seen, statuses, stop } = connect(fake);
    await vi.waitFor(() => expect(seen).toHaveLength(1));
    stop();

    expect(fake.opens).toBe(2);
    expect(statuses).toContain('reconnecting');
    expect(seen.map((event) => event.seq)).toEqual([1]);
  });

  it('ignores frames after the transport has declared the stream dropped', async () => {
    const fake = new Fake();
    fake.responses = [[]];
    const { seen, statuses, stop } = connect(fake);
    await vi.waitFor(() => expect(statuses).toContain('live'));

    fake.drop?.(new ApiError(404, 'this session no longer exists'));
    fake.frames?.(text(1, 'arrived after the drop'));
    await vi.waitFor(() => expect(statuses).toContain('failed'));
    stop();

    expect(seen).toEqual([]);
    expect(statuses.at(-1)).toBe('failed');
  });

  it('uses full jitter while exponential backoff climbs to its ceiling', async () => {
    const fake = new Fake();
    fake.responses = [new Error('offline'), new Error('still offline'), []];
    const delays: number[] = [];
    const connection = new AguiConnection(fake, 'sesn_1', {
      backoffMs: 10,
      maxBackoffMs: 20,
      jitterRatio: 1,
      random: () => 0.5,
      sleep: (milliseconds) => {
        delays.push(milliseconds);
        return Promise.resolve();
      },
    });
    const stop = connection.subscribe(() => undefined);

    await vi.waitFor(() => expect(fake.opens).toBe(3));
    stop();

    expect(delays).toEqual([5, 10]);
  });

  it('fails on an invalid random source instead of retrying unsafely', async () => {
    const fake = new Fake();
    fake.responses = [new Error('offline')];
    const errors: Error[] = [];
    const statuses: string[] = [];
    const connection = new AguiConnection(fake, 'sesn_1', {
      backoffMs: 10,
      random: () => 1,
      sleep: () => Promise.resolve(),
    });
    const stop = connection.subscribe(
      () => undefined,
      (status) => statuses.push(status),
      (error) => errors.push(error),
    );

    await vi.waitFor(() => expect(statuses).toContain('failed'));
    stop();

    expect(errors[0]).toBeInstanceOf(ValidationError);
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

  it('reports an unreadable frame and closes the connection', async () => {
    const fake = new Fake();
    fake.responses = [[text(1, 'a'), 'not an object', text(2, 'b')]];
    const { errors, seen, statuses, stop } = connect(fake);
    await tick();
    await tick();
    stop();

    expect(seen.map((e) => e.type)).toEqual(['TEXT_MESSAGE_CONTENT']);
    expect(errors[0]).toBeInstanceOf(ProtocolError);
    expect(statuses).toContain('failed');
  });

  it('fails once without reconnecting on an impossible unnumbered final', async () => {
    const fake = new Fake();
    fake.responses = [
      [
        {
          type: 'TEXT_MESSAGE_END',
          threadId: 'sesn_1',
          runId: 'run_1',
          seq: 0,
          messageId: 'm_impossible',
        },
        text(1, 'must not be delivered'),
      ],
    ];
    const { errors, seen, statuses, stop } = connect(fake);

    await vi.waitFor(() =>
      expect(statuses.filter((status) => status === 'failed')).toHaveLength(1),
    );
    await tick();
    await tick();
    stop();

    expect(errors).toHaveLength(1);
    expect(errors[0]).toBeInstanceOf(ProtocolError);
    expect(errors[0]?.message).toContain('TEXT_MESSAGE_END');
    expect(seen).toEqual([]);
    expect(statuses).not.toContain('reconnecting');
    expect(fake.opens).toBe(1);
    expect(fake.historyReads).toEqual([0]);
  });
});

describe('history and stream agree', () => {
  it('renders the same conversation despite the replay route run-id signature', async () => {
    // Web_Style.md §18.1's equivalence rule, at the layer that decides it. A
    // component drawing from history must produce what one watching produced,
    // even though the two routes deliberately identify their envelopes
    // differently: a run response echoes the turn id, while replay identifies
    // itself with runId == threadId. Neither the connection nor the fold may
    // turn that route marker into per-run state.
    const watchedFrames = [
      ...committedMessage(1, 'm_1', 'a', 'run_1'),
      ...committedMessage(2, 'm_2', 'b', 'run_1'),
    ];
    const replayedFrames = [
      ...committedMessage(1, 'm_1', 'a', 'sesn_1'),
      ...committedMessage(2, 'm_2', 'b', 'sesn_1'),
    ];

    const streamed = new Fake();
    streamed.onOpen = watchedFrames;
    streamed.responses = [[]];
    const watching = connect(streamed);
    await tick();
    await tick();
    watching.stop();

    const stored = new Fake();
    stored.responses = [replayedFrames];
    const replaying = connect(stored);
    await tick();
    await tick();
    replaying.stop();

    expect(watching.seen).toHaveLength(6);
    expect(watching.seen.every((event) => event.runId === 'run_1')).toBe(true);
    expect(replaying.seen.every((event) => event.runId === 'sesn_1')).toBe(true);
    expect(fold(watching.seen)).toEqual(fold(replaying.seen));
  });
});

describe('closing', () => {
  it('is safe to call twice and reports one terminal transition', async () => {
    const fake = new Fake();
    fake.responses = [[]];
    const { statuses, stop } = connect(fake);
    await tick();
    stop();
    expect(() => {
      stop();
    }).not.toThrow();
    expect(statuses.filter((status) => status === 'closed')).toHaveLength(1);
  });

  it('delivers nothing after closing during a history read', async () => {
    const fake = new Fake();
    let resolveHistory: (frames: readonly unknown[]) => void = () => {};
    fake.history = (_session: string, cursor: number) => {
      fake.historyReads.push(cursor);
      return new Promise((resolve) => {
        resolveHistory = resolve;
      });
    };

    const { seen, statuses, stop } = connect(fake);
    stop();
    resolveHistory([text(1, 'too late')]);
    await tick();

    expect(seen).toEqual([]);
    expect(statuses).toEqual(['closed']);
  });

  it('reports caller abort once and ignores later cleanup', async () => {
    const fake = new Fake();
    fake.responses = [[]];
    const controller = new AbortController();
    const statuses: string[] = [];
    const errors: Error[] = [];
    const connection = new AguiConnection(fake, 'sesn_1', {
      signal: controller.signal,
      backoffMs: 0,
      maxBackoffMs: 0,
    });
    const stop = connection.subscribe(
      () => undefined,
      (status) => statuses.push(status),
      (error) => errors.push(error),
    );
    await tick();

    controller.abort('test cancellation');
    stop();
    await tick();

    expect(errors).toHaveLength(1);
    expect(errors[0]).toBeInstanceOf(RequestAborted);
    expect(statuses.filter((status) => status === 'closed')).toHaveLength(1);
  });

  it('does not need a DOM', () => {
    // §116: the suite runs without jsdom. This file declares the node
    // environment, and this asserts the assumption rather than trusting it.
    expect(typeof globalThis.document).toBe('undefined');
    expect(vi.isMockFunction(() => {})).toBe(false);
  });
});

describe('liveness is not history', () => {
  /**
   * A piece of a message being typed, exactly as the wire delivers one.
   *
   * `seq` is what `EventSource` reported as `lastEventId` — which, for a frame
   * the server sent without an id, is the id of the *last* frame that had one.
   * That is precisely why the server states the absence rather than leaving a
   * client to notice it.
   */
  const typing = (inheritedSeq: number, body: string) => ({
    ...text(inheritedSeq, body),
    messageId: 'msg_live',
    'cma:live': true,
  });

  it('delivers every piece, even though they all carry the same sequence', async () => {
    // The failure this guards. Deduplicating liveness on `seq` drops the
    // second piece of every message and every piece after it, silently, and
    // the message appears to stop three characters in.
    const fake = new Fake();
    const { seen, stop } = connect(fake);
    await tick();

    fake.frames?.(text(4, 'settled'));
    fake.frames?.(typing(4, 'Hel'));
    fake.frames?.(typing(4, 'lo '));
    fake.frames?.(typing(4, 'there'));
    await tick();
    stop();

    const live = seen.filter((event) => event.ephemeral === true);
    expect(live).toHaveLength(3);
    expect(
      live.map((event) => (event.type === 'TEXT_MESSAGE_CONTENT' ? event.delta : '')).join(''),
    ).toBe('Hello there');
  });

  it('never moves the cursor, so a reconnect resumes where the log is', async () => {
    // A liveness frame's sequence is meaningless. Treating it as a position
    // would send the next history read past events the log does hold, and
    // everything between there and the real position would be skipped.
    const fake = new Fake();
    const { connection, stop } = connect(fake);
    await tick();

    fake.frames?.(text(7, 'settled'));
    fake.frames?.(typing(99, 'a piece'));
    await tick();

    expect(connection.cursor).toBe(7);

    fake.drop?.('the stream closed');
    await tick();
    stop();

    expect(fake.historyReads.at(-1)).toBe(7);
  });

  it('does not let liveness mark a durable event as already seen', async () => {
    // The other half of the same mistake. If a live frame recorded its
    // inherited sequence, the durable event carrying that sequence would be
    // dropped when it arrived — losing the committed message rather than a
    // piece of it.
    const fake = new Fake();
    const { seen, stop } = connect(fake);
    await tick();

    fake.frames?.(typing(5, 'in flight'));
    fake.frames?.(text(5, 'the committed message'));
    await tick();
    stop();

    const durable = seen.filter((event) => event.ephemeral !== true);
    expect(durable).toHaveLength(1);
    expect(durable[0]?.type === 'TEXT_MESSAGE_CONTENT' && durable[0].delta).toBe(
      'the committed message',
    );
  });
});

describe('a durable event that projects to several frames', () => {
  it('delivers every frame and advances only on the final one', async () => {
    const fake = new Fake();
    fake.onOpen = [
      {
        type: 'TEXT_MESSAGE_START',
        threadId: 'sesn_1',
        runId: 'run_1',
        seq: 0,
        messageId: 'm_7',
        role: 'assistant',
      },
      {
        type: 'TEXT_MESSAGE_CONTENT',
        threadId: 'sesn_1',
        runId: 'run_1',
        seq: 0,
        messageId: 'm_7',
        delta: 'hello',
      },
      { type: 'TEXT_MESSAGE_END', threadId: 'sesn_1', runId: 'run_1', seq: 7, messageId: 'm_7' },
    ];
    const { connection, seen, stop } = connect(fake);
    await tick();
    stop();

    expect(seen.map((e) => e.type)).toEqual([
      'TEXT_MESSAGE_START',
      'TEXT_MESSAGE_CONTENT',
      'TEXT_MESSAGE_END',
    ]);
    expect(seen.map((event) => event.seq)).toEqual([0, 0, 7]);
    expect(connection.cursor).toBe(7);
  });

  it('rejects a final from another thread before it can poison the session cursor', async () => {
    const fake = new Fake();
    fake.responses = [
      [
        {
          type: 'FUTURE_PREFIX',
          threadId: 'sesn_1',
          runId: 'run_1',
          seq: 0,
        },
        { type: 'RUN_FINISHED', threadId: 'sesn_other', runId: 'run_1', seq: 900 },
      ],
    ];

    const { connection, errors, seen, statuses, stop } = connect(fake);
    await vi.waitFor(() => expect(statuses).toContain('failed'));
    stop();

    expect(connection.cursor).toBe(0);
    expect(seen).toEqual([]);
    expect(errors[0]).toBeInstanceOf(ProtocolError);
    expect(errors[0]?.message).toContain('another threadId');
  });

  it('replays an interrupted projection whole without delivering its prefix twice', async () => {
    const fake = new Fake();
    const start = {
      type: 'TEXT_MESSAGE_START',
      threadId: 'sesn_1',
      runId: 'run_1',
      seq: 0,
      messageId: 'm_7',
      role: 'assistant',
    };
    const content = {
      type: 'TEXT_MESSAGE_CONTENT',
      threadId: 'sesn_1',
      runId: 'run_1',
      seq: 0,
      messageId: 'm_7',
      delta: 'nothing after the drop is lost',
    };
    const end = {
      type: 'TEXT_MESSAGE_END',
      threadId: 'sesn_1',
      runId: 'run_1',
      seq: 7,
      messageId: 'm_7',
    };
    fake.responses = [
      [{ type: 'RUN_STARTED', threadId: 'sesn_1', runId: 'run_1', seq: 6 }],
      [start, content, end, text(8, 'the next event')],
    ];

    const { connection, seen, stop } = connect(fake);
    await vi.waitFor(() => expect(connection.cursor).toBe(6));

    fake.frames?.(start);
    expect(connection.cursor).toBe(6);

    fake.drop?.('the connection closed after START');
    await vi.waitFor(() => expect(connection.cursor).toBe(8));
    stop();

    expect(fake.openCursors).toEqual([0, 6]);
    expect(fake.historyReads).toEqual([0, 6]);
    expect(seen.map((event) => event.type)).toEqual([
      'RUN_STARTED',
      'TEXT_MESSAGE_START',
      'TEXT_MESSAGE_CONTENT',
      'TEXT_MESSAGE_END',
      'TEXT_MESSAGE_CONTENT',
    ]);
    expect(seen.filter((event) => event.type === 'TEXT_MESSAGE_START')).toHaveLength(1);
  });

  it('folds a normal history and buffered-stream overlap exactly once', async () => {
    const fake = new Fake();
    const turn = [
      { type: 'RUN_STARTED', threadId: 'sesn_1', runId: 'run_1', seq: 1 },
      ...committedMessage(2, 'm_2', 'one answer'),
      {
        type: 'RUN_FINISHED',
        threadId: 'sesn_1',
        runId: 'run_1',
        seq: 3,
      },
    ];
    fake.onOpen = turn;
    fake.responses = [turn];

    const { seen, statuses, stop } = connect(fake);
    await vi.waitFor(() => expect(statuses).toContain('live'));
    stop();

    expect(seen.map((event) => event.type)).toEqual([
      'RUN_STARTED',
      'TEXT_MESSAGE_START',
      'TEXT_MESSAGE_CONTENT',
      'TEXT_MESSAGE_END',
      'RUN_FINISHED',
    ]);
    expect(fold(seen).entries).toEqual([
      expect.objectContaining({
        kind: 'message',
        id: 'm_2',
        text: 'one answer',
        committed: true,
        seq: 2,
      }),
    ]);
  });

  it('retains every completed projection until a multi-projection overlap drains', async () => {
    const fake = new Fake();
    const turn = [
      { type: 'RUN_STARTED', threadId: 'sesn_1', runId: 'run_1', seq: 1 },
      ...committedMessage(2, 'm_2', 'first'),
      ...committedMessage(3, 'm_3', 'second'),
      { type: 'RUN_FINISHED', threadId: 'sesn_1', runId: 'run_1', seq: 4 },
    ];
    fake.onOpen = turn;
    fake.responses = [turn];

    const { connection, seen, statuses, stop } = connect(fake);
    await vi.waitFor(() => expect(statuses).toContain('live'));
    stop();

    expect(connection.cursor).toBe(4);
    expect(seen.map((event) => event.seq)).toEqual([1, 0, 0, 2, 0, 0, 3, 4]);
    expect(fold(seen).entries).toEqual([
      expect.objectContaining({ id: 'm_2', text: 'first', committed: true, seq: 2 }),
      expect.objectContaining({ id: 'm_3', text: 'second', committed: true, seq: 3 }),
    ]);
  });

  it('preserves repeated durable updates for the same tool call', async () => {
    const fake = new Fake();
    fake.responses = [
      [...toolUpdate(2, 'call-1', 'running'), ...toolUpdate(3, 'call-1', 'completed')],
    ];

    const { connection, seen, stop } = connect(fake);
    await vi.waitFor(() => expect(connection.cursor).toBe(3));
    stop();

    expect(seen.flatMap((event) => (event.type === 'TOOL_CALL_END' ? [event.status] : []))).toEqual(
      ['running', 'completed'],
    );
    expect(
      seen.flatMap((event) => (event.type === 'TOOL_CALL_RESULT' ? [event.content] : [])),
    ).toEqual(['running result', 'completed result']);
  });

  it('deduplicates history overlap by projection occurrence, then keeps a newer same-call update', async () => {
    const fake = new Fake();
    const running = toolUpdate(2, 'call-1', 'running');
    const completed = toolUpdate(3, 'call-1', 'completed');
    fake.responses = [[...running]];
    fake.onOpen = [...running, ...completed];

    const { connection, seen, stop } = connect(fake);
    await vi.waitFor(() => expect(connection.cursor).toBe(3));
    stop();

    expect(seen.flatMap((event) => (event.type === 'TOOL_CALL_END' ? [event.status] : []))).toEqual(
      ['running', 'completed'],
    );
    expect(seen.map((event) => event.type)).toEqual([
      'TOOL_CALL_END',
      'TOOL_CALL_RESULT',
      'TOOL_CALL_END',
      'TOOL_CALL_RESULT',
    ]);
  });

  it('delivers an unknown cursorless future prefix without advancing early', async () => {
    const fake = new Fake();
    fake.responses = [
      [
        {
          type: 'FUTURE_PREFIX',
          threadId: 'sesn_1',
          runId: 'run_1',
          seq: 0,
          futureValue: 7,
        },
        { type: 'RUN_FINISHED', threadId: 'sesn_1', runId: 'run_1', seq: 1 },
      ],
    ];

    const { connection, seen, stop } = connect(fake);
    await vi.waitFor(() => expect(connection.cursor).toBe(1));
    stop();

    expect(seen.map((event) => event.type)).toEqual(['UNKNOWN', 'RUN_FINISHED']);
    expect(seen[0]).toMatchObject({ type: 'UNKNOWN', kind: 'FUTURE_PREFIX', seq: 0 });
  });

  it('gives every unknown prefix a stable occurrence identity and folds an overlap once', async () => {
    const fake = new Fake();
    const history = [
      {
        type: 'FUTURE:ALPHA',
        threadId: 'sesn:opaque',
        runId: 'sesn_1',
        seq: 0,
        futureValue: 'different kind',
      },
      {
        type: 'FUTURE:REPEAT',
        threadId: 'sesn:opaque',
        runId: 'sesn_1',
        seq: 0,
        futureValue: 'identical occurrence',
      },
      {
        type: 'FUTURE:REPEAT',
        threadId: 'sesn:opaque',
        runId: 'sesn_1',
        seq: 0,
        futureValue: 'identical occurrence',
      },
      { type: 'RUN_FINISHED', threadId: 'sesn:opaque', runId: 'sesn_1', seq: 7 },
    ];
    fake.responses = [history];
    fake.onOpen = history.map((frame) => ({ ...frame, runId: 'run_live' }));

    const { seen, statuses, stop } = connect(fake, 'sesn:opaque');
    await vi.waitFor(() => expect(statuses).toContain('live'));
    stop();

    expect(seen.map((event) => event.type)).toEqual([
      'UNKNOWN',
      'UNKNOWN',
      'UNKNOWN',
      'RUN_FINISHED',
    ]);
    const unknowns = seen.filter((event) => event.type === 'UNKNOWN');
    expect(unknowns.map((event) => event.projectionId)).toEqual([
      '["sesn:opaque",7,0,"FUTURE:ALPHA"]',
      '["sesn:opaque",7,1,"FUTURE:REPEAT"]',
      '["sesn:opaque",7,2,"FUTURE:REPEAT"]',
    ]);

    const state = fold([...seen, ...seen]);
    expect(state.entries).toEqual([
      expect.objectContaining({
        kind: 'unknown',
        id: '["sesn:opaque",7,0,"FUTURE:ALPHA"]',
        eventKind: 'FUTURE:ALPHA',
      }),
      expect.objectContaining({
        kind: 'unknown',
        id: '["sesn:opaque",7,1,"FUTURE:REPEAT"]',
        eventKind: 'FUTURE:REPEAT',
      }),
      expect.objectContaining({
        kind: 'unknown',
        id: '["sesn:opaque",7,2,"FUTURE:REPEAT"]',
        eventKind: 'FUTURE:REPEAT',
      }),
    ]);
  });

  it('keeps unknown identities byte-identical while retrying only an unacknowledged frame', async () => {
    const fake = new Fake();
    const projection = [
      {
        type: 'FUTURE_FIRST',
        threadId: 'sesn_1',
        runId: 'run_1',
        seq: 0,
        futureValue: 1,
      },
      {
        type: 'FUTURE_SECOND',
        threadId: 'sesn_1',
        runId: 'run_1',
        seq: 0,
        futureValue: 2,
      },
      { type: 'RUN_FINISHED', threadId: 'sesn_1', runId: 'run_1', seq: 7 },
    ];
    fake.responses = [projection, projection];
    let resume: () => void = () => {};
    const paused = new Promise<void>((resolve) => {
      resume = resolve;
    });
    const connection = new AguiConnection(fake, 'sesn_1', {
      backoffMs: 0,
      maxBackoffMs: 0,
      sleep: () => paused,
    });
    const seen: AguiEvent[] = [];
    const statuses: string[] = [];
    const secondAttempts: string[] = [];
    let failed = false;
    const stop = connection.subscribe(
      (event) => {
        if (event.type === 'UNKNOWN' && event.kind === 'FUTURE_SECOND') {
          secondAttempts.push(event.projectionId ?? 'missing');
          if (!failed) {
            failed = true;
            throw new Error('consumer failed before acknowledging the second prefix');
          }
        }
        seen.push(event);
      },
      (status) => statuses.push(status),
    );

    await vi.waitFor(() => expect(statuses).toContain('reconnecting'));
    expect(seen.map((event) => event.type)).toEqual(['UNKNOWN']);
    resume();
    await vi.waitFor(() => expect(connection.cursor).toBe(7));
    stop();

    expect(secondAttempts).toEqual([
      '["sesn_1",7,1,"FUTURE_SECOND"]',
      '["sesn_1",7,1,"FUTURE_SECOND"]',
    ]);
    expect(seen.map((event) => event.type)).toEqual(['UNKNOWN', 'UNKNOWN', 'RUN_FINISHED']);
    expect(seen.flatMap((event) => (event.type === 'UNKNOWN' ? [event.projectionId] : []))).toEqual(
      ['["sesn_1",7,0,"FUTURE_FIRST"]', '["sesn_1",7,1,"FUTURE_SECOND"]'],
    );
    expect(fake.historyReads).toEqual([0, 0]);
  });

  it('retries an unacknowledged prefix without advancing the cursor', async () => {
    const fake = new Fake();
    const message = committedMessage(7, 'm_7', 'retry the prefix');
    fake.responses = [message, message];
    let resume: () => void = () => {};
    const paused = new Promise<void>((resolve) => {
      resume = resolve;
    });
    const connection = new AguiConnection(fake, 'sesn_1', {
      backoffMs: 0,
      maxBackoffMs: 0,
      sleep: () => paused,
    });
    const seen: AguiEvent[] = [];
    const statuses: string[] = [];
    let startAttempts = 0;
    const stop = connection.subscribe(
      (event) => {
        if (event.type === 'TEXT_MESSAGE_START') {
          startAttempts += 1;
          if (startAttempts === 1) {
            throw new Error('consumer failed before acknowledging START');
          }
        }
        seen.push(event);
      },
      (status) => statuses.push(status),
    );

    await vi.waitFor(() => expect(statuses).toContain('reconnecting'));
    expect(connection.cursor).toBe(0);
    expect(seen).toEqual([]);
    resume();
    await vi.waitFor(() => expect(connection.cursor).toBe(7));
    stop();

    expect(startAttempts).toBe(2);
    expect(seen.map((event) => event.type)).toEqual([
      'TEXT_MESSAGE_START',
      'TEXT_MESSAGE_CONTENT',
      'TEXT_MESSAGE_END',
    ]);
    expect(fake.historyReads).toEqual([0, 0]);
  });

  it('retries only an unacknowledged final and advances after it succeeds', async () => {
    const fake = new Fake();
    const message = committedMessage(7, 'm_7', 'retry the final');
    fake.responses = [message, message];
    let resume: () => void = () => {};
    const paused = new Promise<void>((resolve) => {
      resume = resolve;
    });
    const connection = new AguiConnection(fake, 'sesn_1', {
      backoffMs: 0,
      maxBackoffMs: 0,
      sleep: () => paused,
    });
    const seen: AguiEvent[] = [];
    const statuses: string[] = [];
    let endAttempts = 0;
    const stop = connection.subscribe(
      (event) => {
        if (event.type === 'TEXT_MESSAGE_END') {
          endAttempts += 1;
          if (endAttempts === 1) {
            throw new Error('consumer failed before acknowledging END');
          }
        }
        seen.push(event);
      },
      (status) => statuses.push(status),
    );

    await vi.waitFor(() => expect(statuses).toContain('reconnecting'));
    expect(connection.cursor).toBe(0);
    expect(seen.map((event) => event.type)).toEqual(['TEXT_MESSAGE_START', 'TEXT_MESSAGE_CONTENT']);
    resume();
    await vi.waitFor(() => expect(connection.cursor).toBe(7));
    stop();

    expect(endAttempts).toBe(2);
    expect(seen.map((event) => event.type)).toEqual([
      'TEXT_MESSAGE_START',
      'TEXT_MESSAGE_CONTENT',
      'TEXT_MESSAGE_END',
    ]);
    expect(fake.historyReads).toEqual([0, 0]);
  });

  it('stops between projection frames when a consumer unsubscribes', async () => {
    const fake = new Fake();
    fake.responses = [committedMessage(7, 'm_7', 'must not be delivered')];
    const connection = new AguiConnection(fake, 'sesn_1');
    const seen: AguiEvent[] = [];
    const statuses: string[] = [];
    let stop: () => void = () => undefined;
    stop = connection.subscribe(
      (event) => {
        seen.push(event);
        stop();
      },
      (status) => statuses.push(status),
    );

    await vi.waitFor(() => expect(statuses).toContain('closed'));

    expect(seen.map((event) => event.type)).toEqual(['TEXT_MESSAGE_START']);
    expect(connection.cursor).toBe(0);
    expect(fake.opens).toBe(1);
  });

  it('still drops a frame the reconnect overlap replays', async () => {
    // The overlap the dedup exists for stays harmless: the same frame,
    // arriving twice, is delivered once.
    const fake = new Fake();
    fake.onOpen = [text(9, 'x'), text(9, 'x')];
    const { seen, stop } = connect(fake);
    await tick();
    stop();

    expect(seen).toHaveLength(1);
  });

  it('ignores frames behind the cursor while retaining current-sequence siblings', async () => {
    const fake = new Fake();
    fake.responses = [
      [
        text(1, 'first'),
        text(2, 'second'),
        text(1, 'stale replay'),
        {
          type: 'TEXT_MESSAGE_END',
          threadId: 'sesn_1',
          runId: 'run_1',
          seq: 2,
          messageId: 'm_2',
        },
      ],
    ];

    const { connection, seen, stop } = connect(fake);
    await tick();
    stop();

    expect(connection.cursor).toBe(2);
    expect(seen.map((event) => [event.seq, event.type])).toEqual([
      [1, 'TEXT_MESSAGE_CONTENT'],
      [2, 'TEXT_MESSAGE_CONTENT'],
      [2, 'TEXT_MESSAGE_END'],
    ]);
  });
});
