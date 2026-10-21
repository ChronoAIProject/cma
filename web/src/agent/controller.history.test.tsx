import { act, renderHook, waitFor } from '@testing-library/react';
import { ApiError } from '@chronoai/cma-sdk';
import { afterEach, expect, it, vi } from 'vitest';
import type { AgentEvent, ResourcePage } from '../api/types';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, AGENT_USAGE, agentClient } from '../test/agent';
import { loadAgent, useAgentController } from './controller';

afterEach(() => vi.useRealTimers());

type EventPage = ResourcePage<AgentEvent, number>;

function deferredPage() {
  let resolve!: (page: EventPage) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<EventPage>((accept, decline) => {
    resolve = accept;
    reject = decline;
  });
  return { promise, resolve, reject };
}

const events: AgentEvent[] = [
  { type: 'turn_started', turn_sequence: 1 },
  { type: 'owner_message', content: 'Inspect this repository.' },
  { type: 'tool_call_started', call_id: 'read', title: 'Read files' },
  { type: 'approval_requested', approval_id: 'approval', action: 'Write files' },
  { type: 'approval_resolved', approval_id: 'approval', decision: 'allow' },
  { type: 'tool_call_updated', call_id: 'read', status: 'completed' },
  { type: 'agent_message', content: 'The repository is ready.' },
  { type: 'turn_completed', turn_sequence: 1 },
].map((kind, index) => ({ seq: index + 1, appendedAt: AGENT_DETAIL.createdAt, kind }));

const transcript = {
  ...AGENT_TRANSCRIPT,
  throughSeq: 8,
  turns: AGENT_TRANSCRIPT.turns.map((turn) => ({
    ...turn,
    messages: turn.messages.map((message) => ({
      ...message,
      sequence: message.role === 'assistant' ? 7 : 2,
    })),
  })),
};

it('publishes the latest authorized transcript while hydrating rich activity silently', async () => {
  vi.useFakeTimers();
  const remaining = deferredPage();
  const readAgentEvents = vi
    .fn()
    .mockResolvedValueOnce({ items: events.slice(0, 4), next: 4 })
    .mockReturnValueOnce(remaining.promise);
  const client = agentClient({
    readAgent: vi.fn().mockResolvedValue({ ...AGENT_DETAIL, status: 'starting' }),
    readAgentTranscript: vi.fn().mockResolvedValue(transcript),
    readAgentEvents,
    replayAgent: vi.fn(() => new Promise<void>(() => undefined)),
    runAgent: vi.fn(),
    interruptAgent: vi.fn(),
    stopAgent: vi.fn(),
    deleteAgent: vi.fn(),
  });
  const { result, unmount } = renderHook(() => useAgentController(client, 'agt_1'));
  try {
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(result.current.state).toMatchObject({
      status: 'ready',
      historyLoading: true,
      historyUnavailable: false,
      conversation: {
        cursor: 8,
        messages: [
          expect.objectContaining({ content: 'Inspect this repository.' }),
          expect.objectContaining({ content: 'The repository is ready.' }),
        ],
        toolCalls: [],
        approvals: [],
        activeRunId: undefined,
      },
    });
    await act(async () => {
      await result.current.send('Must wait');
      await result.current.approve('approval', 'allow');
      await result.current.interrupt();
      await result.current.stop();
      await result.current.delete();
      await vi.advanceTimersByTimeAsync(15000);
    });
    for (const command of [
      client.runAgent,
      client.interruptAgent,
      client.stopAgent,
      client.deleteAgent,
      client.replayAgent,
    ])
      expect(command).not.toHaveBeenCalled();
    expect(client.readAgent).toHaveBeenCalledTimes(1);
    expect(readAgentEvents.mock.calls.map((call) => call[1].cursor)).toEqual([0, 4]);
    await act(async () => remaining.resolve({ items: events.slice(4) }));
    expect(result.current.state).toMatchObject({
      status: 'ready',
      historyLoading: false,
      historyUnavailable: false,
      conversation: {
        cursor: 8,
        messages: [
          expect.objectContaining({ content: 'Inspect this repository.' }),
          expect.objectContaining({ content: 'The repository is ready.' }),
        ],
        toolCalls: [expect.objectContaining({ id: 'read', status: 'completed' })],
        approvals: [expect.objectContaining({ id: 'approval', decision: 'allow' })],
        activeRunId: undefined,
      },
    });
    expect(client.replayAgent).toHaveBeenCalledWith('agt_1', expect.objectContaining({ after: 8 }));
  } finally {
    unmount();
  }
});

it('never uses the projected transcript cursor to skip remaining event pages', async () => {
  const progress = vi.fn();
  const readAgentEvents = vi.fn(async (_id: string, options?: { cursor?: number }) => {
    const cursor = options?.cursor ?? 0;
    return { items: [events[cursor]!], next: cursor < 7 ? cursor + 1 : undefined };
  });
  const ready = await loadAgent(
    agentClient({ readAgentTranscript: async () => transcript, readAgentEvents }),
    'agt_1',
    new AbortController().signal,
    true,
    progress,
  );
  expect(readAgentEvents.mock.calls.map((call) => call[1]?.cursor)).toEqual([
    0, 1, 2, 3, 4, 5, 6, 7,
  ]);
  expect(progress).toHaveBeenCalledOnce();
  expect(progress.mock.calls[0]?.[0].conversation.cursor).toBe(8);
  expect(ready.durableEvents).toEqual(events);
  expect(ready.conversation.messages).toHaveLength(2);
  expect(ready.conversation.toolCalls[0]?.status).toBe('completed');
  expect(ready.conversation.approvals[0]?.decision).toBe('allow');
  expect(ready.historyLoading).toBe(false);
});

it.each([401, 403])(
  'fails closed on history denial %s after a visible partial snapshot',
  async (status) => {
    const remaining = deferredPage();
    const client = agentClient({
      readAgentEvents: vi
        .fn()
        .mockResolvedValueOnce({ items: events.slice(0, 3), next: 3 })
        .mockReturnValueOnce(remaining.promise),
      replayAgent: vi.fn(),
    });
    const { result } = renderHook(() => useAgentController(client, 'agt_1'));
    await waitFor(() =>
      expect(result.current.state).toMatchObject({ status: 'ready', historyLoading: true }),
    );
    await act(async () => remaining.reject(new ApiError(status, 'Denied')));
    expect(result.current.state).toEqual({ status: 'error', message: 'Denied' });
    expect(client.replayAgent).not.toHaveBeenCalled();
  },
);

it('ignores a late hydration page after navigating to another Agent', async () => {
  const remaining = deferredPage();
  const readAgentEvents = vi
    .fn()
    .mockResolvedValueOnce({ items: events.slice(0, 3), next: 3 })
    .mockReturnValueOnce(remaining.promise)
    .mockResolvedValue({ items: [] });
  const client = agentClient({
    readAgent: async (id) => ({ ...AGENT_DETAIL, id }),
    readAgentTranscript: async (agentId) => ({ ...AGENT_TRANSCRIPT, agentId }),
    readAgentUsage: async (agentId) => ({ ...AGENT_USAGE, agentId }),
    readAgentEvents,
  });
  const { result, rerender } = renderHook(({ id }) => useAgentController(client, id), {
    initialProps: { id: 'agt_1' },
  });
  await waitFor(() =>
    expect(result.current.state).toMatchObject({ status: 'ready', historyLoading: true }),
  );
  rerender({ id: 'agt_2' });
  await waitFor(() =>
    expect(result.current.state).toMatchObject({ status: 'ready', detail: { id: 'agt_2' } }),
  );
  await act(async () => remaining.resolve({ items: events.slice(3), next: 8 }));
  expect(readAgentEvents.mock.calls[1]?.[1].signal.aborted).toBe(true);
  expect(readAgentEvents).toHaveBeenCalledTimes(3);
  expect(result.current.state).toMatchObject({
    status: 'ready',
    detail: { id: 'agt_2' },
    historyLoading: false,
  });
});

it('changes a transient later-page failure to the existing incomplete-history retry state', async () => {
  const remaining = deferredPage();
  const client = agentClient({
    readAgentEvents: vi
      .fn()
      .mockResolvedValueOnce({ items: events.slice(0, 3), next: 3 })
      .mockReturnValueOnce(remaining.promise),
    replayAgent: vi.fn(),
  });
  const { result } = renderHook(() => useAgentController(client, 'agt_1'));
  await waitFor(() =>
    expect(result.current.state).toMatchObject({ status: 'ready', historyLoading: true }),
  );
  await act(async () => remaining.reject(new ApiError(503, 'Unavailable')));
  expect(result.current.state).toMatchObject({
    status: 'ready',
    historyLoading: false,
    historyUnavailable: true,
    conversation: { toolCalls: [], activeRunId: undefined },
  });
  expect(client.replayAgent).not.toHaveBeenCalled();
});

it('does not publish progress for a malformed first page', async () => {
  const progress = vi.fn();
  await expect(
    loadAgent(
      agentClient({ readAgentEvents: async () => ({ items: events.slice(0, 3), next: 0 }) }),
      'agt_1',
      new AbortController().signal,
      true,
      progress,
    ),
  ).rejects.toThrow('Agent event pagination did not advance.');
  expect(progress).not.toHaveBeenCalled();
});

it('holds interruption of a known active turn while its event history is still loading', async () => {
  const remaining = deferredPage();
  const interruptAgent = vi.fn();
  const client = agentClient({
    readAgentTranscript: async () => ({
      ...transcript,
      turns: transcript.turns.map((turn) => ({ ...turn, outcome: { type: 'active' } })),
    }),
    readAgentEvents: vi
      .fn()
      .mockResolvedValueOnce({ items: events.slice(0, 3), next: 3 })
      .mockReturnValueOnce(remaining.promise),
    interruptAgent,
  });
  const { result } = renderHook(() => useAgentController(client, 'agt_1'));
  await waitFor(() =>
    expect(result.current.state).toMatchObject({
      status: 'ready',
      historyLoading: true,
      conversation: { activeRunId: '1' },
    }),
  );
  await act(() => result.current.interrupt());
  expect(interruptAgent).not.toHaveBeenCalled();
});
