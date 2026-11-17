import { act, renderHook, waitFor } from '@testing-library/react';
import { ApiError, type V2AgentCursor } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import { foldV2Item } from './transport/projection';
import { useRetainedOutput } from './useRetainedOutput';

const at = '2026-11-17T00:00:00Z';
const retainedFailure = { class: 'agent_log_capacity_exceeded', detail: 'This Agent turn failed.' };
const publicFailure = { class: retainedFailure.class, detail: 'The public history is full.' };
const page = {
  output: {
    id: 'retained_checkpoint',
    agent_id: 'agent',
    turn_sequence: 3,
    checkpoint_sequence: 5,
    failure_sequence: 6,
    failed_at: at,
    byte_length: 12,
    partial: true,
  },
  offset: 0,
  content: 'Saved prefix',
  next: null,
};
function history(type = 'failed', failure = retainedFailure, available = true) {
  return conversationFromTranscript({
    agentId: 'agent',
    throughSeq: 6,
    turns: [
      {
        turnSequence: 3,
        retainedOutput: available ? { available: true } : undefined,
        startedAt: at,
        ownerMessage: 'Original question',
        agentMessages: [],
        messages: [],
        outcome: { type, at, failure },
      },
    ],
  });
}

it.each([
  ['empty', conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] })],
  ['active', history('active')],
  ['completed', history('completed')],
  ['interrupted', history('interrupted')],
  [
    'ordinary failure',
    history('failed', { class: 'runtime_failed', detail: 'Unavailable' }, false),
  ],
  ['public capacity prefix', history('failed', publicFailure, false)],
  [
    'untyped private wording',
    history(
      'failed',
      {
        class: retainedFailure.class,
        detail:
          'This Agent cannot continue because its conversation history is full. Some partial output could not be saved in the conversation history. It is retained privately; contact support to recover it through an operator export.',
      },
      false,
    ),
  ],
] as const)(
  'never probes retained output for %s, including refreshes and retries',
  async (_, initial) => {
    const read = vi.fn().mockResolvedValue(page);
    const access = {
      client: agentClient({ readRetainedOutput: read }),
      agentId: 'agent',
      viewerId: 'owner',
    };
    const view = renderHook(({ conversation }) => useRetainedOutput(access, conversation), {
      initialProps: { conversation: initial },
    });
    view.rerender({
      conversation: {
        ...initial,
        cursor: 20,
        turnOutcomes: { ...initial.turnOutcomes, '8': 'failed' },
      },
    });
    await act(async () => view.result.current.retry());
    expect(read).not.toHaveBeenCalled();
    expect(view.result.current.error).toBeUndefined();
  },
);

it.each(['durable', 'v2', 'live'] as const)(
  'starts retrieval when %s history first identifies private retention, without re-reading on later updates',
  async (source) => {
    const read = vi.fn().mockResolvedValue(page);
    const access = {
      client: agentClient({ readRetainedOutput: read }),
      agentId: 'agent',
      viewerId: 'owner',
    };
    const initial = foldAgentEvent(
      conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] }),
      { seq: 3, appendedAt: at, kind: { type: 'turn_started', turn_sequence: 3 } },
    );
    const view = renderHook(({ conversation }) => useRetainedOutput(access, conversation), {
      initialProps: { conversation: initial },
    });
    await act(async () => {});
    expect(read).not.toHaveBeenCalled();
    const kind = { type: 'turn_failed' as const, turn_sequence: 3, failure: retainedFailure };
    const failed =
      source === 'v2'
        ? foldV2Item(initial, {
            sequence: 6,
            appended_at: at,
            cursor: 'cursor_6' as V2AgentCursor,
            kind,
            response_binding: null,
            retained_output: { available: true },
          })
        : source === 'live'
          ? foldLiveEvent(
              initial,
              {
                type: 'RUN_ERROR',
                'cma:retainedOutput': { available: true },
                threadId: 'agent',
                runId: '3',
                timestamp: at,
                code: retainedFailure.class,
                message: retainedFailure.detail,
              },
              'agent',
            )
          : foldAgentEvent(initial, {
              seq: 6,
              appendedAt: at,
              kind,
              retainedOutput: { available: true },
            });
    view.rerender({ conversation: failed });
    await waitFor(() =>
      expect(view.result.current.conversation.messages[0]?.content).toBe('Saved prefix'),
    );
    view.rerender({
      conversation: {
        ...failed,
        cursor: 30,
        turnOutcomes: { ...failed.turnOutcomes, '8': 'failed' },
      },
    });
    await act(async () => {});
    expect(read).toHaveBeenCalledOnce();
    expect(view.result.current.conversation.messages[0]).toMatchObject({
      runId: '3',
      status: 'error',
      finalAnswer: false,
    });
    expect(view.result.current.conversation.turnOutcomes?.['8']).toBe('failed');
  },
);

it.each(['absent', 'unavailable'] as const)(
  'does not retry an %s retained read on ordinary history or polling updates',
  async (result) => {
    const read = vi.fn();
    if (result === 'absent')
      read.mockResolvedValue({ output: null, offset: 0, content: '', next: null });
    else read.mockRejectedValue(new ApiError(503, 'Temporary read failure'));
    const access = {
      client: agentClient({ readRetainedOutput: read }),
      agentId: 'agent',
      viewerId: 'owner',
    };
    const initial = history();
    const view = renderHook(({ conversation }) => useRetainedOutput(access, conversation), {
      initialProps: { conversation: initial },
    });
    await act(async () => {});
    expect(read).toHaveBeenCalledOnce();
    view.rerender({
      conversation: {
        ...initial,
        cursor: 30,
        turnOutcomes: { ...initial.turnOutcomes, '8': 'failed' },
      },
    });
    await act(async () => {});
    expect(read).toHaveBeenCalledOnce();
    if (result === 'unavailable') {
      expect(view.result.current.error).toBe('Retained partial output could not be fully loaded.');
      read.mockResolvedValue(page);
      await act(async () => view.result.current.retry());
      expect(read).toHaveBeenCalledTimes(2);
      expect(view.result.current.conversation.messages[0]?.content).toBe('Saved prefix');
    }
  },
);

it('refuses a retained prefix for a different turn without showing its bytes', async () => {
  const read = vi.fn().mockResolvedValue({ ...page, output: { ...page.output, turn_sequence: 8 } });
  const access = {
    client: agentClient({ readRetainedOutput: read }),
    agentId: 'agent',
    viewerId: 'owner',
  };
  const view = renderHook(() => useRetainedOutput(access, history()));
  await waitFor(() =>
    expect(view.result.current.error).toBe('Retained partial output could not be fully loaded.'),
  );
  expect(read).toHaveBeenCalledOnce();
  expect(view.result.current.conversation.messages).toEqual([]);
});
