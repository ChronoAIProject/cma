import { act, renderHook, waitFor } from '@testing-library/react';
import { ApiError, createClient } from '@cma/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useConsoleAgentAccess } from './consoleAgentAccess';
import { useAgentController } from '../agent/controller';
import { harness } from '../agent/transport/controller.fixture';
import { WIRE_AGENT } from '../test/rest';
import { AGENT_DETAIL } from '../test/agent';

beforeEach(() => sessionStorage.clear());
afterEach(() => vi.restoreAllMocks());

it('reopens retained personal routes without preparing another transport and resets on client replacement', async () => {
  const h = harness();
  const sdk = createClient({ fetch: async () => Response.json({ ...WIRE_AGENT, id: 'agt_1' }) });
  const first = { ...h.client, resourceViews: sdk };
  const prepare = vi.spyOn(h.client.agentTransport!, 'prepareOpening');
  const view = renderHook(({ client, id }) => useConsoleAgentAccess(client, id, 'alice'), {
    initialProps: { client: first, id: 'agt_1' as string | undefined },
  });
  await waitFor(() => expect(view.result.current.agentAccess.personalReady).toBe(true));
  expect(prepare).toHaveBeenCalledOnce();
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
  expect(h.head).toHaveBeenCalledOnce();
  view.rerender({ client: first, id: undefined });
  view.rerender({ client: first, id: 'agt_1' });
  await waitFor(() => expect(view.result.current.agentAccess.state.status).toBe('ready'));
  expect(prepare).toHaveBeenCalledOnce();
  // A retained route reuses its selection; only a new client prepares fresh reads.
  expect(h.snapshot).toHaveBeenCalledOnce();
  expect(h.head).toHaveBeenCalledOnce();
  view.rerender({ client: { ...first }, id: 'agt_1' });
  await waitFor(() => expect(view.result.current.agentAccess.personalReady).toBe(true));
  expect(prepare).toHaveBeenCalledTimes(2);
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledTimes(2));
  expect(h.head).toHaveBeenCalledTimes(2);
  expect(h.post).not.toHaveBeenCalled();
  view.unmount();
});

it.each(['personal', 'shared', 'denied'] as const)(
  'keeps transport selection and protected loading behind %s access classification',
  async (access) => {
    const h = harness();
    const sdk = createClient({ fetch: async () => Response.json({ ...WIRE_AGENT, id: 'agt_1' }) });
    const detail = await sdk.agents.readAccessible('agt_1');
    let finish!: (value: typeof detail) => void;
    let refuse!: (error: Error) => void;
    vi.spyOn(sdk.agents, 'readAccessible').mockReturnValue(
      new Promise((resolve, reject) => {
        finish = resolve;
        refuse = reject;
      }),
    );
    vi.spyOn(sdk.resourceAccess, 'effective').mockResolvedValue({
      capabilities: ['view_details', 'view_history'],
      owner: { kind: 'person', is_current_person: false },
      revalidate_at: null,
      policy_revision: 1,
    });
    const client = { ...h.client, resourceViews: sdk };
    const agentRead = vi.spyOn(client, 'readAgent');
    const view = renderHook(() => {
      const state = useConsoleAgentAccess(client, 'agt_1', 'alice');
      const controller = useAgentController(client, state.personalAgentId, {
        viewerId: 'alice',
        suspended: state.accessUncertain,
      });
      return { state, controller };
    });
    await act(async () => {});
    expect(h.eligibility).not.toHaveBeenCalled();
    expect(h.snapshot).not.toHaveBeenCalled();
    expect(h.head).not.toHaveBeenCalled();
    expect(agentRead).not.toHaveBeenCalled();
    await act(async () => {
      if (access === 'denied') refuse(new ApiError(403, 'Access denied'));
      else finish(access === 'shared' ? { ...detail, access: 'shared' } : detail);
    });
    if (access === 'personal') {
      await waitFor(() => expect(view.result.current.controller.state.status).toBe('ready'));
      expect(agentRead).toHaveBeenCalledWith(AGENT_DETAIL.id, expect.anything());
      expect(h.snapshot).toHaveBeenCalled();
      expect(h.head).toHaveBeenCalled();
    } else {
      expect(view.result.current.state.personalAgentId).toBeUndefined();
      expect(agentRead).not.toHaveBeenCalled();
      expect(h.snapshot).not.toHaveBeenCalled();
      expect(h.head).not.toHaveBeenCalled();
      expect(view.result.current.controller.state.status).toBe('idle');
    }
    expect(h.post).not.toHaveBeenCalled();
    view.unmount();
  },
);

it('classifies a directly opened retired Agent without preparing any runtime projections', async () => {
  const h = harness();
  const sdk = createClient({
    fetch: async () =>
      Response.json({
        ...WIRE_AGENT,
        id: 'agt_1',
        sandbox: { ...WIRE_AGENT.sandbox, status: 'retired' },
      }),
  });
  const client = { ...h.client, resourceViews: sdk };
  const prepare = vi.spyOn(client.agentTransport!, 'prepareOpening');
  const read = vi.spyOn(client, 'readAgent');
  const view = renderHook(() => useConsoleAgentAccess(client, 'agt_1', 'alice'));
  await waitFor(() => expect(view.result.current.agentAccess.state.status).toBe('ready'));
  expect(view.result.current.personalAgentId).toBe('agt_1');
  expect(prepare).not.toHaveBeenCalled();
  expect(h.eligibility).not.toHaveBeenCalled();
  expect(h.snapshot).not.toHaveBeenCalled();
  expect(h.head).not.toHaveBeenCalled();
  expect(read).not.toHaveBeenCalled();
  expect(h.post).not.toHaveBeenCalled();
});

it('requires a new personal classification after an authentication epoch changes', async () => {
  const h = harness();
  const sdk = createClient({ fetch: async () => Response.json({ ...WIRE_AGENT, id: 'agt_1' }) });
  const read = vi.spyOn(sdk.agents, 'readAccessible');
  const client = { ...h.client, resourceViews: sdk };
  const view = renderHook(({ epoch }) => useConsoleAgentAccess(client, 'agt_1', 'alice', epoch), {
    initialProps: { epoch: 7 },
  });
  await waitFor(() => expect(view.result.current.agentAccess.personalReady).toBe(true));
  let finish!: (value: Awaited<ReturnType<typeof read>>) => void;
  read.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  view.rerender({ epoch: 8 });
  expect(view.result.current.personalAgentId).toBeUndefined();
  expect(view.result.current.agentAccess.personalReady).toBe(false);
  await act(async () => {
    finish(await sdk.agents.read('agt_1'));
  });
  await waitFor(() => expect(view.result.current.agentAccess.personalReady).toBe(true));
});

it.each(['client', 'agent', 'viewer', 'epoch'] as const)(
  'isolates download notices across a %s change and return to the original scope',
  async (changed) => {
    const h = harness();
    const sdk = createClient({
      fetch: async () => new Response(null, { status: 404 }),
    });
    const original = {
      client: { ...h.client, resourceViews: sdk },
      agent: 'agt_1',
      viewer: 'alice',
      epoch: 7,
    };
    const replacement = { client: { ...original.client }, agent: 'agt_2', viewer: 'bob', epoch: 8 };
    const view = renderHook(
      ({ client, agent, viewer, epoch }) => useConsoleAgentAccess(client, agent, viewer, epoch),
      { initialProps: original },
    );
    await waitFor(() => expect(view.result.current.agentAccess.state.status).toBe('error'));
    const first = view.result.current.fileDownload.sidePanel;
    act(() => first.beginFileDownload()('First interruption'));
    expect(view.result.current.fileDownload.message).toBe('First interruption');
    const dismissFirst = view.result.current.fileDownload.dismiss;
    let finishFirst!: ReturnType<typeof first.beginFileDownload>;
    act(() => {
      finishFirst = first.beginFileDownload();
    });
    view.rerender({ ...original, [changed]: replacement[changed] });
    expect(view.result.current.fileDownload.message).toBeUndefined();
    await waitFor(() => expect(view.result.current.agentAccess.state.status).toBe('error'));
    act(() => view.result.current.fileDownload.sidePanel.beginFileDownload()('New interruption'));
    act(() => {
      first.beginFileDownload()('Stale attempt');
      finishFirst('Stale interruption');
      first.onAccessChanged('Stale refusal');
      dismissFirst();
    });
    expect(view.result.current.fileDownload.message).toBe('New interruption');
    view.rerender(original);
    await waitFor(() => expect(view.result.current.agentAccess.state.status).toBe('error'));
    act(() => finishFirst('Late interruption after returning'));
    expect(view.result.current.fileDownload.message).toBeUndefined();
  },
);

it('ignores a late interruption from an older download in the same scope', async () => {
  const h = harness();
  const fetch = vi.fn(async () => new Response(null, { status: 404 }));
  const client = { ...h.client, resourceViews: createClient({ fetch }) };
  const view = renderHook(() => useConsoleAgentAccess(client, 'agt_1', 'alice'));
  await waitFor(() => expect(view.result.current.agentAccess.state.status).toBe('error'));
  const requests = fetch.mock.calls.length;
  const { beginFileDownload } = view.result.current.fileDownload.sidePanel;
  act(() => beginFileDownload()('Prior result'));
  const dismissPrior = view.result.current.fileDownload.dismiss;
  let finishFirst!: ReturnType<typeof beginFileDownload>;
  let finishSecond!: ReturnType<typeof beginFileDownload>;
  act(() => {
    finishFirst = beginFileDownload();
    finishSecond = beginFileDownload();
  });
  expect(view.result.current.fileDownload.message).toBeUndefined();
  act(() => finishFirst('Late first interruption'));
  expect(view.result.current.fileDownload.message).toBeUndefined();
  act(() => finishSecond('Second interruption'));
  act(dismissPrior);
  expect(view.result.current.fileDownload.message).toBe('Second interruption');
  act(view.result.current.fileDownload.dismiss);
  act(() => finishSecond('Late second interruption'));
  expect(view.result.current.fileDownload.message).toBeUndefined();
  expect(fetch).toHaveBeenCalledTimes(requests);
});
