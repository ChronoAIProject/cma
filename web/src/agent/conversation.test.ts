import { describe, expect, it } from 'vitest';
import type { AgentEvent, AgentLiveEvent, AgentTranscript } from '../api/types';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';

const TRANSCRIPT: AgentTranscript = {
  agentId: 'agt_1',
  throughSeq: 4,
  turns: [
    {
      turnSequence: 1,
      startedAt: '2026-09-06T00:00:00Z',
      ownerMessage: 'Inspect the repository.',
      agentMessages: ['I found the issue.'],
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
    expect(state.recovery[0]?.detail).toContain('3 live frame');
    expect(state.cursor).toBe(10);
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
    expect(state.reasoning[0]).toEqual(
      expect.objectContaining({ content: 'Check types', status: 'complete' }),
    );
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
