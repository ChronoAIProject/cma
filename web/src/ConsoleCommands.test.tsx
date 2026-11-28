import { fireEvent, render, screen, within } from '@testing-library/react';
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

it.each(['environment', 'environments'])(
  'opens the sidebar Environments view from %s search',
  async (query) => {
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
    await screen.findByRole('tree', { name: 'Personal Agents' });
    expect(screen.getByRole('button', { name: 'Environments', pressed: false })).toBeVisible();
    await actor.click(screen.getByRole('button', { name: 'Search' }));
    await actor.type(screen.getByRole('combobox', { name: 'Search commands' }), query);
    expect(
      within(screen.getByRole('group', { name: 'Go to' })).getByRole('option', {
        name: 'Environments',
        selected: true,
      }),
    ).toBeVisible();
    await actor.keyboard('{Enter}');
    expect(screen.queryByRole('dialog', { name: 'Command menu' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Environments', pressed: true })).toBeVisible();
    expect(await screen.findByRole('button', { name: 'Options for Atlas' })).toBeVisible();
  },
);

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

it('keeps global New agent unscoped and existing-Sandbox creation in its row menu', async () => {
  const onCreate = vi.fn();
  const actor = userEvent.setup();
  render(
    <CmaClientProvider client={agentClient()}>
      <Console
        user={OWNER}
        route={{ kind: 'agent_manager' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
        onCreate={onCreate}
      />
    </CmaClientProvider>,
  );
  const agents = await screen.findByRole('tree', { name: 'Personal Agents' });
  expect(agents).not.toHaveTextContent('Atlas');
  expect(agents).not.toHaveTextContent('/workspaces/chrono-bot');
  await actor.click(
    within(screen.getByRole('complementary', { name: 'Application navigation' })).getByRole(
      'button',
      { name: 'New agent' },
    ),
  );
  expect(onCreate).toHaveBeenLastCalledWith({ kind: 'new_agent' });
  await actor.click(
    within(screen.getByRole('complementary', { name: 'Application navigation' })).getByRole(
      'button',
      { name: 'Environments' },
    ),
  );
  await actor.click(await screen.findByRole('button', { name: 'Options for Atlas' }));
  await actor.click(screen.getByRole('menuitem', { name: 'New agent' }));
  expect(onCreate).toHaveBeenLastCalledWith({ kind: 'new_agent', sandboxId: 'sbx_1' });
  await actor.click(
    within(screen.getByRole('complementary', { name: 'Application navigation' })).getByRole(
      'button',
      { name: 'New agent' },
    ),
  );
  expect(onCreate).toHaveBeenLastCalledWith({ kind: 'new_agent' });
});
