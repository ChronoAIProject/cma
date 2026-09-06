import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { TreePermissions } from '../shell/Tree';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { SandboxPage } from './SandboxPage';

const permissions: TreePermissions = {
  createWorkspace: false,
  createAgent: false,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
};

it.each(['active', 'paused', 'reviving'] as const)(
  'allows normal child creation from a %s parent without an explicit revive action',
  async (status) => {
    const onAction = vi.fn();
    render(
      <SandboxPage
        client={agentClient({ readSandbox: async () => sandboxFixture(status) })}
        sandboxId="sbx_1"
        revision={0}
        permissions={{ ...permissions, createWorkspace: true, createAgent: true }}
        busy={false}
        onAction={onAction}
        onNavigate={vi.fn()}
      />,
    );
    const workspace = await screen.findByRole('button', { name: 'New workspace' });
    const agent = screen.getByRole('button', { name: 'New agent' });
    expect(workspace).toBeEnabled();
    expect(agent).toBeEnabled();
    workspace.click();
    agent.click();
    expect(onAction.mock.calls).toEqual([
      [{ kind: 'new_workspace', sandboxId: 'sbx_1' }],
      [{ kind: 'new_agent', workspaceId: 'wsp_1' }],
    ]);
  },
);

it.each(['provisioning', 'pausing', 'retiring'] as const)(
  'keeps child creation disabled for a %s parent',
  async (status) => {
    render(
      <SandboxPage
        client={agentClient({ readSandbox: async () => sandboxFixture(status) })}
        sandboxId="sbx_1"
        revision={0}
        permissions={{ ...permissions, createWorkspace: true, createAgent: true }}
        busy={false}
        onAction={vi.fn()}
        onNavigate={vi.fn()}
      />,
    );
    expect(await screen.findByRole('button', { name: 'New workspace' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'New agent' })).toBeDisabled();
  },
);

it('pages the complete Sandbox event timeline before displaying it', async () => {
  const readSandboxEvents = vi
    .fn()
    .mockResolvedValueOnce([
      { sandboxId: 'sbx_1', seq: 1, appendedAt: '2026-09-06T00:00:00Z', kind: { type: 'created' } },
    ])
    .mockResolvedValueOnce([
      {
        sandboxId: 'sbx_1',
        seq: 2,
        appendedAt: '2026-09-06T00:01:00Z',
        kind: { type: 'workspace_created' },
      },
    ])
    .mockResolvedValueOnce([]);
  render(
    <SandboxPage
      client={agentClient({ readSandboxEvents })}
      sandboxId="sbx_1"
      revision={0}
      permissions={permissions}
      busy={false}
      onAction={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  expect(await screen.findByText('Workspace created')).toBeInTheDocument();
  expect(screen.getByText('Sandbox created')).toBeInTheDocument();
  expect(readSandboxEvents.mock.calls.map((call) => call[1].cursor)).toEqual([0, 1, 2]);
});

it('fails closed for a nonadvancing Sandbox event page', async () => {
  const readSandboxEvents = vi
    .fn()
    .mockResolvedValue([
      { sandboxId: 'sbx_1', seq: 1, appendedAt: '2026-09-06T00:00:00Z', kind: { type: 'created' } },
    ]);
  render(
    <SandboxPage
      client={agentClient({ readSandboxEvents })}
      sandboxId="sbx_1"
      revision={0}
      permissions={permissions}
      busy={false}
      onAction={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  expect(await screen.findByRole('alert')).toHaveTextContent('Sandbox events did not advance.');
  expect(readSandboxEvents).toHaveBeenCalledTimes(2);
});
