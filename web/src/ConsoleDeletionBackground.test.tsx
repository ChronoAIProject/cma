import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import type { User } from './auth/types';
import { Console } from './Console';
import { agentClient } from './test/agent';
import { sandboxFixture } from './test/hierarchy';
import { persistDeletion, restoreDeletions } from './deletionRecovery';

it('keeps accepted cleanup optional across navigation and allows deleting another Sandbox', async () => {
  const actor = userEvent.setup();
  const user: User = {
    id: 'owner',
    handle: 'Owner',
    visitor: false,
    capabilities: ['create_sandbox', 'use_own_sandboxes', 'delete_own_sandbox'],
  };
  const sandboxes = ['first', 'second'].map((id) => ({
    ...sandboxFixture(),
    id,
    name: id,
    workspaces: [],
    workspaceCount: 0,
    worktreeCount: 0,
    agentCount: 0,
  }));
  const deleteSandbox = vi.fn().mockResolvedValue(undefined);
  const client = agentClient({
    deleteSandbox,
    listSandboxes: async () => ({ items: sandboxes }),
    readSandbox: async (id) => sandboxes.find((sandbox) => sandbox.id === id)!,
  });
  const draw = (id: string) => (
    <CmaClientProvider client={client}>
      <Console
        user={user}
        route={{ kind: 'sandbox', sandboxId: id }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>
  );
  const view = render(draw('first'));
  const remove = async () => {
    await actor.click(await screen.findByRole('button', { name: 'More actions' }));
    await actor.click(screen.getByRole('menuitem', { name: 'Delete Sandbox' }));
    await actor.click(
      await screen.findByRole('checkbox', {
        name: 'I understand this Sandbox will be deleted.',
      }),
    );
    await actor.click(screen.getByRole('button', { name: 'Request deletion' }));
  };
  await remove();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.queryByRole('treeitem', { name: /first/ })).not.toBeInTheDocument();
  expect(screen.getByText('Cleaning up 1 sandboxes')).toBeInTheDocument();
  view.rerender(draw('second'));
  await act(async () => {});
  expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
  await remove();
  expect(deleteSandbox.mock.calls.map(([id]) => id)).toEqual(['first', 'second']);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByText('Cleaning up 2 sandboxes')).toBeInTheDocument();
  view.unmount();
  render(draw('second'));
  await act(async () => {});
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'View deletion status' }));
  const progress = screen.getByRole('dialog', { name: 'Deletion progress' });
  const second = within(progress).getByText('second', { selector: 'code' }).closest('li')!;
  await actor.click(within(second).getByRole('button'));
  expect(screen.getByRole('dialog', { name: 'Deleting Sandbox' })).toBeInTheDocument();
  expect(deleteSandbox).toHaveBeenCalledTimes(2);
});

it('reconciles accepted notices when an existing page read proves retirement after re-entry', async () => {
  const request = {
    action: { kind: 'delete_sandbox' as const, sandboxId: 'sbx_1' },
    key: 'synthetic-background-request',
    accepted: true,
  };
  persistDeletion('owner', request);
  const readSandbox = vi.fn().mockResolvedValue(sandboxFixture('retiring'));
  const client = agentClient({ readSandbox, listSandboxes: async () => ({ items: [] }) });
  const draw = (home: boolean) => (
    <CmaClientProvider client={client}>
      <Console
        user={{ id: 'owner', handle: 'Owner', visitor: false, capabilities: ['use_own_sandboxes'] }}
        route={home ? { kind: 'home' } : { kind: 'sandbox', sandboxId: 'sbx_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>
  );
  const view = render(draw(false));
  expect(await screen.findByRole('region', { name: 'Deletion request' })).toHaveTextContent(
    'Deleting 1 environment',
  );
  view.rerender(draw(true));
  await act(async () => {});
  expect(restoreDeletions('owner')).toEqual([request]);
  readSandbox.mockResolvedValue(sandboxFixture('retired'));
  view.rerender(draw(false));
  await act(async () => {});
  expect(screen.queryByRole('region', { name: 'Deletion request' })).not.toBeInTheDocument();
  expect(screen.queryByText('Cleaning up 1 sandboxes')).not.toBeInTheDocument();
  expect(restoreDeletions('owner')).toEqual([]);
});
