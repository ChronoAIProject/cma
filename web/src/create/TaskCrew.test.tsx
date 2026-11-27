import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { renderTree } from '../shell/Tree.test-support';
import { navigationFixture, parkedRuntimeUpdateFixture } from '../test/hierarchy';
import { TaskCrew } from './TaskCrew';

it.each(['working', 'ready'] as const)('shows parent artwork for a %s Agent', (status) => {
  const [item] = navigationFixture('active', status);
  const [workspace] = item!.workspaces;
  const agents = workspace!.agents.map((agent) => ({
    ...agent,
    taskStatus: status === 'working' ? ('working' as const) : ('completed' as const),
    taskAcceptance: {
      task_id: 'task-fixture',
      turn_sequence: 1,
      verdict: { status: 'succeeded' as const },
      unconfirmed_outputs: [],
    },
  }));
  const parked = parkedRuntimeUpdateFixture();
  const updating = { ...parked, runtimeUpdate: undefined };
  for (const [sandbox, poster, label] of [
    [item!.sandbox, status === 'working' ? 'working' : 'love', ''],
    [updating, 'preparing', 'Updating runtime'],
    [{ ...updating, status: 'active' }, 'preparing', 'Updating runtime'],
    [parked, 'failed', 'Runtime update failed'],
  ] as const) {
    const items = [{ sandbox, workspaces: [{ ...workspace!, agents }] }];
    const crew = render(<TaskCrew navigation={{ status: 'ready', items }} onNavigate={vi.fn()} />);
    const tree = renderTree({ view: 'agent', items, route: { kind: 'agent', agentId: 'agt_1' } });
    for (const role of ['button', 'treeitem']) {
      const row = screen.getByRole(role, { name: new RegExp(`^Agent one.*${label}`) });
      expect(row.querySelector('img')?.getAttribute('src')).toContain(`/${poster}/poster-`);
    }
    crew.unmount();
    tree.unmount();
  }
});

it('renders only a real reply and its source time, never a resource timestamp or placeholder', () => {
  const navigation = { status: 'ready' as const, items: navigationFixture('active', 'working') };
  const { rerender } = render(<TaskCrew navigation={navigation} onNavigate={vi.fn()} />);
  expect(screen.queryByText(/Updated|Replied|Open the conversation for details/)).toBeNull();
  expect(screen.queryByText('Atlas')).toBeNull();
  const text = '<img src=x onerror=synthetic> **Stored reply**';
  rerender(
    <TaskCrew
      navigation={navigation}
      onNavigate={vi.fn()}
      previews={{
        agt_1: {
          text,
          truncated: false,
          messageSequence: 9,
          turnSequence: 3,
          appendedAt: '2026-11-23T10:00:00Z',
        },
      }}
    />,
  );
  expect(screen.getByText(text)).toBeVisible();
  const card = screen.getByRole('button', { name: 'Agent one, Working' });
  expect(card.querySelectorAll('img')).toHaveLength(1);
  expect(card.querySelector('img')?.getAttribute('src')).toContain('/working/poster-');
  expect(card.querySelector('time')).toHaveAttribute('datetime', '2026-11-23T10:00:00Z');
  expect(within(card).queryByText('Working')).toBeNull();
  expect(screen.getByText('Working')).toBeVisible();
});

it('clears real reply content when the navigation observation becomes stale', () => {
  render(
    <TaskCrew
      navigation={{ status: 'ready', items: navigationFixture(), refreshError: 'Stale' }}
      onNavigate={vi.fn()}
      previews={{
        agt_1: {
          text: 'Previously read reply',
          truncated: false,
          messageSequence: 9,
          turnSequence: 3,
          appendedAt: '2026-11-23T10:00:00Z',
        },
      }}
    />,
  );
  expect(screen.queryByText('Previously read reply')).toBeNull();
  expect(screen.queryByText(/Replied/)).toBeNull();
  expect(screen.getByText('Status unavailable')).toBeVisible();
});

it('uses the existing lifecycle presentation and navigates only on an explicit card click', () => {
  const onNavigate = vi.fn();
  render(
    <TaskCrew
      navigation={{ status: 'ready', items: navigationFixture('paused', 'working') }}
      onNavigate={onNavigate}
    />,
  );
  expect(screen.getByText('Paused')).toBeVisible();
  expect(screen.queryByText('Working')).not.toBeInTheDocument();
  expect(onNavigate).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: /Agent one/ }));
  expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'agent', agentId: 'agt_1' });
});

it('does not report stale work as currently working or a failed runtime update as ready', () => {
  const items = navigationFixture('active', 'working');
  const { rerender } = render(
    <TaskCrew
      navigation={{ status: 'ready', items, refreshError: 'Refresh unavailable' }}
      onNavigate={vi.fn()}
    />,
  );
  expect(screen.getByText('Status unavailable')).toBeVisible();
  expect(screen.queryByText('The Agent is working on your task.')).not.toBeInTheDocument();
  rerender(
    <TaskCrew
      navigation={{
        status: 'ready',
        items: [{ ...items[0]!, sandbox: parkedRuntimeUpdateFixture() }],
      }}
      onNavigate={vi.fn()}
    />,
  );
  expect(screen.getByText('Needs attention')).toBeVisible();
  expect(screen.getByText('Runtime update failed')).toBeVisible();
});

it('keeps loading and failed reads distinct from an empty authorized list', () => {
  const retry = vi.fn();
  const { rerender } = render(<TaskCrew navigation={{ status: 'loading' }} onNavigate={vi.fn()} />);
  expect(screen.getByRole('status')).toHaveTextContent('Loading your crew');
  rerender(
    <TaskCrew
      navigation={{ status: 'error', message: 'Access could not be checked' }}
      onRetry={retry}
      onNavigate={vi.fn()}
    />,
  );
  expect(screen.getByRole('alert')).toHaveTextContent('Access could not be checked');
  fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
  expect(retry).toHaveBeenCalledOnce();
  rerender(
    <TaskCrew
      navigation={{ status: 'ready', items: navigationFixture('retiring') }}
      onNavigate={vi.fn()}
    />,
  );
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});
