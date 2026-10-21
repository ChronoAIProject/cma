import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { CmaWebClient } from '../api/client';
import type { SandboxDetail, WorkspaceDetail } from '../api/types';
import { agentClient } from '../test/agent';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { AgentLifecycleCard } from './AgentLifecycleCard';
import { lifecycleStorageKey } from './lifecycleIntent';

beforeEach(() => sessionStorage.clear());
afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

const atlas: SandboxDetail = {
  ...sandboxFixture(),
  owner: 'owner',
  workspaces: [
    { ...workspaceFixture(), id: 'wsp_ready', name: 'research' },
    { ...workspaceFixture(), id: 'wsp_building', name: 'building', status: 'preparing' },
  ],
};
const dormant: SandboxDetail = {
  ...sandboxFixture('paused'),
  id: 'sbx_dormant',
  name: 'Dormant',
  owner: 'owner',
  workspaces: [{ ...workspaceFixture(), id: 'wsp_dormant', sandboxId: 'sbx_dormant', name: 'old' }],
};
const foreign: SandboxDetail = {
  ...sandboxFixture(),
  id: 'sbx_foreign',
  name: 'Shared',
  owner: 'x',
};
const sandboxes = [atlas, dormant, foreign];

function hierarchyClient(overrides: Partial<CmaWebClient> = {}) {
  return agentClient({
    listSandboxes: async () => ({ items: sandboxes }),
    readSandbox: async (id) => sandboxes.find((sandbox) => sandbox.id === id)!,
    readWorkspace: async (id): Promise<WorkspaceDetail> => {
      const sandbox = sandboxes.find((item) => item.workspaces.some((ws) => ws.id === id))!;
      const summary = sandbox.workspaces.find((ws) => ws.id === id)!;
      return { ...workspaceFixture(), ...summary, sandboxId: sandbox.id };
    },
    ...overrides,
  });
}
function show(client: CmaWebClient, props: { workspaceId?: string; sandboxId?: string } = {}) {
  return render(
    <AgentLifecycleCard
      client={client}
      viewerId="owner"
      onClose={vi.fn()}
      onCreated={vi.fn()}
      {...props}
    />,
  );
}
const radio = (name: string) => screen.getByRole('radio', { name });

it('lists every location and keeps the unusable ones with their reasons', async () => {
  show(hierarchyClient());
  const group = screen.getByRole('radiogroup', { name: 'Where it works' });
  await within(group).findByRole('radio', { name: 'Atlas / research' });
  expect(
    within(group)
      .getAllByRole('radio')
      .map((item) => item.textContent),
  ).toEqual([
    'New SandboxLatest CMA runtime image',
    'New workspace in AtlasLive',
    'New workspace in DormantPausedAvailable when the Sandbox is active.',
    'Atlas / researchReady',
    'Atlas / buildingStartingThe Workspace is not ready.',
    'Dormant / oldReadyAvailable when the Sandbox is active.',
  ]);
  expect(radio('New Sandbox')).toHaveAttribute('aria-checked', 'true');
  expect(radio('New workspace in Atlas')).not.toHaveAttribute('aria-disabled');
  expect(radio('New workspace in Dormant')).toHaveAttribute('aria-disabled', 'true');
  expect(radio('New workspace in Dormant')).toHaveAccessibleDescription(
    /Available when the Sandbox is active\./,
  );
  expect(radio('Atlas / building')).toHaveAttribute('aria-disabled', 'true');
  expect(radio('Atlas / building')).toHaveAccessibleDescription(/The Workspace is not ready\./);
  expect(radio('Dormant / old')).toHaveAttribute('aria-disabled', 'true');
  expect(screen.queryByRole('radio', { name: /Shared/ })).not.toBeInTheDocument();
  await userEvent.click(radio('Atlas / building'));
  expect(radio('Atlas / building')).toHaveAttribute('aria-checked', 'false');
  expect(radio('New Sandbox')).toHaveAttribute('aria-checked', 'true');
});

it.each([
  ['Atlas / research', 'Start an Agent in the selected workspace.', { workspace_id: 'wsp_ready' }],
  [
    'New workspace in Atlas',
    'Start an Agent with a new workspace in this Sandbox.',
    { sandbox_id: 'sbx_1' },
  ],
  ['New Sandbox', 'Start an Agent in a new Sandbox with an empty workspace.', {}],
])('creates in %s with only that location in the request', async (name, description, ids) => {
  const create = vi.fn().mockRejectedValue(new Error('Lost response'));
  show(hierarchyClient({ createAgentLifecycle: create }), { workspaceId: 'wsp_ready' });
  const user = userEvent.setup();
  await user.click(await screen.findByRole('radio', { name }));
  expect(screen.getByText(description)).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Create agent' }));
  await waitFor(() => expect(create).toHaveBeenCalledOnce());
  expect(create.mock.calls[0]?.[0]).toEqual({ name: 'New agent', ...ids });
  if (!('workspace_id' in ids))
    expect(create.mock.calls[0]?.[0]).not.toHaveProperty('workspace_id');
});

it('moves with the arrow keys and selects only usable locations', async () => {
  show(hierarchyClient());
  const user = userEvent.setup();
  await screen.findByRole('radio', { name: 'Dormant / old' });
  expect(radio('New Sandbox')).toHaveAttribute('tabindex', '0');
  expect(radio('New workspace in Atlas')).toHaveAttribute('tabindex', '-1');
  radio('New Sandbox').focus();
  await user.keyboard('{ArrowDown}');
  expect(radio('New workspace in Atlas')).toHaveFocus();
  expect(radio('New workspace in Atlas')).toHaveAttribute('aria-checked', 'true');
  await user.keyboard('{ArrowDown}');
  expect(radio('New workspace in Dormant')).toHaveFocus();
  expect(radio('New workspace in Atlas')).toHaveAttribute('aria-checked', 'true');
  await user.keyboard('{End}');
  expect(radio('Dormant / old')).toHaveFocus();
  await user.keyboard('{ArrowRight}');
  expect(radio('New Sandbox')).toHaveFocus();
  expect(radio('New Sandbox')).toHaveAttribute('aria-checked', 'true');
});

it('keeps the opened Workspace selected while the hierarchy loads and fails to load', async () => {
  const listSandboxes = vi
    .fn()
    .mockRejectedValueOnce(new Error('offline'))
    .mockResolvedValue({ items: sandboxes });
  const create = vi.fn().mockRejectedValue(new Error('Lost response'));
  show(hierarchyClient({ listSandboxes, createAgentLifecycle: create }), {
    workspaceId: 'wsp_ready',
  });
  expect(screen.getByRole('radio', { name: 'Selected workspace' })).toHaveAttribute(
    'aria-checked',
    'true',
  );
  expect(screen.getByRole('status', { name: 'Loading your Sandboxes' })).toBeInTheDocument();
  expect(await screen.findByRole('alert')).toHaveTextContent('Your Sandboxes could not be loaded.');
  expect(screen.getByRole('radio', { name: 'research' })).toHaveAttribute('aria-checked', 'true');
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Try again' }));
  expect(await screen.findByRole('radio', { name: 'Atlas / research' })).toHaveAttribute(
    'aria-checked',
    'true',
  );
  expect(screen.queryByRole('radio', { name: 'research' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Create agent' }));
  await waitFor(() => expect(create).toHaveBeenCalledOnce());
  expect(create.mock.calls[0]?.[0]).toEqual({ name: 'New agent', workspace_id: 'wsp_ready' });
});

it('shows the retained request of the chosen location and keeps the others untouched', async () => {
  sessionStorage.setItem(
    lifecycleStorageKey('owner', 'wsp_ready'),
    JSON.stringify({
      version: 1,
      intent: { key: 'kept-key', body: { name: 'Research', workspace_id: 'wsp_ready' } },
    }),
  );
  const create = vi.fn().mockRejectedValue(new Error('Lost response'));
  show(hierarchyClient({ createAgentLifecycle: create }));
  const user = userEvent.setup();
  expect(screen.getByLabelText('GitHub repository (optional)')).toBeInTheDocument();
  await user.click(await screen.findByRole('radio', { name: 'Atlas / research' }));
  expect(screen.getByText(/Creation of “Research” has not been confirmed/)).toBeInTheDocument();
  expect(screen.queryByLabelText('GitHub repository (optional)')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Recover creation' }));
  await waitFor(() => expect(create).toHaveBeenCalledOnce());
  expect(create.mock.calls[0]?.[0]).toEqual({ name: 'Research', workspace_id: 'wsp_ready' });
  expect(create.mock.calls[0]?.[1].idempotencyKey).toBe('kept-key');
  await user.click(radio('New Sandbox'));
  expect(screen.getByRole('button', { name: 'Create agent' })).toBeEnabled();
  expect(screen.queryByText('Lost response')).not.toBeInTheDocument();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner', 'wsp_ready'))).toContain('kept-key');
});

it('locks the location while a request is pending', async () => {
  const create = vi.fn(() => new Promise<never>(() => undefined));
  show(hierarchyClient({ createAgentLifecycle: create }));
  const user = userEvent.setup();
  await screen.findByRole('radio', { name: 'Atlas / research' });
  await user.click(screen.getByRole('button', { name: 'Create agent' }));
  expect(screen.getByRole('button', { name: 'Starting provisioning…' })).toBeDisabled();
  expect(radio('Atlas / research')).toHaveAttribute('aria-disabled', 'true');
  await user.click(radio('Atlas / research'));
  expect(radio('New Sandbox')).toHaveAttribute('aria-checked', 'true');
  expect(create).toHaveBeenCalledOnce();
});
