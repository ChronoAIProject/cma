import { describe, expect, it } from 'vitest';
import { MockSession } from '../../src/testing/mock.js';
import { AguiConnection } from '../../src/agui/connection.js';
import { AguiOperations } from '../../src/agui/operations.js';
import { fold } from '../../src/conversation/fold.js';
import { TRANSCRIPTS, transcriptNamed } from '../../src/testing/transcripts.js';
import type { AguiEvent } from '../../src/agui/events.js';
import type { Transcript } from '../../src/testing/types.js';

/**
 * The mock driver — #117.
 *
 * The property that matters is that it drives the **real** client. A mock a
 * component can tell apart from the wire is a mock that proves nothing about
 * the wire.
 *
 * @vitest-environment node
 */

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

async function replay(session: MockSession) {
  const seen: AguiEvent[] = [];
  const statuses: string[] = [];
  const connection = new AguiConnection(session.transport, session.id, {
    backoffMs: 0,
    maxBackoffMs: 0,
    sleep: () => Promise.resolve(),
  });
  const stop = connection.subscribe(
    (event) => seen.push(event),
    (status) => statuses.push(status),
  );
  for (let i = 0; i < 30; i += 1) {
    await tick();
  }
  stop();
  return { seen, statuses, cursor: connection.cursor };
}

function projectionTranscript(
  name: string,
  events: readonly Record<string, unknown>[],
): Transcript {
  return {
    name,
    about: 'A focused durable-projection reconnect fixture.',
    provenance: 'authored',
    source: 'sdk/typescript/tests/testing/mock.test.ts',
    frames: events.map((event) => ({
      afterMs: 0,
      event: { threadId: 'sesn_projection_reconnect', ...event },
    })),
  };
}

describe('every fixture', () => {
  it('states whether it was recorded or authored', () => {
    // #117: "Replacing an authored fixture with a recorded one is a change
    // nobody should have to guess at, so the provenance is stated in the
    // fixture itself."
    for (const transcript of TRANSCRIPTS) {
      expect(['authored', 'recorded'], transcript.name).toContain(transcript.provenance);
      expect(transcript.source, transcript.name).not.toBe('');
      expect(transcript.about, transcript.name).not.toBe('');
    }
  });

  it('is currently authored, and says so', () => {
    // The honest state today: #76 has not produced Codex wire transcripts and
    // #77 is not running, so none of these is a recording. When one becomes a
    // recording this test is what makes the change visible.
    expect(TRANSCRIPTS.every((t) => t.provenance === 'authored')).toBe(true);
  });

  it('exposes the thread identity carried by every recorded frame', () => {
    for (const transcript of TRANSCRIPTS) {
      const session = new MockSession(transcript);
      expect(
        transcript.frames.every((frame) => frame.event.threadId === session.id),
        transcript.name,
      ).toBe(true);
    }
  });

  it('replays through the real client', async () => {
    for (const transcript of TRANSCRIPTS) {
      const { seen } = await replay(new MockSession(transcript));
      expect(seen.length, transcript.name).toBe(transcript.frames.length);
      // Sequence order, exactly once each — which is the connection's work,
      // not the mock's, and is the point.
      expect(
        seen.map((e) => e.seq),
        transcript.name,
      ).toEqual(transcript.frames.map((f) => f.event.seq));
    }
  });
});

describe('the cases the milestone needs', () => {
  it('covers a plain turn', async () => {
    const { seen } = await replay(new MockSession(transcriptNamed('plain-turn')));
    expect(seen.map((e) => e.type)).toEqual([
      'RUN_STARTED',
      'TEXT_MESSAGE_START',
      'TEXT_MESSAGE_CONTENT',
      'TEXT_MESSAGE_END',
      'RUN_FINISHED',
    ]);
  });

  it('covers two tool calls, one still running when the turn is read', async () => {
    const { seen } = await replay(new MockSession(transcriptNamed('two-tool-calls')));
    const starts = seen.filter((e) => e.type === 'TOOL_CALL_START');
    const ends = seen.filter((e) => e.type === 'TOOL_CALL_END');
    expect(starts).toHaveLength(2);
    expect(ends).toHaveLength(1);
  });

  it('covers an approval granted and an approval denied', async () => {
    for (const name of ['approval-granted', 'approval-denied']) {
      const { seen } = await replay(new MockSession(transcriptNamed(name)));
      const finished = seen.filter((e) => e.type === 'RUN_FINISHED');
      const interrupted = finished.filter(
        (e) => e.type === 'RUN_FINISHED' && e.outcome?.type === 'interrupt',
      );
      expect(interrupted, name).toHaveLength(1);
    }
  });

  it('covers a turn interrupted mid-stream', async () => {
    const { seen } = await replay(new MockSession(transcriptNamed('interrupted-turn')));
    const last = seen.at(-1);
    expect(last?.type).toBe('RUN_FINISHED');
    expect(last?.type === 'RUN_FINISHED' && last.outcome?.type).toBe('other');
  });

  it('covers a session that rotates mid-conversation', async () => {
    const { seen } = await replay(new MockSession(transcriptNamed('rotates')));
    const gap = seen.find(
      (e) => e.type === 'STATE_DELTA' && e.patch.some((p) => p.path.startsWith('/gaps')),
    );
    expect(gap).toBeDefined();
  });

  it('covers a session with no usage capability', async () => {
    const { seen } = await replay(new MockSession(transcriptNamed('no-usage')));
    expect(seen.some((e) => e.type === 'STATE_DELTA')).toBe(false);
  });

  it('covers an unknown event kind', async () => {
    const { seen } = await replay(new MockSession(transcriptNamed('unknown-kind')));
    const unknown = seen.find((e) => e.type === 'UNKNOWN');
    expect(unknown).toBeDefined();
    expect(unknown?.type === 'UNKNOWN' && unknown.kind).toBe('THINKING_START');
    // And the turn around it is untouched.
    expect(seen.filter((e) => e.type === 'TEXT_MESSAGE_CONTENT')).toHaveLength(1);
  });

  it('covers the complete conversation review surface', async () => {
    const { seen } = await replay(new MockSession(transcriptNamed('full-conversation')));

    const messages = seen.filter((event) => event.type === 'TEXT_MESSAGE_START');
    expect(
      messages.filter((event) => event.type === 'TEXT_MESSAGE_START' && event.role === 'user'),
    ).toHaveLength(2);
    expect(
      seen.some(
        (event) =>
          event.type === 'TEXT_MESSAGE_CONTENT' &&
          event.delta.includes('| Check | Result |') &&
          event.delta.includes('```ts') &&
          event.delta.includes('![Conversation surface diagram]'),
      ),
    ).toBe(true);

    const toolOutcomes = seen.flatMap((event) =>
      event.type === 'TOOL_CALL_END' ? [event.status] : [],
    );
    expect(toolOutcomes).toEqual(['ok', 'failed']);
    expect(
      seen.some((event) => event.type === 'REASONING_MESSAGE_CONTENT' && event.ephemeral === true),
    ).toBe(true);
    expect(seen.some((event) => event.type === 'RUN_ERROR')).toBe(true);

    const patches = seen.flatMap((event) => (event.type === 'STATE_DELTA' ? event.patch : []));
    expect(patches.map((patch) => patch.path)).toEqual(
      expect.arrayContaining(['/usage/inputTokens', '/usage/outputTokens', '/signIn', '/gaps/-']),
    );
    expect(
      seen.some(
        (event) =>
          event.type === 'RUN_FINISHED' &&
          event.outcome?.type === 'interrupt' &&
          event.outcome.interrupts[0]?.reason === 'cma:release_review',
      ),
    ).toBe(true);
  });

  it.each([
    ['TEXT_MESSAGE_START', 2],
    ['TEXT_MESSAGE_CONTENT', 3],
  ] as const)(
    'replays a complete message after disconnecting after %s',
    async (_prefix, dropAfter) => {
      // The durable message already exists when the server begins projecting its
      // cursorless prefixes. History must therefore return the whole projection
      // after a drop at either prefix.
      const session = new MockSession(transcriptNamed('plain-turn'), { dropAfter });
      const { cursor, seen, statuses } = await replay(session);

      expect(statuses).toContain('reconnecting');
      expect(seen.map((event) => event.type)).toEqual([
        'RUN_STARTED',
        'TEXT_MESSAGE_START',
        'TEXT_MESSAGE_CONTENT',
        'TEXT_MESSAGE_END',
        'RUN_FINISHED',
      ]);
      expect(seen.map((event) => event.seq)).toEqual([1, 0, 0, 2, 3]);
      expect(cursor).toBe(3);
      expect(fold(seen).entries).toEqual([
        expect.objectContaining({
          kind: 'message',
          id: 'm_1',
          role: 'assistant',
          text: 'The auth module now uses the new token type. Two call sites changed.',
          committed: true,
          seq: 2,
        }),
      ]);
    },
  );

  it('replays a complete tool start after disconnecting after TOOL_CALL_START', async () => {
    const transcript = projectionTranscript('tool-start-prefix', [
      { type: 'RUN_STARTED', runId: 'run_tool_start', seq: 1 },
      {
        type: 'TOOL_CALL_START',
        runId: 'run_tool_start',
        seq: 0,
        toolCallId: 'tool_start_1',
        toolCallName: 'cargo test',
        'cma:toolKind': 'shell',
      },
      {
        type: 'TOOL_CALL_ARGS',
        runId: 'run_tool_start',
        seq: 2,
        toolCallId: 'tool_start_1',
        delta: '{"package":"cma-sdk"}',
        'cma:truncated': false,
      },
      { type: 'RUN_FINISHED', runId: 'run_tool_start', seq: 3 },
    ]);
    const { cursor, seen, statuses } = await replay(new MockSession(transcript, { dropAfter: 2 }));

    expect(statuses).toContain('reconnecting');
    expect(seen.map((event) => event.type)).toEqual([
      'RUN_STARTED',
      'TOOL_CALL_START',
      'TOOL_CALL_ARGS',
      'RUN_FINISHED',
    ]);
    expect(seen.map((event) => event.seq)).toEqual([1, 0, 2, 3]);
    expect(seen.filter((event) => event.type === 'TOOL_CALL_START')).toHaveLength(1);
    expect(cursor).toBe(3);
    expect(fold(seen).entries).toEqual([
      expect.objectContaining({
        kind: 'tool',
        id: 'tool_start_1',
        toolKind: 'shell',
        target: 'cargo test',
        arguments: { text: '{"package":"cma-sdk"}', truncated: false },
        seq: 2,
      }),
    ]);
  });

  it('replays a complete tool update after disconnecting after TOOL_CALL_END', async () => {
    const transcript = projectionTranscript('tool-end-prefix', [
      { type: 'RUN_STARTED', runId: 'run_tool_end', seq: 1 },
      {
        type: 'TOOL_CALL_START',
        runId: 'run_tool_end',
        seq: 2,
        toolCallId: 'tool_end_1',
        toolCallName: 'cargo test',
        'cma:toolKind': 'shell',
      },
      {
        type: 'TOOL_CALL_END',
        runId: 'run_tool_end',
        seq: 0,
        toolCallId: 'tool_end_1',
        'cma:status': 'ok',
      },
      {
        type: 'TOOL_CALL_RESULT',
        runId: 'run_tool_end',
        seq: 3,
        toolCallId: 'tool_end_1',
        content: 'all tests passed',
        'cma:truncated': false,
      },
      { type: 'RUN_FINISHED', runId: 'run_tool_end', seq: 4 },
    ]);
    const { cursor, seen, statuses } = await replay(new MockSession(transcript, { dropAfter: 3 }));

    expect(statuses).toContain('reconnecting');
    expect(seen.map((event) => event.type)).toEqual([
      'RUN_STARTED',
      'TOOL_CALL_START',
      'TOOL_CALL_END',
      'TOOL_CALL_RESULT',
      'RUN_FINISHED',
    ]);
    expect(seen.map((event) => event.seq)).toEqual([1, 2, 0, 3, 4]);
    expect(seen.filter((event) => event.type === 'TOOL_CALL_END')).toHaveLength(1);
    expect(cursor).toBe(4);
    expect(fold(seen).entries).toEqual([
      expect.objectContaining({
        kind: 'tool',
        id: 'tool_end_1',
        status: 'ok',
        result: { text: 'all tests passed', truncated: false },
      }),
    ]);
  });
});

describe('the mock drives the real client', () => {
  it('leaves projection assembly and cursor advancement to the real client', async () => {
    // The mock supplies wire-shaped frames and complete history projections.
    // Parsing, assembly, cursor advancement and deduplication remain the real
    // connection's work.
    const session = new MockSession(transcriptNamed('plain-turn'));
    const { cursor, seen } = await replay(session);
    expect(cursor).toBe(3);
    expect(seen.map((event) => event.seq)).toEqual([1, 0, 0, 2, 3]);
  });

  it('keeps liveness out of history and the cursor', async () => {
    const transcript = projectionTranscript('live-tail', [
      { type: 'RUN_STARTED', runId: 'run_live', seq: 1 },
      {
        type: 'REASONING_MESSAGE_CONTENT',
        runId: 'run_live',
        seq: 0,
        messageId: 'reasoning_live_1',
        delta: 'Still working.',
        'cma:live': true,
      },
    ]);
    const session = new MockSession(transcript);
    const { cursor, seen } = await replay(session);
    const history = await session.transport.history(session.id, 0);

    expect(seen.map((event) => event.type)).toEqual(['RUN_STARTED', 'REASONING_MESSAGE_CONTENT']);
    expect(seen.map((event) => event.seq)).toEqual([1, 0]);
    expect(cursor).toBe(1);
    expect(history).toEqual([transcript.frames[0]?.event]);
  });

  it('records what the operations sent, without answering for the control plane', async () => {
    // A mock that refused would be asserting something about the control
    // plane it is in no position to know.
    const session = new MockSession(transcriptNamed('approval-granted'));
    const operations = new AguiOperations(session.id, session.operations);

    await operations.resolveApproval('apr_7Q2', 'allow', 'run_5');
    await operations.interrupt();

    expect(session.sent.map((s) => s.path)).toEqual([
      '/api/v1/agui/run',
      `/api/v1/sessions/${session.id}/interrupt`,
    ]);
    expect(session.sent[0]?.body).toMatchObject({
      resume: [{ interruptId: 'apr_7Q2', payload: { decision: 'allow' } }],
    });
  });

  it('honours recorded timing when asked to', async () => {
    // At speed 0 a test gets its frames at once; at any other speed the delays
    // are real, so streaming looks like streaming rather than appearing whole.
    const waits: number[] = [];
    const session = new MockSession(transcriptNamed('plain-turn'), {
      speed: 1,
      sleep: (ms) => {
        waits.push(ms);
        return Promise.resolve();
      },
    });
    await replay(session);
    expect(waits).toEqual([0, 120, 400, 40, 30]);
  });

  it('lets a test emit something the transcript could not express', async () => {
    const session = new MockSession(transcriptNamed('plain-turn'));
    const seen: AguiEvent[] = [];
    const connection = new AguiConnection(session.transport, session.id, {
      backoffMs: 0,
      sleep: () => Promise.resolve(),
    });
    const stop = connection.subscribe((event) => seen.push(event));
    await tick();
    await tick();

    // Whatever a component just did, answered live.
    session.emit(
      { threadId: session.id, runId: 'run_1', seq: 99, type: 'RUN_ERROR', message: 'x' },
      () => {},
    );
    await tick();
    stop();

    // It is in history, so a reconnect would find it.
    const later = new AguiConnection(session.transport, session.id, {
      backoffMs: 0,
      sleep: () => Promise.resolve(),
    });
    const replayed: AguiEvent[] = [];
    const stopLater = later.subscribe((event) => replayed.push(event));
    await tick();
    await tick();
    stopLater();

    expect(replayed.some((e) => e.seq === 99)).toBe(true);
  });

  it.each([
    {
      name: 'message',
      frames: [
        {
          threadId: 'sesn_projection_reconnect',
          runId: 'run_emitted_message',
          seq: 0,
          type: 'TEXT_MESSAGE_START',
          messageId: 'emitted_message_1',
          role: 'assistant',
        },
        {
          threadId: 'sesn_projection_reconnect',
          runId: 'run_emitted_message',
          seq: 0,
          type: 'TEXT_MESSAGE_CONTENT',
          messageId: 'emitted_message_1',
          delta: 'The complete message survives.',
        },
        {
          threadId: 'sesn_projection_reconnect',
          runId: 'run_emitted_message',
          seq: 1,
          type: 'TEXT_MESSAGE_END',
          messageId: 'emitted_message_1',
        },
      ],
    },
    {
      name: 'tool start',
      frames: [
        {
          threadId: 'sesn_projection_reconnect',
          runId: 'run_emitted_tool_start',
          seq: 0,
          type: 'TOOL_CALL_START',
          toolCallId: 'emitted_tool_start_1',
          toolCallName: 'cargo test',
          'cma:toolKind': 'shell',
        },
        {
          threadId: 'sesn_projection_reconnect',
          runId: 'run_emitted_tool_start',
          seq: 1,
          type: 'TOOL_CALL_ARGS',
          toolCallId: 'emitted_tool_start_1',
          delta: '--workspace',
          'cma:truncated': false,
        },
      ],
    },
    {
      name: 'tool result',
      frames: [
        {
          threadId: 'sesn_projection_reconnect',
          runId: 'run_emitted_tool_result',
          seq: 0,
          type: 'TOOL_CALL_END',
          toolCallId: 'emitted_tool_result_1',
          'cma:status': 'ok',
        },
        {
          threadId: 'sesn_projection_reconnect',
          runId: 'run_emitted_tool_result',
          seq: 1,
          type: 'TOOL_CALL_RESULT',
          toolCallId: 'emitted_tool_result_1',
          content: 'all tests passed',
          'cma:truncated': false,
        },
      ],
    },
  ])(
    'makes an emitted $name projection replayable before delivering its first prefix',
    async ({ frames }) => {
      const session = new MockSession(
        projectionTranscript('dynamic-projection-base', [
          {
            type: 'REASONING_MESSAGE_CONTENT',
            runId: 'run_live_only',
            seq: 0,
            messageId: 'live_only_1',
            delta: 'Still working.',
            'cma:live': true,
          },
        ]),
      );
      const historiesAtFirstFrame: Promise<readonly unknown[]>[] = [];

      expect(() =>
        session.emitProjection(frames, () => {
          historiesAtFirstFrame.push(session.transport.history(session.id, 0));
          throw new Error('the mock stream disconnected');
        }),
      ).toThrow('the mock stream disconnected');

      expect(historiesAtFirstFrame).toHaveLength(1);
      expect((await Promise.all(historiesAtFirstFrame))[0]).toEqual(frames);

      const { cursor, seen } = await replay(session);
      const runId = frames[0]?.runId;
      const replayed = seen.filter((event) => event.runId === runId);
      expect(replayed.map((event) => event.type)).toEqual(frames.map((frame) => frame.type));
      expect(replayed.map((event) => event.seq)).toEqual(frames.map((frame) => frame.seq));
      expect(cursor).toBe(1);
    },
  );

  it('rejects malformed emitted projections without adding partial history', async () => {
    const session = new MockSession(
      projectionTranscript('dynamic-validation-base', [
        {
          type: 'REASONING_MESSAGE_CONTENT',
          runId: 'run_live_only',
          seq: 0,
          messageId: 'live_only_2',
          delta: 'Still working.',
          'cma:live': true,
        },
      ]),
    );
    const prefix = {
      threadId: session.id,
      runId: 'run_dynamic_validation',
      seq: 0,
      type: 'TEXT_MESSAGE_START',
      messageId: 'dynamic_validation_1',
      role: 'assistant',
    } as const;
    const final = {
      threadId: session.id,
      runId: 'run_dynamic_validation',
      seq: 1,
      type: 'TEXT_MESSAGE_END',
      messageId: 'dynamic_validation_1',
    } as const;

    expect(() => session.emit(prefix, () => {})).toThrow(/ended inside a durable projection/);
    expect(() => session.emitProjection([], () => {})).toThrow(/was empty/);
    expect(() =>
      session.emitProjection([prefix, { ...final, threadId: 'sesn_another' }], () => {}),
    ).toThrow(/another threadId/);
    expect(() =>
      session.emitProjection([prefix, { ...final, runId: 'run_another' }], () => {}),
    ).toThrow(/changed runId/);
    expect(() =>
      session.emitProjection(
        [
          { ...final, seq: 1 },
          { ...final, seq: 2 },
        ],
        () => {},
      ),
    ).toThrow(/completed before its final frame/);
    expect(await session.transport.history(session.id, 0)).toEqual([]);
  });

  it('emits sequence-zero liveness without retaining it in history', async () => {
    const session = new MockSession(transcriptNamed('plain-turn'));
    const live = {
      threadId: session.id,
      runId: 'run_dynamic_live',
      seq: 0,
      type: 'REASONING_MESSAGE_CONTENT',
      messageId: 'dynamic_live_1',
      delta: 'Still working.',
      'cma:live': true,
    } as const;
    const seen: unknown[] = [];

    session.emit(live, (raw) => seen.push(raw));

    expect(seen).toEqual([live]);
    expect(await session.transport.history(session.id, 0)).toEqual([]);
    expect(() => session.emit({ ...live, seq: 1 }, () => {})).toThrow(
      /liveness must have sequence zero/,
    );
  });
});
