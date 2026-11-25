import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { AgentLifecycleOperationView } from '../api/client';
import { agentClient } from '../test/agent';
import { connectionClient } from '../connections/client';
import { AgentLifecycleFollowup, type LifecycleFollowupView } from './AgentLifecycleFollowup';
import { lifecycleScopeForAgent, lifecycleStorageKey } from './lifecycleIntent';
import { lifecycleRecoveryKey } from './lifecycleIntentStorage';

const body = {
  name: 'Created agent',
  workspace_id: 'wsp_1',
  repository: { url: 'https://github.com/org/repo' },
};
const key = lifecycleStorageKey('owner', 'wsp_1');
const recoveryKey = lifecycleRecoveryKey(
  { owner: 'owner', workspaceId: 'wsp_1' },
  { agentId: 'agt_1', operationId: 'operation-one' },
);
const running: AgentLifecycleOperationView = {
  id: 'operation-one',
  status: 'running',
  phase: 'agent_startup',
  agent_id: 'agt_1',
  events: [],
};
beforeEach(() => {
  sessionStorage.clear();
  sessionStorage.setItem(
    key,
    JSON.stringify({
      version: 1,
      intent: { key: 'original-key', body, receipt: { id: 'operation-one' }, agentId: 'agt_1' },
    }),
  );
});
afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

it('continues observation after reload without resubmitting, and clears only confirmed success', async () => {
  let operation = running;
  const read = vi.fn(async () => operation);
  const create = vi.fn();
  const client = agentClient({ readAgentLifecycle: read, createAgentLifecycle: create });
  const first = render(<AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />);
  await waitFor(() => expect(read).toHaveBeenCalledOnce());
  expect(
    await screen.findByRole('region', { name: 'Agent provisioning progress' }),
  ).toBeInTheDocument();
  expect(screen.getByText('Sandbox')).toBeInTheDocument();
  expect(screen.getByText('Workspace')).toBeInTheDocument();
  expect(screen.getByText('Service access')).toBeInTheDocument();
  expect(screen.getByText('Agent startup')).toBeInTheDocument();
  expect(sessionStorage.getItem(key)).toBeNull();
  expect(sessionStorage.getItem(recoveryKey)).toContain('original-key');
  first.unmount();
  operation = { ...running, status: 'succeeded' };
  render(<AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />);
  await waitFor(() => expect(sessionStorage.getItem(recoveryKey)).toBeNull());
  expect(sessionStorage.getItem(key)).toBeNull();
  expect(create).not.toHaveBeenCalled();
});

it('suspends reads and stale recovery callbacks without losing the retained request or mounted child', async () => {
  let finish!: (operation: AgentLifecycleOperationView) => void;
  const read = vi.fn<
    (id: string, options?: { signal?: AbortSignal }) => Promise<AgentLifecycleOperationView>
  >(
    () =>
      new Promise<AgentLifecycleOperationView>((resolve) => {
        finish = resolve;
      }),
  );
  const create = vi.fn();
  const client = agentClient({ readAgentLifecycle: read, createAgentLifecycle: create });
  let observed: LifecycleFollowupView | undefined;
  const show = (enabled: boolean) => (
    <AgentLifecycleFollowup
      client={client}
      viewerId="owner"
      agentId="agt_1"
      enabled={enabled}
      render={(value) => {
        observed = value;
        return <input aria-label="Mounted draft" />;
      }}
    />
  );
  const view = render(show(true));
  await waitFor(() => expect(read).toHaveBeenCalledOnce());
  const input = screen.getByLabelText('Mounted draft');
  fireEvent.change(input, { target: { value: 'Unsent text' } });
  const stale = observed!.lifecycle!;
  const retained = sessionStorage.getItem(recoveryKey);
  view.rerender(show(false));
  expect(read.mock.calls[0]![1]!.signal!.aborted).toBe(true);
  await act(async () => {
    finish({ ...running, status: 'succeeded' });
    await stale.submit();
    stale.clear();
  });
  expect(observed!.lifecycle!.operation).toBeUndefined();
  expect(screen.getByLabelText('Mounted draft')).toBe(input);
  expect(input).toHaveValue('Unsent text');
  expect(sessionStorage.getItem(recoveryKey)).toBe(retained);
  expect(create).not.toHaveBeenCalled();
  view.rerender(show(true));
  await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  expect(screen.getByLabelText('Mounted draft')).toBe(input);
});

it('recovers a worker connection gate with the original creation body/key only after an explicit action', async () => {
  const target = { type: 'workspace' as const, id: 'wsp_1' };
  let resumed = false;
  const read = vi.fn(async () =>
    resumed
      ? { ...running, status: 'succeeded' }
      : {
          ...running,
          status: 'waiting_for_connection',
          connection: { services: ['api-github'], target },
        },
  );
  const create = vi.fn(async () => {
    resumed = true;
    return { id: running.id };
  });
  const check = vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'owner',
    ready: true,
    checked_at: '2026-10-14T00:00:00Z',
    services: [],
  });
  const client = agentClient({ readAgentLifecycle: read, createAgentLifecycle: create });
  const view = render(<AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />);
  await screen.findByRole('button', { name: 'Continue Agent setup' });
  expect(create).not.toHaveBeenCalled();
  expect(check).not.toHaveBeenCalled();
  view.unmount();
  render(<AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />);
  fireEvent.click(await screen.findByRole('button', { name: 'Continue Agent setup' }));
  await waitFor(() => expect(create).toHaveBeenCalledOnce());
  expect(create).toHaveBeenCalledWith(
    body,
    expect.objectContaining({ idempotencyKey: 'original-key' }),
  );
  expect(check).toHaveBeenCalledWith(target, undefined, expect.any(AbortSignal));
  await waitFor(() => expect(sessionStorage.getItem(recoveryKey)).toBeNull());
  expect(sessionStorage.getItem(key)).toBeNull();
});

it.each(['owner', 'agent', 'client', 'unmount'] as const)(
  'blocks a delayed resume check after a %s change',
  async (change) => {
    const target = { type: 'workspace' as const, id: 'wsp_1' };
    const read = vi.fn(async () => ({
      ...running,
      status: 'waiting_for_connection',
      connection: { services: ['api-github'], target },
    }));
    const create = vi.fn();
    let resolve!: (value: Awaited<ReturnType<typeof connectionClient.check>>) => void;
    const check = vi.spyOn(connectionClient, 'check').mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const client = agentClient({ readAgentLifecycle: read, createAgentLifecycle: create });
    const view = render(
      <AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />,
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Continue Agent setup' }));
    await waitFor(() => expect(check).toHaveBeenCalledOnce());
    if (change === 'unmount') view.unmount();
    else
      view.rerender(
        <AgentLifecycleFollowup
          client={change === 'client' ? agentClient() : client}
          viewerId={change === 'owner' ? 'someone-else' : 'owner'}
          agentId={change === 'agent' ? 'another-agent' : 'agt_1'}
        />,
      );
    expect(check.mock.calls[0]?.[2]?.aborted).toBe(true);
    await act(async () =>
      resolve({ owner: 'owner', ready: true, checked_at: '2026-10-14T00:00:00Z', services: [] }),
    );
    expect(create).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(key)).toBeNull();
    expect(sessionStorage.getItem(recoveryKey)).toContain('original-key');
  },
);

it('refuses mismatched operation lineage and preserves the recoverable record', async () => {
  const create = vi.fn();
  render(
    <AgentLifecycleFollowup
      client={agentClient({
        readAgentLifecycle: async () => ({
          ...running,
          agent_id: 'another-agent',
          status: 'succeeded',
        }),
        createAgentLifecycle: create,
      })}
      viewerId="owner"
      agentId="agt_1"
    />,
  );
  await screen.findByText('Agent setup identity could not be verified.');
  expect(sessionStorage.getItem(key)).toBeNull();
  expect(sessionStorage.getItem(recoveryKey)).toContain('original-key');
  expect(create).not.toHaveBeenCalled();
});

it('does not discover another owner, unbound intent, or a mismatched storage scope', () => {
  expect(lifecycleScopeForAgent('someone-else', 'agt_1')).toBeUndefined();
  expect(lifecycleScopeForAgent('owner', 'another-agent')).toBeUndefined();
  const raw = sessionStorage.getItem(key)!;
  sessionStorage.clear();
  sessionStorage.setItem(lifecycleStorageKey('owner'), raw);
  expect(lifecycleScopeForAgent('owner', 'agt_1')).toBeUndefined();
});
