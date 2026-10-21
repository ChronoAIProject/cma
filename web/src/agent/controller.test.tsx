import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AgentEvent, AgentLiveEvent, AgentTranscript } from '../api/types';
import { testClient } from '../test/client';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, AGENT_USAGE } from '../test/agent';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { loadAgent, useAgentController } from './controller';
import { ApiError } from '@chronoai/cma-sdk';

afterEach(() => vi.useRealTimers());

function readableClient(overrides = {}) {
  return testClient({
    readAgent: vi.fn().mockResolvedValue(AGENT_DETAIL),
    readSandbox: vi.fn().mockResolvedValue(sandboxFixture()),
    readWorkspace: vi.fn().mockResolvedValue(workspaceFixture()),
    readAgentTranscript: vi.fn().mockResolvedValue(AGENT_TRANSCRIPT),
    readAgentUsage: vi.fn().mockResolvedValue(AGENT_USAGE),
    readAgentEvents: vi.fn().mockResolvedValue({ items: [] }),
    replayAgent: vi.fn(() => new Promise<void>(() => undefined)),
    ...overrides,
  });
}

describe('Agent controller loading', () => {
  it('keeps the current transcript without replaying incomplete historical tools, then recovers tools on retry', async () => {
    vi.useFakeTimers();
    let failSecondPage = true;
    const events: AgentEvent[] = [
      {
        seq: 1,
        appendedAt: AGENT_DETAIL.createdAt,
        kind: { type: 'turn_started', turn_sequence: 1 },
      },
      {
        seq: 2,
        appendedAt: AGENT_DETAIL.createdAt,
        kind: { type: 'tool_call_started', call_id: 'read', title: 'Read files' },
      },
      {
        seq: 3,
        appendedAt: AGENT_DETAIL.createdAt,
        kind: { type: 'tool_call_updated', call_id: 'read', status: 'completed' },
      },
      {
        seq: 4,
        appendedAt: AGENT_DETAIL.createdAt,
        kind: { type: 'turn_completed', turn_sequence: 1 },
      },
    ];
    const readAgentEvents = vi.fn(async (_id: string, options: { cursor: number }) => {
      if (options.cursor === 0) return { items: events.slice(0, 2), next: 2 };
      if (failSecondPage)
        throw new ApiError(
          503,
          'Authority temporarily unavailable',
          'auth_temporarily_unavailable',
        );
      return { items: events.slice(2) };
    });
    const client = readableClient({ readAgentEvents, runAgent: vi.fn() });
    const { result, unmount } = renderHook(() => useAgentController(client, 'agt_1'));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(result.current.state).toMatchObject({
      status: 'ready',
      historyUnavailable: true,
      conversation: {
        messages: expect.arrayContaining([
          expect.objectContaining({ content: 'The repository is ready.' }),
        ]),
        toolCalls: [],
        activeRunId: undefined,
      },
    });
    expect(client.replayAgent).not.toHaveBeenCalled();
    await act(() => result.current.send('Must wait'));
    expect(client.runAgent).not.toHaveBeenCalled();
    failSecondPage = false;
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(readAgentEvents.mock.calls.map((call) => call[1].cursor)).toEqual([0, 2, 0, 2]);
    expect(result.current.state).toMatchObject({
      status: 'ready',
      historyUnavailable: false,
      conversation: {
        cursor: 4,
        toolCalls: [expect.objectContaining({ id: 'read', status: 'completed' })],
        activeRunId: undefined,
      },
    });
    expect(client.replayAgent).toHaveBeenCalledWith('agt_1', expect.objectContaining({ after: 4 }));
    unmount();
  });

  it.each([401, 403])(
    'does not mask event-history denial %s behind the transcript',
    async (status) => {
      const client = readableClient({
        readAgentEvents: vi.fn().mockRejectedValue(new ApiError(status, 'Denied')),
      });
      const { result } = renderHook(() => useAgentController(client, 'agt_1'));
      await waitFor(() =>
        expect(result.current.state).toEqual({ status: 'error', message: 'Denied' }),
      );
    },
  );

  it.each([401, 403])('fails closed when a recovery read changes to denial %s', async (status) => {
    vi.useFakeTimers();
    const client = readableClient({
      readAgentEvents: vi
        .fn()
        .mockRejectedValueOnce(new ApiError(503, 'Unavailable'))
        .mockRejectedValue(new ApiError(status, 'Denied')),
    });
    const { result } = renderHook(() => useAgentController(client, 'agt_1'));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(result.current.state).toMatchObject({ status: 'ready', historyUnavailable: true });
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(result.current.state).toEqual({ status: 'error', message: 'Denied' });
    expect(client.replayAgent).not.toHaveBeenCalled();
  });

  it('backs off repeated transient recovery failures and cancels retries on unmount', async () => {
    vi.useFakeTimers();
    const readAgentEvents = vi.fn().mockRejectedValue(new ApiError(503, 'Unavailable'));
    const client = readableClient({ readAgentEvents });
    const { result, unmount } = renderHook(() => useAgentController(client, 'agt_1'));
    await act(() => vi.advanceTimersByTimeAsync(0));
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(readAgentEvents).toHaveBeenCalledTimes(2);
    await act(() => vi.advanceTimersByTimeAsync(9999));
    expect(readAgentEvents).toHaveBeenCalledTimes(2);
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(readAgentEvents).toHaveBeenCalledTimes(3);
    expect(result.current.state).toMatchObject({ status: 'ready', historyUnavailable: true });
    unmount();
    await act(() => vi.advanceTimersByTimeAsync(60000));
    expect(readAgentEvents).toHaveBeenCalledTimes(3);
  });

  it('ignores a late recovery result after switching Agents', async () => {
    vi.useFakeTimers();
    let resolvePage: ((page: { items: readonly AgentEvent[] }) => void) | undefined;
    const readAgentEvents = vi
      .fn()
      .mockRejectedValueOnce(new ApiError(503, 'Unavailable'))
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolvePage = resolve;
          }),
      )
      .mockResolvedValue({ items: [] });
    const client = readableClient({
      readAgent: async (id: string) => ({ ...AGENT_DETAIL, id }),
      readAgentTranscript: async (agentId: string) => ({ ...AGENT_TRANSCRIPT, agentId }),
      readAgentUsage: async (agentId: string) => ({ ...AGENT_USAGE, agentId }),
      readAgentEvents,
    });
    const { result, rerender } = renderHook(({ id }) => useAgentController(client, id), {
      initialProps: { id: 'agt_1' },
    });
    await act(() => vi.advanceTimersByTimeAsync(0));
    await act(() => vi.advanceTimersByTimeAsync(5000));
    rerender({ id: 'agt_2' });
    await act(() => vi.advanceTimersByTimeAsync(0));
    await act(async () => {
      resolvePage?.({ items: [] });
    });
    expect(result.current.state).toMatchObject({
      status: 'ready',
      detail: { id: 'agt_2' },
      historyUnavailable: false,
    });
    expect(readAgentEvents.mock.calls[1]?.[1].signal.aborted).toBe(true);
  });

  it.each(['idempotency_in_flight', 'idempotency_recovery_required'])(
    'retains an unsettled %s lifecycle command key',
    async (code) => {
      const stopAgent = vi
        .fn()
        .mockRejectedValueOnce(new ApiError(409, 'The command has not settled.', code))
        .mockResolvedValue(undefined);
      const client = readableClient({ stopAgent });
      const { result } = renderHook(() => useAgentController(client, 'agt_1'));
      await waitFor(() => expect(result.current.state.status).toBe('ready'));
      await act(() => result.current.stop());
      expect(result.current.actionError).toBe('The command has not settled.');
      await act(() => result.current.stop());
      expect(stopAgent).toHaveBeenCalledTimes(2);
      expect(stopAgent.mock.calls[1]?.[1].idempotencyKey).toBe(
        stopAgent.mock.calls[0]?.[1].idempotencyKey,
      );
    },
  );
  it('restores historical tool calls and approvals without duplicating transcript messages', async () => {
    const events = [
      { type: 'turn_started', turn_sequence: 1 },
      { type: 'owner_message', content: 'Inspect this repository.' },
      { type: 'tool_call_started', call_id: 'tool', title: 'Read files', status: 'completed' },
      { type: 'approval_requested', approval_id: 'approval', action: 'Write files' },
      { type: 'approval_resolved', approval_id: 'approval', decision: 'allow' },
      { type: 'agent_message', content: 'The repository is ready.' },
      { type: 'turn_completed', turn_sequence: 1 },
    ].map((kind, index) => ({ seq: index + 1, appendedAt: '2026-09-06T00:00:00Z', kind }));
    const ready = await loadAgent(
      readableClient({
        readAgentEvents: vi.fn().mockResolvedValue({ items: events }),
        readAgentTranscript: vi.fn().mockResolvedValue({
          ...AGENT_TRANSCRIPT,
          throughSeq: 7,
          turns: [
            {
              ...AGENT_TRANSCRIPT.turns[0]!,
              messages: AGENT_TRANSCRIPT.turns[0]!.messages.map((message) => ({
                ...message,
                sequence: message.role === 'assistant' ? 6 : 2,
              })),
            },
          ],
        }),
      }),
      'agt_1',
      new AbortController().signal,
    );
    expect(ready.conversation.messages).toHaveLength(2);
    expect(ready.conversation.toolCalls[0]?.title).toBe('Read files');
    expect(ready.conversation.approvals[0]?.decision).toBe('allow');
    expect(ready.conversation.activeRunId).toBeUndefined();
  });

  it.each(['empty', 'partial', 'full'] as const)(
    'restores interleaved steering and active turn from %s event replay',
    async (replay) => {
      const turn = AGENT_TRANSCRIPT.turns[0]!;
      const messages = [
        { ...turn.messages[0]!, sequence: 2 },
        { ...turn.messages[1]!, sequence: 4 },
        {
          sequence: 6,
          role: 'user' as const,
          content: 'Check the tests.',
          appendedAt: '2026-09-06T00:02:00Z',
        },
      ];
      const events: AgentEvent[] = [
        { seq: 1, appendedAt: turn.startedAt, kind: { type: 'turn_started', turn_sequence: 1 } },
        ...messages.map((message) => ({
          seq: message.sequence,
          appendedAt: message.appendedAt,
          kind: {
            type: message.role === 'user' ? 'owner_message' : 'agent_message',
            content: message.content,
          },
        })),
        {
          seq: 5,
          appendedAt: turn.startedAt,
          kind: { type: 'tool_call_started', call_id: 'tool', title: 'Inspect tests' },
        },
      ].sort((left, right) => left.seq - right.seq);
      const selected =
        replay === 'empty'
          ? []
          : replay === 'partial'
            ? events.filter((event) => [1, 2, 5].includes(event.seq))
            : events;
      const ready = await loadAgent(
        readableClient({
          readAgentTranscript: vi.fn().mockResolvedValue({
            ...AGENT_TRANSCRIPT,
            throughSeq: 6,
            turns: [{ ...turn, messages, outcome: { type: 'active' } }],
          }),
          readAgentEvents: vi.fn().mockResolvedValue({ items: selected }),
        }),
        'agt_1',
        new AbortController().signal,
      );
      expect(
        ready.conversation.messages.map(({ id, runId, role, content, at, sequence }) => ({
          id,
          runId,
          role,
          content,
          at,
          sequence,
        })),
      ).toEqual(
        messages.map((message) => ({
          id: `event:${String(message.sequence)}`,
          runId: '1',
          role: message.role,
          content: message.content,
          at: message.appendedAt,
          sequence: message.sequence,
        })),
      );
      expect(ready.conversation.activeRunId).toBe('1');
      expect(ready.conversation.cursor).toBe(6);
      if (replay !== 'empty') {
        const chronology = [...ready.conversation.messages, ...ready.conversation.toolCalls]
          .sort((left, right) => left.order! - right.order!)
          .map((item) => item.id);
        expect(chronology).toEqual(['event:2', 'event:4', 'tool', 'event:6']);
      }
    },
  );

  it('keeps a newer replayed completion over an active transcript snapshot', async () => {
    const ready = await loadAgent(
      readableClient({
        readAgentTranscript: vi.fn().mockResolvedValue({
          ...AGENT_TRANSCRIPT,
          turns: [{ ...AGENT_TRANSCRIPT.turns[0]!, outcome: { type: 'active' } }],
        }),
        readAgentEvents: vi.fn().mockResolvedValue({
          items: [
            {
              seq: 5,
              appendedAt: AGENT_DETAIL.updatedAt,
              kind: { type: 'turn_completed', turn_sequence: 1 },
            },
          ],
        }),
      }),
      'agt_1',
      new AbortController().signal,
    );
    expect(ready.conversation.messages).toHaveLength(2);
    expect(ready.conversation.activeRunId).toBeUndefined();
    expect(ready.conversation.cursor).toBe(5);
  });

  it.each(['empty', 'partial', 'full'] as const)(
    'retains failed-turn recovery from %s event replay without duplicating the failure',
    async (replay) => {
      const turn = AGENT_TRANSCRIPT.turns[0]!;
      const failure = { class: 'provider_lost', detail: 'Provider disconnected.' };
      const events: AgentEvent[] = [
        { seq: 1, appendedAt: turn.startedAt, kind: { type: 'turn_started', turn_sequence: 1 } },
        ...turn.messages.map((message) => ({
          seq: message.sequence,
          appendedAt: message.appendedAt,
          kind: {
            type: message.role === 'user' ? 'owner_message' : 'agent_message',
            content: message.content,
          },
        })),
        {
          seq: 4,
          appendedAt: AGENT_DETAIL.updatedAt,
          kind: { type: 'turn_failed', turn_sequence: 1, failure },
        },
      ];
      const ready = await loadAgent(
        readableClient({
          readAgentTranscript: vi.fn().mockResolvedValue({
            ...AGENT_TRANSCRIPT,
            turns: [{ ...turn, outcome: { type: 'failed', failure } }],
          }),
          readAgentEvents: vi.fn().mockResolvedValue({
            items: replay === 'empty' ? [] : replay === 'partial' ? events.slice(0, 2) : events,
          }),
        }),
        'agt_1',
        new AbortController().signal,
      );
      expect(ready.conversation.recovery).toHaveLength(1);
      expect(ready.conversation.recovery[0]).toMatchObject({ runId: '1', detail: failure.detail });
      expect(ready.conversation.recovery[0]!.order).toBeGreaterThan(
        ready.conversation.messages.at(-1)!.order!,
      );
      expect(ready.conversation.activeRunId).toBeUndefined();
    },
  );
  it('pages durable events from the start including facts absent from the transcript', async () => {
    const readAgentEvents = vi
      .fn()
      .mockResolvedValueOnce({
        items: [
          {
            seq: 5,
            appendedAt: '2026-09-06T00:06:00Z',
            kind: { type: 'agent_message', content: 'A recovered message.' },
          },
        ],
        next: 5,
      })
      .mockResolvedValueOnce({ items: [] });
    const ready = await loadAgent(
      readableClient({ readAgentEvents }),
      'agt_1',
      new AbortController().signal,
    );
    expect(readAgentEvents).toHaveBeenNthCalledWith(
      1,
      'agt_1',
      expect.objectContaining({ cursor: 0 }),
    );
    expect(readAgentEvents).toHaveBeenNthCalledWith(
      2,
      'agt_1',
      expect.objectContaining({ cursor: 5 }),
    );
    expect(ready.conversation.messages.at(-1)?.content).toBe('A recovered message.');
  });

  it('fails closed when public projections disagree on Agent identity', async () => {
    await expect(
      loadAgent(
        readableClient({
          readAgentUsage: vi.fn().mockResolvedValue({ ...AGENT_USAGE, agentId: 'agt_2' }),
        }),
        'agt_1',
        new AbortController().signal,
      ),
    ).rejects.toThrow(/different resources/i);
  });

  it('rejects the first event page when its cursor does not advance', async () => {
    const readAgentEvents = vi.fn().mockResolvedValue({ items: [], next: 0 });
    await expect(
      loadAgent(readableClient({ readAgentEvents }), 'agt_1', new AbortController().signal),
    ).rejects.toThrow(/pagination did not advance/i);
    expect(readAgentEvents).toHaveBeenCalledOnce();
  });
});

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
        messageId: 'msg_2',
        role: 'assistant',
      });
      options.onEvent({
        type: 'TEXT_MESSAGE_CONTENT',
        runId: '2',
        threadId: 'agt_1',
        messageId: 'msg_2',
        delta: 'Live answer',
      });
      options.onEvent({
        type: 'TEXT_MESSAGE_END',
        runId: '2',
        threadId: 'agt_1',
        messageId: 'msg_2',
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
