import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { SandboxDetail } from '../api/types';
import { AGENT_DETAIL } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { useAgentController } from './controller';
import { accepted, harness, json, page } from './transport/controller.fixture';

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((done) => (resolve = done));
  return { promise, resolve };
}

const authorizedLineage = {
  agentId: AGENT_DETAIL.id,
  workspaceId: AGENT_DETAIL.workspace.id,
  sandboxId: AGENT_DETAIL.sandbox.id,
};

beforeEach(() => sessionStorage.clear());
afterEach(() => vi.restoreAllMocks());

it('allows an authorized input before the first history page resolves', async () => {
  const h = harness();
  const history = deferred<Response>();
  h.snapshot.mockReturnValueOnce(history.promise);
  const parentRead = vi
    .spyOn(h.client, 'readSandbox')
    .mockResolvedValue({ ...sandboxFixture(), owner: 'alice' });
  const view = renderHook(() =>
    useAgentController(h.client, AGENT_DETAIL.id, { viewerId: 'alice', authorizedLineage }),
  );
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
  expect(parentRead).toHaveBeenCalledOnce();
  await waitFor(() => expect(view.result.current.inputAvailability.allowed).toBe(true));
  expect(view.result.current.state).toMatchObject({ status: 'ready', historyLoading: true });
  const accepted = vi.fn();
  await act(async () => view.result.current.send('Send while history loads', accepted));
  expect(h.post).toHaveBeenCalledOnce();
  expect(accepted).toHaveBeenCalledOnce();
  await act(async () => history.resolve(json(page([], 0))));
  await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
  expect(view.result.current.inputAvailability.allowed).toBe(true);
  expect(parentRead).toHaveBeenCalledOnce();
});

it('keeps input held after loading until the overlapping parent read resolves', async () => {
  const h = harness();
  const parent = deferred<SandboxDetail>();
  vi.spyOn(h.client, 'readSandbox').mockReturnValue(parent.promise);
  const view = renderHook(() =>
    useAgentController(h.client, AGENT_DETAIL.id, { viewerId: 'alice', authorizedLineage }),
  );
  await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
  expect(view.result.current.inputAvailability.allowed).toBe(false);
  await act(async () => parent.resolve({ ...sandboxFixture(), owner: 'alice' }));
  await waitFor(() => expect(view.result.current.inputAvailability.allowed).toBe(true));
});

it('aborts early parent observation when current personal access becomes uncertain', async () => {
  const h = harness();
  const parent = deferred<SandboxDetail>();
  const parentRead = vi.spyOn(h.client, 'readSandbox').mockReturnValue(parent.promise);
  h.snapshot.mockReturnValueOnce(new Promise(() => {}));
  const view = renderHook(
    ({ suspended }) =>
      useAgentController(h.client, AGENT_DETAIL.id, {
        viewerId: 'alice',
        authorizedLineage,
        suspended,
      }),
    { initialProps: { suspended: false } },
  );
  await waitFor(() => expect(parentRead).toHaveBeenCalledOnce());
  const signal = parentRead.mock.calls[0]?.[1]?.signal;
  view.rerender({ suspended: true });
  expect(signal?.aborted).toBe(true);
  await act(async () => parent.resolve({ ...sandboxFixture(), owner: 'alice' }));
  expect(view.result.current.inputAvailability.allowed).toBe(false);
  expect(h.post).not.toHaveBeenCalled();
});

it.each(['unknown owner', 'different Agent'] as const)(
  'does not start hinted parent observation for %s',
  async (reason) => {
    const h = harness();
    const parentRead = vi.spyOn(h.client, 'readSandbox');
    h.currentUser.mockReturnValue(new Promise(() => {}));
    renderHook(() =>
      useAgentController(h.client, AGENT_DETAIL.id, {
        viewerId: reason === 'unknown owner' ? undefined : 'alice',
        authorizedLineage:
          reason === 'different Agent'
            ? { ...authorizedLineage, agentId: 'different' }
            : authorizedLineage,
      }),
    );
    await act(async () => undefined);
    expect(parentRead).not.toHaveBeenCalled();
  },
);

it.each(['owner', 'client'] as const)(
  'discards a pending preload when the current %s changes',
  async (change) => {
    const h = harness();
    const next = harness();
    const oldParent = deferred<SandboxDetail>();
    const parentRead = vi
      .spyOn(h.client, 'readSandbox')
      .mockReturnValueOnce(oldParent.promise)
      .mockReturnValue(new Promise(() => {}));
    vi.spyOn(next.client, 'readSandbox').mockReturnValue(new Promise(() => {}));
    h.snapshot.mockReturnValue(new Promise(() => {}));
    next.snapshot.mockReturnValue(new Promise(() => {}));
    const view = renderHook(
      ({ client, owner }) =>
        useAgentController(client, AGENT_DETAIL.id, { viewerId: owner, authorizedLineage }),
      { initialProps: { client: h.client, owner: 'alice' } },
    );
    await waitFor(() => expect(parentRead).toHaveBeenCalledOnce());
    const oldSignal = parentRead.mock.calls[0]?.[1]?.signal;
    if (change === 'owner') h.currentUser.mockResolvedValue('bob');
    view.rerender({
      client: change === 'client' ? next.client : h.client,
      owner: change === 'owner' ? 'bob' : 'alice',
    });
    expect(oldSignal?.aborted).toBe(true);
    await act(async () => oldParent.resolve({ ...sandboxFixture(), owner: 'alice' }));
    expect(view.result.current.progressObservation).toBeUndefined();
    expect(view.result.current.inputAvailability.allowed).toBe(false);
    expect(h.post).not.toHaveBeenCalled();
    expect(next.post).not.toHaveBeenCalled();
  },
);

it('requires a new paired Agent and parent observation when loading reveals bootstrap authority', async () => {
  const h = harness();
  const detail = {
    ...AGENT_DETAIL,
    status: 'starting' as const,
    bootstrap: {
      operationId: 'creation-one',
      sandboxId: authorizedLineage.sandboxId,
      workspaceId: authorizedLineage.workspaceId,
    },
  };
  vi.spyOn(h.client, 'readAgent').mockResolvedValue(detail);
  const history = deferred<Response>();
  h.snapshot.mockReturnValueOnce(history.promise);
  const bootstrapParent = deferred<SandboxDetail>();
  const parentRead = vi
    .spyOn(h.client, 'readSandbox')
    .mockResolvedValueOnce({ ...sandboxFixture(), owner: 'alice' })
    .mockReturnValue(bootstrapParent.promise);
  const view = renderHook(() =>
    useAgentController(h.client, AGENT_DETAIL.id, { viewerId: 'alice', authorizedLineage }),
  );
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
  await waitFor(() => expect(parentRead).toHaveBeenCalledTimes(2));
  await act(async () => history.resolve(json(page([], 0))));
  expect(view.result.current.state.status).toBe('ready');
  expect(view.result.current.inputAvailability.allowed).toBe(false);
  await act(async () =>
    bootstrapParent.resolve({ ...sandboxFixture('provisioning'), owner: 'alice' }),
  );
  await waitFor(() => expect(view.result.current.inputAvailability.allowed).toBe(true));
});

it('reloads and fails closed if a later access read unexpectedly changes immutable lineage', async () => {
  const h = harness();
  const view = renderHook(
    ({ lineage }) =>
      useAgentController(h.client, AGENT_DETAIL.id, {
        viewerId: 'alice',
        authorizedLineage: lineage,
      }),
    { initialProps: { lineage: authorizedLineage } },
  );
  await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
  view.rerender({ lineage: { ...authorizedLineage, workspaceId: 'different-workspace' } });
  await waitFor(() => expect(view.result.current.state.status).toBe('error'));
  expect(h.post).not.toHaveBeenCalled();
});

it('shows the latest transcript while the first rich-history response is still pending', async () => {
  const h = harness();
  const history = deferred<Response>();
  h.snapshot.mockReturnValueOnce(history.promise);
  vi.spyOn(h.client, 'readAgentUsage').mockReturnValue(new Promise(() => {}));
  vi.spyOn(h.client, 'readAgentTranscript').mockResolvedValue({
    agentId: 'agt_1',
    throughSeq: 0,
    turns: [
      {
        turnSequence: 0,
        startedAt: AGENT_DETAIL.createdAt,
        ownerMessage: 'Latest question',
        agentMessages: ['Latest answer'],
        messages: [
          {
            role: 'assistant',
            content: 'Latest answer',
            sequence: 0,
            appendedAt: AGENT_DETAIL.createdAt,
          },
        ],
        outcome: { type: 'completed' },
      },
    ],
  });
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() =>
    expect(view.result.current.state).toMatchObject({
      historyLoading: true,
      conversation: { messages: [expect.objectContaining({ content: 'Latest answer' })] },
    }),
  );
  expect(h.streaming).toBe(false);
  view.unmount();
});

it('does not lose an in-flight send receipt when history finishes', async () => {
  const h = harness();
  const history = deferred<Response>();
  const response = deferred<Response>();
  h.snapshot.mockReturnValueOnce(history.promise);
  h.post.mockReturnValueOnce(response.promise);
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(view.result.current.inputAvailability.allowed).toBe(true));
  const onAccepted = vi.fn();
  let sent!: Promise<void>;
  act(() => {
    sent = view.result.current.send('Message during history', onAccepted);
  });
  await waitFor(() => expect(h.post).toHaveBeenCalledOnce());
  await act(async () => history.resolve(json(page([], 0))));
  await waitFor(() => expect(h.streaming).toBe(true));
  await act(async () => {
    response.resolve(accepted());
    await sent;
  });
  expect(onAccepted).toHaveBeenCalledOnce();
  expect(view.result.current.acceptedMessage).toBe('Message during history');
  expect(h.post).toHaveBeenCalledOnce();
  view.unmount();
});
