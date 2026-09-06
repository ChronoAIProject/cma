import { act, renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { testClient } from '../test/client';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, AGENT_USAGE } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { loadAgent, useAgentController } from './controller';

function readableClient(overrides = {}) {
  return testClient({
    readAgent: vi.fn().mockResolvedValue(AGENT_DETAIL),
    readWorkspace: vi.fn().mockResolvedValue(workspaceFixture()),
    readAgentTranscript: vi.fn().mockResolvedValue(AGENT_TRANSCRIPT),
    readAgentUsage: vi.fn().mockResolvedValue(AGENT_USAGE),
    readAgentEvents: vi.fn().mockResolvedValue({ items: [] }),
    replayAgent: vi.fn(() => new Promise<void>(() => undefined)),
    ...overrides,
  });
}

describe('Agent controller loading', () => {
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
      readableClient({ readAgentEvents: vi.fn().mockResolvedValue({ items: events }) }),
      'agt_1',
      new AbortController().signal,
    );
    expect(ready.conversation.messages).toHaveLength(2);
    expect(ready.conversation.toolCalls[0]?.title).toBe('Read files');
    expect(ready.conversation.approvals[0]?.decision).toBe('allow');
    expect(ready.conversation.activeRunId).toBeUndefined();
  });
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
