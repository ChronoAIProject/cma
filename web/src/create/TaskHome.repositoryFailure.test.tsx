import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { CmaWebClient } from '../api/client';
import { connectionClient, type ConnectionReport } from '../connections/client';
import { AGENT_DETAIL } from '../test/agent';
import { lifecycleStorageKey } from './lifecycleIntent';
import { TaskHome } from './TaskHome';
import {
  confirmedConnections,
  repositoryClient,
  repositoryFailure,
  repositoryTask,
  retainedRepositoryTask,
} from './repositoryFailure.fixtures';

afterEach(() => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
  sessionStorage.clear();
});

function home(client: CmaWebClient, viewerId = 'owner', workspaceId?: string) {
  return (
    <TaskHome
      client={client}
      viewerId={viewerId}
      workspaceId={workspaceId}
      canCreateSandbox
      onCreated={vi.fn()}
    />
  );
}

async function button() {
  const check = await screen.findByRole('button', { name: 'Check repository connection' });
  await waitFor(() => expect(check).toBeEnabled());
  return check;
}

it('keeps the submitted task, repository and original key across connection checks and reopening', async () => {
  const retained = retainedRepositoryTask('owner', false);
  const client = repositoryClient();
  const check = vi.spyOn(connectionClient, 'check').mockResolvedValue(confirmedConnections);
  const view = render(home(client));
  const action = await button();
  const task = screen.getByRole('region', { name: 'Original request' });
  expect(task).toHaveTextContent(repositoryTask.initial_message);
  fireEvent.click(screen.getByText('Setup details'));
  expect(screen.getByRole('region', { name: 'Requested repository' })).toHaveTextContent(
    'example/project',
  );
  expect(screen.getByRole('region', { name: 'Requested repository' })).toHaveTextContent('main');
  expect(check).not.toHaveBeenCalled();
  fireEvent.click(action);
  expect(await screen.findByText(/Connections confirmed. The original repository/)).toBeVisible();
  expect(sessionStorage.getItem(retained.key)).toBe(retained.raw);
  expect(client.createAgentLifecycle).not.toHaveBeenCalled();
  view.unmount();
  render(home(client));
  await button();
  expect(check).toHaveBeenCalledTimes(1);
  expect(client.createAgentLifecycle).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(retained.key)).toBe(retained.raw);
});

it('explains an unconfirmed target without guessing an Agent, Workspace or Sandbox', async () => {
  const retained = retainedRepositoryTask('owner', false);
  const readAgent = vi.fn();
  const readWorkspace = vi.fn();
  const readSandbox = vi.fn();
  const client = repositoryClient({
    readAgent,
    readWorkspace,
    readSandbox,
    readAgentLifecycle: async () => ({ ...repositoryFailure, agent_id: undefined }),
  });
  render(home(client));
  expect(await screen.findByText(/has not confirmed an Agent or Workspace/)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Check repository connection' })).toBeDisabled();
  expect(readAgent).not.toHaveBeenCalled();
  expect(readWorkspace).not.toHaveBeenCalled();
  expect(readSandbox).not.toHaveBeenCalled();
  expect(client.createAgentLifecycle).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(retained.key)).toBe(retained.raw);
});

it('checks the retained Workspace only after reading and verifying its Sandbox owner', async () => {
  const key = lifecycleStorageKey('owner', 'wsp_1');
  const raw = JSON.stringify({
    version: 1,
    intent: {
      key: 'original-workspace-task',
      body: { ...repositoryTask, workspace_id: 'wsp_1' },
      receipt: { id: repositoryFailure.id },
    },
  });
  sessionStorage.setItem(key, raw);
  const readAgent = vi.fn();
  const client = repositoryClient({
    readAgent,
    readAgentLifecycle: async () => ({ ...repositoryFailure, agent_id: undefined }),
  });
  const check = vi.spyOn(connectionClient, 'check').mockResolvedValue(confirmedConnections);
  render(home(client, 'owner', 'wsp_1'));
  fireEvent.click(await button());
  expect(await screen.findByText(/Connections confirmed. The original repository/)).toBeVisible();
  expect(check).toHaveBeenCalledWith(
    { type: 'workspace', id: 'wsp_1' },
    undefined,
    expect.any(AbortSignal),
  );
  expect(readAgent).not.toHaveBeenCalled();
  expect(client.createAgentLifecycle).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(key)).toBe(raw);
});

it('allows an explicit resource-read retry without changing or resending the task', async () => {
  const retained = retainedRepositoryTask('owner', false);
  const readAgent = vi
    .fn()
    .mockRejectedValueOnce(new Error('private read diagnostic'))
    .mockResolvedValue(AGENT_DETAIL);
  const client = repositoryClient({ readAgent });
  render(home(client));
  const retry = await screen.findByRole('button', { name: 'Retry resource check' });
  expect(screen.queryByText(/private read diagnostic/)).not.toBeInTheDocument();
  expect(readAgent).toHaveBeenCalledTimes(1);
  fireEvent.click(retry);
  await button();
  expect(readAgent).toHaveBeenCalledTimes(2);
  expect(client.createAgentLifecycle).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(retained.key)).toBe(retained.raw);
});

it('discards target verification when the viewer changes', async () => {
  const retained = retainedRepositoryTask('owner', false);
  let resolve!: (value: typeof AGENT_DETAIL) => void;
  const readAgent = vi.fn(
    () =>
      new Promise<typeof AGENT_DETAIL>((done) => {
        resolve = done;
      }),
  );
  const client = repositoryClient({ readAgent });
  const check = vi.spyOn(connectionClient, 'check');
  const view = render(home(client));
  await waitFor(() => expect(readAgent).toHaveBeenCalledOnce());
  view.rerender(home(client, 'other-owner'));
  await act(async () => resolve(AGENT_DETAIL));
  expect(
    screen.queryByRole('region', { name: 'Repository preparation help' }),
  ).not.toBeInTheDocument();
  expect(screen.queryByText(repositoryTask.initial_message)).not.toBeInTheDocument();
  expect(check).not.toHaveBeenCalled();
  expect(client.createAgentLifecycle).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(retained.key)).toBe(retained.raw);
});

it('cancels the wizard and ignores its late verification result', async () => {
  const retained = retainedRepositoryTask('owner', false);
  const client = repositoryClient();
  const missing = {
    ...confirmedConnections,
    ready: false,
    services: [
      {
        service: 'api-github',
        display_name: 'GitHub',
        kind: 'nyxid' as const,
        state: 'missing' as const,
        connection_mode: 'hosted' as const,
      },
    ],
  };
  const check = vi.spyOn(connectionClient, 'check').mockResolvedValue(missing);
  render(home(client));
  fireEvent.click(await button());
  const dialog = await screen.findByRole('dialog');
  let resolve!: (value: typeof confirmedConnections) => void;
  check.mockImplementation(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  await waitFor(() => expect(check).toHaveBeenCalledTimes(3), { timeout: 3500 });
  fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  await act(async () => resolve(confirmedConnections));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.queryByText(/Connections confirmed/)).not.toBeInTheDocument();
  await button();
  expect(client.createAgentLifecycle).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(retained.key)).toBe(retained.raw);
});

it('completes an owner-bound sign-in handoff without retrying the failed operation', async () => {
  const retained = retainedRepositoryTask('owner', false);
  const client = repositoryClient();
  const popup = {
    closed: false,
    opener: window,
    location: { href: '' },
    focus: vi.fn(),
    close: vi.fn(),
  };
  vi.spyOn(window, 'open').mockReturnValue(popup as unknown as Window);
  let connected = false;
  const check = vi.spyOn(connectionClient, 'check').mockImplementation(async () => ({
    ...confirmedConnections,
    ready: connected,
    services: [
      {
        service: 'api-github',
        display_name: 'GitHub',
        kind: 'nyxid',
        state: connected ? 'satisfied' : 'missing',
        connection_mode: 'hosted',
      },
    ],
  }));
  const start = vi.spyOn(connectionClient, 'start').mockImplementation(async (_target, active) => ({
    ...active,
    owner: 'owner',
    hosted_url: 'https://example.test/connect',
    expires_at: new Date(Date.now() + 60_000).toISOString(),
  }));
  render(home(client));
  fireEvent.click(await button());
  const dialog = await screen.findByRole('dialog');
  fireEvent.click(within(dialog).getByRole('button', { name: 'Connect all' }));
  await waitFor(() => expect(popup.location.href).toBe('https://example.test/connect'));
  const active = start.mock.calls[0]![1];
  expect(start.mock.calls[0]![0]).toEqual({ type: 'agent', id: 'agt_1' });
  connected = true;
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument(), {
    timeout: 3500,
  });
  expect(check.mock.calls.some((call) => call[1]?.intent_id === active.intent_id)).toBe(true);
  expect(screen.getByText(/Connections confirmed. The original repository/)).toBeVisible();
  expect(client.createAgentLifecycle).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(retained.key)).toBe(retained.raw);
});

it.each([
  ['failed preparation', 'Check repository connection'],
  ['waiting task', 'Continue original task'],
] as const)('offers existing GitHub connections for a %s', async (state, action) => {
  vi.stubEnv('VITE_CONNECTION_INTENT_RECOVERY_ENABLED', 'true');
  const retained = retainedRepositoryTask('owner', false);
  const client =
    state === 'failed preparation'
      ? repositoryClient()
      : repositoryClient({
          readAgentLifecycle: async () => ({
            ...repositoryFailure,
            status: 'waiting_for_connection',
            connection: {
              services: ['api-github'],
              target: { type: 'agent', id: 'agt_1' },
            },
          }),
        });
  const check = vi.spyOn(connectionClient, 'check').mockResolvedValue({
    ...confirmedConnections,
    ready: false,
    services: [
      {
        service: 'api-github',
        display_name: 'GitHub',
        kind: 'nyxid',
        state: 'missing',
        connection_mode: 'manual',
      },
    ],
  });
  const start = vi.spyOn(connectionClient, 'start');
  vi.stubGlobal(
    'fetch',
    vi.fn(async () =>
      Response.json({
        owner: 'owner',
        github: {
          manage_url: 'https://example.test/keys',
          connections: [
            {
              id: 'github-existing',
              label: 'Existing GitHub account',
              manage_url: 'https://example.test/keys/github-existing',
              granted_scopes: ['repo'],
            },
          ],
        },
      }),
    ),
  );
  try {
    render(home(client));
    fireEvent.click(await screen.findByRole('button', { name: action }));
    const dialog = await screen.findByRole('dialog');
    expect(
      await within(dialog).findByRole('link', { name: /Existing GitHub account/ }),
    ).toHaveAttribute('href', 'https://example.test/keys/github-existing');
    expect(within(dialog).queryByRole('button', { name: 'Connect all' })).not.toBeInTheDocument();
    let complete!: (report: ConnectionReport) => void;
    check.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }),
    );
    fireEvent.click(within(dialog).getByRole('button', { name: 'Verify again' }));
    await waitFor(() => expect(complete).toBeTypeOf('function'));
    await act(async () => complete(confirmedConnections));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(start).not.toHaveBeenCalled();
    expect(client.createAgentLifecycle).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(retained.key)).toBe(retained.raw);
  } finally {
    vi.unstubAllGlobals();
  }
});
