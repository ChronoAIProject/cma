import { render, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { navigationFixture } from '../test/hierarchy';
import { PersonalAgentTree } from './PersonalAgentTree';
import type { TreePermissions, TreeProps } from './Tree';

const permissions: TreePermissions = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};

function tree(
  onAgentIntent: (agentId: string, active: boolean) => void,
  options: Partial<Pick<TreeProps, 'active' | 'route' | 'refreshError' | 'items'>> = {},
) {
  const items = navigationFixture().map((item) => ({
    ...item,
    workspaces: item.workspaces.map((workspace) => ({
      ...workspace,
      agents: [
        ...workspace.agents,
        { ...workspace.agents[0]!, id: 'agt_2', taskActivityAt: '2026-09-07T00:00:00Z' },
      ],
    })),
  }));
  return (
    <PersonalAgentTree
      viewerId="owner"
      active
      items={items}
      statusFilter="all"
      route={{ kind: 'home' }}
      permissions={permissions}
      lifecycleBusy={false}
      busySandboxIds={new Set()}
      onNavigate={vi.fn()}
      onAgentIntent={onAgentIntent}
      onAction={vi.fn()}
      {...options}
    />
  );
}

describe('PersonalAgentTree list preparation', () => {
  it('prepares only the Agent with the most recent task activity and releases on unmount', () => {
    const onAgentIntent = vi.fn();
    const view = render(tree(onAgentIntent));
    expect(onAgentIntent.mock.calls).toEqual([['agt_2', true]]);
    view.unmount();
    expect(onAgentIntent.mock.calls).toEqual([
      ['agt_2', true],
      ['agt_2', false],
    ]);
  });

  it.each([
    { active: false },
    { refreshError: 'Navigation refresh unavailable' },
    { route: { kind: 'agent' as const, agentId: 'agt_route' } },
  ])('does not prepare an inactive, stale, or route-owned list: %j', (options) => {
    const onAgentIntent = vi.fn();
    render(tree(onAgentIntent, options));
    expect(onAgentIntent).not.toHaveBeenCalled();
  });

  it('releases the list intent as soon as route-owned opening starts', () => {
    const onAgentIntent = vi.fn();
    const view = render(tree(onAgentIntent));
    view.rerender(tree(onAgentIntent, { route: { kind: 'agent', agentId: 'agt_2' } }));
    expect(onAgentIntent.mock.calls).toEqual([
      ['agt_2', true],
      ['agt_2', false],
    ]);
  });
});

afterEach(() => vi.useRealTimers());

it('keeps completed task ordering, groups and times stable through connection maintenance', () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 10, 20, 12));
  const yesterday = new Date(2026, 10, 19, 17).toISOString();
  const today = new Date(2026, 10, 20, 9).toISOString();
  const recent = new Date(2026, 10, 19, 18).toISOString();
  const items = navigationFixture().map((item) => ({
    ...item,
    workspaces: item.workspaces.map((workspace) => ({
      ...workspace,
      agents: [
        {
          ...workspace.agents[0]!,
          id: 'idle',
          title: 'Idle task',
          taskStatus: 'completed' as const,
          taskActivityAt: yesterday,
          updatedAt: today,
        },
        {
          ...workspace.agents[0]!,
          id: 'recent',
          title: 'Recent task',
          taskStatus: 'completed' as const,
          taskActivityAt: recent,
          updatedAt: recent,
        },
      ],
    })),
  }));
  const view = render(tree(vi.fn(), { items }));
  const group = screen.getByRole('group', { name: 'Yesterday' });
  expect(
    within(group)
      .getAllByRole('treeitem')
      .map((row) => row.textContent),
  ).toEqual([expect.stringContaining('Recent task'), expect.stringContaining('Idle task')]);
  expect(within(group).getByRole('treeitem', { name: /Idle task/ })).toHaveTextContent('Nov 19');
  expect(screen.queryByRole('group', { name: 'Today' })).not.toBeInTheDocument();
  for (let publication = 0; publication < 32; publication++) {
    items[0]!.workspaces[0]!.agents[0]!.updatedAt = new Date(
      2026,
      10,
      20,
      9,
      publication,
    ).toISOString();
    view.rerender(tree(vi.fn(), { items: [...items] }));
    expect(
      screen.getByRole('group', { name: 'Yesterday' }).querySelectorAll('time')[1],
    ).toHaveAttribute('datetime', yesterday);
  }
  items[0]!.workspaces[0]!.agents[0]!.taskActivityAt = today;
  view.rerender(tree(vi.fn(), { items: [...items] }));
  expect(
    within(screen.getByRole('group', { name: 'Today' })).getByRole('treeitem'),
  ).toHaveAccessibleName(/Idle task/);
});

it('shows missing task time explicitly and orders undated tasks by ID', () => {
  const items = navigationFixture().map((item) => ({
    ...item,
    workspaces: item.workspaces.map((workspace) => ({
      ...workspace,
      agents: ['z', 'a'].map((id) => ({
        ...workspace.agents[0]!,
        id,
        title: `Task ${id}`,
        taskStatus: 'completed' as const,
        taskActivityAt: undefined,
      })),
    })),
  }));
  render(tree(vi.fn(), { items }));
  const group = screen.getByRole('group', { name: 'Task time unavailable' });
  expect(
    within(group)
      .getAllByRole('treeitem')
      .map((row) => row.getAttribute('aria-label')),
  ).toEqual([
    'Task a, Turn ended, Task time unavailable',
    'Task z, Turn ended, Task time unavailable',
  ]);
  expect(group.querySelector('time')).toBeNull();
});
