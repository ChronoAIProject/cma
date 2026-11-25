import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import type { User } from './auth/types';
import { CmaClientProvider } from './api/context';
import { Console } from './Console';
import { agentClient, AGENT_DETAIL } from './test/agent';
import { sandboxFixture } from './test/hierarchy';
import { ApiError, createClient } from '@cma/client';
import { WIRE_AGENT } from './test/rest';
import { chooseOption, expandNavigationFilters } from './test/select';

const OWNER: User = {
  id: 'owner',
  handle: 'Owner',
  visitor: false,
  capabilities: ['create_sandbox', 'use_own_sandboxes', 'delete_own_sandbox'],
};

it('acknowledges pause immediately and refreshes after admission without claiming completion', async () => {
  let accept: () => void = () => {};
  const pauseSandbox = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        accept = resolve;
      }),
  );
  const client = agentClient({ pauseSandbox });
  const readSandbox = vi.spyOn(client, 'readSandbox');
  const actor = userEvent.setup();
  render(
    <CmaClientProvider client={client}>
      <Console
        user={OWNER}
        route={{ kind: 'sandbox', sandboxId: 'sbx_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  await actor.click(await screen.findByRole('button', { name: 'Pause' }));
  expect(screen.getByText('Submitting pause request…')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Pause' })).toBeDisabled();
  const reads = readSandbox.mock.calls.length;
  await act(async () => accept());
  expect(screen.getByText('Pause request accepted.')).toBeVisible();
  expect(screen.getByText('Waiting for the Sandbox to pause.')).not.toBeVisible();
  expect(screen.getByRole('button', { name: 'Pause' })).toBeDisabled();
  expect(screen.queryByText('Sandbox paused.')).not.toBeInTheDocument();
  expect(readSandbox.mock.calls.length).toBeGreaterThan(reads);
  await actor.click(screen.getByText('Operation progress'));
  expect(screen.getByText('Waiting for the Sandbox to pause.')).toBeVisible();
  expect(screen.getByRole('button', { name: 'View Sandbox progress' })).toBeVisible();
  expect(pauseSandbox).toHaveBeenCalledTimes(1);
});

it('removes an accepted deletion from active navigation before cleanup and keeps progress reachable', async () => {
  let accept: () => void = () => {};
  const deleteSandbox = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        accept = resolve;
      }),
  );
  const actor = userEvent.setup();
  render(
    <CmaClientProvider client={agentClient({ deleteSandbox })}>
      <Console
        user={OWNER}
        route={{ kind: 'sandbox', sandboxId: 'sbx_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  await actor.click(await screen.findByRole('button', { name: 'More actions' }));
  await actor.click(screen.getByRole('menuitem', { name: 'Delete Sandbox' }));
  await actor.click(
    await screen.findByRole('checkbox', { name: 'I understand this Sandbox will be deleted.' }),
  );
  await actor.click(screen.getByRole('button', { name: 'Request deletion' }));
  expect(screen.getByText('Submitting delete request.')).toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'Environments' }));
  expect(screen.getByRole('treeitem', { name: /Atlas/ })).toBeInTheDocument();
  await act(async () => accept());
  expect(screen.queryByRole('treeitem', { name: /Atlas/ })).not.toBeInTheDocument();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await actor.click(
    screen.getAllByText('Cleaning up 1 sandboxes').find((element) => !element.closest('[hidden]'))!,
  );
  await actor.click(screen.getByRole('button', { name: 'View status for Atlas' }));
  expect(screen.getByRole('dialog', { name: 'Deleting Sandbox' })).toBeInTheDocument();
  expect(deleteSandbox).toHaveBeenCalledTimes(1);
});

it('keeps composer settings available while Details and tree navigation remain usable', async () => {
  const onNavigate = vi.fn();
  const actor = userEvent.setup();
  render(
    <CmaClientProvider
      client={agentClient({
        readAgentSettings: async () => ({
          model: 'gpt-6',
          reasoning_effort: 'high',
          permission_mode: 'autonomous',
        }),
        updateAgentSettings: vi.fn(),
      })}
    >
      <Console
        user={OWNER}
        route={{ kind: 'agent', agentId: 'agt_1' }}
        onNavigate={onNavigate}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  expect(await screen.findByRole('heading', { name: 'Agent one' })).toBeInTheDocument();
  await waitFor(() => expect(screen.getByRole('textbox', { name: 'Message' })).toHaveFocus());
  expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
  const composer = within(screen.getByRole('form', { name: 'Message Agent' }));
  await actor.click(await composer.findByRole('button', { name: /^Agent settings:/ }));
  expect(
    await composer.findByRole('button', { name: /^Model and reasoning effort/ }),
  ).toHaveTextContent('gpt-6 High');
  expect(composer.getByLabelText('Permission mode')).toHaveTextContent('Autonomous');
  await actor.click(screen.getByRole('button', { name: 'More Agent actions' }));
  await actor.click(screen.getByRole('button', { name: 'Show Agent details' }));
  const panel = within(screen.getByRole('complementary', { name: 'Agent details' }));
  expect(panel.queryByLabelText('Model')).not.toBeInTheDocument();
  expect(panel.queryByLabelText(/Model and reasoning effort/)).not.toBeInTheDocument();
  expect(panel.queryByLabelText('Reasoning effort')).not.toBeInTheDocument();
  expect(panel.queryByLabelText('Permission mode')).not.toBeInTheDocument();
  expect(panel.getByRole('link', { name: 'Manage on Sandbox' })).toHaveAttribute(
    'href',
    '/sandboxes/sbx_1?tab=access',
  );
  expect(panel.getByRole('button', { name: 'Sandbox operation progress' })).toBeVisible();
  expect(panel.queryByText('https://example.test/chrono-bot.git')).not.toBeInTheDocument();
  expect(panel.queryByRole('heading', { name: 'Creation' })).not.toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'Close Agent details' }));
  await actor.click(screen.getByRole('button', { name: 'Options for Agent one' }));
  expect(screen.getByRole('menuitem', { name: 'Stop' })).toBeEnabled();
  expect(screen.getByRole('menuitem', { name: /^Delete / })).toHaveAttribute(
    'aria-disabled',
    'true',
  );
  expect(screen.getByRole('menuitem', { name: 'Export raw history' })).toBeEnabled();
  await actor.keyboard('{Escape}');
  await actor.click(screen.getByRole('button', { name: 'Environments' }));
  const sandbox = screen.getByRole('treeitem', { name: /Atlas/ });
  sandbox.focus();
  await actor.keyboard('{Enter}');
  expect(onNavigate).toHaveBeenCalledWith({ kind: 'sandbox', sandboxId: 'sbx_1' });
});

it.each(['network', 'idempotency_in_flight', 'idempotency_recovery_required'])(
  'retains the Sandbox pause key after a %s failure',
  async (code) => {
    const actor = userEvent.setup();
    const pauseSandbox = vi
      .fn()
      .mockRejectedValueOnce(
        code === 'network'
          ? new Error('Connection lost')
          : new ApiError(409, 'Connection lost', code),
      )
      .mockResolvedValue(undefined);
    const client = agentClient({
      pauseSandbox,
      readSandbox: vi.fn().mockResolvedValue(sandboxFixture()),
    });
    render(
      <CmaClientProvider client={client}>
        <Console
          user={OWNER}
          route={{ kind: 'sandbox', sandboxId: 'sbx_1' }}
          onNavigate={vi.fn()}
          onSignOut={vi.fn()}
        />
      </CmaClientProvider>,
    );
    await actor.click(await screen.findByRole('button', { name: 'Pause' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Connection lost');
    await actor.click(screen.getByRole('button', { name: 'Pause' }));
    await waitFor(() => expect(pauseSandbox).toHaveBeenCalledTimes(2));
    const first = pauseSandbox.mock.calls[0]!;
    expect(first[0]).toBe('sbx_1');
    expect(first[1].idempotencyKey).not.toBe('');
    expect(pauseSandbox.mock.calls[1]![1].idempotencyKey).toBe(first[1].idempotencyKey);
  },
);

it('hides creation controls for visitors while retaining their trigger-created hierarchy', async () => {
  render(
    <CmaClientProvider client={agentClient()}>
      <Console
        user={{ ...OWNER, visitor: true }}
        route={{ kind: 'home' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
        onCreate={vi.fn()}
      />
    </CmaClientProvider>,
  );
  expect(await screen.findByRole('tree', { name: 'Personal Agents' })).toHaveTextContent(
    'Agent one',
  );
  expect(screen.queryByRole('button', { name: 'New agent' })).not.toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'Welcome to CMA' })).toBeInTheDocument();
});

it('disables personal creation with a reason in organization scope without replacing the open conversation or draft', async () => {
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    if (url.pathname === '/api/v1/organizations')
      return Response.json({
        items: [{ id: 'org_research', role: 'admin', display_name: 'Research' }],
      });
    if (url.pathname === '/api/v1/agents/agt_1')
      return Response.json({
        ...WIRE_AGENT,
        id: AGENT_DETAIL.id,
        worktree: { ...WIRE_AGENT.worktree, id: AGENT_DETAIL.worktree.id },
        workspace: { ...WIRE_AGENT.workspace, id: AGENT_DETAIL.workspace.id },
        sandbox: { ...WIRE_AGENT.sandbox, id: AGENT_DETAIL.sandbox.id },
      });
    if (url.pathname.includes('/resource-access/'))
      return Response.json({
        owner: { kind: 'person', is_current_person: true },
        capabilities: ['view_details'],
        policy_revision: 1,
        revalidate_at: null,
      });
    if (url.pathname === '/api/v1/sandboxes' || url.pathname === '/api/v1/agents/provenance') {
      expect(url.searchParams.get('organization')).toBe('org_research');
      return Response.json({ items: [] });
    }
    if (url.pathname.endsWith('/channel-usage'))
      return Response.json({
        agent_id: 'agt_1',
        channel_usage: {
          active: false,
          has_history: false,
          first_used_at: null,
          last_used_at: null,
        },
      });
    throw new Error('Unexpected scope-test path: ' + url.pathname);
  });
  const resourceViews = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const create = vi.fn();
  render(
    <CmaClientProvider client={agentClient({ resourceViews })}>
      <Console
        user={OWNER}
        route={{ kind: 'agent', agentId: 'agt_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
        onCreate={create}
      />
    </CmaClientProvider>,
  );
  const actor = userEvent.setup();
  const composer = await screen.findByRole('textbox', { name: 'Message' });
  await actor.type(composer, 'Retain my personal draft');
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'New agent' })).not.toHaveAttribute('aria-disabled'),
  );
  await expandNavigationFilters(actor);
  await chooseOption(actor, 'Navigation scope', 'Research (Organization)');
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'New agent' })).toHaveAttribute(
      'aria-disabled',
      'true',
    ),
  );
  expect(screen.getByRole('button', { name: 'New agent' })).toHaveAccessibleDescription(
    'Switch to Personal to create an Agent.',
  );
  await actor.click(screen.getByRole('button', { name: 'New agent' }));
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('Retain my personal draft');
  expect(create).not.toHaveBeenCalled();
  await chooseOption(actor, 'Navigation scope', 'Personal');
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'New agent' })).not.toHaveAttribute('aria-disabled'),
  );
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
});
import './test/connectedAccounts';
