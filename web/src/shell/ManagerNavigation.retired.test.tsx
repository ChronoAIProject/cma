import { useNavigationFilterState } from './navigationFilterState';
import { act, render, renderHook, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createRef, useState } from 'react';
import { expect, it, vi } from 'vitest';
import type { AppRoute } from '../routing/routes';
import { AGENT_DETAIL } from '../test/agent';
import { delayedRetirementFixture, navigationFixture } from '../test/hierarchy';
import { chooseOption, expandNavigationFilters } from '../test/select';
import { ManagerNavigation, type EnvironmentNavigationHandle } from './ManagerNavigation';

const viewerId = 'retired-viewer';
const detail = {
  ...AGENT_DETAIL,
  ownerTitle: 'The retained task',
  sandbox: { ...AGENT_DETAIL.sandbox, status: 'retired' as const },
};
const witness = { detail, viewerId, authEpoch: 4 };
const props = {
  viewerId,
  authEpoch: 4,
  scope: '',
  choices: { options: [{ value: '', label: 'Personal' }], selected: undefined, unavailable: false },
  route: { kind: 'agent', agentId: detail.id } as const,
  navigation: { status: 'ready' as const, items: [], refresh: vi.fn() },
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
  pending: false,
  busySandboxIds: new Set<string>(),
  deletingSandboxIds: new Set<string>(),
  onScope: vi.fn(),
  onViewDeletion: vi.fn(),
  onNavigate: vi.fn(),
  onAction: vi.fn(),
};

it('retains the current authorized retired Agent once without inventing hierarchy', () => {
  const { container } = render(<ManagerNavigation {...props} selectedRetired={witness} />);
  expect(screen.getByRole('group', { name: 'Retired' })).toBeVisible();
  expect(screen.getAllByRole('treeitem')).toHaveLength(1);
  expect(screen.getByRole('link', { name: 'The retained task, Retired' })).toHaveAttribute(
    'aria-current',
    'page',
  );
  expect(screen.getByText('Shown because you’re viewing it.')).toBeVisible();
  expect(screen.getByText('No other agents to show.')).toBeVisible();
  expect(container.querySelector('img')?.src).toContain('/rest/poster-');
  expect(screen.queryByRole('button', { name: /Options/ })).not.toBeInTheDocument();
  expect(props.navigation.items).toEqual([]);
});

it('keeps the selected retired row through filtering and Show all clears only this scope', async () => {
  const user = userEvent.setup();
  const onScope = vi.fn();
  render(<ManagerNavigation {...props} onScope={onScope} selectedRetired={witness} />);
  await expandNavigationFilters(user);
  await chooseOption(user, 'Agent status filter', 'Working');
  expect(screen.getByRole('link', { name: 'The retained task, Retired' })).toBeVisible();
  expect(screen.getByText('No agents match these filters.')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Show all' }));
  expect(screen.getByText('No other agents to show.')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Agent status filter' })).toHaveTextContent(
    'All statuses',
  );
  expect(onScope).not.toHaveBeenCalled();
});

it('removes a supplemental row on viewer, authorization epoch, route or scope change', () => {
  const view = render(<ManagerNavigation {...props} selectedRetired={witness} />);
  for (const change of [
    { viewerId: 'another-viewer' },
    { authEpoch: 5 },
    { scope: 'shared' },
    { route: { kind: 'agent', agentId: 'agt_other' } as const },
  ]) {
    view.rerender(<ManagerNavigation {...props} {...change} selectedRetired={witness} />);
    expect(screen.queryByRole('group', { name: 'Retired' })).not.toBeInTheDocument();
  }
});

it('requires retirement and matching Agent parent identities', () => {
  const view = render(<ManagerNavigation {...props} />);
  for (const invalid of [
    { ...detail, sandbox: { ...detail.sandbox, status: 'retiring' as const } },
    { ...detail, worktree: { ...detail.worktree, agentId: 'agt_other' } },
    { ...detail, worktree: { ...detail.worktree, workspaceId: 'wsp_other' } },
    { ...detail, worktree: { ...detail.worktree, sandboxId: 'sbx_other' } },
  ]) {
    view.rerender(
      <ManagerNavigation {...props} selectedRetired={{ ...witness, detail: invalid }} />,
    );
    expect(screen.queryByRole('group', { name: 'Retired' })).not.toBeInTheDocument();
  }
});

it('keeps one cleanup footer outside the scrolling tree, including UNKNOWN without a hierarchy row', async () => {
  const user = userEvent.setup();
  render(
    <ManagerNavigation
      {...props}
      navigation={{ ...props.navigation, items: navigationFixture('retiring') }}
      unknownDeletingSandboxIds={new Set(['sbx_unknown'])}
    />,
  );
  const summary = screen.getByText('2 deletions need checking');
  expect(summary.closest('.manager-navigation__list')).toBeNull();
  await user.click(summary);
  expect(screen.getByText('Deletion not confirmed')).toBeVisible();
  expect(screen.getByText('Deleting')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Environments' }));
  expect(screen.getAllByText('2 deletions need checking')).toHaveLength(1);
  expect(screen.getByText('sbx_unknown')).toBeVisible();
});

it('repeated personal Environment entry preserves the original Tasks route, scope and scroll', async () => {
  const navigationRef = createRef<EnvironmentNavigationHandle>();
  const user = userEvent.setup();
  let observedScope = '';
  let observedRoute: AppRoute = props.route;
  function Host() {
    const [scope, setScope] = useState('organization:example');
    const [route, setRoute] = useState<AppRoute>(props.route);
    observedScope = scope;
    observedRoute = route;
    return (
      <ManagerNavigation
        {...props}
        ref={navigationRef}
        scope={scope}
        route={route}
        navigation={{ ...props.navigation, items: navigationFixture() }}
        onScope={setScope}
        onNavigate={setRoute}
      />
    );
  }
  const view = render(<Host />);
  const scroll = view.container.querySelector<HTMLElement>('.manager-navigation__list')!;
  scroll.scrollTop = 120;
  act(() => navigationRef.current!.openEnvironments({ scope: 'personal' }));
  expect(observedScope).toBe('');
  act(() => navigationRef.current!.openEnvironments({ scope: 'personal', sandboxId: 'sbx_1' }));
  expect(observedRoute).toEqual({ kind: 'sandbox', sandboxId: 'sbx_1' });
  await user.click(screen.getByRole('button', { name: 'Tasks' }));
  expect(observedScope).toBe('organization:example');
  expect(observedRoute).toEqual(props.route);
  expect(scroll.scrollTop).toBe(120);
});

it('keeps the authorized retired row when navigation fails without inventing an empty inventory', () => {
  render(
    <ManagerNavigation
      {...props}
      selectedRetired={witness}
      navigation={{ status: 'error', message: 'Navigation read unavailable', refresh: vi.fn() }}
    />,
  );
  expect(screen.getByRole('group', { name: 'Retired' })).toBeVisible();
  expect(screen.getByRole('alert')).toHaveTextContent('Navigation read unavailable');
  expect(screen.queryByText('No other agents to show.')).not.toBeInTheDocument();
});

it('targets the personal Environment filter without replacing another scope’s preferences', () => {
  const view = renderHook(({ scope }) => useNavigationFilterState(viewerId, scope), {
    initialProps: { scope: 'organization:example' },
  });
  act(() => view.result.current.select('agent', 'working'));
  act(() => view.result.current.select('sandbox', 'paused'));
  act(() => view.result.current.select('sandbox', 'failed', ''));
  expect(view.result.current.statuses).toEqual({ agent: 'working', sandbox: 'paused' });
  view.rerender({ scope: '' });
  expect(view.result.current.statuses).toEqual({ agent: 'all', sandbox: 'failed' });
  act(() => view.result.current.select('agent', 'stopped'));
  view.rerender({ scope: 'organization:example' });
  expect(view.result.current.statuses).toEqual({ agent: 'working', sandbox: 'paused' });
});

it('exposes the current retirement deadline in the persistent deletion footer', async () => {
  const delayed = delayedRetirementFixture();
  const user = userEvent.setup();
  const draw = (sandbox = delayed, refreshError?: string) => (
    <ManagerNavigation
      {...props}
      navigation={{ ...props.navigation, items: [{ sandbox, workspaces: [] }], refreshError }}
    />
  );
  const view = render(draw());
  await user.click(screen.getByText('1 deletions need checking'));
  expect(screen.getByText('Cleanup taking longer')).toBeVisible();
  expect(screen.getByText(/expected deletion time has passed/)).toBeVisible();
  expect(screen.getByRole('button', { name: 'View status for Atlas' })).toBeVisible();
  view.rerender(draw(delayed, 'Status unavailable'));
  expect(screen.queryByText('Cleanup taking longer')).not.toBeInTheDocument();
  expect(screen.getByText('Deleting')).toBeVisible();
  view.rerender(draw({ ...delayed, status: 'retired' }));
  expect(screen.queryByRole('button', { name: 'View status for Atlas' })).not.toBeInTheDocument();
});
