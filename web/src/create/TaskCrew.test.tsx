import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { navigationFixture, parkedRuntimeUpdateFixture } from '../test/hierarchy';
import { TaskCrew } from './TaskCrew';

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
