import { act, renderHook, waitFor } from '@testing-library/react';
import { ApiError } from '@cma/client';
import { afterEach, expect, it, vi } from 'vitest';
import { useAgentController } from './controller';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { WorkspaceReadUnavailable } from '../api/rest/workspaceRead';
import type { AgentDetail } from '../api/types';
import { binding, harness } from './transport/controller.fixture';
import { IdentityChangedError } from './transport/identity';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('coalesces five refreshes into two authorized reads without replacing v2 history or live state', async () => {
  const h = harness();
  const readAgent = vi.spyOn(h.client, 'readAgent');
  const readWorkspace = vi.spyOn(h.client, 'readWorkspace');
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.streaming).toBe(true));
  // Consume a live frame after the stream's approval projection has been applied.
  await act(async () => h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding));
  await waitFor(() =>
    expect(view.result.current.state).toMatchObject({ conversation: { activeRunId: '1' } }),
  );
  const before = view.result.current.state;
  expect(before.status).toBe('ready');
  if (before.status !== 'ready') throw new Error('Expected ready fixture');
  const authorize = vi.spyOn(before.v2!, 'authorize');
  const pending = deferred<AgentDetail>();
  readAgent
    .mockReturnValueOnce(pending.promise)
    .mockResolvedValue({ ...AGENT_DETAIL, title: 'Latest' });
  readWorkspace.mockResolvedValue({ ...workspaceFixture(), name: 'Updated workspace' });
  readAgent.mockClear();
  readWorkspace.mockClear();
  const snapshots = h.snapshot.mock.calls.length;
  const heads = h.head.mock.calls.length;
  const transcripts = vi.mocked(h.client.readAgentTranscript).mock.calls.length;
  act(() => {
    for (let i = 0; i < 5; i++) view.result.current.refreshDetail();
  });
  await waitFor(() => expect(readAgent).toHaveBeenCalledOnce());
  await act(async () => pending.resolve({ ...AGENT_DETAIL, title: 'Intermediate' }));
  await waitFor(() =>
    expect(view.result.current.state).toMatchObject({ detail: { title: 'Latest' } }),
  );
  expect(readAgent).toHaveBeenCalledTimes(2);
  expect(readWorkspace).toHaveBeenCalledTimes(2);
  expect(authorize).toHaveBeenCalledTimes(4);
  const after = view.result.current.state;
  if (after.status !== 'ready') throw new Error('Expected retained view');
  expect(after.workspace.name).toBe('Updated workspace');
  for (const key of [
    'v2',
    'v2Items',
    'conversation',
    'historyLoading',
    'historyUnavailable',
  ] as const)
    expect(after[key]).toBe(before[key]);
  expect(h.snapshot).toHaveBeenCalledTimes(snapshots);
  expect(h.head).toHaveBeenCalledTimes(heads);
  expect(h.client.readAgentTranscript).toHaveBeenCalledTimes(transcripts);
  expect(h.cancel).not.toHaveBeenCalled();
  view.unmount();
});

it('reports a Workspace read outage while preserving the live conversation and clears it on recovery', async () => {
  vi.useFakeTimers();
  vi.spyOn(console, 'warn').mockImplementation(() => undefined);
  const readWorkspace = vi.fn().mockResolvedValue(workspaceFixture());
  const client = agentClient({ readWorkspace });
  const replayAgent = vi.spyOn(client, 'replayAgent');
  const view = renderHook(() => useAgentController(client, 'agt_1'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  const before = view.result.current.state;
  if (before.status !== 'ready') throw new Error('Expected retained view');
  readWorkspace.mockRejectedValueOnce(new WorkspaceReadUnavailable(new ApiError(503, 'Busy')));
  await act(async () => view.result.current.refreshDetail());
  expect(view.result.current.state).toMatchObject({ status: 'ready', workspaceUnavailable: true });
  expect(view.result.current.state).toMatchObject({ conversation: before.conversation });
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(view.result.current.state).toMatchObject({ status: 'ready', workspaceUnavailable: false });
  expect(replayAgent).toHaveBeenCalledOnce();
  view.unmount();
});

it.each([false, true])(
  'retains metadata on 503 and retries once after five seconds (retry fails: %s)',
  async (failsAgain) => {
    vi.useFakeTimers();
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const readAgent = vi.fn().mockResolvedValue(AGENT_DETAIL);
    const client = agentClient({ readAgent });
    const view = renderHook(() => useAgentController(client, 'agt_1'));
    await act(() => vi.advanceTimersByTimeAsync(0));
    const before = view.result.current.state;
    readAgent.mockRejectedValueOnce(new ApiError(503, 'Busy'));
    if (failsAgain) readAgent.mockRejectedValueOnce(new ApiError(503, 'Still busy'));
    else readAgent.mockResolvedValue({ ...AGENT_DETAIL, title: 'Recovered' });
    await act(async () => view.result.current.refreshDetail());
    expect(view.result.current.state).toBe(before);
    await act(() => vi.advanceTimersByTimeAsync(4999));
    expect(readAgent).toHaveBeenCalledTimes(2);
    act(() => view.result.current.refreshDetail());
    expect(readAgent).toHaveBeenCalledTimes(2);
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(readAgent).toHaveBeenCalledTimes(3);
    if (failsAgain) expect(view.result.current.state).toBe(before);
    else expect(view.result.current.state).toMatchObject({ detail: { title: 'Recovered' } });
    await act(() => vi.advanceTimersByTimeAsync(10000));
    expect(readAgent).toHaveBeenCalledTimes(3);
    expect(warning).toHaveBeenCalledTimes(failsAgain ? 2 : 1);
    await act(async () => view.result.current.refreshDetail());
    expect(readAgent).toHaveBeenCalledTimes(4);
    view.unmount();
  },
);

it.each([
  [429, false],
  [429, true],
  [503, false],
  [503, true],
] as const)(
  'respects %s Retry-After for refresh triggers and its one retry (retry fails: %s)',
  async (status, failsAgain) => {
    vi.useFakeTimers();
    vi.spyOn(Math, 'random').mockReturnValue(0);
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const readAgent = vi.fn().mockResolvedValue(AGENT_DETAIL);
    const client = agentClient({ readAgent });
    const view = renderHook(() => useAgentController(client, 'agt_1'));
    await act(() => vi.advanceTimersByTimeAsync(0));
    const before = view.result.current.state;
    readAgent.mockRejectedValueOnce(new ApiError(status, 'Busy', undefined, undefined, 60_000));
    if (failsAgain)
      readAgent.mockRejectedValueOnce(
        new ApiError(status, 'Still busy', undefined, undefined, 120_000),
      );
    readAgent.mockResolvedValue({ ...AGENT_DETAIL, title: 'Recovered' });
    await act(async () => view.result.current.refreshDetail());
    await act(() => vi.advanceTimersByTimeAsync(5000));
    await act(async () => view.result.current.refreshDetail());
    expect(readAgent).toHaveBeenCalledTimes(2);
    expect(view.result.current.state).toBe(before);
    await act(() => vi.advanceTimersByTimeAsync(54_999));
    await act(async () => view.result.current.refreshDetail());
    expect(readAgent).toHaveBeenCalledTimes(2);
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(readAgent).toHaveBeenCalledTimes(3);
    if (failsAgain) {
      expect(view.result.current.state).toBe(before);
      await act(async () => view.result.current.refreshDetail());
      await act(() => vi.advanceTimersByTimeAsync(119_999));
      await act(async () => view.result.current.refreshDetail());
      expect(readAgent).toHaveBeenCalledTimes(3);
      await act(() => vi.advanceTimersByTimeAsync(1));
      // Expiry permits a future event; it must not schedule a third automatic attempt.
      expect(readAgent).toHaveBeenCalledTimes(3);
      await act(async () => view.result.current.refreshDetail());
      expect(readAgent).toHaveBeenCalledTimes(4);
    }
    expect(view.result.current.state).toMatchObject({ detail: { title: 'Recovered' } });
    view.unmount();
  },
);

it.each([401, 403, 404])(
  'uses full-load error handling after metadata access returns %s',
  async (status) => {
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const readAgent = vi.fn().mockResolvedValue(AGENT_DETAIL);
    const client = agentClient({ readAgent });
    const view = renderHook(() => useAgentController(client, 'agt_1'));
    await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
    readAgent.mockRejectedValue(new ApiError(status, 'No longer accessible'));
    act(() => view.result.current.refreshDetail());
    await waitFor(() => expect(view.result.current.state.status).toBe('error'));
    expect(readAgent).toHaveBeenCalledTimes(3);
    view.unmount();
  },
);

it.each(['agent', 'workspace', 'sandbox'] as const)(
  'revalidates a changed %s projection instead of merging it',
  async (field) => {
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const readAgent = vi.fn().mockResolvedValue(AGENT_DETAIL);
    const client = agentClient({ readAgent });
    const view = renderHook(() => useAgentController(client, 'agt_1'));
    await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
    const changed =
      field === 'agent'
        ? { ...AGENT_DETAIL, id: 'other' }
        : {
            ...AGENT_DETAIL,
            [field]: { ...AGENT_DETAIL[field], id: 'other' },
          };
    readAgent.mockResolvedValueOnce(changed).mockRejectedValue(new ApiError(404, 'Deleted'));
    act(() => view.result.current.refreshDetail());
    await waitFor(() => expect(view.result.current.state.status).toBe('error'));
    expect(readAgent).toHaveBeenCalledTimes(3);
    view.unmount();
  },
);

it('invalidates the view on an identity change without starting another load', async () => {
  const h = harness();
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.streaming).toBe(true));
  const readAgent = vi.spyOn(h.client, 'readAgent');
  h.currentUser.mockRejectedValue(new IdentityChangedError());
  act(() => view.result.current.refreshDetail());
  await waitFor(() =>
    expect(view.result.current.state).toMatchObject({
      status: 'error',
      message: new IdentityChangedError().message,
    }),
  );
  expect(readAgent).not.toHaveBeenCalled();
  await waitFor(() => expect(h.cancel).toHaveBeenCalledOnce());
  view.unmount();
});

it.each(['viewer', 'agent', 'client', 'suspension', 'retry', 'unmount'] as const)(
  'cancels metadata work across %s and rejects late publication',
  async (change) => {
    const readAgent = vi.fn().mockResolvedValue(AGENT_DETAIL);
    const client = agentClient({ readAgent });
    const second = agentClient({
      readAgent: async () => ({ ...AGENT_DETAIL, title: 'Next account' }),
    });
    const props = { client, viewerId: 'alice', agentId: 'agt_1', suspended: false };
    const view = renderHook((value) => useAgentController(value.client, value.agentId, value), {
      initialProps: props,
    });
    await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
    const pending = deferred<AgentDetail>();
    readAgent.mockReturnValueOnce(pending.promise);
    act(() => view.result.current.refreshDetail());
    await waitFor(() => expect(readAgent).toHaveBeenCalledTimes(2));
    const signal = readAgent.mock.lastCall![1].signal as AbortSignal;
    if (change === 'unmount') view.unmount();
    else if (change === 'retry') act(() => view.result.current.reload());
    else
      view.rerender({
        ...props,
        ...(change === 'viewer' ? { viewerId: 'bob' } : {}),
        ...(change === 'agent' ? { agentId: 'agt_2' } : {}),
        ...(change === 'client' ? { client: second } : {}),
        ...(change === 'suspension' ? { suspended: true } : {}),
      });
    expect(signal.aborted).toBe(true);
    await act(async () => pending.resolve({ ...AGENT_DETAIL, title: 'Stale response' }));
    expect(view.result.current.state).not.toMatchObject({ detail: { title: 'Stale response' } });
    if (change === 'suspension') {
      const count = readAgent.mock.calls.length;
      act(() => view.result.current.refreshDetail());
      expect(readAgent).toHaveBeenCalledTimes(count);
    }
    view.unmount();
  },
);

it('does not read metadata before the Agent is ready and cancels a scheduled retry on unmount', async () => {
  vi.useFakeTimers();
  vi.spyOn(console, 'warn').mockImplementation(() => undefined);
  const pending = deferred<AgentDetail>();
  const readAgent = vi
    .fn()
    .mockReturnValueOnce(pending.promise)
    .mockRejectedValue(new ApiError(503, 'Busy'));
  const client = agentClient({ readAgent });
  const view = renderHook(() => useAgentController(client, 'agt_1'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  act(() => view.result.current.refreshDetail());
  expect(readAgent).toHaveBeenCalledOnce();
  await act(async () => pending.resolve(AGENT_DETAIL));
  await act(async () => view.result.current.refreshDetail());
  expect(readAgent).toHaveBeenCalledTimes(2);
  view.unmount();
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(readAgent).toHaveBeenCalledTimes(2);
});

it.each([false, true])(
  'accepts fresh retry metadata unless a newer resource refresh overtakes it (%s)',
  async (overtaken) => {
    vi.useFakeTimers();
    let finish!: (value: { items: [] }) => void;
    const pending = new Promise<{ items: [] }>((resolve) => {
      finish = resolve;
    });
    const readAgentEvents = vi.fn().mockRejectedValueOnce(new ApiError(503, 'History unavailable'));
    readAgentEvents.mockReturnValueOnce(pending);
    const readAgent = vi.fn().mockResolvedValue(AGENT_DETAIL);
    const client = agentClient({ readAgent, readAgentEvents });
    const view = renderHook(() => useAgentController(client, 'agt_1'));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(view.result.current.state).toMatchObject({ historyUnavailable: true });
    readAgent.mockResolvedValue({ ...AGENT_DETAIL, title: 'Fresh retry metadata' });
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(readAgentEvents).toHaveBeenCalledTimes(2);
    if (overtaken) {
      readAgent.mockResolvedValue({ ...AGENT_DETAIL, title: 'Newest resource metadata' });
      await act(async () => view.result.current.refreshDetail());
    }
    await act(async () => finish({ items: [] }));
    expect(view.result.current.state).toMatchObject({
      historyUnavailable: false,
      detail: { title: overtaken ? 'Newest resource metadata' : 'Fresh retry metadata' },
    });
    view.unmount();
  },
);
