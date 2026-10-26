import { act, fireEvent, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { connectionClient, type ConnectionReport } from '../connections/client';
import { prepareAgentConnections } from '../connections/preparation';
import { useAgentConnectionReadiness } from './connectionReadiness';

const ready: ConnectionReport = {
  owner: 'alice',
  ready: true,
  checked_at: '2026-10-15T00:00:00Z',
  services: [],
};
beforeEach(() => {
  vi.spyOn(connectionClient, 'check').mockResolvedValue(ready);
  vi.spyOn(performance, 'now').mockReturnValue(1000);
});
afterEach(() => vi.restoreAllMocks());

async function prepared() {
  const context = {};
  const preparation = prepareAgentConnections(context, 'agt_1', 'alice');
  preparation.handoff();
  preparation.release();
  const view = renderHook(
    ({ agent, owner, client, enabled }) =>
      useAgentConnectionReadiness(agent, owner, client, enabled),
    { initialProps: { agent: 'agt_1', owner: 'alice', client: context, enabled: true } },
  );
  await waitFor(() => expect(view.result.current.verified).toEqual(ready));
  return { ...view, context };
}

it('offers an authorized creation result for one immediate send without another check', async () => {
  const view = await prepared();
  expect(connectionClient.check).toHaveBeenCalledOnce();
  expect(view.result.current.takePrepared()).toEqual(ready);
  expect(view.result.current.takePrepared()).toBeUndefined();
  expect(view.result.current.verified).toEqual(ready);
});

it.each([2001, 999])(
  'refuses a prepared result when its monotonic time becomes %s',
  async (now) => {
    const view = await prepared();
    vi.mocked(performance.now).mockReturnValue(now);
    expect(view.result.current.takePrepared()).toBeUndefined();
    expect(view.result.current.verified).toEqual(ready);
  },
);

it('measures age from the actual response, not delayed navigation or consumption', async () => {
  const context = {};
  const preparation = prepareAgentConnections(context, 'agt_1', 'alice');
  await act(async () => undefined);
  vi.mocked(performance.now).mockReturnValue(2500);
  preparation.handoff();
  const view = renderHook(() => useAgentConnectionReadiness('agt_1', 'alice', context, true));
  await waitFor(() => expect(view.result.current.verified).toEqual(ready));
  expect(view.result.current.takePrepared()).toBeUndefined();
  expect(connectionClient.check).toHaveBeenCalledOnce();
});

it.each(['owner', 'agent', 'client', 'access'] as const)(
  'discards the report on %s change, including the old callback',
  async (change) => {
    const view = await prepared();
    const old = view.result.current.takePrepared;
    view.rerender({
      owner: change === 'owner' ? 'bob' : 'alice',
      agent: change === 'agent' ? 'agt_2' : 'agt_1',
      client: change === 'client' ? {} : view.context,
      enabled: change !== 'access',
    });
    expect(old()).toBeUndefined();
    expect(view.result.current.takePrepared()).toBeUndefined();
    await act(async () => undefined);
  },
);

it.each(['blur', 'visibilitychange'] as const)(
  'discards readiness handoff after %s',
  async (event) => {
    const view = await prepared();
    fireEvent(event === 'blur' ? window : document, new Event(event));
    expect(view.result.current.takePrepared()).toBeUndefined();
  },
);

it('never replenishes a handoff from a pending reply after the view lost focus', async () => {
  let finish!: (report: ConnectionReport) => void;
  vi.mocked(connectionClient.check).mockReturnValueOnce(
    new Promise((resolve) => (finish = resolve)),
  );
  const context = {};
  prepareAgentConnections(context, 'agt_1', 'alice').handoff();
  const view = renderHook(() => useAgentConnectionReadiness('agt_1', 'alice', context, true));
  fireEvent(window, new Event('blur'));
  await act(async () => finish(ready));
  expect(view.result.current.verified).toEqual(ready);
  expect(view.result.current.takePrepared()).toBeUndefined();
});

it.each([
  { ...ready, owner: 'bob' },
  {
    ...ready,
    ready: false,
    services: [
      {
        service: 'provider',
        display_name: 'Provider',
        kind: 'codex' as const,
        state: 'missing' as const,
        connection_mode: 'hosted' as const,
      },
    ],
  },
])('does not offer an unverified or missing connection report', async (report) => {
  vi.mocked(connectionClient.check).mockResolvedValue(report);
  const context = {};
  prepareAgentConnections(context, 'agt_1', 'alice').handoff();
  const view = renderHook(() => useAgentConnectionReadiness('agt_1', 'alice', context, true));
  await act(async () => undefined);
  expect(view.result.current.verified).toBeUndefined();
  expect(view.result.current.takePrepared()).toBeUndefined();
  expect(report.ready ? view.result.current.error : view.result.current.needed).toBeTruthy();
});

it('keeps ordinary loading and wizard reports on the existing per-send verification path', async () => {
  const view = renderHook(() => useAgentConnectionReadiness('agt_1', 'alice', undefined, true));
  await waitFor(() => expect(view.result.current.verified).toEqual(ready));
  expect(view.result.current.takePrepared()).toBeUndefined();
  act(() => view.result.current.accept(ready));
  expect(view.result.current.takePrepared()).toBeUndefined();
});
