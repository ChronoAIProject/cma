import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AgentLiveEvent, AgentTranscript } from '../api/types';
import { readableClient } from '../test/agentController';
import { AGENT_DETAIL, AGENT_TRANSCRIPT } from '../test/agent';
import { loadAgent, useAgentController } from './controller';

afterEach(() => vi.useRealTimers());

describe('Agent controller operations', () => {
  it.each(['live-fragment', 'committed-stream'] as const)(
    'reconciles a %s on snapshot-only progress without duplicating messages or regressing live state',
    async (messageMode) => {
      vi.useFakeTimers();
      const turn = AGENT_TRANSCRIPT.turns[0]!;
      let transcript: AgentTranscript = {
        ...AGENT_TRANSCRIPT,
        throughSeq: 2,
        turns: [{ ...turn, messages: [turn.messages[0]!], outcome: { type: 'active' } }],
      };
      let onEvent: ((event: AgentLiveEvent) => void) | undefined;
      const client = readableClient({
        readAgent: vi.fn().mockResolvedValue({
          ...AGENT_DETAIL,
          firstMessage: { state: 'execution_observed', queuedAt: AGENT_DETAIL.createdAt },
        }),
        readAgentTranscript: async () => transcript,
        readSandboxEvents: async () => [],
        replayAgent: async (_id: string, options: { onEvent: (event: AgentLiveEvent) => void }) => {
          onEvent = options.onEvent;
        },
      });
      const { result, unmount } = renderHook(() => useAgentController(client, 'agt_1'));
      await act(() => vi.advanceTimersByTimeAsync(0));
      act(() => {
        if (messageMode === 'live-fragment') {
          onEvent?.({
            type: 'TEXT_MESSAGE_CONTENT',
            threadId: 'agt_1',
            runId: '1',
            messageId: 'live-1',
            delta: 'Inspecting',
            'cma:live': true,
          });
        } else {
          onEvent?.({
            type: 'TEXT_MESSAGE_START',
            threadId: 'agt_1',
            runId: '1',
            messageId: 'msg_3',
            role: 'assistant',
          });
          onEvent?.({
            type: 'TEXT_MESSAGE_CONTENT',
            threadId: 'agt_1',
            runId: '1',
            messageId: 'msg_3',
            delta: 'The repository is ready.',
          });
          onEvent?.({
            type: 'TEXT_MESSAGE_END',
            threadId: 'agt_1',
            runId: '1',
            messageId: 'msg_3',
            cursor: 3,
          });
        }
        onEvent?.({
          type: 'REASONING_MESSAGE_CONTENT',
          threadId: 'agt_1',
          runId: '1',
          messageId: 'reason-1',
          delta: 'Checking tests.',
        });
      });
      transcript = {
        ...transcript,
        throughSeq: 4,
        turns: [
          {
            ...turn,
            messages: [
              ...turn.messages,
              {
                sequence: 4,
                role: 'user',
                content: 'Check the tests.',
                appendedAt: AGENT_DETAIL.updatedAt,
              },
            ],
            outcome: { type: 'active' },
          },
        ],
      };
      await act(() => vi.advanceTimersByTimeAsync(1500));
      if (result.current.state.status !== 'ready') throw new Error('Agent did not load');
      expect(result.current.state.conversation.messages.map((message) => message.id)).toEqual([
        'event:2',
        'event:3',
        'event:4',
      ]);
      expect(result.current.state.conversation.messages[1]?.content).toBe(
        'The repository is ready.',
      );
      expect(result.current.state.conversation.reasoning[0]?.content).toBe('Checking tests.');
      expect(result.current.state.conversation.activeRunId).toBe('1');
      act(() => onEvent?.({ type: 'RUN_STARTED', threadId: 'agt_1', runId: '6', cursor: 6 }));
      await act(() => vi.advanceTimersByTimeAsync(1500));
      if (result.current.state.status !== 'ready') throw new Error('Agent did not load');
      expect(result.current.state.conversation.activeRunId).toBe('6');
      expect(result.current.state.conversation.cursor).toBe(6);
      unmount();
    },
  );
  it('reports replay transport loss without inventing a failed Agent run', async () => {
    const replayAgent = vi.fn().mockRejectedValue(new Error('The stream disconnected.'));
    const client = readableClient({ replayAgent });
    const expected = await loadAgent(client, 'agt_1', new AbortController().signal);
    const { result } = renderHook(() => useAgentController(client, 'agt_1'));
    await waitFor(() =>
      expect(result.current.actionError).toBe(
        'Live Agent updates are unavailable. The stream disconnected.',
      ),
    );
    expect(result.current.state.status).toBe('ready');
    if (result.current.state.status !== 'ready') throw new Error('Agent was not loaded');
    expect(result.current.state.conversation).toEqual(expected.conversation);
    act(() => result.current.clearActionError());
    expect(result.current.actionError).toBeUndefined();
  });

  it('ignores replay failures after navigation cancels the stream', async () => {
    let rejectReplay: ((error: Error) => void) | undefined;
    const client = readableClient({
      replayAgent: vi.fn(
        () =>
          new Promise<void>((_resolve, reject) => {
            rejectReplay = reject;
          }),
      ),
    });
    const { result, rerender } = renderHook(
      ({ agentId }: { agentId: string | undefined }) => useAgentController(client, agentId),
      { initialProps: { agentId: 'agt_1' as string | undefined } },
    );
    await waitFor(() => expect(rejectReplay).toBeDefined());
    rerender({ agentId: undefined });
    await act(async () => rejectReplay?.(new Error('Cancelled connection')));
    await waitFor(() => expect(result.current.state.status).toBe('idle'));
    expect(result.current.actionError).toBeUndefined();
  });

  it('does not accept a draft when admission fails before a run starts', async () => {
    const runAgent = vi.fn().mockRejectedValue(new Error('Agent is unavailable'));
    const client = readableClient({ runAgent });
    const ready = renderHook(() => useAgentController(client, 'agt_1'));
    await waitFor(() => expect(ready.result.current.state.status).toBe('ready'));
    const onAccepted = vi.fn();
    await act(() => ready.result.current.send('Retain this draft', onAccepted));
    expect(onAccepted).not.toHaveBeenCalled();
    expect(ready.result.current.actionError).toBe('Agent is unavailable');
  });
  it('uses the Agent id as AG-UI threadId for replay and normal message runs', async () => {
    const replaySignals: AbortSignal[] = [];
    const replayAgent = vi.fn((_agentId, options) => {
      if (options.signal !== undefined) replaySignals.push(options.signal);
      return new Promise<void>(() => undefined);
    });
    const runAgent = vi.fn(async (_request, options) => {
      expect(replaySignals[0]?.aborted).toBe(true);
      options.onEvent({
        type: 'TEXT_MESSAGE_START',
        runId: '2',
        threadId: 'agt_1',
        messageId: 'msg_5',
        role: 'assistant',
      });
      options.onEvent({
        type: 'TEXT_MESSAGE_CONTENT',
        runId: '2',
        threadId: 'agt_1',
        messageId: 'msg_5',
        delta: 'Live answer',
      });
      options.onEvent({
        type: 'TEXT_MESSAGE_END',
        runId: '2',
        threadId: 'agt_1',
        messageId: 'msg_5',
      });
    });
    const client = readableClient({ replayAgent, runAgent });
    const { result } = renderHook(() => useAgentController(client, 'agt_1'));
    await waitFor(() => expect(result.current.state.status).toBe('ready'));
    await waitFor(() =>
      expect(replayAgent).toHaveBeenCalledWith(
        'agt_1',
        expect.objectContaining({ after: 4, onEvent: expect.any(Function) }),
      ),
    );

    await act(() => result.current.send('Continue.'));
    expect(runAgent).toHaveBeenCalledWith(
      expect.objectContaining({
        threadId: 'agt_1',
        messages: [{ role: 'user', content: 'Continue.' }],
      }),
      expect.objectContaining({ onEvent: expect.any(Function) }),
    );
    expect(
      result.current.state.status === 'ready'
        ? result.current.state.conversation.messages.at(-1)?.content
        : undefined,
    ).toBe('Live answer');
    await waitFor(() => expect(replayAgent).toHaveBeenCalledTimes(2));
  });

  it('submits approval resumes through the same public AG-UI run operation', async () => {
    const runAgent = vi.fn().mockResolvedValue(undefined);
    const client = readableClient({ runAgent });
    const { result } = renderHook(() => useAgentController(client, 'agt_1'));
    await waitFor(() => expect(result.current.state.status).toBe('ready'));
    await act(() => result.current.approve('approval_1', 'allow'));
    expect(runAgent).toHaveBeenCalledWith(
      expect.objectContaining({
        threadId: 'agt_1',
        resume: [
          {
            interruptId: 'approval_1',
            status: 'resolved',
            payload: { decision: 'allow' },
          },
        ],
      }),
      expect.any(Object),
    );
  });

  it('cancels an active run and resets the resource when the route changes', async () => {
    let runSignal: AbortSignal | undefined;
    const runAgent = vi.fn(
      (_request, options) =>
        new Promise<void>((_resolve, reject) => {
          runSignal = options.signal;
          options.signal?.addEventListener('abort', () => reject(new Error('aborted')), {
            once: true,
          });
        }),
    );
    const readAgent = vi.fn((id: string) =>
      id === 'agt_1' ? Promise.resolve(AGENT_DETAIL) : new Promise(() => undefined),
    );
    const client = readableClient({ readAgent, runAgent });
    const { result, rerender } = renderHook(
      ({ agentId }: { agentId: string | undefined }) => useAgentController(client, agentId),
      { initialProps: { agentId: 'agt_1' } },
    );
    await waitFor(() => expect(result.current.state.status).toBe('ready'));
    act(() => {
      void result.current.send('Keep working.');
    });
    await waitFor(() => expect(result.current.pending).toBe('send'));

    rerender({ agentId: 'agt_2' });
    await waitFor(() => expect(runSignal?.aborted).toBe(true));
    await waitFor(() => {
      expect(result.current.pending).toBeUndefined();
      expect(result.current.state.status).toBe('loading');
    });
  });
});

it('propagates a failed public interrupt to desktop takeover without discarding retained history', async () => {
  const interruptAgent = vi.fn().mockRejectedValue(new Error('Stop request failed'));
  const client = readableClient({
    interruptAgent,
    readAgent: async () => ({ ...AGENT_DETAIL, status: 'working' }),
    readAgentTranscript: async () => ({
      ...AGENT_TRANSCRIPT,
      turns: AGENT_TRANSCRIPT.turns.map((turn) => ({ ...turn, outcome: { type: 'active' } })),
    }),
  });
  const { result } = renderHook(() => useAgentController(client, 'agt_1'));
  await waitFor(() => expect(result.current.state.status).toBe('ready'));
  await act(async () => {
    await expect(result.current.interruptForControl!()).rejects.toThrow('Stop request failed');
  });
  expect(interruptAgent).toHaveBeenCalledWith(
    'agt_1',
    expect.objectContaining({
      idempotencyKey: expect.any(String),
      signal: expect.any(AbortSignal),
    }),
  );
  expect(result.current.state.status).toBe('ready');
  expect(result.current.actionError).toContain('Stop request failed');
});
