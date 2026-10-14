import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { creationClient } from '../test/creation';
import type { AgentLifecycleOperationView, CmaWebClient } from '../api/client';
import { connectionClient, type ConnectionReport } from '../connections/client';
import { AgentLifecycleCard } from './AgentLifecycleCard';
import { lifecycleStorageKey } from './lifecycleIntent';
import { CreationFlow } from './CreationFlow';

beforeEach(() => sessionStorage.clear());
afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});
const running: AgentLifecycleOperationView = {
  id: 'operation-one',
  status: 'running',
  phase: 'sandbox_ready',
  events: [
    {
      sequence: 2,
      phase: 'sandbox_ready',
      status: 'ready',
      message: 'Sandbox ready',
      occurred_at: '2026-10-13T00:00:00Z',
    },
  ],
};
function show(client: CmaWebClient, viewerId = 'owner', onCreated = vi.fn()) {
  return render(
    <AgentLifecycleCard
      client={client}
      viewerId={viewerId}
      onClose={vi.fn()}
      onCreated={onCreated}
    />,
  );
}
async function review() {
  const user = userEvent.setup();
  await user.type(
    screen.getByLabelText('GitHub repository (optional)'),
    'https://github.com/org/Research',
  );
  return user;
}
it('creates from only the optional repository and retries the exact request after remount', async () => {
  const create = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('connection lost'))
    .mockResolvedValue({ id: running.id });
  const client = agentClient({
    createAgentLifecycle: create,
    readAgentLifecycle: async () => running,
  });
  const first = show(client);
  const user = await review();
  await user.click(screen.getByRole('button', { name: 'Create agent' }));
  await screen.findByText('connection lost');
  const [body, options] = create.mock.calls[0]!;
  expect(body).toEqual({
    name: 'Research',
    repository: { url: 'https://github.com/org/Research' },
  });
  expect(screen.queryByLabelText('GitHub repository (optional)')).not.toBeInTheDocument();
  first.unmount();
  show(client);
  await user.click(screen.getByRole('button', { name: 'Recover creation' }));
  await screen.findByRole('region', { name: 'Agent provisioning progress' });
  expect(create.mock.calls[1]?.[0]).toEqual(body);
  expect(create.mock.calls[1]?.[1].idempotencyKey).toEqual(options.idempotencyKey);
  expect(screen.getByRole('button', { name: 'Open agent' })).toBeDisabled();
});
it('resumes receipt polling after reload, waits for final success and clears the saved request on opening', async () => {
  sessionStorage.setItem(
    lifecycleStorageKey('owner'),
    JSON.stringify({
      version: 1,
      intent: { key: 'same-key', body: { name: 'Research' }, receipt: { id: running.id } },
    }),
  );
  const read = vi
    .fn()
    .mockResolvedValueOnce({ ...running, agent_id: 'starting-agent' })
    .mockResolvedValue({ ...running, status: 'succeeded', agent_id: 'ready-agent' });
  const create = vi.fn();
  const onCreated = vi.fn();
  show(agentClient({ createAgentLifecycle: create, readAgentLifecycle: read }), 'owner', onCreated);
  await waitFor(() => expect(read).toHaveBeenCalledOnce());
  expect(screen.getByRole('button', { name: 'Open agent' })).toBeDisabled();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Open agent' })).toBeEnabled(), {
    timeout: 2500,
  });
  fireEvent.click(screen.getByRole('button', { name: 'Open agent' }));
  expect(onCreated).toHaveBeenCalledWith({ id: 'ready-agent', status: 'ready' });
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBeNull();
  expect(create).not.toHaveBeenCalled();
});
it('keeps owner recovery separate and refuses an unpersisted request', async () => {
  sessionStorage.setItem(
    lifecycleStorageKey('another-owner'),
    JSON.stringify({ version: 1, intent: { key: 'other-key', body: { name: 'Other agent' } } }),
  );
  const create = vi.fn();
  show(agentClient({ createAgentLifecycle: create }));
  const user = await review();
  vi.spyOn(Object.getPrototypeOf(sessionStorage), 'setItem').mockImplementation(() => {
    throw new Error('Storage full');
  });
  await user.click(screen.getByRole('button', { name: 'Create agent' }));
  await screen.findByRole('alert');
  expect(create).not.toHaveBeenCalled();
});
it('cancels a connection intent on Escape, then verifies and resumes the same admitted operation', async () => {
  const target = { type: 'workspace' as const, id: 'owned-workspace' };
  sessionStorage.setItem(
    lifecycleStorageKey('owner'),
    JSON.stringify({
      version: 1,
      intent: { key: 'resume-key', body: { name: 'Research' }, receipt: { id: running.id } },
    }),
  );
  const report: ConnectionReport = {
    owner: 'owner',
    ready: false,
    checked_at: '2026-10-13T00:00:00Z',
    services: [
      {
        service: 'api-github',
        display_name: 'GitHub',
        kind: 'nyxid',
        state: 'missing',
        connection_mode: 'hosted',
      },
    ],
  };
  const check = vi.spyOn(connectionClient, 'check').mockResolvedValue(report);
  const start = vi.spyOn(connectionClient, 'start').mockImplementation(async (_target, active) => ({
    ...active,
    owner: 'owner',
    hosted_url: 'https://identity.example/connect',
    expires_at: new Date(Date.now() + 600000).toISOString(),
  }));
  const cancel = vi.spyOn(connectionClient, 'cancel').mockResolvedValue();
  vi.spyOn(window, 'open').mockReturnValue({
    location: { href: '' },
    close: vi.fn(),
    focus: vi.fn(),
    closed: false,
    opener: null,
  } as unknown as Window);
  const create = vi.fn().mockResolvedValue({ id: running.id });
  const read = vi
    .fn()
    .mockResolvedValueOnce({
      ...running,
      status: 'waiting_for_connection',
      connection: { services: ['api-github'], target },
    })
    .mockResolvedValue(running);
  show(agentClient({ createAgentLifecycle: create, readAgentLifecycle: read }));
  const user = userEvent.setup();
  await screen.findByRole('dialog', { name: 'Connect required services' });
  expect(screen.queryByRole('dialog', { name: 'New agent' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Connect all' }));
  await waitFor(() => expect(start).toHaveBeenCalledOnce());
  await user.keyboard('{Escape}');
  await waitFor(() => expect(cancel).toHaveBeenCalledOnce());
  expect(create).not.toHaveBeenCalled();
  check.mockResolvedValue({
    ...report,
    ready: true,
    services: report.services.map((row) => ({ ...row, state: 'satisfied' })),
  });
  await user.click(await screen.findByRole('button', { name: 'Connect services' }));
  await waitFor(() => expect(create).toHaveBeenCalledOnce());
  expect(create).toHaveBeenCalledWith(
    { name: 'Research' },
    expect.objectContaining({ idempotencyKey: 'resume-key' }),
  );
});
it('reports status failures and permits retry without submitting creation again', async () => {
  sessionStorage.setItem(
    lifecycleStorageKey('owner'),
    JSON.stringify({
      version: 1,
      intent: { key: 'same-key', body: { name: 'Research' }, receipt: { id: running.id } },
    }),
  );
  const read = vi
    .fn()
    .mockRejectedValueOnce(new Error('Read interrupted'))
    .mockResolvedValue(running);
  const create = vi.fn();
  show(agentClient({ createAgentLifecycle: create, readAgentLifecycle: read }));
  await screen.findByText('Read interrupted');
  fireEvent.click(screen.getByRole('button', { name: 'Retry status' }));
  await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  expect(create).not.toHaveBeenCalled();
});

it('keeps contextual Agent creation in the selected workspace and recovers only that target', async () => {
  const create = vi
    .fn()
    .mockRejectedValueOnce(new Error('Lost create response'))
    .mockResolvedValue({ id: running.id });
  const client = agentClient({
    readWorkspace: async () => workspaceFixture(),
    createAgentLifecycle: create,
    readAgentLifecycle: async () => running,
  });
  const draw = () =>
    render(
      <CreationFlow
        request={{ kind: 'new_agent', workspaceId: 'wsp_1' }}
        client={client}
        viewerId="owner"
        catalog={creationClient()}
        onClose={vi.fn()}
        onCreated={vi.fn()}
        onChanged={vi.fn()}
        canCreateSandbox={false}
      />,
    );
  const first = draw();
  expect(await screen.findByText(/Existing Sandbox · chrono-bot/)).toBeInTheDocument();
  expect(screen.queryByRole('group', { name: 'Runtime image' })).not.toBeInTheDocument();
  const user = await review();
  await user.click(screen.getByRole('button', { name: 'Create agent' }));
  await screen.findByText('Lost create response');
  const [body, options] = create.mock.calls[0]!;
  expect(body.workspace_id).toBe('wsp_1');
  expect(body).not.toHaveProperty('image');
  expect(body.repository).toEqual({ url: 'https://github.com/org/Research' });
  expect(body).not.toHaveProperty('profile');
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBeNull();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner', 'wsp_1'))).toContain('wsp_1');
  first.unmount();
  const global = show(client);
  expect(screen.getByLabelText('GitHub repository (optional)')).toHaveValue('');
  expect(screen.queryByRole('button', { name: 'Recover creation' })).not.toBeInTheDocument();
  global.unmount();
  draw();
  await user.click(screen.getByRole('button', { name: 'Recover creation' }));
  await screen.findByRole('region', { name: 'Agent provisioning progress' });
  expect(create.mock.calls[1]?.[0]).toEqual(body);
  expect(create.mock.calls[1]?.[1].idempotencyKey).toBe(options.idempotencyKey);
});

it('isolates a global creation from contextual creation and rejects a mismatched stored target', async () => {
  const global = JSON.stringify({
    version: 1,
    intent: { key: 'global-key', body: { name: 'Global' } },
  });
  sessionStorage.setItem(lifecycleStorageKey('owner'), global);
  const create = vi.fn();
  const client = agentClient({ createAgentLifecycle: create });
  const first = render(
    <AgentLifecycleCard
      client={client}
      viewerId="owner"
      workspaceId="wsp_1"
      onClose={vi.fn()}
      onCreated={vi.fn()}
    />,
  );
  expect(screen.getByLabelText('GitHub repository (optional)')).toHaveValue('');
  expect(screen.queryByText(/Creation of “Global”/)).not.toBeInTheDocument();
  first.unmount();
  sessionStorage.setItem(lifecycleStorageKey('owner', 'wsp_1'), global);
  render(
    <AgentLifecycleCard
      client={client}
      viewerId="owner"
      workspaceId="wsp_1"
      onClose={vi.fn()}
      onCreated={vi.fn()}
    />,
  );
  expect(screen.getByRole('alert')).toHaveTextContent('saved Agent request could not be read');
  expect(screen.getByRole('button', { name: 'Create agent' })).toBeDisabled();
  expect(create).not.toHaveBeenCalled();
  await waitFor(() =>
    expect(screen.getByText(/Existing Sandbox · chrono-bot/)).toBeInTheDocument(),
  );
});

it('uses server defaults without image, workspace or profile selectors and allows an empty repository', async () => {
  const create = vi.fn().mockResolvedValue({ id: running.id });
  const listSandboxes = vi.fn();
  show(
    agentClient({
      listSandboxes,
      createAgentLifecycle: create,
      readAgentLifecycle: async () => running,
    }),
  );
  expect(screen.getAllByRole('textbox')).toHaveLength(1);
  expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
  for (const label of [
    'Agent name',
    'Model',
    'Reasoning effort',
    'Permission mode',
    'System prompt',
  ])
    expect(screen.queryByLabelText(label)).not.toBeInTheDocument();
  expect(screen.queryByText('Detailed settings')).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Create agent' }));
  await waitFor(() => expect(create).toHaveBeenCalledOnce());
  expect(create.mock.calls[0]?.[0]).toEqual({ name: 'New agent' });
  expect(listSandboxes).not.toHaveBeenCalled();
});

it('retains the contextual Sandbox and keeps its recovery separate from global creation', async () => {
  const create = vi.fn().mockRejectedValue(new Error('Lost response'));
  const client = agentClient({ createAgentLifecycle: create });
  const draw = () =>
    render(
      <CreationFlow
        request={{ kind: 'new_agent', sandboxId: 'sbx_1' }}
        client={client}
        viewerId="owner"
        catalog={creationClient()}
        onClose={vi.fn()}
        onCreated={vi.fn()}
        onChanged={vi.fn()}
        canCreateSandbox={false}
      />,
    );
  const first = draw();
  const user = userEvent.setup();
  expect(screen.queryByLabelText('Agent location')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Create agent' }));
  await screen.findByText('Lost response');
  expect(create.mock.calls[0]?.[0]).toEqual({ name: 'New agent', sandbox_id: 'sbx_1' });
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBeNull();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner', undefined, 'sbx_1'))).toContain(
    'sbx_1',
  );
  first.unmount();
  const global = show(client);
  expect(screen.queryByRole('button', { name: 'Recover creation' })).not.toBeInTheDocument();
  global.unmount();
  draw();
  expect(screen.getByRole('button', { name: 'Recover creation' })).toBeEnabled();
  await user.click(screen.getByRole('button', { name: 'Recover creation' }));
  await waitFor(() => expect(create).toHaveBeenCalledTimes(2));
  expect(create.mock.calls[1]?.[0]).toEqual(create.mock.calls[0]?.[0]);
  expect(create.mock.calls[1]?.[1].idempotencyKey).toBe(create.mock.calls[0]?.[1].idempotencyKey);
});

it('keeps global new-Sandbox creation unavailable without its capability', async () => {
  const create = vi.fn();
  render(
    <AgentLifecycleCard
      client={agentClient({ createAgentLifecycle: create })}
      viewerId="owner"
      canCreateSandbox={false}
      onClose={vi.fn()}
      onCreated={vi.fn()}
    />,
  );
  expect(screen.queryByLabelText('Agent location')).not.toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent(
    'Use New agent from an existing Sandbox or workspace',
  );
  expect(screen.getByRole('button', { name: 'Create agent' })).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Create agent' }));
  expect(create).not.toHaveBeenCalled();
});

it('refuses invalid repository URLs before admission', async () => {
  const create = vi.fn();
  show(agentClient({ createAgentLifecycle: create }));
  const input = screen.getByLabelText('GitHub repository (optional)');
  fireEvent.change(input, { target: { value: 'https://github.com/org/repo/tree/main' } });
  expect(screen.getByRole('button', { name: 'Create agent' })).toBeDisabled();
  expect(screen.getByRole('alert')).toHaveTextContent('HTTPS GitHub repository URL');
  expect(create).not.toHaveBeenCalled();
});
