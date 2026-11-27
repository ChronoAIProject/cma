import { NavigationSettingsContext } from './sidebarContext';
import { createRef, type ReactElement } from 'react';
import { act, fireEvent, render as renderView, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ManagerNavigation, type EnvironmentNavigationHandle } from './ManagerNavigation';
import type { TreePermissions, TreeProps } from './Tree';
import type { AgentStatus } from '../api/types';
import { navigationFixture } from '../test/hierarchy';
import { chooseOption, expandNavigationFilters } from '../test/select';
import { AGENT_DETAIL } from '../test/agent';
import { createClient } from '@cma/client';
import { CmaClientProvider } from '../api/context';
import { unavailableClient } from '../api/client';

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
  expect(screen.getByText('Tasks')).toBeVisible();
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
  await user.click(screen.getByRole('button', { name: 'Environments' }));
  expect(filter).toHaveAttribute('aria-expanded', 'false');
  expect(screen.queryByRole('group', { name: 'Navigation filters' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Environments' })).toHaveFocus();
  await expandNavigationFilters(user);
  expect(screen.getByRole('button', { name: 'Sandbox status filter' })).toHaveTextContent(
    'All statuses',
  );
  // Each view has its own status, so the Sandbox view is back at its defaults.
  expect(document.querySelector('.navigation-filters__summary')).toBeNull();
  expect(filter.querySelector('.navigation-filters__dot')).toBeNull();
  expect(screen.getByRole('tree', { name: 'Live sandboxes' })).toHaveTextContent('chrono-bot');
  await user.click(screen.getByRole('button', { name: 'Collapse Atlas' }));
  await user.click(screen.getByRole('button', { name: 'Tasks' }));
  expect(screen.getByRole('button', { name: 'Agent status filter' })).toHaveTextContent('Working');
  await user.click(screen.getByRole('button', { name: 'Environments' }));
  expect(screen.getByRole('button', { name: 'Expand Atlas' })).toBeInTheDocument();
  expect(screen.queryByRole('link', { name: /Agent one/ })).not.toBeInTheDocument();
});

it('shows visible task states alongside static posters and respects parent lifecycle barriers', async () => {
  const refresh = vi.fn();
  const draw = (sandbox: 'active' | 'paused', agent: AgentStatus) => (
    <ManagerNavigation
      {...props}
      navigation={{ status: 'ready', items: navigationFixture(sandbox, agent), refresh }}
    />
  );
  const view = render(draw('active', 'ready'));
  const cases = [
    ['active', 'ready', 'rest', 'Status unavailable'],
    ['active', 'starting', 'preparing', 'Preparing'],
    ['active', 'working', 'working', 'Working'],
    ['active', 'waiting_approval', 'question', 'Waiting for approval'],
    ['active', 'failed', 'failed', 'Failed'],
    ['active', 'stopped', 'rest', 'Stopped'],
    ['paused', 'working', 'rest', 'Paused'],
  ] as const;
  for (const [sandbox, agent, action, taskWord] of cases) {
    view.rerender(draw(sandbox, agent));
    const name = new RegExp(`^Agent one,\\s*${taskWord}`);
    const row = screen.getByRole('treeitem', { name });
    expect(screen.getByRole('link', { name })).toBe(row.querySelector('a'));
    expect(within(row).getByText(taskWord)).toBeVisible();
    expect(row.querySelector('img')?.getAttribute('src')).toContain(`/${action}/poster-`);
    expect(row.querySelector('.agent-mascot__sprite')).toBeNull();
    expect(row.querySelector('.agent-mascot__artwork')).toHaveAttribute('aria-hidden', 'true');
    expect(row.getAttribute('aria-label')).not.toMatch(/\d{4}-\d\d-\d\dT/);
    expect(row.querySelector('.task-running__dot') !== null).toBe(taskWord === 'Working');
    if (taskWord === 'Preparing')
      expect(row.querySelector('.task-ready')).toHaveTextContent('Preparing');
    else if (taskWord !== 'Working') expect(row.querySelector('time')).toHaveAttribute('dateTime');
  }
  const user = userEvent.setup();
  await expandNavigationFilters(user);
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
  await user.click(screen.getByRole('button', { name: 'Environments' }));
  const live = screen.getByRole('tree', { name: 'Live sandboxes' });
  expect(live.querySelector('.tree-row__state .statedot')).toHaveAttribute('data-state', 'neutral');
  expect(live.querySelectorAll('.statedot[data-state="done"]')).toHaveLength(0);
});

it('keeps the summary status while refreshing, then shows the refreshed completion', () => {
  const refresh = vi.fn();
  const base = navigationFixture()[0]!;
  const known = {
    ...base,
    workspaces: base.workspaces.map((workspace) => ({
      ...workspace,
      agents: workspace.agents.map((agent) => ({ ...agent, taskStatus: 'working' as const })),
    })),
  };
  const view = render(
    <ManagerNavigation
      {...props}
      currentAgent={AGENT_DETAIL}
      navigation={{ status: 'ready', items: [known], refresh }}
    />,
  );
  expect(screen.getByRole('treeitem', { name: /Agent one.*Working/ })).toBeInTheDocument();
  view.rerender(
    <ManagerNavigation
      {...props}
      currentAgent={{ ...AGENT_DETAIL, updatedAt: '2026-11-06T00:00:00Z' }}
      navigation={{ status: 'ready', items: [known], refresh }}
    />,
  );
  expect(screen.getByRole('treeitem', { name: /Agent one.*Working/ })).toBeInTheDocument();
  expect(refresh).toHaveBeenCalled();
  view.rerender(
    <ManagerNavigation
      {...props}
      currentAgent={{ ...AGENT_DETAIL, updatedAt: '2026-11-06T00:00:00Z' }}
      navigation={{
        status: 'ready',
        items: [
          {
            ...known,
            workspaces: known.workspaces.map((workspace) => ({
              ...workspace,
              agents: workspace.agents.map((agent) => ({
                ...agent,
                taskStatus: 'completed' as const,
              })),
            })),
          },
        ],
        refresh,
      }}
    />,
  );
  expect(screen.getByRole('treeitem', { name: /Agent one.*Turn ended/ })).toBeInTheDocument();
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

it('keeps task rows and their action menus operable by keyboard', async () => {
  const onAction = vi.fn();
  const onNavigate = vi.fn();
  render(
    <ManagerNavigation
      {...props}
      route={{ kind: 'agent', agentId: 'agt_1' }}
      permissions={{ ...permissions, stopAgent: true }}
      onAction={onAction}
      onNavigate={onNavigate}
      navigation={{
        status: 'ready',
        items: navigationFixture('active', 'working'),
        refresh: vi.fn(),
      }}
    />,
  );
  const user = userEvent.setup();
  const row = screen.getByRole('treeitem', { name: /^Agent one,\s*Working/ });
  expect(row).toHaveAttribute('aria-selected', 'true');
  row.focus();
  await user.keyboard('{Enter}');
  expect(onNavigate).toHaveBeenCalledWith({ kind: 'agent', agentId: 'agt_1' });
  // The hidden-at-rest menu key is still the next Tab stop after its row.
  await user.tab();
  const options = screen.getByRole('button', { name: 'Options for Agent one' });
  expect(options).toHaveFocus();
  await user.keyboard('{Enter}');
  await user.click(await screen.findByRole('menuitem', { name: 'Stop' }));
  expect(onAction).toHaveBeenCalledWith({ kind: 'stop_agent', agentId: 'agt_1' });
});

it('uses one accessible view control and closes task filters on entry to Environments', async () => {
  render(
    <ManagerNavigation
      {...props}
      route={{ kind: 'agent', agentId: 'agt_1' }}
      navigation={{ status: 'ready', items: navigationFixture(), refresh: vi.fn() }}
    />,
  );
  const user = userEvent.setup();
  const task = screen.getByRole('treeitem', { name: /Agent one/ });
  task.focus();
  const filter = screen.getByRole('button', { name: 'Filter' });
  filter.focus();
  await user.keyboard('{Enter}');
  const panel = screen.getByRole('group', { name: 'Navigation filters' });
  expect(panel).toBeVisible();
  expect(screen.queryByRole('menu', { name: 'Navigation filters' })).not.toBeInTheDocument();
  expect(filter).not.toHaveAttribute('aria-haspopup');
  const picker = screen.getByRole('button', { name: 'Agent status filter' });
  await user.tab();
  expect(picker).toHaveFocus();
  await chooseOption(user, 'Agent status filter', 'Working');
  expect(panel).toBeVisible();
  expect(filter).toHaveAttribute('aria-expanded', 'true');
  expect(screen.getByRole('button', { name: 'Clear filters' })).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Clear filters' }));
  expect(panel).toBeVisible();
  expect(picker).toHaveTextContent('All statuses');
  filter.focus();
  await user.tab({ shift: true });
  const environments = screen.getByRole('button', { name: 'Environments', pressed: false });
  const tasks = screen.getByRole('button', { name: 'Tasks', pressed: true });
  expect(environments).toHaveFocus();
  expect(environments.parentElement).toBe(screen.getByRole('group', { name: 'Navigation view' }));
  await user.keyboard('{Enter}');
  expect(environments).toHaveFocus();
  expect(environments).toHaveAttribute('aria-pressed', 'true');
  expect(tasks).toHaveAttribute('aria-pressed', 'false');
  expect(filter).toHaveAttribute('aria-expanded', 'false');
  expect(panel).not.toBeVisible();
  expect(screen.queryByRole('button', { name: 'Sandbox status filter' })).not.toBeInTheDocument();
  await user.tab({ shift: true });
  expect(tasks).toHaveFocus();
  await user.keyboard('{Enter}');
  expect(tasks).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByRole('treeitem', { name: /Agent one/ })).toHaveFocus();
  expect(filter).toHaveAttribute('aria-expanded', 'false');
  await expandNavigationFilters(user);
  expect(screen.getByRole('button', { name: 'Agent status filter' })).toHaveTextContent(
    'All statuses',
  );
  expect(screen.getByRole('button', { name: 'Environments' })).toBeVisible();
});

it('opens the sole Environments destination from Home and restores the focused task', async () => {
  const navigationRef = createRef<EnvironmentNavigationHandle>();
  render(
    <ManagerNavigation
      {...props}
      ref={navigationRef}
      route={{ kind: 'agent', agentId: 'agt_1' }}
      navigation={{ status: 'ready', items: navigationFixture(), refresh: vi.fn() }}
    />,
  );
  const user = userEvent.setup();
  const task = screen.getByRole('treeitem', { name: /Agent one/ });
  task.focus();
  act(() => navigationRef.current!.openEnvironments({ scope: 'personal' }));
  expect(screen.getAllByRole('button', { name: 'Environments' })).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Environments', pressed: true })).toHaveFocus();
  expect(screen.queryByRole('menu', { name: 'Navigation settings' })).not.toBeInTheDocument();
  await user.tab({ shift: true });
  await user.keyboard('{Enter}');
  expect(screen.getByRole('treeitem', { name: /Agent one/ })).toHaveFocus();
  expect(screen.queryByRole('button', { name: 'Settings' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Manage environments' })).not.toBeInTheDocument();
});

function render(view: ReactElement) {
  const wrap = (content: ReactElement) => (
    <NavigationSettingsContext value={document.body}>{content}</NavigationSettingsContext>
  );
  const result = renderView(wrap(view));
  return { ...result, rerender: (content: ReactElement) => result.rerender(wrap(content)) };
}

it('keeps the Sandbox colour beside the poster and uses a documented task fallback only when absent', () => {
  const items = navigationFixture().map((item) => ({
    ...item,
    sandbox: { ...item.sandbox, color: '#62d079' },
  }));
  const draw = (records: TreeProps['items']) => (
    <ManagerNavigation
      {...props}
      navigation={{ status: 'ready', items: records, refresh: vi.fn() }}
    />
  );
  const view = render(draw(items));
  const marker = () =>
    screen.getByRole('treeitem', { name: /^Agent one,/ }).querySelector('.tree-row__signal-color')!;
  expect(marker()).toHaveStyle({ backgroundColor: '#62d079' });
  view.rerender(
    draw(items.map((item) => ({ ...item, sandbox: { ...item.sandbox, color: undefined } }))),
  );
  expect(marker().getAttribute('style')).toContain('var(--dsw-alias-workspace-');
});

it('keeps only the visible personal tree polling after visiting environments', async () => {
  vi.useFakeTimers();
  const provenance = vi.fn(async (id: string) => ({
    agent_id: id,
    created_at: '2026-09-06T00:00:00Z',
    creation_origin: 'manual' as const,
  }));
  const channel = vi.fn(async () =>
    Response.json({
      agent_id: 'agt_1',
      channel_usage: {
        active: false,
        has_history: false,
        first_used_at: null,
        last_used_at: null,
      },
    }),
  );
  const resources = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) =>
      new URL(String(input)).pathname.endsWith('/channel-usage')
        ? channel()
        : Response.json({ items: [] }),
  });
  const client = { ...unavailableClient, readAgentProvenance: provenance };
  const view = render(
    <CmaClientProvider client={client}>
      <ManagerNavigation
        {...props}
        client={resources}
        navigation={{ status: 'ready', items: navigationFixture(), refresh: vi.fn() }}
      />
    </CmaClientProvider>,
  );
  try {
    await act(async () => {});
    expect(channel).toHaveBeenCalledOnce();
    expect(provenance).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole('button', { name: 'Environments' }));
    await act(async () => {});
    expect(channel).toHaveBeenCalledTimes(2);
    fireEvent.click(screen.getByRole('button', { name: 'Tasks' }));
    await act(async () => {});
    expect(channel).toHaveBeenCalledTimes(3);
    await act(() => vi.advanceTimersByTimeAsync(120000));
    expect(channel).toHaveBeenCalledTimes(7);
    expect(provenance).toHaveBeenCalledOnce();
  } finally {
    view.unmount();
    vi.useRealTimers();
  }
});
