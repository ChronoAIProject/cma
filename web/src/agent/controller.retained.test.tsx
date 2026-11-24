import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, AGENT_USAGE, agentClient } from '../test/agent';
import { useAgentController } from './controller';
import type { SelectedRetiredAgent } from '../useDeletionNoticeReconciliation';
import { harness } from './transport/controller.fixture';

const retiredWitness: SelectedRetiredAgent = {
  viewerId: 'alice',
  authEpoch: 1,
  detail: { ...AGENT_DETAIL, sandbox: { ...AGENT_DETAIL.sandbox, status: 'retired' } },
};
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function fixture() {
  return agentClient({
    readAgent: vi.fn(async (id) => ({ ...AGENT_DETAIL, id })),
    readAgentTranscript: vi.fn(async (agentId) => ({ ...AGENT_TRANSCRIPT, agentId })),
    readAgentUsage: vi.fn(async (agentId) => ({ ...AGENT_USAGE, agentId })),
    readAgentEvents: vi.fn(async () => ({ items: [] })),
    replayAgent: vi.fn(
      async (_id, { signal }) =>
        new Promise<void>((resolve) => {
          signal?.addEventListener('abort', () => resolve(), { once: true });
        }),
    ),
    runAgent: vi.fn(),
  });
}

it('restores a visited Agent immediately and resumes replay without downloading its history again', async () => {
  const client = fixture();
  const view = renderHook(
    ({ id, suspended }) =>
      useAgentController(client, id, {
        viewerId: 'alice',
        suspended,
      }),
    { initialProps: { id: 'agt_1', suspended: false } },
  );
  await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
  view.rerender({ id: 'agt_2', suspended: false });
  await waitFor(() =>
    expect(view.result.current.state).toMatchObject({ status: 'ready', detail: { id: 'agt_2' } }),
  );
  view.rerender({ id: 'agt_1', suspended: true });
  expect(view.result.current.state).toMatchObject({
    status: 'ready',
    detail: { id: 'agt_1' },
    conversation: {
      messages: expect.arrayContaining([
        expect.objectContaining({ content: 'The repository is ready.' }),
      ]),
    },
  });
  await act(() => view.result.current.send('Not authorized yet'));
  expect(client.runAgent).not.toHaveBeenCalled();
  view.rerender({ id: 'agt_1', suspended: false });
  await waitFor(() => expect(client.readAgent).toHaveBeenCalledTimes(3));
  expect(client.readAgentTranscript).toHaveBeenCalledTimes(2);
  expect(client.readAgentEvents).toHaveBeenCalledTimes(2);
  expect(client.replayAgent).toHaveBeenLastCalledWith(
    'agt_1',
    expect.objectContaining({ after: 4 }),
  );
  view.unmount();
});

it('never shows the previous account view while the next account is still being checked', async () => {
  const client = fixture();
  const view = renderHook(
    ({ owner, suspended }) =>
      useAgentController(client, 'agt_1', {
        viewerId: owner,
        suspended,
      }),
    { initialProps: { owner: 'alice', suspended: false } },
  );
  await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
  view.rerender({ owner: 'bob', suspended: true });
  expect(view.result.current.state.status).toBe('loading');
  expect(view.result.current.inputAvailability.allowed).toBe(false);
  view.unmount();
});

it('discards a retained view when its fresh metadata read denies access', async () => {
  const client = fixture();
  const view = renderHook(({ id }) => useAgentController(client, id, { viewerId: 'alice' }), {
    initialProps: { id: 'agt_1' },
  });
  await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
  view.rerender({ id: 'agt_2' });
  await waitFor(() =>
    expect(view.result.current.state).toMatchObject({ status: 'ready', detail: { id: 'agt_2' } }),
  );
  vi.mocked(client.readAgent).mockRejectedValueOnce(new ApiError(403, 'Access removed'));
  view.rerender({ id: 'agt_1' });
  await waitFor(() =>
    expect(view.result.current.state).toEqual({ status: 'error', message: 'Access removed' }),
  );
  expect(client.runAgent).not.toHaveBeenCalled();
  view.unmount();
});

it('uses only the retained transcript when mounted with a current retirement witness', async () => {
  const client = fixture();
  const readWorkspace = vi.spyOn(client, 'readWorkspace');
  const readSandbox = vi.spyOn(client, 'readSandbox');
  const hook = renderHook(() =>
    useAgentController(client, 'agt_1', {
      viewerId: 'alice',
      authEpoch: 1,
      selectedRetired: retiredWitness,
    }),
  );
  await waitFor(() => expect(hook.result.current.retained?.status).toBe('ready'));
  expect(hook.result.current.inputAvailability.allowed).toBe(false);
  for (const method of [
    client.readAgent,
    readWorkspace,
    readSandbox,
    client.readAgentUsage,
    client.readAgentEvents,
    client.replayAgent,
  ])
    expect(method).not.toHaveBeenCalled();
  expect(client.readAgentTranscript).toHaveBeenCalledTimes(1);
});

it('suspends live work and captured commands without reopening on loss of the witness', async () => {
  vi.useFakeTimers();
  const client = fixture();
  const readSandbox = vi.spyOn(client, 'readSandbox');
  const hook = renderHook(
    ({ witness }: { witness?: SelectedRetiredAgent }) =>
      useAgentController(client, 'agt_1', {
        viewerId: 'alice',
        authEpoch: 1,
        selectedRetired: witness,
      }),
    { initialProps: {} },
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  const captured = hook.result.current;
  expect(captured.state.status).toBe('ready');
  hook.rerender({ witness: retiredWitness });
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(hook.result.current.retained?.status).toBe('ready');
  const counts = [
    client.readAgentUsage,
    client.readAgentEvents,
    readSandbox,
    client.replayAgent,
  ].map((read) => vi.mocked(read).mock.calls.length);
  for (const control of [captured, hook.result.current]) {
    await act(async () => {
      await control.send('Refuse after retirement');
      await control.approve('approval', 'allow');
      await control.stop();
      await control.delete();
    });
  }
  hook.rerender({ witness: undefined });
  expect(hook.result.current.retained?.status).toBe('unconfirmed');
  expect(hook.result.current.inputAvailability.allowed).toBe(false);
  await act(() => vi.advanceTimersByTimeAsync(60_000));
  expect(
    [client.readAgentUsage, client.readAgentEvents, readSandbox, client.replayAgent].map(
      (read) => vi.mocked(read).mock.calls.length,
    ),
  ).toEqual(counts);
  expect(client.runAgent).not.toHaveBeenCalled();
  expect(hook.result.current.state).toBe(captured.state);
});

it.each(['pending', 'accepted'] as const)(
  'keeps exact %s v2 responsibility when retirement suspends the runtime',
  async (admission) => {
    sessionStorage.clear();
    const h = harness();
    if (admission === 'pending')
      h.post.mockImplementationOnce(
        (request) =>
          new Promise((_resolve, reject) => {
            request!.signal!.addEventListener(
              'abort',
              () => reject(new DOMException('Aborted', 'AbortError')),
              { once: true },
            );
          }),
      );
    const hook = renderHook(
      ({ witness }: { witness?: SelectedRetiredAgent }) =>
        useAgentController(h.client, 'agt_1', {
          viewerId: 'alice',
          authEpoch: 1,
          selectedRetired: witness,
        }),
      { initialProps: {} },
    );
    await waitFor(() =>
      expect(h.streaming && hook.result.current.inputAvailability.allowed).toBe(true),
    );
    const captured = hook.result.current;
    let sent!: Promise<void>;
    act(() => {
      sent = captured.send('Exact original responsibility');
    });
    await waitFor(() => expect(h.post).toHaveBeenCalledTimes(1));
    if (admission === 'accepted') await act(() => sent);
    const active = hook.result.current.state;
    if (active.status !== 'ready' || !active.v2) throw new Error('Expected real v2 session');
    const original = structuredClone(active.v2.journal.read());
    expect(original?.state).toBe(admission);
    hook.rerender({ witness: retiredWitness });
    await act(() => sent);
    await waitFor(() => expect(hook.result.current.retained?.status).toBe('ready'));
    expect(active.v2.journal.read()).toEqual(original);
    expect(hook.result.current.state).toBe(active);
    await act(async () => {
      await captured.send('Cannot replace');
      await captured.recoverPending?.();
    });
    expect(h.post).toHaveBeenCalledTimes(1);
    expect(h.inputReceipt).not.toHaveBeenCalled();
    expect(active.v2.journal.read()).toEqual(original);
  },
);
