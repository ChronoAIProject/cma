import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { TreePermissions } from '../shell/Tree';
import { agentClient } from '../test/agent';
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
  expect(await screen.findByText('workspace created')).toBeInTheDocument();
  expect(screen.getByText('created')).toBeInTheDocument();
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
