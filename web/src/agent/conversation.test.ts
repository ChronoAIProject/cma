import { describe, expect, it } from 'vitest';
import type { AgentEvent, AgentLiveEvent, AgentTranscript } from '../api/types';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import { mergeTranscriptSnapshot } from './chronology';

const TRANSCRIPT: AgentTranscript = {
  agentId: 'agt_1',
  throughSeq: 4,
  turns: [
    {
      turnSequence: 1,
      startedAt: '2026-09-06T00:00:00Z',
      ownerMessage: 'Inspect the repository.',
      agentMessages: ['I found the issue.'],
      messages: [
        {
          sequence: 2,
          role: 'user',
          content: 'Inspect the repository.',
          appendedAt: '2026-09-06T00:00:01Z',
        },
        {
          sequence: 3,
          role: 'assistant',
          content: 'I found the issue.',
          appendedAt: '2026-09-06T00:00:02Z',
        },
      ],
      outcome: { type: 'completed' },
    },
  ],
};

function durable(seq: number, type: string, fields: Record<string, unknown> = {}): AgentEvent {
  return { seq, appendedAt: '2026-09-06T00:01:00Z', kind: { type, ...fields } };
}

function live(type: string, fields: Record<string, unknown> = {}): AgentLiveEvent {
  return { type, runId: '2', threadId: 'agt_1', ...fields };
}

describe('durable Agent conversation folding', () => {
  it('starts from the transcript snapshot and ignores events already covered by it', () => {
    const state = conversationFromTranscript(TRANSCRIPT);
    expect(state.messages.map(({ role, content }) => ({ role, content }))).toEqual([
      { role: 'user', content: 'Inspect the repository.' },
      { role: 'assistant', content: 'I found the issue.' },
    ]);
    expect(foldAgentEvent(state, durable(4, 'agent_message', { content: 'duplicate' }))).toBe(
      state,
    );
  });

  it('folds tools, approvals, usage, and recovery facts from later durable events', () => {
    const events = [
      durable(5, 'turn_started', { turn_sequence: 2 }),
      durable(6, 'tool_call_started', {
        call_id: 'call_1',
        title: 'Read files',
        kind: 'shell',
        status: 'running',
      }),
      durable(7, 'tool_call_updated', { call_id: 'call_1', status: 'completed' }),
      durable(8, 'approval_requested', {
        approval_id: 'approval_1',
        action: 'write',
        detail: 'Edit app.ts',
      }),
      durable(9, 'usage', {
        input_tokens: 120,
        output_tokens: 40,
        context_used: 300,
        context_window: 1000,
      }),
      durable(10, 'conversation_gap', { frames: 3 }),
    ];
    const state = events.reduce(foldAgentEvent, conversationFromTranscript(TRANSCRIPT));
    expect(state.toolCalls).toEqual([
      expect.objectContaining({ id: 'call_1', title: 'Read files', status: 'completed' }),
    ]);
    expect(state.approvals).toEqual([
      expect.objectContaining({ id: 'approval_1', reason: 'write', message: 'Edit app.ts' }),
    ]);
    expect(state.usage).toEqual(expect.objectContaining({ inputTokens: 120, contextWindow: 1000 }));
    expect(state.recovery[0]?.detail).toContain('3 live frame(s) were lost');
    expect(state.cursor).toBe(10);
  });

  it('retains the loss notice and later messages when a turn with missing output completes', () => {
    const active = [
      durable(5, 'turn_started', { turn_sequence: 5 }),
      durable(6, 'conversation_gap', { frames: 35 }),
      durable(7, 'agent_message', { content: 'Stored after the gap.' }),
    ].reduce(foldAgentEvent, conversationFromTranscript(TRANSCRIPT));
    expect(active.activeRunId).toBe('5');
    expect(active.recovery).toEqual([
      expect.objectContaining({
        title: 'Incomplete conversation output',
        detail:
          '35 live frame(s) were lost. Stored messages remain available, but the output may be incomplete.',
      }),
    ]);
    const completed = foldAgentEvent(active, durable(8, 'turn_completed', { turn_sequence: 5 }));
    expect(completed.activeRunId).toBeUndefined();
    expect(completed.messages.map(({ content }) => content)).toEqual([
      'Inspect the repository.',
      'I found the issue.',
      'Stored after the gap.',
    ]);
    expect(completed.recovery).toEqual(active.recovery);
    expect(completed.cursor).toBe(8);
    expect(completed.turnOutcomes?.['5']).toBe('failed');
  });

  it('keeps recorded loss failed when AG-UI finishes and allows the next intact turn to complete', () => {
    const incomplete = [
      durable(5, 'turn_started', { turn_sequence: 5 }),
      durable(6, 'conversation_gap', { frames: 1 }),
      durable(7, 'agent_message', { content: 'Retained output' }),
    ].reduce(foldAgentEvent, conversationFromTranscript(TRANSCRIPT));
    const complete = (state: typeof incomplete, runId: string) =>
      foldLiveEvent(
        state,
        { type: 'RUN_FINISHED', threadId: 'agt_1', runId, outcome: { type: 'complete' } },
        'agt_1',
      );
    const finished = complete(incomplete, '5');
    expect(finished.turnOutcomes?.['5']).toBe('failed');
    expect(finished.recovery).toEqual(incomplete.recovery);
    const later = foldAgentEvent(finished, durable(8, 'turn_started', { turn_sequence: 8 }));
    expect(complete(later, '8').turnOutcomes?.['8']).toBe('completed');
  });

  it('hydrates chronological steering messages and continues the same active turn after reload', () => {
    const turn = TRANSCRIPT.turns[0]!;
    const steering = {
      sequence: 4,
      role: 'user' as const,
      content: 'Check the tests first.',
      appendedAt: '2026-09-06T00:00:03Z',
    };
    const snapshot = conversationFromTranscript({
      ...TRANSCRIPT,
      turns: [{ ...turn, messages: [...turn.messages, steering], outcome: { type: 'active' } }],
    });
    expect(snapshot.activeRunId).toBe('1');
    expect(snapshot.messages).toEqual([
      {
        id: 'event:2',
        runId: '1',
        role: 'user',
        content: 'Inspect the repository.',
        status: 'complete',
        at: '2026-09-06T00:00:01Z',
        order: 2,
        sequence: 2,
      },
      {
        id: 'event:3',
        runId: '1',
        role: 'assistant',
        content: 'I found the issue.',
        status: 'complete',
        at: '2026-09-06T00:00:02Z',
        order: 3,
        sequence: 3,
      },
      {
        id: 'event:4',
        runId: '1',
        role: 'user',
        content: 'Check the tests first.',
        status: 'complete',
        at: '2026-09-06T00:00:03Z',
        order: 4,
        sequence: 4,
      },
    ]);
    expect(
      foldAgentEvent(snapshot, durable(4, 'owner_message', { content: steering.content })),
    ).toBe(snapshot);
    const continued = foldAgentEvent(
      snapshot,
      durable(5, 'agent_message', { content: 'Tests pass.' }),
    );
    expect(continued.messages.at(-1)).toMatchObject({
      id: 'event:5',
      runId: '1',
      role: 'assistant',
      content: 'Tests pass.',
    });
    expect(
      foldAgentEvent(continued, durable(6, 'turn_completed', { turn_sequence: 1 })).activeRunId,
    ).toBeUndefined();
  });

  it('does not recreate summary text when the authoritative messages array is empty', () => {
    const state = conversationFromTranscript({
      ...TRANSCRIPT,
      turns: [{ ...TRANSCRIPT.turns[0]!, messages: [] }],
    });
    expect(state.messages).toEqual([]);
    expect(state.activeRunId).toBeUndefined();
  });

  it('deduplicates queued owner input reused by a pre-write retry and retains the latest turn', () => {
    const first = TRANSCRIPT.turns[0]!;
    const state = conversationFromTranscript({
      ...TRANSCRIPT,
      throughSeq: 7,
      turns: [
        { ...first, messages: [first.messages[0]!], outcome: { type: 'failed' } },
        {
          ...first,
          turnSequence: 5,
          messages: [first.messages[0]!, { ...first.messages[1]!, sequence: 6 }],
          outcome: { type: 'active' },
        },
      ],
    });
    expect(state.messages.map(({ id, runId }) => ({ id, runId }))).toEqual([
      { id: 'event:2', runId: '5' },
      { id: 'event:6', runId: '5' },
    ]);
    expect(state.activeRunId).toBe('5');
  });

  it('preserves a fractional live position across steering and tool rows when the reply commits', () => {
    const initial = conversationFromTranscript({
      ...TRANSCRIPT,
      turns: [{ ...TRANSCRIPT.turns[0]!, outcome: { type: 'active' } }],
    });
    const streamed = {
      ...initial,
      messages: [
        ...initial.messages,
        {
          id: 'live-1',
          runId: '1',
          role: 'assistant' as const,
          content: 'Working',
          status: 'streaming' as const,
          liveOnly: true,
          order: 4.5,
        },
      ],
    };
    const steered = foldAgentEvent(
      streamed,
      durable(5, 'owner_message', { content: 'Focus on tests.' }),
    );
    const withTool = foldAgentEvent(
      steered,
      durable(6, 'tool_call_started', {
        call_id: 'tool',
        title: 'Run tests',
      }),
    );
    const committed = foldAgentEvent(
      withTool,
      durable(7, 'agent_message', { content: 'Tests pass.' }),
    );
    expect(
      committed.messages
        .filter((message) => message.role === 'assistant')
        .map((message) => message.id),
    ).toEqual(['event:3', 'event:7']);
    expect(committed.messages.find((message) => message.id === 'event:7')?.order).toBe(4.5);
    const rows = [...committed.messages, ...committed.toolCalls].sort(
      (left, right) => left.order! - right.order!,
    );
    expect(rows.map((row) => row.id)).toEqual(['event:2', 'event:3', 'event:7', 'event:5', 'tool']);
    expect(new Set(rows.map((row) => row.order)).size).toBe(rows.length);
  });

  it('inserts missing snapshot messages around durable anchors after several live rows', () => {
    let current = conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] });
    current = foldAgentEvent(current, durable(1, 'turn_started', { turn_sequence: 1 }));
    for (const event of [
      { type: 'REASONING_MESSAGE_CONTENT', messageId: 'reason-1', delta: 'Inspecting' },
      { type: 'TOOL_CALL_START', toolCallId: 'live-tool', toolCallName: 'Read' },
      { type: 'REASONING_MESSAGE_CONTENT', messageId: 'reason-2', delta: 'Comparing' },
    ])
      current = foldLiveEvent(current, { ...event, runId: '1', threadId: 'agt_1' }, 'agt_1');
    current = foldAgentEvent(current, durable(3, 'owner_message', { content: 'Check tests.' }));
    current = foldAgentEvent(
      current,
      durable(5, 'tool_call_started', { call_id: 'tool', title: 'Run tests' }),
    );
    expect(current.messages[0]!.order).toBeGreaterThan(current.messages[0]!.sequence!);
    const turn = TRANSCRIPT.turns[0]!;
    const snapshot = conversationFromTranscript({
      ...TRANSCRIPT,
      throughSeq: 5,
      turns: [
        {
          ...turn,
          outcome: { type: 'active' },
          messages: [
            turn.messages[0]!,
            { sequence: 3, role: 'user', content: 'Check tests.', appendedAt: turn.startedAt },
            { ...turn.messages[1]!, sequence: 4 },
          ],
        },
      ],
    });
    const merged = mergeTranscriptSnapshot(current, snapshot);
    const rows = [...merged.messages, ...merged.toolCalls, ...merged.reasoning].sort(
      (left, right) => left.order! - right.order!,
    );
    expect(rows.filter((row) => row.sequence !== undefined).map((row) => row.id)).toEqual([
      'event:2',
      'event:3',
      'event:4',
      'tool',
    ]);
    expect(rows.filter((row) => row.sequence === undefined).map((row) => row.id)).toEqual([
      'reason-1',
      'live-tool',
      'reason-2',
    ]);
    expect(rows.map((row) => row.order)).toEqual([1, 2, 3, 4, 5, 6, 7]);
    const updated = foldAgentEvent(
      merged,
      durable(6, 'tool_call_updated', { call_id: 'tool', status: 'completed' }),
    );
    expect(updated.toolCalls.find((tool) => tool.id === 'tool')).toMatchObject({
      sequence: 6,
      order: 8,
    });
  });

  it('repairs same-watermark omissions without resurrecting a newer failed live run', () => {
    const turn = TRANSCRIPT.turns[0]!;
    let current = conversationFromTranscript({
      ...TRANSCRIPT,
      turns: [{ ...turn, messages: [turn.messages[1]!], outcome: { type: 'failed' } }],
    });
    current = foldLiveEvent(
      current,
      {
        type: 'RUN_ERROR',
        threadId: 'agt_1',
        runId: '1',
        message: 'Provider disconnected.',
      },
      'agt_1',
    );
    const merged = mergeTranscriptSnapshot(
      current,
      conversationFromTranscript({
        ...TRANSCRIPT,
        turns: [{ ...turn, outcome: { type: 'active' } }],
      }),
    );
    expect(merged.messages.map((message) => message.id)).toEqual(['event:2', 'event:3']);
    expect(merged.messages[1]?.status).toBe('error');
    expect(merged.activeRunId).toBeUndefined();
    expect(merged.recovery.map(({ id, detail }) => ({ id, detail }))).toEqual(
      current.recovery.map(({ id, detail }) => ({ id, detail })),
    );
  });

  it('retains a failed transcript outcome as a visible recovery fact', () => {
    const state = conversationFromTranscript({
      ...TRANSCRIPT,
      turns: [
        {
          ...TRANSCRIPT.turns[0]!,
          outcome: {
            type: 'failed',
            failure: { class: 'provider_lost', detail: 'Provider disconnected.' },
          },
        },
      ],
    });
    expect(state.recovery).toEqual([
      expect.objectContaining({ detail: 'Provider disconnected.', code: 'provider_lost' }),
    ]);
  });
});

describe('live AG-UI folding', () => {
  it('streams messages, reasoning, tools, usage, and approvals into one state', () => {
    const events = [
      live('RUN_STARTED'),
      live('TEXT_MESSAGE_START', { messageId: 'msg_2', role: 'assistant' }),
      live('TEXT_MESSAGE_CONTENT', { messageId: 'msg_2', delta: 'Working' }),
      live('TEXT_MESSAGE_END', { messageId: 'msg_2' }),
      live('REASONING_MESSAGE_CONTENT', { messageId: 'why_2', delta: 'Check types' }),
      live('TOOL_CALL_START', {
        toolCallId: 'call_2',
        toolCallName: 'Typecheck',
        'cma:status': 'running',
      }),
      live('TOOL_CALL_ARGS', { toolCallId: 'call_2', delta: '{"path":' }),
      live('TOOL_CALL_ARGS', { toolCallId: 'call_2', delta: '"app.ts"}' }),
      live('TOOL_CALL_END', { toolCallId: 'call_2' }),
      live('STATE_DELTA', {
        delta: [
          { op: 'replace', path: '/usage/inputTokens', value: 10 },
          {
            op: 'add',
            path: '/pendingApprovals/approval~11',
            value: { id: 'approval/1', reason: 'cma:write', message: 'Apply patch' },
          },
        ],
      }),
      live('RUN_FINISHED'),
    ];
    const state = events.reduce(
      (current, event) => foldLiveEvent(current, event, 'agt_1'),
      conversationFromTranscript(TRANSCRIPT),
    );
    expect(state.messages.at(-1)).toEqual(
      expect.objectContaining({ content: 'Working', status: 'complete' }),
    );
    expect(state.reasoning).toEqual([]);
    expect(state.toolCalls[0]).toEqual(
      expect.objectContaining({ title: 'Typecheck', detail: '{"path":"app.ts"}' }),
    );
    expect(state.usage?.inputTokens).toBe(10);
    expect(state.approvals[0]?.id).toBe('approval/1');
  });

  it('rejects live frames for any other thread', () => {
    const state = conversationFromTranscript(TRANSCRIPT);
    const foreign = { ...live('RUN_STARTED'), threadId: 'agt_other' };
    expect(foldLiveEvent(state, foreign, 'agt_1')).toBe(state);
  });

  it('surfaces run failures as recovery state', () => {
    const state = foldLiveEvent(
      conversationFromTranscript(TRANSCRIPT),
      live('RUN_ERROR', { message: 'Provider disconnected', code: 'provider_lost' }),
      'agt_1',
    );
    expect(state.recovery[0]).toEqual(
      expect.objectContaining({ detail: 'Provider disconnected', code: 'provider_lost' }),
    );
  });
});

it('keeps lifetime consumption through compaction and correlates current-turn usage with durable replay', () => {
  const started = foldAgentEvent(
    conversationFromTranscript(TRANSCRIPT),
    durable(5, 'turn_started', { turn_sequence: 5 }),
  );
  const observed = foldAgentEvent(
    started,
    durable(6, 'usage_updated', {
      input_tokens: 1000,
      output_tokens: 200,
      context_used: 400,
      context_window: 10000,
      turn_sequence: 5,
      turn_input_tokens: 100,
      turn_output_tokens: 20,
    }),
  );
  const compacted = foldAgentEvent(
    observed,
    durable(7, 'usage_updated', {
      input_tokens: 1000,
      output_tokens: 200,
      context_used: 80,
      context_window: 10000,
      turn_sequence: 5,
      turn_input_tokens: 100,
      turn_output_tokens: 20,
    }),
  );
  expect(compacted.usage).toMatchObject({
    inputTokens: 1000,
    outputTokens: 200,
    contextUsed: 80,
    currentTurn: { turnSequence: 5, inputTokens: 100, outputTokens: 20 },
  });
  expect(
    foldAgentEvent(
      compacted,
      durable(6, 'usage_updated', { input_tokens: 900, output_tokens: 100 }),
    ),
  ).toBe(compacted);
  const metadata = foldAgentEvent(
    compacted,
    durable(8, 'runtime_metadata_changed', {
      metadata: { working_directory: '/home/cma/research', description: 'Review native workspace' },
    }),
  );
  expect(metadata.runtimeMetadata).toEqual({
    workingDirectory: '/home/cma/research',
    description: 'Review native workspace',
  });
  expect(metadata.messages).toEqual(compacted.messages);
});
