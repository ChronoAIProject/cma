import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useAgentController } from '../controller';
import { at, binding, cursor, harness, item, json, page } from './controller.fixture';
import { agentDisplayTitle } from '../presentation';

beforeEach(() => sessionStorage.clear());
afterEach(() => vi.useRealTimers());

it('preserves setup and native title provenance through decoded v2 replay and reconnect', async () => {
  const h = harness();
  const setup = 'Retained setup prompt';
  h.snapshot.mockResolvedValue(
    json(
      page(
        [
          item(1, {
            type: 'runtime_metadata_changed',
            metadata: {
              description: setup,
              description_source: 'prompt_preview',
              description_is_setup_preview: true,
            },
          }),
        ],
        1,
      ),
    ),
  );
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.streaming).toBe(true));
  if (result.current.state.status !== 'ready') throw new Error('Agent did not load');
  expect(agentDisplayTitle(result.current.state.detail)).toBe('Agent one');
  await act(async () =>
    h.durable(2, {
      type: 'runtime_metadata_changed',
      metadata: { description: setup, description_source: 'provider_title' },
    }),
  );
  await waitFor(() => {
    if (result.current.state.status !== 'ready') throw new Error('Agent did not load');
    expect(agentDisplayTitle(result.current.state.detail)).toBe('Agent one');
  });
  await act(async () => {
    h.gap('reconnected', null);
    h.durable(3, {
      type: 'runtime_metadata_changed',
      metadata: {
        description: 'Older fallback preview',
        description_source: 'prompt_preview',
        description_is_setup_preview: true,
      },
    });
  });
  expect(result.current.state).toMatchObject({
    status: 'ready',
    detail: { runtimeMetadata: { description: setup, descriptionSource: 'provider_title' } },
    conversation: { cursor: 3 },
  });
  expect(h.post).not.toHaveBeenCalled();
  unmount();
});

it('keeps a short reconnect quiet, reports a persistent outage, and clears it on a valid frame', async () => {
  const h = harness();
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.streaming).toBe(true));
  vi.useFakeTimers();
  await act(async () => {
    h.fail();
    await vi.advanceTimersByTimeAsync(2000);
  });
  expect(result.current.actionError).toBeUndefined();
  await act(async () => h.gap('reconnected', null));
  await act(async () => vi.advanceTimersByTimeAsync(1000));
  expect(result.current.actionError).toBeUndefined();
  await act(async () => {
    h.fail();
    await vi.advanceTimersByTimeAsync(2600);
  });
  expect(result.current.actionError).toContain('Live updates are reconnecting');
  await act(async () => h.gap('reconnected', null));
  expect(result.current.actionError).toBeUndefined();
  expect(h.post).not.toHaveBeenCalled();
  unmount();
  await act(async () => vi.advanceTimersByTimeAsync(10000));
});

it('does not invent a lost-update warning when opening an active turn from recorded history', async () => {
  const h = harness();
  h.snapshot.mockResolvedValue(
    json(
      page(
        [
          item(1, { type: 'turn_started', turn_sequence: 1 }, binding),
          item(2, {
            type: 'tool_started',
            call_id: 'read',
            title: 'Read file',
            kind: 'read',
            status: 'running',
          }),
        ],
        2,
      ),
    ),
  );
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.streaming).toBe(true));
  await act(async () => {
    h.gap('reconnected', 1);
    h.gap('reconnected', 1);
  });
  expect(result.current.state).toMatchObject({
    conversation: {
      activeRunId: '1',
      cursor: 2,
      recovery: [],
      toolCalls: [expect.objectContaining({ id: 'read', status: 'running' })],
    },
  });
  unmount();
});

it('keeps an idle conversation clear of loss notices when its stream connects after pause and resume', async () => {
  const h = harness();
  h.snapshot.mockResolvedValue(
    json(
      page(
        [
          item(1, { type: 'turn_started', turn_sequence: 1 }, binding),
          item(2, { type: 'message', role: 'assistant', content: 'Recorded before pausing' }),
          item(3, { type: 'turn_completed', turn_sequence: 1 }),
        ],
        3,
      ),
    ),
  );
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.streaming).toBe(true));
  await act(async () => {
    h.gap('reconnected', null);
    h.durable(4, { type: 'status_changed', from: 'ready', to: 'paused' });
    h.gap('reconnected', null);
    h.durable(5, { type: 'status_changed', from: 'paused', to: 'ready' });
    h.gap('reconnected', null);
  });
  expect(result.current.state).toMatchObject({
    conversation: {
      cursor: 5,
      agentCursor: cursor(5),
      agentStatus: 'ready',
      activeRunId: undefined,
      messages: [
        expect.objectContaining({ content: 'Recorded before pausing', status: 'complete' }),
      ],
      recovery: [],
    },
  });
  expect(result.current.actionError).toBeUndefined();
  expect(h.post).not.toHaveBeenCalled();
  unmount();
});

it('keeps a reasoning-only refresh frozen until durable history confirms the turn', async () => {
  const h = harness();
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.streaming).toBe(true));
  await act(async () => {
    h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding);
    h.live('reasoning_summary_delta', 'An unfinished thought', 1);
    h.gap('reconnected', 1);
  });
  expect(result.current.state).toMatchObject({
    conversation: { recovery: [expect.objectContaining({ clearsOn: 'live-update' })] },
  });
  await act(async () => h.live('reasoning_summary_delta', 'A fresh thought', 1));
  expect(result.current.state).toMatchObject({
    conversation: { recovery: [expect.objectContaining({ clearsOn: 'live-update' })] },
  });
  await act(async () => {
    h.durable(2, { type: 'history_gap', frames: 2 });
    h.gap('lagged', 1);
    h.durable(3, { type: 'turn_completed', turn_sequence: 1 });
    h.durable(4, { type: 'turn_started', turn_sequence: 4 });
  });
  expect(result.current.state).toMatchObject({
    conversation: {
      recovery: [
        expect.objectContaining({ id: 'history-gap:2', title: 'Incomplete conversation history' }),
      ],
    },
  });
  unmount();
});

it.each([1, null])(
  'keeps fragment recovery until the recorded reply restores server turn %s',
  async (turn) => {
    const h = harness();
    const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
    await waitFor(() => expect(h.streaming).toBe(true));
    await act(async () => {
      h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding);
      h.live('text_delta', 'Uncommitted text', 1);
      h.live('reasoning_summary_delta', 'Uncommitted thought', 1);
      h.gap('reconnected', turn);
    });
    expect(result.current.state).toMatchObject({
      conversation: {
        messages: [expect.objectContaining({ content: 'Uncommitted text' })],
        reasoning: [expect.objectContaining({ content: 'Uncommitted thought' })],
        recovery: [
          expect.objectContaining({ id: 'live-gap:1', title: 'Refreshing live updates…' }),
        ],
      },
    });
    await act(async () => {
      h.durable(2, { type: 'message', role: 'assistant', content: 'Durable completion' });
      h.durable(3, { type: 'turn_completed', turn_sequence: 1 });
      h.gap('reconnected', null);
    });
    expect(result.current.state).toMatchObject({
      conversation: {
        messages: [expect.objectContaining({ content: 'Durable completion' })],
        recovery: [],
      },
    });
    unmount();
  },
);

it('still reports server-confirmed lag and durable history gaps while idle', async () => {
  const h = harness();
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.streaming).toBe(true));
  await act(async () => {
    h.gap('lagged', null);
    h.durable(1, { type: 'history_gap', frames: 2 });
    h.gap('reconnected', null);
  });
  expect(result.current.state).toMatchObject({
    conversation: {
      recovery: [
        expect.objectContaining({ id: 'live-gap:0', title: 'Live updates interrupted' }),
        expect.objectContaining({
          id: 'history-gap:1',
          detail: 'Some conversation updates were lost before they could be recorded.',
        }),
      ],
    },
  });
  unmount();
});

it('does not silently discard unfinished text when an idle reconnect follows cancellation', async () => {
  const h = harness();
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.streaming).toBe(true));
  await act(async () => {
    h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding);
    h.live('text_delta', 'Uncommitted cancelled text', 1);
    h.durable(2, { type: 'turn_cancelled', turn_sequence: 1 });
    h.gap('reconnected', null);
  });
  expect(result.current.state).toMatchObject({
    conversation: {
      activeRunId: undefined,
      messages: [expect.objectContaining({ content: 'Uncommitted cancelled text' })],
      recovery: [expect.objectContaining({ title: 'Incomplete live output' })],
    },
  });
  unmount();
});

it('reconnects a typed stream failure from applied durable activity without replaying input', async () => {
  const h = harness();
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.streaming).toBe(true));
  await act(async () => {
    h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding);
    h.durable(2, {
      type: 'tool_started',
      call_id: 'tool',
      title: 'Check files',
      kind: 'read',
      status: 'completed',
    });
    h.live('reasoning_summary_delta', 'Live thought', 2);
    h.fail();
  });
  await waitFor(() =>
    expect(
      h.fetch.mock.calls.filter(([url]) => String(url).startsWith('/api/v2/agents/agt_1/events?')),
    ).toHaveLength(2),
  );
  const requests = h.fetch.mock.calls.filter(([url]) => String(url).includes('/events?'));
  expect(
    new URL(String(requests[1]![0]), 'https://cma.example').searchParams.get('after_cursor'),
  ).toBe(cursor(2));
  expect(result.current.state).toMatchObject({
    conversation: {
      cursor: 2,
      agentCursor: cursor(2),
      activeRunStartedAt: at,
      toolCalls: [expect.objectContaining({ status: 'completed' })],
    },
  });
  await act(async () =>
    h.durable(3, { type: 'tool_updated', call_id: 'tool', title: null, status: 'running' }),
  );
  expect(result.current.state).toMatchObject({
    conversation: {
      activeRunStartedAt: at,
      toolCalls: [expect.objectContaining({ status: 'completed' })],
    },
  });
  expect(result.current.actionError).toBeUndefined();
  expect(h.post).not.toHaveBeenCalled();
  expect(h.client.runAgent).not.toHaveBeenCalled();
  unmount();
});

it('clears uncommitted fragments on a gap and suppresses text already covered by a durable message', async () => {
  const h = harness();
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.streaming).toBe(true));
  await act(async () => {
    h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding);
    h.live('text_delta', 'prefix', 1);
    h.live('reasoning_summary_delta', 'thought', 1);
    h.gap();
    h.live('text_delta', 'suffix', 1);
  });
  expect(result.current.state).toMatchObject({
    conversation: {
      cursor: 1,
      messages: [expect.objectContaining({ content: 'prefix' })],
      reasoning: [expect.objectContaining({ content: 'thought' })],
      recovery: [expect.objectContaining({ title: 'Refreshing live updates…' })],
    },
  });
  await act(async () => {
    h.durable(2, { type: 'message', role: 'assistant', content: 'Complete recorded message' });
    h.live('text_delta', 'old prefix', 1);
  });
  expect(result.current.state).toMatchObject({
    conversation: {
      messages: [
        expect.objectContaining({ content: 'Complete recorded message', status: 'complete' }),
      ],
    },
  });
  unmount();
});

it('keeps the latest transcript and input available while incomplete rich history holds controls and tail', async () => {
  const h = harness();
  vi.mocked(h.client.readAgentTranscript).mockResolvedValue({
    agentId: 'agt_1',
    throughSeq: 3,
    turns: [
      {
        turnSequence: 1,
        startedAt: at,
        ownerMessage: '',
        agentMessages: ['Known recorded work'],
        messages: [
          { sequence: 2, role: 'assistant', content: 'Known recorded work', appendedAt: at },
        ],
        outcome: { type: 'completed' },
      },
    ],
  });
  h.snapshot
    .mockResolvedValueOnce(
      json(
        page(
          [
            item(1, { type: 'turn_started', turn_sequence: 1 }, binding),
            item(2, { type: 'message', role: 'assistant', content: 'Known recorded work' }),
          ],
          3,
          2,
        ),
      ),
    )
    .mockResolvedValueOnce(
      json(
        {
          error: {
            code: 'agent_history_unavailable',
            message: 'Temporarily unavailable.',
            type: 'server_error',
            param: null,
          },
        },
        503,
      ),
    );
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() =>
    expect(result.current.state).toMatchObject({ status: 'ready', historyUnavailable: true }),
  );
  expect(result.current.state).toMatchObject({
    conversation: {
      messages: [expect.objectContaining({ content: 'Known recorded work' })],
      activeRunId: undefined,
      cursor: 3,
    },
  });
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  await act(async () => {
    await result.current.send('Continue while activity recovers');
    await result.current.interrupt();
    await result.current.approve('approval', 'allow');
    await result.current.stop();
  });
  expect(h.streaming).toBe(false);
  expect(h.post).toHaveBeenCalledOnce();
  expect(h.control).not.toHaveBeenCalled();
  unmount();
});

it('drops the prior account conversation when a reconnect detects another signed-in user', async () => {
  const h = harness();
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.streaming).toBe(true));
  await act(async () => {
    h.durable(1, {
      type: 'message',
      role: 'assistant',
      content: 'Private first-account observation',
    });
    h.currentUser.mockResolvedValue('bob');
    h.end();
  });
  await waitFor(() =>
    expect(result.current.state).toMatchObject({
      status: 'error',
      message: expect.stringContaining('sign-in changed'),
    }),
  );
  expect(result.current.state).not.toHaveProperty('conversation');
  expect(h.fetch.mock.calls.filter(([url]) => String(url).includes('/events?'))).toHaveLength(1);
  expect(h.post).not.toHaveBeenCalled();
  unmount();
});
