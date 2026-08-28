import { describe, expect, it } from 'vitest';
import { fold, initialConversation, reduce } from '../../src/conversation/fold.js';
import type { AguiEvent } from '../../src/agui/index.js';
import { initialState } from '../../src/wire.js';

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
function delta(seq: number, path: string, value: unknown): AguiEvent {
  return {
    type: 'STATE_DELTA',
    seq,
    threadId: 'sesn_1',
    runId: 'run_1',
    // A parsed event, not a wire frame: `delta` is what the service sends and
    // `patch` is what this client calls it once read (`wire.ts`).
    patch: [{ op: 'add', path, value }],
  };
}

function accountingDelta(seq: number, inputTokens: number, outputTokens: number): AguiEvent {
  return {
    type: 'STATE_DELTA',
    seq,
    threadId: 'sesn_1',
    runId: 'run_1',
    patch: [
      { op: 'replace', path: '/usage/inputTokens', value: inputTokens },
      { op: 'replace', path: '/usage/outputTokens', value: outputTokens },
    ],
  };
}

/** The stream as a client would have read it. */
const stream = (events: readonly AguiEvent[]) => fold(events);

describe('an unknown future event constructed directly', () => {
  it('retains the legacy sequence identity when no connection metadata exists', () => {
    const state = stream([
      {
        type: 'UNKNOWN',
        seq: 7,
        threadId: 'sesn_1',
        runId: 'run_1',
        kind: 'FUTURE_EVENT',
        raw: { type: 'FUTURE_EVENT', seq: 7 },
      },
    ]);

    expect(state.entries).toEqual([
      expect.objectContaining({
        kind: 'unknown',
        id: 'unknown-7',
        eventKind: 'FUTURE_EVENT',
        seq: 7,
      }),
    ]);
  });
});

describe('a runtime the session lost', () => {
  it('folds the first loss into the conversation state', () => {
    const state = stream([
      delta(8, '/runtimeLosses/-', {
        runtime: 'runtime-first',
        cause: 'out_of_memory',
        noticedAt: '2026-08-23T04:12:00Z',
      }),
    ]);

    expect(state.runtimeLoss).toEqual({
      runtime: 'runtime-first',
      cause: 'out_of_memory',
      noticedAt: '2026-08-23T04:12:00Z',
      seq: 8,
    });
  });

  it('lets the latest replayed loss supersede the first', () => {
    const state = stream([
      delta(8, '/runtimeLosses/-', {
        runtime: 'runtime-first',
        cause: 'failed',
      }),
      delta(19, '/runtimeLosses/-', {
        runtime: 'runtime-second',
        cause: 'displaced',
      }),
    ]);

    expect(state.runtimeLoss).toEqual({
      runtime: 'runtime-second',
      cause: 'displaced',
      noticedAt: undefined,
      seq: 19,
    });
  });

  it('ignores a malformed loss rather than building a partial fact', () => {
    const state = stream([
      delta(8, '/runtimeLosses/-', {
        cause: 'failed',
        noticedAt: '2026-08-23T04:12:00Z',
      }),
    ]);

    expect(state.runtimeLoss).toBeUndefined();
  });

  it('retains an unrelated additive state field without projecting it', () => {
    const state = stream([
      delta(8, '/somethingElse', {
        runtime: 'not-a-runtime-loss',
        cause: 'failed',
      }),
    ]);

    expect(state.runtimeLoss).toBeUndefined();
    expect(state.entries).toEqual([]);
    expect(state.aguiState.somethingElse).toEqual({
      runtime: 'not-a-runtime-loss',
      cause: 'failed',
    });
  });
});

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
      delta(4, '/gaps/-', { frames: 7 }),
      delta(5, '/planeChanges/-', { to: 'terminal', snapshotAt: '2026-08-18T10:00:00Z' }),
    ]);

    const gaps = state.entries.filter((entry) => entry.kind === 'gap');
    expect(gaps).toHaveLength(2);
    expect(gaps[0]).toMatchObject({ frames: 7 });
    expect(gaps[0]?.kind === 'gap' && gaps[0].movedTo).toBeUndefined();
    expect(gaps[1]).toMatchObject({ movedTo: 'terminal' });
  });

  it.each([{}, { frames: 'seven' }, { frames: -1 }])(
    'keeps a malformed conversation gap visible without inventing a count %#',
    (value) => {
      const state = stream([delta(4, '/gaps/-', value)]);
      expect(state.entries[0]).toMatchObject({ kind: 'gap', frames: undefined });
    },
  );

  it('does not interpret a lookalike patch path as a conversation gap', () => {
    const state = stream([delta(4, '/gaps-not-the-log', [{ frames: 7 }])]);
    expect(state.entries).toEqual([]);
  });
});

describe('a durable pending approval', () => {
  it('disappears for a watcher and a full replay when another tab resolves it', () => {
    const interrupted: AguiEvent = {
      type: 'RUN_FINISHED',
      seq: 0,
      threadId: 'sesn_1',
      runId: 'run_1',
      timestamp: '2026-08-26T01:02:03Z',
      outcome: {
        type: 'interrupt',
        interrupts: [
          {
            id: 'approval/~one',
            reason: 'cma:command_execution',
            message: 'Run the command?',
          },
        ],
      },
    };
    const requested: AguiEvent = {
      type: 'STATE_DELTA',
      seq: 7,
      threadId: 'sesn_1',
      runId: 'run_1',
      patch: [
        {
          op: 'add',
          path: '/pendingApprovals/approval~1~0one',
          value: {
            id: 'approval/~one',
            reason: 'cma:command_execution',
            message: 'Run the command?',
          },
        },
      ],
    };
    const resolved: AguiEvent = {
      type: 'STATE_DELTA',
      seq: 8,
      threadId: 'sesn_1',
      runId: 'run_1',
      patch: [{ op: 'remove', path: '/pendingApprovals/approval~1~0one' }],
    };

    const waiting = stream([interrupted, requested]);
    expect(waiting.pending).toMatchObject({
      id: 'approval/~one',
      askedAt: '2026-08-26T01:02:03Z',
    });
    expect(waiting.aguiState.pendingApprovals).toHaveProperty('approval/~one');

    const watched = reduce(waiting, resolved);
    const replayed = stream([interrupted, requested, resolved]);
    expect(watched).toEqual(replayed);
    expect(watched.pending).toBeUndefined();
    expect(watched.aguiState.pendingApprovals).toEqual({});
  });
});

describe('canonical state replay', () => {
  it('starts every production fold from a fresh canonical document', () => {
    const first = initialConversation();
    const second = initialConversation();
    const folded = reduce(first, delta(1, '/gaps/-', { frames: 2 }));

    expect(first.aguiState).toEqual(initialState());
    expect(second.aguiState).toEqual(initialState());
    expect(folded.aguiState.gaps).toEqual([{ frames: 2 }]);
    expect(first.aguiState.gaps).not.toBe(second.aguiState.gaps);
    expect(fold([]).aguiState).toEqual(initialState());
  });

  it('folds the same durable append delta only once', () => {
    const event = delta(8, '/gaps/-', { frames: 5 });
    const once = reduce(initialConversation(), event);
    const twice = reduce(once, event);

    expect(twice).toBe(once);
    expect(twice.aguiState.gaps).toEqual([{ frames: 5 }]);
    expect(twice.entries.filter((entry) => entry.kind === 'gap')).toHaveLength(1);
  });

  it('makes a full replay equal a prefix plus an overlapping resumed suffix', () => {
    const events = [
      delta(4, '/gaps/-', { frames: 2 }),
      delta(5, '/planeChanges/-', {
        to: 'conversation',
        snapshotAt: '2026-08-18T10:00:00Z',
      }),
      delta(6, '/contextCompactions/-', { was: 120, now: 60, window: 200 }),
    ];

    const full = fold(events);
    const prefix = fold(events.slice(0, 2));
    const resumed = fold(events.slice(1), prefix);
    expect(resumed).toEqual(full);
  });

  it('keeps multiple additions in one delta in exact order and cardinality', () => {
    const event: AguiEvent = {
      type: 'STATE_DELTA',
      seq: 9,
      threadId: 'sesn_1',
      runId: 'run_1',
      patch: [
        { op: 'add', path: '/gaps/-', value: { frames: 1 } },
        { op: 'add', path: '/gaps/-', value: { frames: 2 } },
        { op: 'add', path: '/gaps/-', value: { frames: 3 } },
      ],
    };

    const state = reduce(initialConversation(), event);
    expect(state.aguiState.gaps).toEqual([{ frames: 1 }, { frames: 2 }, { frames: 3 }]);
    const gaps = state.entries.filter((entry) => entry.kind === 'gap');
    expect(gaps.map((entry) => entry.frames)).toEqual([1, 2, 3]);
    expect(new Set(gaps.map((entry) => entry.id)).size).toBe(3);
  });

  it('applies separate ephemeral deltas even though they have no durable sequence', () => {
    const live = (frames: number): AguiEvent => ({
      ...delta(0, '/gaps/-', { frames }),
      ephemeral: true,
    });
    const state = fold([live(1), live(2)]);

    expect(state.stateDeltaSeq).toBe(0);
    expect(state.aguiState.gaps).toEqual([{ frames: 1 }, { frames: 2 }]);
    expect(state.entries.filter((entry) => entry.kind === 'gap')).toHaveLength(2);
  });

  it('keeps the existing defensive projections for malformed semantic values', () => {
    const valid: AguiEvent = {
      type: 'STATE_DELTA',
      seq: 10,
      threadId: 'sesn_1',
      runId: 'run_1',
      patch: [
        { op: 'replace', path: '/usage/inputTokens', value: 80 },
        { op: 'replace', path: '/usage/outputTokens', value: 20 },
        { op: 'replace', path: '/mode', value: 'safe' },
        { op: 'replace', path: '/plan', value: [{ title: 'Inspect', status: 'active' }] },
        {
          op: 'replace',
          path: '/configOptions',
          value: [{ id: 'effort', name: 'Effort', kind: 'select' }],
        },
        {
          op: 'replace',
          path: '/availableCommands',
          value: [{ name: 'stop', description: 'Stop this run' }],
        },
        {
          op: 'replace',
          path: '/signIn',
          value: { url: 'https://example.test/sign-in', code: 'ABCD' },
        },
      ],
    };
    const malformed: AguiEvent = {
      type: 'STATE_DELTA',
      seq: 11,
      threadId: 'sesn_1',
      runId: 'run_1',
      patch: [
        { op: 'replace', path: '/usage/inputTokens', value: 'eighty' },
        { op: 'replace', path: '/mode', value: 7 },
        { op: 'replace', path: '/plan', value: { title: 'not-a-list' } },
        { op: 'replace', path: '/configOptions', value: null },
        { op: 'replace', path: '/availableCommands', value: 'stop' },
        { op: 'replace', path: '/signIn', value: { code: 'missing-url' } },
      ],
    };

    const before = reduce(initialConversation(), valid);
    const after = reduce(before, malformed);
    expect(before.usage).toMatchObject({ inputTokens: 80, outputTokens: 20, tokens: 100 });
    expect(after.usage).toEqual(before.usage);
    expect(after.mode).toBe('safe');
    expect(after.plan).toEqual([{ title: 'Inspect', status: 'active' }]);
    expect(after.configOptions).toEqual([{ id: 'effort', name: 'Effort', kind: 'select' }]);
    expect(after.availableCommands).toEqual([{ name: 'stop', description: 'Stop this run' }]);
    // An object-shaped sign-in without a usable URL has always cleared the card.
    expect(after.signIn).toBeUndefined();
    expect(after.aguiState.mode).toBe(7);
  });
});

describe('context readings and summary boundaries', () => {
  it('keeps context unknown when turn accounting carries no reading', () => {
    const state = stream([accountingDelta(1, 0, 0)]);

    expect(state.aguiState.usage).toEqual({
      inputTokens: 0,
      outputTokens: 0,
      contextUsed: null,
      contextWindow: null,
    });
    expect(state.usage).toMatchObject({ inputTokens: 0, outputTokens: 0, tokens: 0 });
    expect(state.usage?.contextUsed).toBeUndefined();
  });

  it('keeps the last positive reading and stated ceiling through contextless accounting', () => {
    const state = stream([
      delta(1, '/usage/contextUsed', 198_000),
      delta(2, '/usage/contextWindow', 200_000),
      // The projection emits the real zero token totals, but no context patch.
      accountingDelta(3, 0, 0),
    ]);

    expect(state.aguiState.usage).toEqual({
      inputTokens: 0,
      outputTokens: 0,
      contextUsed: 198_000,
      contextWindow: 200_000,
    });
    expect(state.usage).toMatchObject({
      inputTokens: 0,
      outputTokens: 0,
      tokens: 0,
      contextUsed: 198_000,
      contextWindow: 200_000,
    });
  });

  it('folds a complete summary boundary and drops incomplete ones', () => {
    const state = stream([
      delta(5, '/contextCompactions/-', { was: 198_000 }),
      delta(6, '/contextCompactions/-', { now: 42_100 }),
      delta(7, '/contextCompactions/-', {
        was: 198_000,
        now: 42_100,
        window: 200_000,
      }),
    ]);

    expect(state.entries).toEqual([
      {
        kind: 'contextCompaction',
        id: 'context-compaction-7',
        was: 198_000,
        now: 42_100,
        window: 200_000,
        seq: 7,
      },
    ]);
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

  it('stays noninteractive through replacement progress and reopens on ready', () => {
    const replace = (seq: number, path: string, value: unknown): AguiEvent => ({
      type: 'STATE_DELTA',
      seq,
      threadId: 'sesn_1',
      runId: 'run_1',
      patch: [{ op: 'replace', path, value }],
    });

    let state = reduce(initialConversation(), replace(1, '/status', 'ready'));
    expect(state.sessionInteractive).toBe(true);

    state = reduce(
      state,
      failed(2, { message: 'moving this session to a replacement runtime', returns: true }),
    );
    expect(state.sessionInteractive).toBe(false);

    state = reduce(state, replace(3, '/progress', 'runtime_requested'));
    expect(state.aguiState.status).toBe('ready');
    expect(state.aguiState.progress).toBe('runtime_requested');
    expect(state.sessionInteractive).toBe(false);

    state = reduce(state, replace(4, '/progress', 'ready'));
    expect(state.sessionInteractive).toBe(false);

    state = reduce(state, replace(5, '/status', 'ready'));
    expect(state.aguiState.status).toBe('ready');
    expect(state.sessionInteractive).toBe(true);

    state = reduce(state, replace(6, '/progress', 'ready'));
    expect(state.sessionInteractive).toBe(true);
  });

  it('does not treat an idle but revivable session as interactive', () => {
    const state = stream([delta(1, '/status', 'idle')]);
    expect(state.aguiState.status).toBe('idle');
    expect(state.sessionInteractive).toBe(false);
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

  it('commits an unnumbered durable prefix at the final frame sequence', () => {
    const projection: readonly AguiEvent[] = [
      {
        type: 'TEXT_MESSAGE_START',
        seq: 0,
        threadId: 'sesn_1',
        runId: 'run_1',
        messageId: 'msg_7',
        role: 'assistant',
      },
      {
        type: 'TEXT_MESSAGE_CONTENT',
        seq: 0,
        threadId: 'sesn_1',
        runId: 'run_1',
        messageId: 'msg_7',
        delta: 'the whole answer',
      },
      {
        type: 'TEXT_MESSAGE_END',
        seq: 7,
        threadId: 'sesn_1',
        runId: 'run_1',
        messageId: 'msg_7',
      },
    ];

    const state = stream(projection);

    expect(state.entries).toEqual([
      expect.objectContaining({
        kind: 'message',
        id: 'msg_7',
        text: 'the whole answer',
        committed: true,
        seq: 7,
      }),
    ]);
    expect(stream([...projection, ...projection])).toEqual(state);
  });

  it('assigns a tool start its sequence when the final arguments frame arrives', () => {
    const state = stream([
      {
        type: 'TOOL_CALL_START',
        seq: 0,
        threadId: 'sesn_1',
        runId: 'run_1',
        toolCallId: 'call-7',
        toolKind: 'read',
      },
      {
        type: 'TOOL_CALL_ARGS',
        seq: 7,
        threadId: 'sesn_1',
        runId: 'run_1',
        toolCallId: 'call-7',
        delta: '{"path":"notes.md"}',
        truncated: false,
      },
    ]);

    expect(state.entries[0]).toMatchObject({
      kind: 'tool',
      id: 'call-7',
      seq: 7,
      arguments: { text: '{"path":"notes.md"}', truncated: false },
    });
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

describe('a turn and a tool call carry their time from the log', () => {
  it('folds the owner turn time onto the message', () => {
    const state = stream([
      {
        type: 'TEXT_MESSAGE_START',
        seq: 1,
        threadId: 'sesn_1',
        runId: 'run_1',
        messageId: 'msg_1',
        role: 'user',
        timestamp: '2026-08-19T04:00:00Z',
      },
    ]);

    expect(state.entries[0]).toMatchObject({
      kind: 'message',
      role: 'user',
      at: '2026-08-19T04:00:00Z',
    });
  });

  it('folds start and finish onto a tool call', () => {
    const state = stream([
      {
        type: 'TOOL_CALL_START',
        seq: 2,
        threadId: 'sesn_1',
        runId: 'run_1',
        toolCallId: 'call-1',
        toolKind: 'read',
        timestamp: '2026-08-19T04:00:00Z',
      },
      {
        type: 'TOOL_CALL_END',
        seq: 3,
        threadId: 'sesn_1',
        runId: 'run_1',
        toolCallId: 'call-1',
        status: 'completed',
        timestamp: '2026-08-19T04:00:02Z',
      },
    ]);

    expect(state.entries[0]).toMatchObject({
      kind: 'tool',
      startedAt: '2026-08-19T04:00:00Z',
      finishedAt: '2026-08-19T04:00:02Z',
    });
  });

  it('renders no time where the log carries none', () => {
    // A control plane predating timestamps wrote no time; that is absence and
    // the entry must carry none, never an invented one.
    const state = stream([
      {
        type: 'TEXT_MESSAGE_START',
        seq: 1,
        threadId: 'sesn_1',
        runId: 'run_1',
        messageId: 'msg_1',
        role: 'assistant',
      },
    ]);

    expect(state.entries[0]).toMatchObject({ kind: 'message', at: undefined });
  });
});
