import { describe, expect, it } from 'vitest';
import { fold } from './conversation';
import type { AguiEvent } from '../agui';

/**
 * The reducer that turns an event stream into a conversation.
 *
 * Pure, and tested as such: what a client draws is a function of what the log
 * said, so the cases worth pinning are the ones where the log says something
 * happened *to* the conversation rather than in it.
 */

/**
 * A state delta as the stream carries it.
 *
 * The run and thread identifiers are on every frame and matter to nothing this
 * reducer does, so they are filled in here rather than repeated in each case.
 */
function delta(seq: number, path: string, value: Record<string, unknown>): AguiEvent {
  return {
    type: 'STATE_DELTA',
    seq,
    threadId: 'sesn_1',
    runId: 'run_1',
    patch: [{ op: 'add', path, value }],
  };
}

/** The stream as a client would have read it. */
const stream = (events: readonly AguiEvent[]) => fold(events);

describe('a plane change is a boundary', () => {
  it('draws where history stopped being continuous, and where the session went', () => {
    // §18.6: a rule across the stream at the snapshot moment, not a status
    // chip. Anything the agent did after that moment did not travel, and a
    // gap somebody can see is one they can recognise rather than read as the
    // agent having forgotten.
    const state = stream([
      delta(9, '/planeChanges/-', {
        from: 'terminal',
        to: 'conversation',
        snapshotAt: '2026-08-18T09:14:22Z',
      }),
    ]);

    const boundary = state.entries.at(-1);
    expect(boundary?.kind).toBe('gap');
    expect(boundary).toMatchObject({
      rotatedAt: '2026-08-18T09:14:22Z',
      movedTo: 'conversation',
    });
  });

  it('keeps a plane change and a lost stretch of conversation apart', () => {
    // Both are boundaries and neither is the other: one is something the owner
    // asked for and the other is something that happened to them.
    const state = stream([
      delta(4, '/gaps/-', { reason: 'behind' }),
      delta(5, '/planeChanges/-', { to: 'terminal', snapshotAt: '2026-08-18T10:00:00Z' }),
    ]);

    const gaps = state.entries.filter((entry) => entry.kind === 'gap');
    expect(gaps).toHaveLength(2);
    expect(gaps[0]?.kind === 'gap' && gaps[0].movedTo).toBeUndefined();
    expect(gaps[1]).toMatchObject({ movedTo: 'terminal' });
  });
});

/** A run ending badly, as the projection sends one. */
function failed(seq: number, body: Partial<Extract<AguiEvent, { type: 'RUN_ERROR' }>>): AguiEvent {
  return {
    type: 'RUN_ERROR',
    seq,
    threadId: 'sesn_1',
    runId: 'run_1',
    message: 'the run failed',
    ...body,
  };
}

describe('a run that stopped badly', () => {
  it('becomes an entry in the stream, where it happened', () => {
    // §18.9. This client already folded the failure into `error` and nothing
    // read it, so a run that failed was drawn exactly like a run still going:
    // nothing at all. In the stream rather than a band because a failure has
    // a position — it happened after the last thing the agent did.
    const state = stream([
      { type: 'RUN_STARTED', seq: 1, threadId: 'sesn_1', runId: 'run_1' },
      failed(2, { message: 'the model endpoint refused', code: 'error' }),
    ]);

    expect(state.entries).toHaveLength(1);
    expect(state.entries[0]).toMatchObject({
      kind: 'failure',
      message: 'the model endpoint refused',
      code: 'error',
    });
    expect(state.turnInFlight).toBe(false);
  });

  it('names the ability, when the failure was a refusal', () => {
    const state = stream([
      failed(3, { message: 'cannot be what it is', capability: 'conversation' }),
    ]);

    expect(state.entries[0]).toMatchObject({ kind: 'failure', capability: 'conversation' });
    expect(state.error).toMatchObject({ capability: 'conversation' });
  });

  it('is folded twice to the same result', () => {
    // The deduplication upstream cannot rule out a frame arriving again
    // across a reconnect, and two failure entries for one failure would read
    // as two failures.
    const once = failed(7, { message: 'the runtime went away' });
    const state = stream([once, once]);

    expect(state.entries).toHaveLength(1);
  });

  it('is cleared by the next run, entry and all', () => {
    const state = stream([
      failed(2, { message: 'the runtime went away' }),
      { type: 'RUN_STARTED', seq: 3, threadId: 'sesn_1', runId: 'run_2' },
    ]);

    expect(state.error).toBeUndefined();
    expect(state.entries).toHaveLength(1);
  });
});

describe('a session being carried onto another runtime is not a failure', () => {
  it('ends the turn and draws nothing that says something went wrong', () => {
    // The handover ends the run — its runtime is being torn down — and
    // reaches this client on the same frame a failure does. §11 forbids a
    // rotating session taking the failure ink, so the two must be told apart
    // here or every rotation is drawn as a fault.
    const state = stream([
      { type: 'RUN_STARTED', seq: 1, threadId: 'sesn_1', runId: 'run_1' },
      failed(2, { message: 'moving this session to the other plane', returns: true }),
    ]);

    expect(state.turnInFlight).toBe(false);
    expect(state.error).toBeUndefined();
    expect(state.entries.filter((entry) => entry.kind === 'failure')).toHaveLength(0);
  });

  it('still draws a real failure on the same frame shape', () => {
    // The guard must turn on what the projection actually sends, not on
    // wording: a control plane that predates the marker sends nothing, which
    // reads as a failure — the safer of the two to be wrong about.
    const state = stream([failed(2, { message: 'the runtime went away' })]);

    expect(state.entries.filter((entry) => entry.kind === 'failure')).toHaveLength(1);
  });
});

/** A piece of a message being typed, as the live lane sends one. */
function typing(delta: string): AguiEvent {
  return {
    type: 'TEXT_MESSAGE_CONTENT',
    seq: 0,
    threadId: 'sesn_1',
    runId: 'run_1',
    messageId: 'msg_live',
    delta,
    ephemeral: true,
  };
}

describe('a message being typed', () => {
  it('accumulates outside the stream, so it is never an entry', () => {
    // §18.1: history has no in-flight message, so if this were an entry a
    // reload would draw a different conversation from the one somebody
    // watched — and the one they reload into is the one they will believe.
    const state = stream([
      { type: 'RUN_STARTED', seq: 1, threadId: 'sesn_1', runId: 'run_1' },
      typing('Hel'),
      typing('lo '),
      typing('there'),
    ]);

    expect(state.live?.text).toBe('Hello there');
    expect(state.entries).toHaveLength(0);
  });

  it('is replaced by the committed message rather than sitting beside it', () => {
    const state = stream([
      { type: 'RUN_STARTED', seq: 1, threadId: 'sesn_1', runId: 'run_1' },
      typing('Hello th'),
      {
        type: 'TEXT_MESSAGE_START',
        seq: 4,
        threadId: 'sesn_1',
        runId: 'run_1',
        messageId: 'msg_4',
        role: 'assistant',
      },
      {
        type: 'TEXT_MESSAGE_CONTENT',
        seq: 4,
        threadId: 'sesn_1',
        runId: 'run_1',
        messageId: 'msg_4',
        delta: 'Hello there',
      },
    ]);

    expect(state.live).toBeUndefined();
    expect(state.entries).toHaveLength(1);
    expect(state.entries[0]).toMatchObject({ kind: 'message', text: 'Hello there' });
  });

  it('folds to the same conversation watched and replayed', () => {
    // The equivalence rule, stated as a test rather than as a promise. The
    // live frames are exactly what a reload would not receive, and dropping
    // them must change nothing about the settled conversation.
    const committed: readonly AguiEvent[] = [
      { type: 'RUN_STARTED', seq: 1, threadId: 'sesn_1', runId: 'run_1' },
      {
        type: 'TEXT_MESSAGE_START',
        seq: 2,
        threadId: 'sesn_1',
        runId: 'run_1',
        messageId: 'msg_2',
        role: 'assistant',
      },
      {
        type: 'TEXT_MESSAGE_CONTENT',
        seq: 2,
        threadId: 'sesn_1',
        runId: 'run_1',
        messageId: 'msg_2',
        delta: 'Hello there',
      },
      { type: 'RUN_FINISHED', seq: 3, threadId: 'sesn_1', runId: 'run_1' },
    ];
    const watched = stream([
      committed[0]!,
      typing('Hel'),
      typing('lo there'),
      ...committed.slice(1),
    ]);
    const replayed = stream(committed);

    expect(watched).toEqual(replayed);
  });

  it('is dropped when the turn ends without committing anything', () => {
    // A runtime that went away mid-message leaves nothing in the log either,
    // so leaving it drawn would show something a reload cannot reproduce.
    const state = stream([
      { type: 'RUN_STARTED', seq: 1, threadId: 'sesn_1', runId: 'run_1' },
      typing('half a th'),
      failed(2, { message: 'the runtime went away' }),
    ]);

    expect(state.live).toBeUndefined();
  });
});

/** A piece of the agent's reasoning, as the live lane sends one. */
function reasoning(delta: string): AguiEvent {
  return {
    type: 'REASONING_MESSAGE_CONTENT',
    seq: 0,
    threadId: 'sesn_1',
    runId: 'run_1',
    messageId: 'reasoning_live',
    delta,
    ephemeral: true,
  };
}

describe('the agent reasoning, §18.8', () => {
  it('accumulates outside the stream and never becomes an entry', () => {
    const state = stream([
      { type: 'RUN_STARTED', seq: 1, threadId: 'sesn_1', runId: 'run_1' },
      reasoning('the file is '),
      reasoning('probably under src/'),
    ]);

    expect(state.reasoning?.text).toBe('the file is probably under src/');
    expect(state.entries).toHaveLength(0);
  });

  it('is not folded into the message the agent commits', () => {
    // The way this could go quietly and permanently wrong. The committed
    // message *is* durable and is what a reload shows, so reasoning leaking
    // into it would publish something the agent never said, for good.
    const state = stream([
      { type: 'RUN_STARTED', seq: 1, threadId: 'sesn_1', runId: 'run_1' },
      reasoning('I should not say this part'),
      typing('Found '),
      typing('it.'),
    ]);

    expect(state.live?.text).toBe('Found it.');
    expect(state.reasoning?.text).toBe('I should not say this part');
  });

  it('is gone when the turn ends', () => {
    const state = stream([
      { type: 'RUN_STARTED', seq: 1, threadId: 'sesn_1', runId: 'run_1' },
      reasoning('working it out'),
      { type: 'RUN_FINISHED', seq: 2, threadId: 'sesn_1', runId: 'run_1' },
    ]);

    expect(state.reasoning).toBeUndefined();
  });

  it('is gone when the run fails, too', () => {
    const state = stream([reasoning('working it out'), failed(2, { message: 'gone' })]);

    expect(state.reasoning).toBeUndefined();
  });

  it('leaves the settled conversation identical to a reload', () => {
    // §18.1, and the reason none of this may be durable: somebody reloading
    // to check what the agent decided must not watch it think again.
    const committed: readonly AguiEvent[] = [
      { type: 'RUN_STARTED', seq: 1, threadId: 'sesn_1', runId: 'run_1' },
      {
        type: 'TEXT_MESSAGE_START',
        seq: 2,
        threadId: 'sesn_1',
        runId: 'run_1',
        messageId: 'msg_2',
        role: 'assistant',
      },
      {
        type: 'TEXT_MESSAGE_CONTENT',
        seq: 2,
        threadId: 'sesn_1',
        runId: 'run_1',
        messageId: 'msg_2',
        delta: 'Found it.',
      },
      { type: 'RUN_FINISHED', seq: 3, threadId: 'sesn_1', runId: 'run_1' },
    ];
    const watched = stream([
      committed[0]!,
      reasoning('thinking hard'),
      typing('Found it.'),
      ...committed.slice(1),
    ]);

    expect(watched).toEqual(stream(committed));
  });
});
