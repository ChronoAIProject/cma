import { act, render, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { AgentLifecycleOperationView } from '../api/client';
import type { AgentDetail } from '../api/types';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { harness } from '../agent/transport/controller.fixture';
import { loadAgent } from '../agent/loadAgent';
import { AgentLifecycleCard } from './AgentLifecycleCard';
import { lifecycleStorageKey } from './lifecycleIntent';
import { useLifecycleTransport } from './lifecycleTransport';
import { connectionClient } from '../connections/client';
import { readAgentConnections } from '../connections/preparation';

const operation: AgentLifecycleOperationView = {
  id: 'operation-one',
  status: 'running',
  phase: 'agent_startup',
  agent_id: 'agt_1',
  events: [],
};
function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((done) => (resolve = done));
  return { promise, resolve };
}
beforeEach(() => {
  sessionStorage.clear();
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'alice',
    ready: true,
    checked_at: new Date().toISOString(),
    services: [],
  });
});
afterEach(() => vi.restoreAllMocks());

it('overlaps actual-Agent preparation with lineage checks and transfers the pending selection across navigation unmount', async () => {
  const h = harness();
  const flag = deferred<Response>();
  const response = await h.eligibility();
  h.eligibility.mockClear();
  h.eligibility.mockReturnValueOnce(flag.promise);
  const detail = deferred<AgentDetail>();
  const readAgent = vi.fn(() => detail.promise);
  const readSandbox = vi.fn(async () => ({ ...sandboxFixture(), owner: 'alice' }));
  const client = {
    ...h.client,
    readAgent,
    readSandbox,
    readAgentLifecycle: vi.fn(async () => operation),
  };
  sessionStorage.setItem(
    lifecycleStorageKey('alice'),
    JSON.stringify({
      version: 1,
      intent: { key: 'same-key', body: { name: 'Agent' }, receipt: { id: operation.id } },
    }),
  );
  const prepare = vi.spyOn(client.agentTransport!, 'prepare');
  const onCreated = vi.fn();
  const view = render(
    <AgentLifecycleCard client={client} viewerId="alice" onClose={vi.fn()} onCreated={onCreated} />,
  );
  await waitFor(() => expect(h.eligibility).toHaveBeenCalledOnce());
  expect(readAgent).toHaveBeenCalledOnce();
  expect(readSandbox).not.toHaveBeenCalled();
  expect(prepare).toHaveBeenCalledExactlyOnceWith('agt_1', 'alice');
  expect(connectionClient.check).toHaveBeenCalledExactlyOnceWith(
    { type: 'agent', id: 'agt_1' },
    undefined,
    expect.any(AbortSignal),
  );
  expect(onCreated).not.toHaveBeenCalled();
  await act(async () => detail.resolve({ ...AGENT_DETAIL, status: 'starting' }));
  await waitFor(() =>
    expect(onCreated).toHaveBeenCalledExactlyOnceWith({ id: 'agt_1', status: 'starting' }),
  );
  view.unmount();
  const entryIdentity = deferred<string | undefined>();
  const finalIdentity = deferred<string | undefined>();
  h.currentUser
    .mockResolvedValueOnce('alice')
    .mockReturnValueOnce(entryIdentity.promise)
    .mockReturnValueOnce(finalIdentity.promise);
  const published = vi.fn();
  const loading = loadAgent(client, 'agt_1', new AbortController().signal).then((ready) => {
    published();
    return ready;
  });
  flag.resolve(response);
  await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(3));
  expect(readAgent).toHaveBeenCalledOnce();
  expect(h.snapshot).not.toHaveBeenCalled();
  expect(published).not.toHaveBeenCalled();
  entryIdentity.resolve('alice');
  await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(4));
  expect(readAgent).toHaveBeenCalledTimes(2);
  expect(h.snapshot).toHaveBeenCalledOnce();
  expect(published).not.toHaveBeenCalled();
  finalIdentity.resolve('alice');
  expect(await loading).toMatchObject({
    detail: { id: 'agt_1', status: 'starting' },
    v2: expect.anything(),
  });
  expect(h.eligibility).toHaveBeenCalledOnce();
  expect(h.currentUser).toHaveBeenCalledTimes(4);
  expect(published).toHaveBeenCalledOnce();
  expect(
    await readAgentConnections(client, 'agt_1', 'alice', new AbortController().signal),
  ).toMatchObject({ report: { owner: 'alice', ready: true }, prepared: true });
  expect(connectionClient.check).toHaveBeenCalledOnce();
});

it.each([
  ['no Agent', { ...operation, agent_id: undefined }, operation.id, 'alice'],
  ['different operation', operation, 'other-operation', 'alice'],
  ['failed operation', { ...operation, status: 'failed' }, operation.id, 'alice'],
  ['unknown viewer', operation, operation.id, undefined],
] as const)('does not prepare with %s', async (_reason, current, receipt, viewer) => {
  const h = harness();
  renderHook(() => useLifecycleTransport(h.client, current, receipt, viewer));
  await act(async () => undefined);
  expect(h.currentUser).not.toHaveBeenCalled();
  expect(h.eligibility).not.toHaveBeenCalled();
  expect(connectionClient.check).not.toHaveBeenCalled();
});

it('reuses one preparation across repeated operation polls and running-to-succeeded updates', () => {
  const h = harness();
  const release = vi.fn();
  const handoff = vi.fn();
  const prepare = vi
    .spyOn(h.client.agentTransport!, 'prepare')
    .mockReturnValue({ release, handoff });
  const view = renderHook(
    ({ current }) => useLifecycleTransport(h.client, current, operation.id, 'alice'),
    { initialProps: { current: operation } },
  );
  view.rerender({ current: { ...operation, events: [] } });
  view.rerender({ current: { ...operation, status: 'succeeded' } });
  expect(prepare).toHaveBeenCalledOnce();
  act(() => view.result.current());
  expect(handoff).toHaveBeenCalledOnce();
  view.unmount();
  expect(release).toHaveBeenCalledOnce();
});

it.each(['owner', 'client', 'operation', 'Agent', 'refusal', 'close'] as const)(
  'releases preparation when %s changes',
  (change) => {
    const h = harness();
    const release = vi.fn();
    const handoff = vi.fn();
    vi.spyOn(h.client.agentTransport!, 'prepare').mockReturnValue({ release, handoff });
    const view = renderHook(
      ({ client, current, receipt, viewer }) =>
        useLifecycleTransport(client, current, receipt, viewer),
      {
        initialProps: {
          client: h.client,
          current: operation,
          receipt: operation.id,
          viewer: 'alice',
        },
      },
    );
    if (change === 'close') view.unmount();
    else
      view.rerender({
        client: change === 'client' ? agentClient() : h.client,
        current:
          change === 'operation'
            ? { ...operation, id: 'new-operation' }
            : change === 'Agent'
              ? { ...operation, agent_id: 'other-agent' }
              : change === 'refusal'
                ? { ...operation, status: 'failed' }
                : operation,
        receipt: change === 'operation' ? 'new-operation' : operation.id,
        viewer: change === 'owner' ? 'bob' : 'alice',
      });
    expect(release).toHaveBeenCalledOnce();
    expect(handoff).not.toHaveBeenCalled();
  },
);
