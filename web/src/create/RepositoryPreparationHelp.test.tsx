import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AgentLifecycleOperationView } from '../api/client';
import { connectionClient } from '../connections/client';
import { AGENT_DETAIL } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { AgentLifecycleFollowup } from './AgentLifecycleFollowup';
import { RepositoryPreparationHelp } from './RepositoryPreparationHelp';
import {
  confirmedConnections,
  repositoryClient,
  repositoryFailure,
  retainedRepositoryTask,
} from './repositoryFailure.fixtures';

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
  sessionStorage.clear();
});

async function checkButton() {
  const button = await screen.findByRole('button', { name: 'Check repository connection' });
  await waitFor(() => expect(button).toBeEnabled());
  return button;
}

it('checks a confirmed repository failure without resubmitting or changing its retained identity', async () => {
  const retained = retainedRepositoryTask();
  const client = repositoryClient();
  const check = vi.spyOn(connectionClient, 'check').mockResolvedValue(confirmedConnections);
  const first = render(<AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />);
  const button = await checkButton();
  expect(check).not.toHaveBeenCalled();
  fireEvent.click(button);
  expect(await screen.findByText(/Connections confirmed. The original repository/)).toBeVisible();
  expect(check).toHaveBeenCalledExactlyOnceWith(
    { type: 'agent', id: 'agt_1' },
    undefined,
    expect.any(AbortSignal),
  );
  expect(screen.getByRole('link', { name: 'Manage this Sandbox' })).toHaveAttribute(
    'href',
    '/sandboxes/sbx_1',
  );
  expect(client.createAgentLifecycle).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(retained.key)).toBe(retained.raw);
  first.unmount();
  render(<AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />);
  await checkButton();
  expect(check).toHaveBeenCalledTimes(1);
  expect(client.createAgentLifecycle).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(retained.key)).toBe(retained.raw);
});

it.each(['missing', 'inactive'] as const)(
  'uses the existing wizard for %s connections and never resumes failed setup',
  async (state) => {
    const retained = retainedRepositoryTask();
    const client = repositoryClient();
    let report = {
      ...confirmedConnections,
      ready: false,
      services: [
        {
          service: 'api-github',
          display_name: 'GitHub',
          kind: 'nyxid' as const,
          state: state as 'missing' | 'inactive' | 'satisfied',
          connection_mode: 'hosted' as const,
        },
      ],
    };
    const check = vi.spyOn(connectionClient, 'check').mockImplementation(async () => report);
    const start = vi.spyOn(connectionClient, 'start');
    render(<AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />);
    fireEvent.click(await checkButton());
    const dialog = await screen.findByRole('dialog', { name: 'Connect your accounts' });
    expect(
      within(dialog).getByText(/this check does not retry repository preparation/),
    ).toBeVisible();
    expect(within(dialog).queryByText(/continues opening your Sandbox/)).not.toBeInTheDocument();
    expect(start).not.toHaveBeenCalled();
    expect(client.createAgentLifecycle).not.toHaveBeenCalled();
    report = { ...report, ready: true, services: [{ ...report.services[0]!, state: 'satisfied' }] };
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument(), {
      timeout: 3500,
    });
    expect(await screen.findByText(/Connections confirmed. The original repository/)).toBeVisible();
    expect(check).toHaveBeenCalled();
    expect(client.createAgentLifecycle).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(retained.key)).toBe(retained.raw);
  },
);

it.each(['unknown', 'error'] as const)(
  'keeps %s checks retryable without showing raw errors or writing lifecycle state',
  async (state) => {
    const retained = retainedRepositoryTask();
    const client = repositoryClient();
    const check = vi.spyOn(connectionClient, 'check');
    if (state === 'error') check.mockRejectedValueOnce(new Error('private upstream diagnostic'));
    else
      check.mockResolvedValueOnce({
        ...confirmedConnections,
        ready: false,
        services: [
          {
            service: 'api-github',
            display_name: 'GitHub',
            kind: 'nyxid',
            state: 'unknown',
            connection_mode: 'unknown',
          },
        ],
      });
    check.mockResolvedValue(confirmedConnections);
    render(<AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />);
    fireEvent.click(await checkButton());
    expect(await screen.findByText(/connection check could not be confirmed/)).toBeVisible();
    expect(
      screen.queryByText(/private upstream diagnostic|try sending again/),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    fireEvent.click(await checkButton());
    expect(await screen.findByText(/Connections confirmed. The original repository/)).toBeVisible();
    expect(client.createAgentLifecycle).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(retained.key)).toBe(retained.raw);
  },
);

it.each(['owner', 'agent', 'client', 'unmount'] as const)(
  'ignores a late check after %s changes',
  async (change) => {
    const retained = retainedRepositoryTask();
    const client = repositoryClient();
    let resolve!: (value: typeof confirmedConnections) => void;
    const check = vi.spyOn(connectionClient, 'check').mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const view = render(
      <AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />,
    );
    fireEvent.click(await checkButton());
    if (change === 'unmount') view.unmount();
    else
      view.rerender(
        <AgentLifecycleFollowup
          client={change === 'client' ? repositoryClient() : client}
          viewerId={change === 'owner' ? 'other' : 'owner'}
          agentId={change === 'agent' ? 'other-agent' : 'agt_1'}
        />,
      );
    expect(check.mock.calls[0]?.[2]?.aborted).toBe(true);
    await act(async () => resolve(confirmedConnections));
    expect(screen.queryByText(/Connections confirmed/)).not.toBeInTheDocument();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(client.createAgentLifecycle).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(retained.key)).toBe(retained.raw);
  },
);

it.each(['owner', 'agent', 'worktree', 'sandbox', 'workspace'] as const)(
  'refuses mismatched %s authority before checking connections',
  async (changed) => {
    retainedRepositoryTask();
    const check = vi.spyOn(connectionClient, 'check');
    const client = repositoryClient({
      readAgent: async () => ({
        ...AGENT_DETAIL,
        ...(changed === 'agent' ? { id: 'another-agent' } : {}),
        worktree: {
          ...AGENT_DETAIL.worktree,
          ...(changed === 'worktree' ? { agentId: 'another-agent' } : {}),
          ...(changed === 'workspace' ? { workspaceId: 'another-workspace' } : {}),
        },
      }),
      readSandbox: async () => ({
        ...sandboxFixture(),
        owner: 'owner',
        ...(changed === 'owner' ? { owner: 'foreign-owner' } : {}),
        ...(changed === 'sandbox' ? { id: 'another-sandbox' } : {}),
      }),
    });
    render(<AgentLifecycleFollowup client={client} viewerId="owner" agentId="agt_1" />);
    expect(await screen.findByText(/resource for this check could not be verified/)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Check repository connection' })).toBeDisabled();
    expect(screen.queryByRole('link', { name: 'Manage this Sandbox' })).not.toBeInTheDocument();
    expect(check).not.toHaveBeenCalled();
  },
);

it('does not open a wizard for a connection report belonging to another owner', async () => {
  retainedRepositoryTask();
  const check = vi.spyOn(connectionClient, 'check').mockResolvedValue({
    ...confirmedConnections,
    owner: 'foreign-owner',
    ready: false,
    services: [
      {
        service: 'private-service',
        display_name: 'Private service name',
        kind: 'nyxid',
        state: 'missing',
        connection_mode: 'hosted',
      },
    ],
  });
  render(<AgentLifecycleFollowup client={repositoryClient()} viewerId="owner" agentId="agt_1" />);
  fireEvent.click(await checkButton());
  expect(await screen.findByText(/connection check could not be confirmed/)).toBeVisible();
  expect(check).toHaveBeenCalledOnce();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.queryByText(/Private service name/)).not.toBeInTheDocument();
});

it.each(['receipt', 'agent', 'phase', 'status', 'repository'] as const)(
  'requires a matching terminal repository operation: %s',
  (changed) => {
    const { intent } = retainedRepositoryTask();
    const operation: AgentLifecycleOperationView = {
      ...repositoryFailure,
      ...(changed === 'receipt' ? { id: 'other-receipt' } : {}),
      ...(changed === 'agent' ? { agent_id: 'other-agent' } : {}),
      ...(changed === 'phase' ? { events: [] } : {}),
      ...(changed === 'status' ? { status: 'running' } : {}),
    };
    render(
      <RepositoryPreparationHelp
        client={repositoryClient()}
        viewerId="owner"
        intent={changed === 'repository' ? { ...intent, body: { name: 'No repository' } } : intent}
        operation={operation}
      />,
    );
    expect(
      screen.queryByRole('region', { name: 'Repository preparation help' }),
    ).not.toBeInTheDocument();
  },
);

it('treats a non-connection workspace failure as neutral help without inferring its cause', async () => {
  retainedRepositoryTask();
  render(
    <AgentLifecycleFollowup
      client={repositoryClient({
        readAgentLifecycle: async () => ({
          ...repositoryFailure,
          error: 'Repository archive could not be extracted.',
        }),
      })}
      viewerId="owner"
      agentId="agt_1"
    />,
  );
  await checkButton();
  expect(screen.getByText(/A connection check may help identify the next step/)).toBeVisible();
  expect(screen.queryByText(/Your GitHub authorization is missing/)).not.toBeInTheDocument();
});

it('bounds a stalled resource read and ignores its result after an explicit retry', async () => {
  vi.useFakeTimers();
  const { intent } = retainedRepositoryTask();
  let resolve!: (value: typeof AGENT_DETAIL) => void;
  const readAgent = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    )
    .mockResolvedValue(AGENT_DETAIL);
  const client = repositoryClient({ readAgent });
  render(
    <RepositoryPreparationHelp
      client={client}
      viewerId="owner"
      intent={intent}
      operation={repositoryFailure}
    />,
  );
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(readAgent.mock.calls[0]?.[1]?.signal.aborted).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Retry resource check' }));
  await act(async () => {});
  expect(screen.getByRole('button', { name: 'Check repository connection' })).toBeEnabled();
  await act(async () => resolve({ ...AGENT_DETAIL, id: 'foreign-agent' }));
  expect(screen.getByRole('button', { name: 'Check repository connection' })).toBeEnabled();
  expect(readAgent).toHaveBeenCalledTimes(2);
  expect(client.createAgentLifecycle).not.toHaveBeenCalled();
});
