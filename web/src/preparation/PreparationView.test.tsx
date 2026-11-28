import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AgentLifecycleOperationEvent, AgentLifecycleOperationView } from '../api/client';
import { agentMascotAssets } from '../brand/agentMascotAssets';
import { PreparationView } from './PreparationView';
import { derivePreparationView, type PreparationFacts } from './preparationPresentation';

it('shows one original request and keeps recovery actions outside collapsed setup details', () => {
  const retry = vi.fn();
  const model = derivePreparationView({
    receipt: { id: 'operation' },
    readError: 'Last setup read failed',
  });
  render(
    <PreparationView
      model={model}
      message="The exact original request"
      actions={<button onClick={retry}>Retry setup status</button>}
      details={<p>Recorded provisioning stages</p>}
      notices={<p role="alert">Conversation updates interrupted</p>}
    />,
  );
  expect(screen.getAllByText('The exact original request')).toHaveLength(1);
  expect(screen.getByRole('heading', { level: 1, name: 'New agent' })).toBeVisible();
  expect(screen.getByText('Acceptance time unavailable')).toBeVisible();
  expect(screen.getByText('Last setup read failed')).toBeVisible();
  expect(screen.getByText('Conversation updates interrupted')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Retry setup status' }).closest('details')).toBeNull();
  expect(retry).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry setup status' }));
  expect(retry).toHaveBeenCalledOnce();
});

it('does not claim acceptance or a task that has not started without evidence', () => {
  render(<PreparationView model={derivePreparationView({})} message="Unknown request" />);
  expect(screen.queryByText('Request accepted')).toBeNull();
  expect(screen.getByText('Checking request acceptance')).toBeVisible();
  expect(screen.queryByText(/It hasn’t started yet/)).toBeNull();
});

it('exits the preparation surface when real task-start evidence is available', () => {
  render(
    <PreparationView
      model={derivePreparationView({ initialTask: 'started' })}
      message="Already started"
    />,
  );
  expect(screen.queryByRole('region', { name: 'Task preparation' })).toBeNull();
  expect(screen.queryByText('Already started')).toBeNull();
});

it('makes the original acceptance timestamp focusable and never turns progress into acceptance', () => {
  const model = derivePreparationView({
    receipt: { id: 'operation' },
    operation: {
      id: 'operation',
      status: 'running',
      phase: 'sandbox_allocation',
      events: [],
      accepted_at: '2026-11-23T09:00:00Z',
    },
  });
  const { container } = render(<PreparationView model={model} />);
  const time = container.querySelector('time')!;
  expect(time).toHaveAttribute('dateTime', '2026-11-23T09:00:00Z');
  expect(time).toHaveAttribute('tabindex', '0');
  expect(time.getAttribute('title')).toBe(time.getAttribute('aria-label'));
  expect(screen.queryByText(/Last progress/)).toBeNull();
});

const receipt = { id: 'operation' };
const succeeded = { id: receipt.id, status: 'succeeded', phase: 'completed', events: [] };

afterEach(() => vi.useRealTimers());

it.each([false, true])('shows live setup immediately with a shared desktop=%s', async (desktop) => {
  vi.useFakeTimers();
  const accepted = Date.now() - 5_000;
  const event = (
    sequence: number,
    phase: string,
    status: string,
    seconds: number,
  ): AgentLifecycleOperationEvent => ({
    sequence,
    phase,
    status,
    message: 'Synthetic event',
    occurred_at: new Date(accepted + seconds * 1_000).toISOString(),
  });
  const operation: AgentLifecycleOperationView = {
    id: receipt.id,
    status: 'running',
    phase: 'key_scope_ready',
    accepted_at: new Date(accepted).toISOString(),
    events: [
      event(1, 'sandbox_ready', 'running', 1),
      event(2, 'sandbox_ready', 'ready', 2),
      event(3, 'key_scope_ready', 'running', 2),
    ],
  };
  const draw = (value: AgentLifecycleOperationView, extra: PreparationFacts = {}) => (
    <PreparationView model={derivePreparationView({ receipt, operation: value, ...extra })} />
  );
  const view = render(draw(operation));
  const list = screen.getByRole('list', { name: 'Agent setup steps' });
  const row = (label: string) => within(list).getByText(label).closest('li')!;
  expect(within(row('Sandbox')).getByText('Ready')).toBeVisible();
  expect(within(row('Workspace')).getByText('Waiting')).toBeVisible();
  expect(row('Service access')).toHaveAttribute('aria-current', 'step');
  expect(row('Service access').querySelector('.statedot__ping')).toBeInTheDocument();
  expect(within(row('Agent startup')).getByText('Waiting')).toBeVisible();
  expect(screen.getByText('Setup elapsed: 5s')).toBeVisible();
  expect(screen.getByRole('status')).toHaveTextContent('Setting up service access.');
  await act(() => vi.advanceTimersByTimeAsync(1_000));
  expect(screen.getByText('Setup elapsed: 6s')).toBeVisible();
  expect(screen.getByRole('status')).toHaveTextContent('Setting up service access.');

  const phase = desktop ? 'desktop_preparation' : 'workspace_runtime';
  const label = desktop ? 'Desktop preparation' : 'Workspace';
  const activity = desktop ? 'preparing the shared desktop' : 'preparing the workspace';
  const running = {
    ...operation,
    phase,
    events: [
      ...operation.events,
      event(4, 'key_scope_ready', 'ready', 6),
      event(5, phase, 'running', 6),
    ],
  };
  view.rerender(draw(running));
  expect(within(row('Service access')).getByText('Ready')).toBeVisible();
  expect(row(label)).toHaveAttribute('aria-current', 'step');
  expect(list.querySelectorAll('.statedot__ping')).toHaveLength(1);
  await act(() => vi.advanceTimersByTimeAsync(15_000));
  expect(screen.getByRole('status')).toHaveTextContent(`Still ${activity}.`);
  expect(screen.queryByText('Retrying')).toBeNull();
  view.rerender(draw({ ...running, status: 'queued' }));
  expect(within(row(label)).getByText('Retrying')).toBeVisible();

  view.rerender(draw(running, { readError: 'The last update could not be read.' }));
  expect(screen.getByRole('status')).toHaveTextContent('Checking setup updates');
  expect(list.querySelector('.statedot__ping')).toBeNull();
  expect(within(row(label)).getByText('In progress')).toBeVisible();
  expect(screen.getByText('Setup elapsed: 21s')).toBeVisible();
  act(() => {
    vi.setSystemTime(accepted + 25_000);
    fireEvent.focus(window);
  });
  expect(screen.getByText('Setup elapsed: 25s')).toBeVisible();

  view.rerender(draw({ ...running, status: 'failed', phase: 'failed' }));
  expect(screen.getByText('Setup elapsed at last update: 6s')).toBeVisible();

  view.rerender(
    draw({
      ...running,
      status: 'failed',
      events: [...running.events, event(6, phase, 'failed', 25)],
    }),
  );
  expect(within(row(label)).getByText('Failed')).toBeVisible();
  expect(screen.getByRole('alert')).toHaveTextContent('Review the setup before trying again.');
  await act(() => vi.advanceTimersByTimeAsync(3_000));
  expect(screen.getByText('Setup elapsed: 25s')).toBeVisible();
  expect(vi.getTimerCount()).toBe(0);
});

it.each([
  ['preparing', { receipt }, 'Getting its cloud computer ready'],
  ['question', { connectionRequired: true }, 'Waiting for connection'],
  ['rest', { receipt, readError: 'Read interrupted' }, 'Setup updates interrupted'],
  ['failed', { receipt, setupFailure: 'Confirmed failure' }, 'Setup needs attention'],
  ['rest', { receipt, environmentReady: true }, 'Cloud computer ready'],
  ['rest', { receipt, operation: succeeded }, 'Cloud computer ready'],
  [
    'ready',
    { receipt, operation: { ...succeeded, agent_id: 'agent' }, agentId: 'agent' },
    'Cloud computer ready',
  ],
  [
    'rest',
    { receipt, operation: { ...succeeded, agent_id: 'old-agent' }, agentId: 'agent' },
    'Cloud computer ready',
  ],
] as const)(
  'uses one decorative %s poster with the existing setup text',
  (action, facts, title) => {
    const model = derivePreparationView(facts satisfies PreparationFacts);
    const { container } = render(<PreparationView model={model} />);
    const poster = container.querySelector('.agent-mascot__poster');
    expect(screen.getByRole('heading', { name: title })).toBeVisible();
    expect(container.querySelectorAll('.agent-mascot')).toHaveLength(1);
    expect(poster).toHaveAttribute('src', agentMascotAssets[action].light.posters[48]);
    expect(poster?.closest('[aria-hidden="true"]')).not.toBeNull();
    expect(screen.queryByRole('progressbar')).toBeNull();
  },
);
