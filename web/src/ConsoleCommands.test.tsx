import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import type { User } from './auth/types';
import { CmaClientProvider } from './api/context';
import { Console } from './Console';
import { agentClient } from './test/agent';

const OWNER: User = {
  id: 'owner',
  handle: 'Owner',
  visitor: false,
  capabilities: ['use_own_sandboxes'],
};

it('opens the command menu from the keyboard and goes to the chosen Agent', async () => {
  const onNavigate = vi.fn();
  render(
    <CmaClientProvider client={agentClient()}>
      <Console
        user={OWNER}
        route={{ kind: 'agent_manager' }}
        onNavigate={onNavigate}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  await screen.findByRole('tree', { name: 'Personal Agents' });
  fireEvent.keyDown(window, { key: 'k', metaKey: true });
  const field = await screen.findByRole('combobox', { name: 'Search commands' });
  const actor = userEvent.setup();
  await actor.type(field, 'Agent one');
  await actor.keyboard('{Enter}');
  expect(onNavigate).toHaveBeenCalledWith({ kind: 'agent', agentId: 'agt_1' });
  expect(screen.queryByRole('combobox', { name: 'Search commands' })).toBeNull();
});

it('opens the same menu from the Search button', async () => {
  const actor = userEvent.setup();
  render(
    <CmaClientProvider client={agentClient()}>
      <Console
        user={OWNER}
        route={{ kind: 'agent_manager' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  await actor.click(await screen.findByRole('button', { name: 'Search' }));
  expect(screen.getByRole('dialog', { name: 'Command menu' })).toBeInTheDocument();
  expect(screen.getByRole('group', { name: 'Go to' })).toBeInTheDocument();
});

it('gives visitors no search and no shortcut', async () => {
  render(
    <CmaClientProvider client={agentClient()}>
      <Console
        user={{ ...OWNER, visitor: true }}
        route={{ kind: 'agent_manager' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  await screen.findByRole('tree', { name: 'Personal Agents' });
  expect(screen.queryByRole('button', { name: 'Search' })).toBeNull();
  fireEvent.keyDown(window, { key: 'k', metaKey: true });
  expect(screen.queryByRole('dialog', { name: 'Command menu' })).toBeNull();
});

it('shows Triggers in navigation only to publishers', async () => {
  const { unmount } = render(
    <CmaClientProvider client={agentClient()}>
      <Console
        user={{ ...OWNER, capabilities: ['use_own_sandboxes', 'publish_triggers'] }}
        route={{ kind: 'agent_manager' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  expect(await screen.findByRole('link', { name: 'Triggers' })).toHaveAttribute(
    'href',
    '/triggers/manage',
  );
  unmount();
  render(
    <CmaClientProvider client={agentClient()}>
      <Console
        user={OWNER}
        route={{ kind: 'agent_manager' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  await screen.findByRole('tree', { name: 'Personal Agents' });
  expect(screen.queryByRole('link', { name: 'Triggers' })).toBeNull();
});
import './test/connectedAccounts';
