import { render, screen, within } from '@testing-library/react';
import { createClient } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import type { AppRoute } from '../routing/routes';
import { chooseOption } from '../test/select';
import { ManagerNavigation } from './ManagerNavigation';
import { navigationFixture } from '../test/hierarchy';

const permissions = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};

const taskNavigationProps = {
  scope: '',
  choices: { options: [{ value: '', label: 'Personal' }], selected: undefined, unavailable: false },
  route: { kind: 'agent_manager' } as const,
  viewerId: 'task-navigation-viewer',
  permissions,
  pending: false,
  busySandboxIds: new Set<string>(),
  deletingSandboxIds: new Set<string>(),
  onScope: vi.fn(),
  onViewDeletion: vi.fn(),
  onNavigate: vi.fn(),
  onAction: vi.fn(),
};

it('distinguishes an unstarted ready task from a successfully completed ready task', () => {
  const items = navigationFixture().map((item) => ({
    ...item,
    workspaces: item.workspaces.map((workspace) => ({
      ...workspace,
      agents: [
        {
          ...workspace.agents[0]!,
          id: 'agt_new',
          title: 'New research',
          taskStatus: 'not_started' as const,
        },
        {
          ...workspace.agents[0]!,
          id: 'agt_done',
          title: 'Finished research',
          taskStatus: 'completed' as const,
        },
      ],
    })),
  }));
  render(
    <ManagerNavigation
      {...taskNavigationProps}
      navigation={{ status: 'ready', items, refresh: vi.fn() }}
    />,
  );
  expect(
    within(screen.getByRole('treeitem', { name: /New research/ })).getByText('Ready to start'),
  ).toBeVisible();
  expect(
    within(screen.getByRole('treeitem', { name: /Finished research/ })).getByText('Completed'),
  ).toBeVisible();
});

it('opens Sandbox management through Settings and returns to the selected filtered task with focus', async () => {
  const user = userEvent.setup();
  function Navigation() {
    const [route, navigate] = useState<AppRoute>({ kind: 'agent', agentId: 'agt_1' });
    return (
      <ManagerNavigation
        {...taskNavigationProps}
        route={route}
        onNavigate={navigate}
        navigation={{
          status: 'ready',
          items: navigationFixture('active', 'working'),
          refresh: vi.fn(),
        }}
      />
    );
  }
  render(<Navigation />);
  expect(screen.queryByRole('button', { name: 'Sandboxes' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Filter' }));
  await chooseOption(user, 'Agent status filter', 'Working');
  const task = screen.getByRole('treeitem', { name: /Agent one/ });
  task.focus();
  await user.keyboard('{Enter}');
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  await user.click(screen.getByRole('menuitem', { name: 'Manage environments' }));
  expect(screen.getByRole('button', { name: 'Back to tasks' })).toHaveFocus();
  await user.click(screen.getByRole('link', { name: /Atlas/ }));
  expect(screen.getByRole('treeitem', { name: /^Atlas/ })).toHaveAttribute('aria-selected', 'true');
  await user.click(screen.getByRole('button', { name: 'Back to tasks' }));
  expect(screen.getByRole('treeitem', { name: /Agent one/ })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  expect(screen.getByRole('treeitem', { name: /Agent one/ })).toHaveFocus();
  expect(screen.getByRole('button', { name: 'Agent status filter' })).toHaveTextContent('Working');
});

it('uses human task titles and keeps an owner title ahead of runtime and configured titles', () => {
  const items = navigationFixture().map((item) => ({
    ...item,
    workspaces: item.workspaces.map((workspace) => ({
      ...workspace,
      agents: [
        { ...workspace.agents[0]!, title: '   ' },
        { ...workspace.agents[0]!, id: 'agt_private', title: 'agt_private' },
        {
          ...workspace.agents[0]!,
          id: 'agt_named',
          title: 'Configured name',
          ownerTitle: 'My chosen task',
          runtimeMetadata: { description: 'Provider title' },
        },
      ],
    })),
  }));
  render(
    <ManagerNavigation
      {...taskNavigationProps}
      navigation={{ status: 'ready', items, refresh: vi.fn() }}
    />,
  );
  expect(screen.getAllByRole('treeitem', { name: /^Untitled task/ })).toHaveLength(2);
  expect(screen.getByRole('treeitem', { name: /^My chosen task/ })).toBeVisible();
  expect(screen.queryByText('agt_private')).not.toBeInTheDocument();
});

it('restores shared task scope, focus and scroll after browsing a different environment scope', async () => {
  const user = userEvent.setup();
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      if (new URL(String(input)).pathname === '/api/v1/agents/provenance')
        return Response.json({
          items: [
            {
              agent_id: 'agt_shared',
              title: 'Shared research',
              status: 'ready',
              task_status: 'completed',
              created_at: '2026-11-03T00:00:00Z',
              updated_at: '2026-11-03T00:00:00Z',
              creation_origin: 'manual',
            },
          ],
        });
      return Response.json({ code: 'not_found' }, { status: 404 });
    },
  });
  function Navigation() {
    const [scope, onScope] = useState('shared');
    const [route, onNavigate] = useState<AppRoute>({ kind: 'agent', agentId: 'agt_shared' });
    return (
      <div className="sidebar__navigation" data-testid="navigation-scroll">
        <ManagerNavigation
          {...taskNavigationProps}
          scope={scope}
          onScope={onScope}
          route={route}
          onNavigate={onNavigate}
          client={client}
          choices={{
            options: [
              { value: '', label: 'Personal' },
              { value: 'shared', label: 'Shared' },
            ],
            selected: undefined,
            unavailable: false,
          }}
          navigation={{ status: 'ready', items: navigationFixture(), refresh: vi.fn() }}
        />
      </div>
    );
  }
  render(<Navigation />);
  const row = await screen.findByRole('treeitem', { name: /^Shared research/ });
  row.focus();
  const scrollport = screen.getByTestId('navigation-scroll');
  scrollport.scrollTop = 240;
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  await user.click(screen.getByRole('menuitem', { name: 'Manage environments' }));
  await user.click(screen.getByRole('button', { name: 'Filter' }));
  await chooseOption(user, 'Navigation scope', 'Personal');
  await user.click(screen.getByRole('link', { name: /Atlas/ }));
  scrollport.scrollTop = 0;
  await user.click(screen.getByRole('button', { name: 'Back to tasks' }));
  expect(screen.getByRole('button', { name: 'Navigation scope' })).toHaveTextContent('Shared');
  expect(await screen.findByRole('treeitem', { name: /^Shared research/ })).toHaveFocus();
  expect(screen.getByRole('treeitem', { name: /^Shared research/ })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  expect(scrollport.scrollTop).toBe(240);
});

it('filters by completed task outcome without confusing ready or failed turns', async () => {
  const user = userEvent.setup();
  const items = navigationFixture().map((item) => ({
    ...item,
    workspaces: item.workspaces.map((workspace) => ({
      ...workspace,
      agents: [
        {
          ...workspace.agents[0]!,
          id: 'agt_done',
          title: 'Finished task',
          taskStatus: 'completed' as const,
        },
        {
          ...workspace.agents[0]!,
          id: 'agt_new',
          title: 'New task',
          taskStatus: 'not_started' as const,
        },
        {
          ...workspace.agents[0]!,
          id: 'agt_failed',
          title: 'Failed task',
          taskStatus: 'failed' as const,
        },
      ],
    })),
  }));
  render(
    <ManagerNavigation
      {...taskNavigationProps}
      navigation={{ status: 'ready', items, refresh: vi.fn() }}
    />,
  );
  await user.click(screen.getByRole('button', { name: 'Filter' }));
  await chooseOption(user, 'Agent status filter', 'Completed');
  expect(screen.getByRole('treeitem', { name: /^Finished task/ })).toBeVisible();
  expect(screen.queryByRole('treeitem', { name: /^New task/ })).not.toBeInTheDocument();
  await chooseOption(user, 'Agent status filter', 'Failed');
  expect(screen.getByRole('treeitem', { name: /^Failed task/ })).toBeVisible();
  expect(screen.queryByRole('treeitem', { name: /^Finished task/ })).not.toBeInTheDocument();
});

it('preserves a failed turn under a paused parent while holding successful tasks', () => {
  const items = navigationFixture('paused').map((item) => ({
    ...item,
    workspaces: item.workspaces.map((workspace) => ({
      ...workspace,
      agents: [
        {
          ...workspace.agents[0]!,
          id: 'agt_done',
          title: 'Finished task',
          taskStatus: 'completed' as const,
        },
        {
          ...workspace.agents[0]!,
          id: 'agt_failed',
          title: 'Failed task',
          taskStatus: 'failed' as const,
        },
      ],
    })),
  }));
  render(
    <ManagerNavigation
      {...taskNavigationProps}
      navigation={{ status: 'ready', items, refresh: vi.fn() }}
    />,
  );
  expect(
    within(screen.getByRole('treeitem', { name: /^Finished task/ })).getByText('Paused'),
  ).toBeVisible();
  expect(
    within(screen.getByRole('treeitem', { name: /^Failed task/ })).getByText('Failed'),
  ).toBeVisible();
});
