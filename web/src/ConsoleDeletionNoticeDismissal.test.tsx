import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import { Console } from './Console';
import { persistDeletion, restoreDeletions } from './deletionRecovery';
import type { AppRoute } from './routing/routes';
import { agentClient } from './test/agent';
import { sandboxFixture } from './test/hierarchy';

it('remembers dismissal across navigation and reload while all progress remains reachable', async () => {
  const actor = userEvent.setup();
  const requests = [
    {
      key: 'environment-request',
      accepted: true,
      action: { kind: 'delete_sandbox' as const, sandboxId: 'sbx_1' },
    },
    {
      key: 'agent-request',
      accepted: true,
      action: { kind: 'delete_agent' as const, agentId: 'agt_1' },
    },
    {
      key: 'workspace-request',
      accepted: true,
      action: { kind: 'remove_workspace' as const, workspaceId: 'ws_1' },
    },
  ];
  requests.forEach((request) => persistDeletion('owner', request));
  const deleteSandbox = vi.fn();
  const readSandbox = vi.fn().mockResolvedValue(sandboxFixture('retiring'));
  const client = agentClient({
    deleteSandbox,
    readSandbox,
    listSandboxes: async () => ({ items: [] }),
  });
  const draw = (route: AppRoute, viewer = 'owner') => (
    <CmaClientProvider client={client}>
      <Console
        user={{ id: viewer, handle: viewer, visitor: false, capabilities: ['use_own_sandboxes'] }}
        route={route}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>
  );
  const environment: AppRoute = { kind: 'sandbox', sandboxId: 'sbx_1' };
  const view = render(draw(environment));
  await actor.click(await screen.findByRole('button', { name: 'Dismiss deletion notice' }));
  expect(screen.queryByRole('region', { name: 'Deletion request' })).not.toBeInTheDocument();
  view.rerender(draw({ kind: 'home' }));
  await act(async () => {});
  expect(screen.queryByRole('region', { name: 'Deletion request' })).not.toBeInTheDocument();
  view.unmount();
  const reloaded = render(draw(environment));
  await act(async () => {});
  expect(screen.queryByRole('region', { name: 'Deletion request' })).not.toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'View deletion status' }));
  const progress = screen.getByRole('dialog', { name: 'Deletion progress' });
  expect(within(progress).getAllByRole('listitem')).toHaveLength(3);
  expect(restoreDeletions('owner')).toEqual(expect.arrayContaining(requests));
  expect(deleteSandbox).not.toHaveBeenCalled();
  await actor.keyboard('{Escape}');
  readSandbox.mockResolvedValue(sandboxFixture('retired'));
  reloaded.rerender(draw({ kind: 'home' }));
  reloaded.rerender(draw(environment));
  await act(async () => {});
  expect(screen.queryByRole('region', { name: 'Deletion request' })).not.toBeInTheDocument();
  expect(restoreDeletions('owner')).toEqual(expect.arrayContaining(requests.slice(1)));
  expect(restoreDeletions('owner')).toHaveLength(2);
});

it('keeps restored notices off unrelated pages and exposes them inside Environments', async () => {
  const actor = userEvent.setup();
  persistDeletion('owner', {
    key: 'old-request',
    accepted: true,
    action: { kind: 'delete_agent', agentId: 'agt_1' },
  });
  const client = agentClient({ listSandboxes: async () => ({ items: [] }) });
  render(
    <CmaClientProvider client={client}>
      <Console
        user={{ id: 'owner', handle: 'Owner', visitor: false, capabilities: ['use_own_sandboxes'] }}
        route={{ kind: 'home' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  await act(async () => {});
  expect(screen.queryByRole('region', { name: 'Deletion request' })).not.toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'Environments' }));
  const notice = screen.getByRole('region', { name: 'Deletion request' });
  expect(notice.closest('main')).toBeNull();
  await actor.click(within(notice).getByRole('button', { name: 'Dismiss deletion notice' }));
  expect(screen.queryByRole('region', { name: 'Deletion request' })).not.toBeInTheDocument();
  expect(restoreDeletions('owner')).toHaveLength(1);
});
