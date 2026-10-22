import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ManagerNavigation } from './ManagerNavigation';
import type { TreePermissions } from './Tree';
import { navigationFixture } from '../test/hierarchy';
import { chooseOption } from '../test/select';

const permissions: TreePermissions = {
  createWorkspace: false,
  createAgent: false,
  pauseSandbox: false,
  deleteSandbox: false,
  removeWorkspace: false,
  stopAgent: false,
  deleteAgent: false,
  exportHistory: false,
};
const props = {
  scope: '',
  choices: { options: [{ value: '', label: 'Personal' }], selected: undefined, unavailable: false },
  route: { kind: 'agent_manager' } as const,
  viewerId: 'navigation-viewer',
  permissions,
  pending: false,
  busySandboxIds: new Set<string>(),
  deletingSandboxIds: new Set<string>(),
  onScope: vi.fn(),
  onViewDeletion: vi.fn(),
  onNavigate: vi.fn(),
  onAction: vi.fn(),
};

it('starts with all personal Agents, retains each status filter and the Sandbox tree disclosure', async () => {
  render(
    <ManagerNavigation
      {...props}
      navigation={{ status: 'ready', items: navigationFixture(), refresh: vi.fn() }}
    />,
  );
  const user = userEvent.setup();
  expect(screen.getByRole('button', { name: 'Agents' })).toHaveAttribute('aria-pressed', 'true');
  const summary = { selector: '.navigation-filters__summary' };
  const filter = screen.getByRole('button', { name: 'Filter' });
  // Defaults are not restated: no dot, no summary line, no description.
  expect(document.querySelector('.navigation-filters__summary')).toBeNull();
  expect(filter.querySelector('.navigation-filters__dot')).toBeNull();
  expect(filter).not.toHaveAttribute('aria-describedby');
  await user.click(filter);
  expect(screen.getByRole('button', { name: 'Agent status filter' })).toHaveTextContent(
    'All statuses',
  );
  expect(screen.getByRole('tree', { name: 'Personal Agents' })).toHaveTextContent('Agent one');
  await chooseOption(user, 'Agent status filter', 'Working');
  expect(screen.queryByRole('link', { name: /Agent one/ })).not.toBeInTheDocument();
  expect(filter).toHaveAccessibleDescription('Working');
  expect(filter.querySelector('.navigation-filters__dot')).not.toBeNull();
  expect(screen.getByText('Working', summary)).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Sandboxes' }));
  expect(screen.getByRole('button', { name: 'Sandbox status filter' })).toHaveTextContent(
    'All statuses',
  );
  // Each view has its own status, so the Sandbox view is back at its defaults.
  expect(document.querySelector('.navigation-filters__summary')).toBeNull();
  expect(filter.querySelector('.navigation-filters__dot')).toBeNull();
  expect(screen.getByRole('tree', { name: 'Live sandboxes' })).toHaveTextContent('chrono-bot');
  await user.click(screen.getByRole('button', { name: 'Collapse Atlas' }));
  await user.click(screen.getByRole('button', { name: 'Agents' }));
  expect(screen.getByRole('button', { name: 'Agent status filter' })).toHaveTextContent('Working');
  await user.click(screen.getByRole('button', { name: 'Sandboxes' }));
  expect(screen.getByRole('button', { name: 'Expand Atlas' })).toBeInTheDocument();
  expect(screen.queryByRole('link', { name: /Agent one/ })).not.toBeInTheDocument();
});

it('uses effective parent lifecycle status and the shared status tones for dots', async () => {
  const refresh = vi.fn();
  const dot = () =>
    screen.getByRole('tree', { name: 'Personal Agents' }).querySelector('.statedot');
  const view = render(
    <ManagerNavigation
      {...props}
      navigation={{ status: 'ready', items: navigationFixture('active', 'ready'), refresh }}
    />,
  );
  // Ready is settled and usable: a quiet dot, never the success colour.
  expect(dot()).toHaveAttribute('data-state', 'neutral');
  view.rerender(
    <ManagerNavigation
      {...props}
      navigation={{ status: 'ready', items: navigationFixture('active', 'working'), refresh }}
    />,
  );
  expect(dot()).toHaveAttribute('data-state', 'ongoing');
  view.rerender(
    <ManagerNavigation
      {...props}
      navigation={{
        status: 'ready',
        items: navigationFixture('active', 'waiting_approval'),
        refresh,
      }}
    />,
  );
  expect(dot()).toHaveAttribute('data-state', 'warning');
  expect(screen.getByText('Needs approval').closest('.tree-row__state')).toHaveAttribute(
    'data-tone',
    'warning',
  );
  view.rerender(
    <ManagerNavigation
      {...props}
      navigation={{ status: 'ready', items: navigationFixture('paused', 'working'), refresh }}
    />,
  );
  // A held state draws no dot at all.
  expect(dot()).toBeNull();
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Filter' }));
  await chooseOption(user, 'Agent status filter', 'Paused');
  expect(screen.getByRole('link', { name: /Agent one/ })).toHaveTextContent('Paused');
  await chooseOption(user, 'Agent status filter', 'Working');
  expect(screen.queryByRole('link', { name: /Agent one/ })).not.toBeInTheDocument();
  view.rerender(
    <ManagerNavigation
      {...props}
      navigation={{ status: 'ready', items: navigationFixture(), refresh }}
    />,
  );
  await user.click(screen.getByRole('button', { name: 'Sandboxes' }));
  const live = screen.getByRole('tree', { name: 'Live sandboxes' });
  expect(live.querySelector('.tree-row__state .statedot')).toHaveAttribute('data-state', 'neutral');
  expect(live.querySelectorAll('.statedot[data-state="done"]')).toHaveLength(0);
});

it('lists Agents without Sandbox or Workspace groups, most recently active first', () => {
  const [item] = navigationFixture();
  if (item === undefined) throw new Error('fixture');
  const [workspace] = item.workspaces;
  if (workspace === undefined) throw new Error('fixture');
  const older = {
    ...workspace.agents[0]!,
    id: 'agt_old',
    title: 'Older',
    updatedAt: '2026-09-01T00:00:00Z',
  };
  const newer = {
    ...workspace.agents[0]!,
    id: 'agt_new',
    title: 'Newer',
    updatedAt: '2026-09-07T00:00:00Z',
  };
  const quiet = {
    ...workspace,
    id: 'wsp_quiet',
    name: 'quiet-workspace',
    agents: [
      {
        ...workspace.agents[0]!,
        id: 'agt_quiet',
        title: 'Quiet',
        workspaceId: 'wsp_quiet',
        updatedAt: '2026-08-01T00:00:00Z',
      },
    ],
  };
  render(
    <ManagerNavigation
      {...props}
      navigation={{
        status: 'ready',
        items: [{ ...item, workspaces: [quiet, { ...workspace, agents: [older, newer] }] }],
        refresh: vi.fn(),
      }}
    />,
  );
  const agents = screen.getByRole('tree', { name: 'Personal Agents' });
  expect(screen.queryByRole('group', { name: 'Atlas' })).not.toBeInTheDocument();
  expect(screen.queryByText('Atlas')).not.toBeInTheDocument();
  expect(screen.queryByText('chrono-bot')).not.toBeInTheDocument();
  expect(screen.queryByText('quiet-workspace')).not.toBeInTheDocument();
  expect([...agents.querySelectorAll('[role="treeitem"]')].map((row) => row.textContent)).toEqual([
    expect.stringContaining('Newer'),
    expect.stringContaining('Older'),
    expect.stringContaining('Quiet'),
  ]);
});

it('clears status and scope together and names the scope in the summary', async () => {
  const onScope = vi.fn();
  render(
    <ManagerNavigation
      {...props}
      client={{} as never}
      scope="shared"
      choices={{
        options: [
          { value: '', label: 'Personal' },
          { value: 'shared', label: 'Shared' },
        ],
        selected: undefined,
        unavailable: false,
      }}
      onScope={onScope}
      navigation={{ status: 'ready', items: navigationFixture(), refresh: vi.fn() }}
    />,
  );
  const user = userEvent.setup();
  const summary = () => document.querySelector('.navigation-filters__summary');
  const filter = screen.getByRole('button', { name: 'Filter' });
  // Only the non-default part is named; the default status is not restated.
  expect(summary()).toHaveTextContent(/^Shared$/);
  expect(filter).toHaveAccessibleDescription('Shared');
  expect(filter.querySelector('.navigation-filters__dot')).not.toBeNull();
  await user.click(filter);
  expect(screen.getByRole('group', { name: 'Navigation filters' })).toBeVisible();
  await chooseOption(user, 'Agent status filter', 'Working');
  expect(summary()).toHaveTextContent(/^Shared · Working$/);
  await user.click(screen.getByRole('button', { name: 'Clear filters' }));
  expect(onScope).toHaveBeenCalledWith('');
  expect(summary()).toHaveTextContent(/^Shared$/);
});
