import { describe, expect, it } from 'vitest';
import { MockSession } from './mock';
import { AguiConnection } from '../connection';
import { AguiOperations } from '../operations';
import { TRANSCRIPTS, transcriptNamed } from '../fixtures/transcripts';
import type { AguiEvent } from '../events';

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
  const connection = new AguiConnection(session.transport, 'sesn_1', {
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

  it('covers a stream that dies mid-turn and reconnects', async () => {
    // The one thing a mock can do that the wire cannot on demand.
    const session = new MockSession(transcriptNamed('plain-turn'), { dropAfter: 2 });
    const { seen, statuses } = await replay(session);

    expect(statuses).toContain('reconnecting');
    // Nothing lost and nothing doubled: the reconnect reads what had already
    // happened out of history and picks the rest up live.
    expect(seen.map((e) => e.seq)).toEqual([1, 2, 3, 4, 5]);
  });
});

describe('the mock drives the real client', () => {
  it('duplicates no protocol logic', async () => {
    // The parsing, the cursor, the deduplication and the reconnect order are
    // all `AguiConnection`'s. A frame that arrives twice is delivered once
    // because the connection says so, not because the mock avoided it.
    const session = new MockSession(transcriptNamed('plain-turn'));
    const { cursor, seen } = await replay(session);
    expect(cursor).toBe(5);
    expect(new Set(seen.map((e) => e.seq)).size).toBe(seen.length);
  });

  it('records what the operations sent, without answering for the control plane', async () => {
    // A mock that refused would be asserting something about the control
    // plane it is in no position to know.
    const session = new MockSession(transcriptNamed('approval-granted'));
    const operations = new AguiOperations('sesn_1', session.operations);

    await operations.resolveApproval('apr_7Q2', 'allow', 'run_5');
    await operations.interrupt();

    expect(session.sent.map((s) => s.path)).toEqual([
      '/api/v1/agui/run',
      '/api/v1/sessions/sesn_1/interrupt',
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
    const connection = new AguiConnection(session.transport, 'sesn_1', {
      backoffMs: 0,
      sleep: () => Promise.resolve(),
    });
    const stop = connection.subscribe((event) => seen.push(event));
    await tick();
    await tick();

    // Whatever a component just did, answered live.
    session.emit(
      { threadId: 'sesn_1', runId: 'run_1', seq: 99, type: 'RUN_ERROR', message: 'x' },
      () => {},
    );
    await tick();
    stop();

    // It is in history, so a reconnect would find it.
    const later = new AguiConnection(session.transport, 'sesn_1', {
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
});
