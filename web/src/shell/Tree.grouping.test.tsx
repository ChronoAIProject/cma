import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { NavigationSandbox } from './navigation';
import type { SandboxStatus } from '../api/types';
import { expect, it, vi } from 'vitest';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { Tree, type TreeProps } from './Tree';

const props: TreeProps = {
  viewerId: 'grouping-viewer',
  items: [],
  route: { kind: 'home' },
  permissions: {
    createWorkspace: false,
    createAgent: false,
    pauseSandbox: false,
    deleteSandbox: false,
    removeWorkspace: false,
    stopAgent: false,
    deleteAgent: false,
    exportHistory: false,
  },
  onNavigate: vi.fn(),
  onAction: vi.fn(),
};

it('keeps failed Sandboxes out of Live and counts them only in Needs attention', () => {
  const sandbox = sandboxFixture('failed');
  render(<Tree {...props} items={[{ sandbox, workspaces: [] }]} />);
  expect(screen.getByRole('tree', { name: 'Live sandboxes' })).not.toHaveTextContent('Atlas');
  const group = screen.getByRole('region', { name: 'Needs attention sandboxes' });
  expect(within(group).getByRole('link', { name: /Atlas/ })).toBeVisible();
  expect(group.querySelector('.resource-tree__count')).toHaveTextContent('1');
  expect(screen.getAllByRole('treeitem')).toHaveLength(1);
});

it('lists failed repository Workspaces under their own Needs attention Sandboxes', () => {
  const items = ['atlas', 'boreal'].map((name) => {
    const id = `sbx_${name}`;
    const workspace = {
      ...workspaceFixture(),
      id: `wsp_${name}`,
      sandboxId: id,
      status: 'failed' as const,
      path: `/workspaces/${name}`,
      agents: [],
    };
    return {
      sandbox: { ...sandboxFixture('failed'), id, name, workspaces: [workspace] },
      workspaces: [workspace],
    };
  });
  render(<Tree {...props} items={items} />);
  const group = within(screen.getByRole('region', { name: 'Needs attention sandboxes' }));
  for (const name of ['atlas', 'boreal']) {
    const link = group.getByRole('link', { name: new RegExp(`/workspaces/${name}`) });
    expect(link).toHaveAttribute('href', `/workspaces/wsp_${name}`);
    expect(link).toHaveTextContent('Needs attention');
  }
});

it('places each visible Sandbox once, excludes deletion progress and preserves exact status filters', async () => {
  const actor = userEvent.setup();
  const statuses = [
    'active',
    'paused',
    'failed',
    'provisioning',
    'installing',
    'pausing',
    'reviving',
    'credential_rotating',
    'rotating',
    'retiring',
    'retired',
  ] as const;
  const items: NavigationSandbox[] = statuses.map((status) => ({
    sandbox: { ...sandboxFixture(status), id: status, name: `${status} fixture` },
    workspaces: [],
  }));
  items.push({
    sandbox: {
      ...sandboxFixture('failed'),
      id: 'deletion-requested',
      name: 'Deletion requested fixture',
    },
    workspaces: [],
  });
  const treeProps = {
    ...props,
    items,
    deletingSandboxIds: new Set(['deletion-requested', 'retired']),
  };
  const view = render(<Tree {...treeProps} />);
  for (const [label, names] of [
    ['Live', ['active']],
    [
      'Changing',
      ['provisioning', 'installing', 'pausing', 'reviving', 'credential_rotating', 'rotating'],
    ],
    ['Paused', ['paused']],
    ['Needs attention', ['failed']],
  ] as const) {
    const region = screen.getByRole('region', { name: `${label} sandboxes` });
    expect(region.querySelector('.resource-tree__count')).toHaveTextContent(String(names.length));
    expect(within(region).getAllByRole('treeitem')).toHaveLength(names.length);
    for (const name of names) expect(within(region).getByText(`${name} fixture`)).toBeVisible();
  }
  expect(screen.getAllByRole('treeitem')).toHaveLength(9);
  await actor.click(screen.getByText('Cleaning up 2 sandboxes'));
  expect(screen.getByRole('button', { name: 'View status for retiring fixture' })).toBeVisible();
  expect(
    screen.getByRole('button', { name: 'View status for Deletion requested fixture' }),
  ).toBeVisible();
  expect(screen.queryByText('retired fixture')).not.toBeInTheDocument();
  view.rerender(<Tree {...treeProps} statusFilter="failed" />);
  expect(screen.getAllByRole('tree')).toHaveLength(1);
  expect(screen.getAllByRole('treeitem')).toHaveLength(1);
  expect(screen.getByRole('treeitem')).toHaveTextContent('failed fixture');
  view.rerender(
    <Tree
      {...treeProps}
      statusFilter="failed"
      items={items.filter(({ sandbox }) => sandbox.status !== 'failed')}
    />,
  );
  expect(screen.getByText('No Sandboxes match Needs attention.')).toBeVisible();
  expect(screen.queryByRole('treeitem')).not.toBeInTheDocument();
  view.rerender(<Tree {...treeProps} />);
  expect(screen.getAllByRole('treeitem')).toHaveLength(9);
  expect(props.onAction).not.toHaveBeenCalled();
});

it('moves lifecycle transitions between groups without losing the selected route or keyboard focus', async () => {
  const actor = userEvent.setup();
  const onNavigate = vi.fn();
  const renderStatus = (status: SandboxStatus) => (
    <Tree
      {...props}
      onNavigate={onNavigate}
      route={{ kind: 'sandbox', sandboxId: 'sbx_1' }}
      items={[{ sandbox: sandboxFixture(status), workspaces: [] }]}
    />
  );
  const view = render(renderStatus('active'));
  screen.getByRole('treeitem').focus();
  for (const [status, group] of [
    ['pausing', 'Changing'],
    ['paused', 'Paused'],
    ['reviving', 'Changing'],
    ['active', 'Live'],
    ['failed', 'Needs attention'],
  ] as const) {
    view.rerender(renderStatus(status));
    const row = within(screen.getByRole('tree', { name: `${group} sandboxes` })).getByRole(
      'treeitem',
    );
    expect(screen.getAllByRole('treeitem')).toHaveLength(1);
    expect(row).toHaveAttribute('aria-selected', 'true');
    expect(row).toHaveFocus();
    await actor.keyboard('{Enter}');
    expect(onNavigate).toHaveBeenLastCalledWith({ kind: 'sandbox', sandboxId: 'sbx_1' });
  }
  view.rerender(renderStatus('retiring'));
  expect(screen.queryByRole('treeitem')).not.toBeInTheDocument();
  await actor.click(screen.getByText('Cleaning up 1 sandboxes'));
  expect(screen.getByRole('button', { name: 'View status for Atlas' })).toBeVisible();
});

it('uses the current rotation for the label and filter while keeping Live and deletion disjoint', () => {
  const active = sandboxFixture();
  const rotation = {
    ...active.lifecycle!,
    operation: {
      purpose: 'image_rotation' as const,
      phase: 'publish_active',
      startedAt: active.updatedAt,
    },
  };
  const historical = {
    ...active.lifecycle!,
    result: { state: 'failed' as const, continuing: false, actions: ['observe' as const] },
  };
  const row = (name: string, status: SandboxStatus, lifecycle = active.lifecycle) => ({
    sandbox: { ...active, id: name, name, status, lifecycle },
    workspaces: [],
  });
  const items = [
    row('Running', 'active', historical),
    row('Updating before publish', 'active', rotation),
    row('Updating after publish starts', 'rotating'),
    row('Refreshing access', 'credential_rotating'),
    row('Failed', 'failed', rotation),
    row('Deleting', 'retiring', rotation),
    row('Locally deleting', 'active', rotation),
    row('Previously deleted', 'retired', rotation),
  ];
  const input = { ...props, items, deletingSandboxIds: new Set(['Locally deleting']) };
  const view = render(<Tree {...input} />);
  const liveInAll = within(screen.getByRole('tree', { name: 'Live sandboxes' })).getAllByRole(
    'treeitem',
  );
  expect(liveInAll).toHaveLength(1);
  expect(liveInAll[0]).toHaveTextContent('Running');
  view.rerender(<Tree {...input} statusFilter="active" />);
  expect(screen.getAllByRole('treeitem')).toHaveLength(1);
  expect(screen.getByRole('treeitem')).toHaveTextContent('Running');
  view.rerender(<Tree {...input} statusFilter="rotating" />);
  const updating = screen.getByRole('region', { name: 'Updating runtime sandboxes' });
  expect(updating.querySelector('.resource-tree__count')).toHaveTextContent('2');
  expect(within(updating).getAllByRole('treeitem')).toHaveLength(2);
  for (const item of within(updating).getAllByRole('treeitem')) {
    expect(item).toHaveTextContent('Updating runtime');
  }
  expect(screen.getByText('Cleaning up 2 sandboxes')).toBeVisible();
  view.rerender(<Tree {...input} statusFilter="credential_rotating" />);
  expect(screen.getByRole('treeitem')).toHaveTextContent('Refreshing access');
  expect(screen.getByRole('treeitem')).toHaveTextContent('Updating access');
});

it('remembers each new group collapse independently across filters and remounts', async () => {
  const actor = userEvent.setup();
  const items = ['failed', 'pausing'].map((status, i) => ({
    sandbox: { ...sandboxFixture(i === 0 ? 'failed' : 'pausing'), id: status, name: status },
    workspaces: [],
  }));
  const view = render(<Tree {...props} items={items} />);
  await actor.click(screen.getByRole('button', { name: 'Collapse Needs attention sandboxes' }));
  expect(screen.queryByText('failed')).not.toBeInTheDocument();
  expect(screen.getByText('pausing')).toBeVisible();
  view.rerender(<Tree {...props} items={items} statusFilter="failed" />);
  expect(screen.getByText('failed')).toBeVisible();
  view.rerender(<Tree {...props} items={items} />);
  expect(screen.queryByText('failed')).not.toBeInTheDocument();
  view.unmount();
  render(<Tree {...props} items={items} />);
  expect(screen.getByRole('button', { name: 'Expand Needs attention sandboxes' })).toHaveAttribute(
    'aria-expanded',
    'false',
  );
  expect(screen.getByText('pausing')).toBeVisible();
});

it('does not reclaim focus after the user moves to a different control', async () => {
  const actor = userEvent.setup();
  const renderStatus = (status: SandboxStatus) => (
    <>
      <button>Elsewhere</button>
      <Tree {...props} items={[{ sandbox: sandboxFixture(status), workspaces: [] }]} />
    </>
  );
  const view = render(renderStatus('active'));
  screen.getByRole('treeitem').focus();
  await actor.click(screen.getByRole('button', { name: 'Elsewhere' }));
  view.rerender(renderStatus('pausing'));
  expect(screen.getByRole('button', { name: 'Elsewhere' })).toHaveFocus();
});

it('keeps a retained Agent focused and preserves Sandbox expansion through regrouping', async () => {
  const actor = userEvent.setup();
  const renderStatus = (status: SandboxStatus) => {
    const sandbox = sandboxFixture(status);
    return (
      <Tree
        {...props}
        route={{ kind: 'agent', agentId: 'agt_1' }}
        items={[{ sandbox, workspaces: [workspaceFixture()] }]}
      />
    );
  };
  const view = render(renderStatus('active'));
  screen
    .getByRole('link', { name: /Agent one/ })
    .closest<HTMLElement>('[role="treeitem"]')!
    .focus();
  view.rerender(renderStatus('paused'));
  expect(
    screen.getByRole('link', { name: /Agent one/ }).closest('[role="treeitem"]'),
  ).toHaveFocus();
  expect(screen.getByRole('link', { name: /Agent one/ })).toHaveAttribute('aria-current', 'page');
  await actor.click(screen.getByRole('button', { name: 'Collapse Atlas' }));
  view.rerender(renderStatus('reviving'));
  expect(screen.queryByRole('link', { name: /Agent one/ })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Expand Atlas' })).toBeVisible();
  await actor.click(screen.getByRole('button', { name: 'Expand Atlas' }));
  expect(screen.getByRole('link', { name: /Agent one/ })).toBeVisible();
});
