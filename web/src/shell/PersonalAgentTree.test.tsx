import { render } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
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
  options: Partial<Pick<TreeProps, 'active' | 'route' | 'refreshError'>> = {},
) {
  const items = navigationFixture().map((item) => ({
    ...item,
    workspaces: item.workspaces.map((workspace) => ({
      ...workspace,
      agents: [
        ...workspace.agents,
        { ...workspace.agents[0]!, id: 'agt_2', updatedAt: '2026-09-07T00:00:00Z' },
      ],
    })),
  }));
  return (
    <PersonalAgentTree
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
  it('prepares only the most recently updated Agent and releases on unmount', () => {
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
