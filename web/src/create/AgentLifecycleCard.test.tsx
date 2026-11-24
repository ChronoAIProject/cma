import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { creationClient } from '../test/creation';
import type { AgentLifecycleOperationView, CmaWebClient } from '../api/client';
import { connectionClient, type ConnectionReport } from '../connections/client';
import { AgentLifecycleCard } from './AgentLifecycleCard';
import { taskExperience } from '../test/taskExperience';
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
  await screen.findByText(
    "We couldn't confirm your Agent was created. Use Recover creation to resend the same request.",
  );
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
it('resumes receipt polling after reload and automatically opens exactly once only on final success', async () => {
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
  expect(onCreated).not.toHaveBeenCalled();
  await waitFor(() =>
    expect(onCreated).toHaveBeenCalledWith({ id: 'ready-agent', status: 'ready' }),
  );
  expect(onCreated).toHaveBeenCalledOnce();
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

it('discards a repository-only creation after leaving a missing GitHub connection', async () => {
  const create = vi
    .fn()
    .mockResolvedValueOnce({ id: running.id })
    .mockResolvedValueOnce({ id: 'new-operation' });
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'owner',
    ready: false,
    checked_at: '2026-11-24T00:00:00Z',
    services: [
      {
        service: 'api-github',
        display_name: 'GitHub',
        kind: 'nyxid',
        state: 'missing',
        connection_mode: 'hosted',
      },
    ],
  });
  show(
    agentClient({
      createAgentLifecycle: create,
      readAgentLifecycle: async (id) =>
        id === running.id
          ? {
              ...running,
              status: 'waiting_for_connection',
              connection: {
                services: ['api-github'],
                target: { type: 'sandbox_profile', profile: { id: 'profile-owned', revision: 1 } },
              },
            }
          : { ...running, id },
    }),
  );
  const actor = await review();
  await actor.click(screen.getByRole('button', { name: 'Create agent' }));
  const wizard = await screen.findByRole('dialog', { name: 'Connect required services' });
  await actor.click(within(wizard).getByRole('button', { name: 'Close' }));
  expect(screen.getByRole('button', { name: 'Connect services' })).toBeVisible();
  await actor.click(screen.getByRole('button', { name: 'Discard creation' }));
  await actor.click(screen.getByRole('button', { name: 'Discard creation' }));
  expect(screen.getByLabelText('GitHub repository (optional)')).toHaveValue('');
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBeNull();

  await actor.click(screen.getByRole('button', { name: 'Create agent' }));
  await waitFor(() => expect(create).toHaveBeenCalledTimes(2));
  expect(create.mock.calls[1]?.[0]).not.toHaveProperty('repository');
  expect(create.mock.calls[1]?.[1].idempotencyKey).not.toBe(
    create.mock.calls[0]?.[1].idempotencyKey,
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
  await screen.findByText("Agent status couldn't be read. Use Retry status to check again.");
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
    readTaskCreationSupport: async () => true,
    taskExperience: taskExperience('owner'),
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
  expect(screen.queryByRole('radiogroup')).not.toBeInTheDocument();
  expect(screen.getByText('Creating in Workspace wsp_1')).toBeInTheDocument();
  expect(screen.queryByRole('group', { name: 'Runtime image' })).not.toBeInTheDocument();
  const user = userEvent.setup();
  await user.type(screen.getByLabelText('Your task'), 'Inspect the workspace');
  await user.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByText(
    "We couldn't confirm your task was received. Use Recover original task to resend the same request.",
  );
  const [body, options] = create.mock.calls[0]!;
  expect(body.workspace_id).toBe('wsp_1');
  expect(body).not.toHaveProperty('image');
  expect(body.initial_message).toBe('Inspect the workspace');
  expect(body).not.toHaveProperty('repository');
  expect(body).not.toHaveProperty('profile');
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBeNull();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner', 'wsp_1'))).toContain('wsp_1');
  first.unmount();
  const global = show(client);
  expect(screen.getByLabelText('GitHub repository (optional)')).toHaveValue('');
  expect(screen.queryByRole('button', { name: 'Recover creation' })).not.toBeInTheDocument();
  global.unmount();
  draw();
  await user.click(screen.getByRole('button', { name: 'Recover original task' }));
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
  expect(screen.queryByRole('radiogroup')).not.toBeInTheDocument();
});

it('uses server defaults without image or profile selectors and allows an empty repository', async () => {
  const create = vi.fn().mockResolvedValue({ id: running.id });
  const listSandboxes = vi.fn(async () => ({ items: [] }));
  show(
    agentClient({
      listSandboxes,
      createAgentLifecycle: create,
      readAgentLifecycle: async () => running,
    }),
  );
  expect(screen.queryByRole('radiogroup')).not.toBeInTheDocument();
  expect(
    screen.getByText(
      'Your Agent gets its own Sandbox, a private computer it works in, starting with an empty Workspace.',
    ),
  ).toBeInTheDocument();
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
  const settings = screen.getByRole('region', { name: 'Agent settings' });
  expect(settings).toHaveTextContent('gpt-6 · High reasoning · Autonomous');
  expect(within(settings).queryByRole('button')).not.toBeInTheDocument();
  expect(
    screen.getByText('Includes access to ChronoAI models and the Ornn skill library.'),
  ).toBeInTheDocument();
  expect(screen.getByRole('dialog', { name: 'New agent' })).not.toHaveTextContent(/YOLO|Codex/);
  await userEvent.click(screen.getByRole('button', { name: 'Create agent' }));
  await waitFor(() => expect(create).toHaveBeenCalledOnce());
  expect(create.mock.calls[0]?.[0]).toEqual({ name: 'New agent' });
  expect(listSandboxes).not.toHaveBeenCalled();
});

it('retains the contextual Sandbox and keeps its recovery separate from global creation', async () => {
  const create = vi.fn().mockRejectedValue(new Error('Lost response'));
  const client = agentClient({
    createAgentLifecycle: create,
    readTaskCreationSupport: async () => true,
    taskExperience: taskExperience('owner'),
  });
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
  await user.type(screen.getByLabelText('Your task'), 'Inspect the Sandbox');
  await user.click(screen.getByRole('button', { name: 'Start task' }));
  await screen.findByText(
    "We couldn't confirm your task was received. Use Recover original task to resend the same request.",
  );
  expect(create.mock.calls[0]?.[0]).toEqual({
    name: 'New task',
    sandbox_id: 'sbx_1',
    initial_message: 'Inspect the Sandbox',
  });
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBeNull();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner', undefined, 'sbx_1'))).toContain(
    'sbx_1',
  );
  first.unmount();
  const global = show(client);
  expect(screen.queryByRole('button', { name: 'Recover creation' })).not.toBeInTheDocument();
  global.unmount();
  draw();
  expect(screen.getByRole('button', { name: 'Recover original task' })).toBeEnabled();
  await user.click(screen.getByRole('button', { name: 'Recover original task' }));
  await waitFor(() => expect(create).toHaveBeenCalledTimes(2));
  expect(create.mock.calls[1]?.[0]).toEqual(create.mock.calls[0]?.[0]);
  expect(create.mock.calls[1]?.[1].idempotencyKey).toBe(create.mock.calls[0]?.[1].idempotencyKey);
});

it('keeps global new-Sandbox creation unavailable without its capability', async () => {
  const create = vi.fn().mockRejectedValue(new Error('Lost response'));
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
  expect(screen.queryByRole('radiogroup')).not.toBeInTheDocument();
  expect(screen.getByText('You don’t have permission to create a Sandbox.')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Create agent' })).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Create agent' }));
  expect(create).not.toHaveBeenCalled();
});

it('validates the repository when the field is left, never while typing', async () => {
  const create = vi.fn();
  show(agentClient({ createAgentLifecycle: create }));
  const user = userEvent.setup();
  const input = screen.getByLabelText('GitHub repository (optional)');
  await user.type(input, 'https://github.com/org/repo/tree/main');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(input).not.toHaveAttribute('aria-invalid');
  await user.tab();
  expect(screen.getByRole('alert')).toHaveTextContent('Enter an HTTPS GitHub repository URL.');
  expect(input).toHaveAttribute('aria-invalid', 'true');
  expect(input).toHaveAccessibleDescription(/Enter an HTTPS GitHub repository URL\./);
  await user.clear(input);
  await user.type(input, 'https://github.com/org/repo');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(input).not.toHaveAttribute('aria-invalid');
  expect(create).not.toHaveBeenCalled();
});

it('refuses invalid repository URLs on submit before admission', async () => {
  const create = vi.fn();
  show(agentClient({ createAgentLifecycle: create }));
  const input = screen.getByLabelText('GitHub repository (optional)');
  fireEvent.change(input, { target: { value: 'http://github.com/org/repo' } });
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  const button = screen.getByRole('button', { name: 'Create agent' });
  await waitFor(() => expect(button).toBeEnabled());
  fireEvent.click(button);
  expect(screen.getByRole('alert')).toHaveTextContent('HTTPS GitHub repository URL');
  expect(input).toHaveFocus();
  expect(create).not.toHaveBeenCalled();
  expect(sessionStorage.length).toBe(0);
});
