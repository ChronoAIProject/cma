import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { CmaWebClient } from '../api/client';
import { agentClient } from '../test/agent';
import { AgentLifecycleCard } from './AgentLifecycleCard';
import { lifecycleStorageKey } from './lifecycleIntent';

beforeEach(() => sessionStorage.clear());
afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

function show(client: CmaWebClient, props: { workspaceId?: string; sandboxId?: string } = {}) {
  const fixed = { client, viewerId: 'owner', onClose: vi.fn(), onCreated: vi.fn() };
  return { ...render(<AgentLifecycleCard {...fixed} {...props} />), fixed };
}

it.each([
  [{}, 'Start an Agent in a new Sandbox with an empty workspace.', {}],
  [
    { sandboxId: 'sbx_1' },
    'Start an Agent with a new workspace in this Sandbox.',
    { sandbox_id: 'sbx_1' },
  ],
  [
    { workspaceId: 'wsp_1' },
    'Start an Agent in the selected workspace.',
    { workspace_id: 'wsp_1' },
  ],
  [
    { workspaceId: 'wsp_1', sandboxId: 'sbx_1' },
    'Start an Agent in the selected workspace.',
    { workspace_id: 'wsp_1', sandbox_id: 'sbx_1' },
  ],
])(
  'keeps invocation %j fixed without a location picker or hierarchy request',
  async (props, description, ids) => {
    const create = vi.fn().mockRejectedValue(new Error('Lost response'));
    const listSandboxes = vi.fn(() => new Promise<never>(() => undefined));
    const readWorkspace = vi.fn(() => new Promise<never>(() => undefined));
    show(agentClient({ createAgentLifecycle: create, listSandboxes, readWorkspace }), props);
    expect(screen.queryByRole('radiogroup')).not.toBeInTheDocument();
    expect(screen.queryByText('Where it works')).not.toBeInTheDocument();
    expect(screen.getByText(description)).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Create agent' }));
    await waitFor(() => expect(create).toHaveBeenCalledOnce());
    expect(create.mock.calls[0]?.[0]).toEqual({ name: 'New agent', ...ids });
    expect(listSandboxes).not.toHaveBeenCalled();
    expect(readWorkspace).not.toHaveBeenCalled();
  },
);

it('keeps scoped recovery out of the global new-Sandbox action', async () => {
  sessionStorage.setItem(
    lifecycleStorageKey('owner', 'wsp_1'),
    JSON.stringify({
      version: 1,
      intent: { key: 'kept-key', body: { name: 'Research', workspace_id: 'wsp_1' } },
    }),
  );
  const create = vi.fn().mockRejectedValue(new Error('Lost response'));
  const client = agentClient({ createAgentLifecycle: create });
  const view = show(client);
  expect(screen.getByLabelText('GitHub repository (optional)')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Recover creation' })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Create agent' }));
  await waitFor(() => expect(create).toHaveBeenCalledOnce());
  expect(create.mock.calls[0]?.[0]).toEqual({ name: 'New agent' });
  expect(sessionStorage.getItem(lifecycleStorageKey('owner', 'wsp_1'))).toContain('kept-key');
  view.rerender(<AgentLifecycleCard {...view.fixed} workspaceId="wsp_1" />);
  expect(screen.getByText(/Creation of “Research” has not been confirmed/)).toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Recover creation' }));
  await waitFor(() => expect(create).toHaveBeenCalledTimes(2));
  expect(create.mock.calls[1]?.[0]).toEqual({ name: 'Research', workspace_id: 'wsp_1' });
  expect(create.mock.calls[1]?.[1].idempotencyKey).toBe('kept-key');
});

it('preserves a global request while it is pending and submits only once', async () => {
  const create = vi.fn<NonNullable<CmaWebClient['createAgentLifecycle']>>(
    () => new Promise<never>(() => undefined),
  );
  show(agentClient({ createAgentLifecycle: create }));
  await userEvent.click(screen.getByRole('button', { name: 'Create agent' }));
  expect(screen.getByRole('button', { name: 'Starting provisioning…' })).toBeDisabled();
  expect(screen.queryByRole('radio')).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Starting provisioning…' }));
  expect(create).toHaveBeenCalledOnce();
  expect(create.mock.calls[0]?.[0]).toEqual({ name: 'New agent' });
});
