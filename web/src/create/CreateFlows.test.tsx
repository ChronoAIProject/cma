import '../test/connectedAccounts';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { agentClient, AGENT_DETAIL } from '../test/agent';
import {
  creationClient,
  CREATION_AGENT,
  CREATION_SANDBOX,
  CREATION_WORKSPACE,
} from '../test/creation';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { chooseOption } from '../test/select';
import { NewAgentCard } from './NewAgentCard';
import { NewSandboxCard } from './NewSandboxCard';
import { NewWorkspaceCard } from './NewWorkspaceCard';
import { InlineWorkspaceCard } from './InlineWorkspaceCard';

describe('Sandbox creation', () => {
  it('requires verbatim foreign command/environment review and sends the exact revision digest', async () => {
    const user = userEvent.setup();
    const createSandbox = vi.fn().mockResolvedValue(sandboxFixture('provisioning'));
    const onCreated = vi.fn();
    render(
      <NewSandboxCard
        client={agentClient({ createSandbox })}
        catalog={creationClient()}
        initial={CREATION_SANDBOX}
        onCreated={onCreated}
        onClose={vi.fn()}
      />,
    );
    expect(screen.getByText('mkdir -p /home/cma/tools')).toBeInTheDocument();
    expect(screen.getByText('RUN_MODE=research')).toBeInTheDocument();
    expect(screen.getByText('mkdir -p reports')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Create sandbox' })).toBeDisabled();
    await user.type(screen.getByLabelText('Name (optional)'), 'My project');
    await user.click(screen.getByRole('checkbox', { name: 'I trust this profile' }));
    await user.click(screen.getByRole('button', { name: 'Create sandbox' }));
    await waitFor(() => expect(onCreated).toHaveBeenCalled());
    expect(createSandbox).toHaveBeenCalledWith(
      {
        sandboxProfile: { id: 'sbp_research', revision: 1 },
        name: 'My project',
        trustConfirmed: true,
        commandsDigest: 'sandbox-content-digest',
      },
      expect.objectContaining({ idempotencyKey: expect.any(String) }),
    );
  });

  it.each(['idempotency_in_flight', 'idempotency_recovery_required', 'transport'])(
    'retains request and key after %s without allowing edits or dismissal',
    async (code) => {
      const user = userEvent.setup();
      const createSandbox = vi
        .fn()
        .mockRejectedValueOnce(
          code === 'transport' ? new TypeError('offline') : { status: 409, code },
        )
        .mockResolvedValue(sandboxFixture());
      const onClose = vi.fn();
      render(
        <NewSandboxCard
          client={agentClient({ createSandbox })}
          catalog={creationClient()}
          initial={CREATION_SANDBOX}
          onCreated={vi.fn()}
          onClose={onClose}
        />,
      );
      await user.click(screen.getByRole('checkbox', { name: 'I trust this profile' }));
      await user.click(screen.getByRole('button', { name: 'Create sandbox' }));
      await screen.findByRole('button', { name: 'Retry creation' });
      expect(screen.getByLabelText('Name (optional)')).toBeDisabled();
      expect(screen.getByRole('checkbox')).toBeDisabled();
      await user.keyboard('{Escape}');
      expect(onClose).not.toHaveBeenCalled();
      await user.click(screen.getByRole('button', { name: 'Retry creation' }));
      expect(createSandbox).toHaveBeenCalledTimes(2);
      expect(createSandbox.mock.calls[0]?.[0]).toEqual(createSandbox.mock.calls[1]?.[0]);
      expect(createSandbox.mock.calls[0]?.[1].idempotencyKey).toBe(
        createSandbox.mock.calls[1]?.[1].idempotencyKey,
      );
    },
  );

  it('validates UTF-8 name length and releases finite refusals for a corrected new command', async () => {
    const user = userEvent.setup();
    const createSandbox = vi
      .fn()
      .mockRejectedValueOnce({ status: 422, code: 'sandbox_invalid_name' })
      .mockResolvedValue(sandboxFixture());
    render(
      <NewSandboxCard
        client={agentClient({ createSandbox })}
        catalog={creationClient()}
        initial={{ ...CREATION_SANDBOX, requiresConfirmation: false }}
        onCreated={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByLabelText('Name (optional)'), {
      target: { value: 'x'.repeat(65) },
    });
    expect(screen.getByRole('button', { name: 'Create sandbox' })).toBeDisabled();
    fireEvent.change(screen.getByLabelText('Name (optional)'), { target: { value: 'First' } });
    await user.click(screen.getByRole('button', { name: 'Create sandbox' }));
    await screen.findByRole('alert');
    expect(screen.getByLabelText('Name (optional)')).toBeEnabled();
    fireEvent.change(screen.getByLabelText('Name (optional)'), { target: { value: 'Second' } });
    await user.click(screen.getByRole('button', { name: 'Create sandbox' }));
    expect(createSandbox.mock.calls[0]?.[1].idempotencyKey).not.toBe(
      createSandbox.mock.calls[1]?.[1].idempotencyKey,
    );
  });
});

it('creates a Workspace inside the selected active Sandbox with its reviewed setup digest', async () => {
  const user = userEvent.setup();
  const createWorkspace = vi.fn().mockResolvedValue(workspaceFixture());
  render(
    <NewWorkspaceCard
      client={agentClient({ createWorkspace })}
      catalog={creationClient()}
      sandboxId="sbx_1"
      initial={CREATION_WORKSPACE}
      onCreated={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await screen.findByText('Atlas');
  await user.click(screen.getByRole('checkbox', { name: 'I trust this profile' }));
  await user.click(screen.getByRole('button', { name: 'Create workspace' }));
  expect(createWorkspace).toHaveBeenCalledWith(
    'sbx_1',
    {
      workspaceProfile: { id: 'wkp_research', revision: 1 },
      trustConfirmed: true,
      commandsDigest: 'workspace-content-digest',
    },
    expect.objectContaining({ idempotencyKey: expect.any(String) }),
  );
});

it('admits an inline empty Workspace without reading a Workspace Profile', async () => {
  const user = userEvent.setup();
  const createInlineWorkspace = vi.fn().mockResolvedValue(workspaceFixture());
  render(
    <InlineWorkspaceCard
      client={agentClient({ createInlineWorkspace })}
      sandboxId="sbx_1"
      onCreated={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await user.click(screen.getByRole('button', { name: 'Create workspace' }));
  expect(createInlineWorkspace).toHaveBeenCalledWith(
    'sbx_1',
    { source: { type: 'empty' }, name: 'workspace' },
    expect.objectContaining({ idempotencyKey: expect.any(String) }),
  );
});

it.each(['paused', 'reviving'] as const)(
  'creates a Workspace in a %s Sandbox without a separate revive request',
  async (status) => {
    const user = userEvent.setup();
    const createWorkspace = vi.fn().mockResolvedValue(workspaceFixture());
    const reviveSandbox = vi.fn();
    render(
      <NewWorkspaceCard
        client={agentClient({
          createWorkspace,
          reviveSandbox,
          readSandbox: async () => sandboxFixture(status),
        })}
        catalog={creationClient()}
        sandboxId="sbx_1"
        initial={CREATION_WORKSPACE}
        onCreated={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    await screen.findByText(`Sandbox: ${status === 'paused' ? 'Paused' : 'Resuming'}`);
    await user.click(screen.getByRole('checkbox', { name: 'I trust this profile' }));
    await user.click(screen.getByRole('button', { name: 'Create workspace' }));
    expect(createWorkspace).toHaveBeenCalledTimes(1);
    expect(reviveSandbox).not.toHaveBeenCalled();
  },
);

it('admits an Agent in a paused location through the normal create request', async () => {
  const user = userEvent.setup();
  const createAgent = vi.fn().mockResolvedValue(AGENT_DETAIL);
  const reviveSandbox = vi.fn();
  render(
    <NewAgentCard
      client={agentClient({
        createAgent,
        reviveSandbox,
        listSandboxes: async () => ({ items: [sandboxFixture('paused')] }),
        readSandbox: async () => sandboxFixture('paused'),
      })}
      catalog={creationClient()}
      initial={CREATION_AGENT}
      onCreated={vi.fn()}
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await chooseOption(user, 'Sandbox', 'Atlas');
  await waitFor(() => expect(screen.getByRole('button', { name: 'New workspace' })).toBeEnabled());
  await chooseOption(user, 'Workspace', 'chrono-bot');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Continue' })).toBeEnabled());
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  await user.click(screen.getByRole('button', { name: 'Create agent' }));
  expect(createAgent).toHaveBeenCalledTimes(1);
  expect(reviveSandbox).not.toHaveBeenCalled();
});

it('steps through location/profile/title and preserves the first message byte for byte', async () => {
  const user = userEvent.setup();
  const createAgent = vi.fn().mockResolvedValue(AGENT_DETAIL);
  const onCreated = vi.fn();
  render(
    <NewAgentCard
      client={agentClient({ createAgent })}
      catalog={creationClient()}
      initial={CREATION_AGENT}
      workspaceId="wsp_1"
      onCreated={onCreated}
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await waitFor(() => expect(screen.getByRole('button', { name: 'Continue' })).toBeEnabled());
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  expect(screen.getByText(/gpt-5/)).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  await user.type(screen.getByLabelText('Title (optional)'), 'Inspect project');
  fireEvent.change(screen.getByLabelText('First message (optional)'), {
    target: { value: '  Inspect this repository.\nKeep the notes.  ' },
  });
  await user.click(screen.getByRole('button', { name: 'Create agent' }));
  expect(createAgent).toHaveBeenCalledWith(
    'wsp_1',
    {
      agentProfile: { id: 'agp_research', revision: 1 },
      title: 'Inspect project',
      firstMessage: '  Inspect this repository.\nKeep the notes.  ',
    },
    expect.objectContaining({ idempotencyKey: expect.any(String) }),
  );
  expect(onCreated).toHaveBeenCalledWith(AGENT_DETAIL);
});

it('replaces the Agent dialog for inline Sandbox/Workspace creation, never stacking dialogs', async () => {
  const user = userEvent.setup();
  const client = agentClient({
    createSandbox: async () => sandboxFixture(),
    createWorkspace: async () => workspaceFixture(),
  });
  render(
    <NewAgentCard
      client={client}
      catalog={creationClient()}
      onCreated={vi.fn()}
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await user.click(screen.getByRole('button', { name: 'New sandbox' }));
  expect(screen.getAllByRole('dialog')).toHaveLength(1);
  expect(screen.getByRole('dialog')).toHaveAccessibleName('New sandbox');
  await user.click(await screen.findByRole('button', { name: /Research sandbox Version/ }));
  await user.click(await screen.findByRole('checkbox', { name: 'I trust this profile' }));
  await user.click(screen.getByRole('button', { name: 'Create sandbox' }));
  await waitFor(() => expect(screen.getByRole('dialog')).toHaveAccessibleName('New agent'));
  await waitFor(() => expect(screen.getByRole('button', { name: 'New workspace' })).toBeEnabled());
  await user.click(screen.getByRole('button', { name: 'New workspace' }));
  expect(screen.getAllByRole('dialog')).toHaveLength(1);
  await user.click(await screen.findByRole('button', { name: /Research workspace Version/ }));
  await user.click(await screen.findByRole('checkbox', { name: 'I trust this profile' }));
  await user.click(screen.getByRole('button', { name: 'Create workspace' }));
  await waitFor(() => expect(screen.getByRole('dialog')).toHaveAccessibleName('New agent'));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Continue' })).toBeEnabled());
});

it('keeps preparing Workspaces unavailable and reconnects after a transient progress failure', async () => {
  vi.useFakeTimers();
  const preparing = {
    ...workspaceFixture(),
    status: 'preparing' as const,
    resolvedCommit: undefined,
  };
  let ready = false;
  let unavailable = false;
  const client = agentClient({
    readSandbox: async () => {
      if (unavailable) throw new Error('offline');
      return {
        ...sandboxFixture(),
        workspaces: [ready ? workspaceFixture() : preparing],
      };
    },
    readWorkspace: async () => (ready ? workspaceFixture() : preparing),
  });
  const view = render(
    <NewAgentCard
      client={client}
      catalog={creationClient()}
      workspaceId="wsp_1"
      onCreated={vi.fn()}
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled();
  expect(
    within(screen.getByRole('region', { name: 'Workspace preparation' })).getByText(
      'Cloning repository',
    ),
  ).toBeInTheDocument();
  unavailable = true;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1500);
  });
  expect(screen.getByRole('alert')).toHaveTextContent('This location could not be loaded.');
  expect(screen.getByRole('region', { name: 'Workspace preparation' })).toBeInTheDocument();
  ready = true;
  unavailable = false;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(screen.getByRole('button', { name: 'Continue' })).toBeEnabled();
  view.unmount();
  vi.useRealTimers();
});

it('allows choosing a Sandbox and Workspace with the shared keyboard-accessible pickers', async () => {
  const user = userEvent.setup();
  render(
    <NewAgentCard
      client={agentClient()}
      catalog={creationClient()}
      onCreated={vi.fn()}
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await chooseOption(user, 'Sandbox', 'Atlas');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Workspace' })).toBeEnabled());
  await chooseOption(user, 'Workspace', 'chrono-bot');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Continue' })).toBeEnabled());
});
