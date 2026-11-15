import { act, renderHook, waitFor } from '@testing-library/react';
import { ApiError, createClient } from '@chronoai/cma-sdk';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useConsoleAgentAccess } from './consoleAgentAccess';
import { useAgentConnectionReadiness } from '../agent/connectionReadiness';
import { connectionClient, type ConnectionReport } from '../connections/client';
import { harness } from '../agent/transport/controller.fixture';
import { WIRE_AGENT } from '../test/rest';

const ready: ConnectionReport = {
  owner: 'alice',
  ready: true,
  checked_at: '2026-11-16T00:00:00Z',
  services: [],
};
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
async function setup() {
  const h = harness();
  const sdk = createClient({ fetch: async () => Response.json({ ...WIRE_AGENT, id: 'agt_1' }) });
  const detail = await sdk.agents.readAccessible('agt_1');
  const access = deferred<typeof detail>();
  vi.spyOn(sdk.agents, 'readAccessible').mockReturnValue(access.promise);
  vi.spyOn(sdk.resourceAccess, 'effective').mockResolvedValue({
    capabilities: ['view_details'],
    owner: { kind: 'person', is_current_person: false },
    revalidate_at: null,
    policy_revision: 1,
  });
  const client = { ...h.client, resourceViews: sdk };
  function useView(id = 'agt_1', owner = 'alice', source = client) {
    const route = useConsoleAgentAccess(source, id, owner);
    const connection = useAgentConnectionReadiness(
      id,
      owner,
      source,
      route.personalAgentId !== undefined && !route.accessUncertain,
    );
    return { route, connection };
  }
  return { h, client, sdk, detail, access, useView };
}
beforeEach(() => {
  sessionStorage.clear();
  vi.spyOn(connectionClient, 'check').mockResolvedValue(ready);
});
afterEach(() => vi.restoreAllMocks());

it.each(['pending', 'completed'] as const)(
  'overlaps one fresh connection request but consumes its %s result only after personal access',
  async (timing) => {
    const f = await setup();
    const report = deferred<ConnectionReport>();
    vi.mocked(connectionClient.check).mockReturnValueOnce(report.promise);
    const view = renderHook(() => f.useView());
    await waitFor(() => expect(connectionClient.check).toHaveBeenCalledOnce());
    if (timing === 'completed') await act(async () => report.resolve(ready));
    expect(view.result.current.route.personalAgentId).toBeUndefined();
    expect(view.result.current.connection.verified).toBeUndefined();
    expect(f.h.snapshot).not.toHaveBeenCalled();
    expect(f.h.post).not.toHaveBeenCalled();
    await act(async () => f.access.resolve(f.detail));
    if (timing === 'pending') {
      expect(view.result.current.connection.verified).toBeUndefined();
      await act(async () => report.resolve(ready));
    }
    await waitFor(() => expect(view.result.current.connection.verified).toEqual(ready));
    expect(view.result.current.connection.takePrepared()).toBeUndefined();
    expect(connectionClient.check).toHaveBeenCalledOnce();
    view.unmount();
  },
);

it.each(['shared', 'forbidden', 'missing', 'wrong detail'] as const)(
  'discards speculative readiness when access is %s',
  async (kind) => {
    const f = await setup();
    const report = deferred<ConnectionReport>();
    vi.mocked(connectionClient.check).mockReturnValueOnce(report.promise);
    const view = renderHook(() => f.useView());
    await waitFor(() => expect(connectionClient.check).toHaveBeenCalledOnce());
    const signal = vi.mocked(connectionClient.check).mock.calls[0]![2];
    await act(async () => {
      if (kind === 'forbidden' || kind === 'missing')
        f.access.reject(new ApiError(kind === 'forbidden' ? 403 : 404, 'Unavailable'));
      else
        f.access.resolve(
          kind === 'shared' ? { ...f.detail, access: 'shared' } : { ...f.detail, id: 'other' },
        );
    });
    await waitFor(() => expect(signal?.aborted).toBe(true));
    await act(async () => report.resolve(ready));
    expect(view.result.current.route.personalAgentId).toBeUndefined();
    expect(view.result.current.connection.verified).toBeUndefined();
    expect(f.h.snapshot).not.toHaveBeenCalled();
    expect(f.h.post).not.toHaveBeenCalled();
    view.unmount();
  },
);

it.each(['owner', 'agent', 'client', 'unmount'] as const)(
  'cancels the old read on %s change and rejects its late success',
  async (kind) => {
    const f = await setup();
    const old = deferred<ConnectionReport>();
    vi.mocked(connectionClient.check).mockReturnValueOnce(old.promise);
    const view = renderHook(({ id, owner, client }) => f.useView(id, owner, client), {
      initialProps: { id: 'agt_1', owner: 'alice', client: f.client },
    });
    await waitFor(() => expect(connectionClient.check).toHaveBeenCalledOnce());
    const signal = vi.mocked(connectionClient.check).mock.calls[0]![2];
    if (kind === 'unmount') view.unmount();
    else
      view.rerender({
        id: kind === 'agent' ? 'agt_2' : 'agt_1',
        owner: kind === 'owner' ? 'bob' : 'alice',
        client: kind === 'client' ? { ...f.client } : f.client,
      });
    expect(signal?.aborted).toBe(true);
    await act(async () => old.resolve(ready));
    expect(view.result.current.connection.verified).toBeUndefined();
    view.unmount();
  },
);

it.each(['wrong owner', 'missing', 'unknown', 'error'] as const)(
  'preserves the %s connection refusal after authorized classification',
  async (kind) => {
    const f = await setup();
    if (kind === 'error')
      vi.mocked(connectionClient.check).mockRejectedValueOnce(new Error('Connection unavailable'));
    else
      vi.mocked(connectionClient.check).mockResolvedValueOnce(
        kind === 'wrong owner'
          ? { ...ready, owner: 'bob' }
          : {
              ...ready,
              ready: false,
              services: [
                {
                  service: 'provider',
                  display_name: 'Provider',
                  kind: 'nyxid',
                  state: kind === 'missing' ? 'missing' : 'unknown',
                  connection_mode: 'unknown',
                },
              ],
            },
      );
    const view = renderHook(() => f.useView());
    await act(async () => f.access.resolve(f.detail));
    await waitFor(() =>
      expect(
        kind === 'missing'
          ? view.result.current.connection.needed
          : view.result.current.connection.error,
      ).toBeTruthy(),
    );
    expect(view.result.current.connection.verified).toBeUndefined();
    expect(view.result.current.connection.takePrepared()).toBeUndefined();
    expect(connectionClient.check).toHaveBeenCalledOnce();
    expect(f.h.post).not.toHaveBeenCalled();
    view.unmount();
  },
);

it('does not retain a StrictMode-abandoned request as the active opening', async () => {
  const f = await setup();
  const view = renderHook(() => f.useView(), {
    reactStrictMode: true,
  });
  await waitFor(() => expect(connectionClient.check).toHaveBeenCalledTimes(2));
  expect(vi.mocked(connectionClient.check).mock.calls[0]![2]?.aborted).toBe(true);
  await act(async () => f.access.resolve(f.detail));
  await waitFor(() => expect(view.result.current.connection.verified).toEqual(ready));
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
  expect(view.result.current.connection.takePrepared()).toBeUndefined();
  view.unmount();
});

it('checks changed connections again on a later visit instead of reusing prior readiness', async () => {
  const f = await setup();
  f.access.resolve(f.detail);
  const first = renderHook(() => f.useView());
  await waitFor(() => expect(first.result.current.connection.verified).toEqual(ready));
  first.unmount();
  vi.mocked(connectionClient.check).mockResolvedValueOnce({
    ...ready,
    ready: false,
    services: [
      {
        service: 'provider',
        display_name: 'Provider',
        kind: 'nyxid',
        state: 'inactive',
        connection_mode: 'unknown',
      },
    ],
  });
  const next = renderHook(() => f.useView());
  await waitFor(() => expect(next.result.current.connection.needed).toBe(true));
  expect(next.result.current.connection.verified).toBeUndefined();
  expect(next.result.current.connection.takePrepared()).toBeUndefined();
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
  expect(f.h.post).not.toHaveBeenCalled();
  next.unmount();
});
