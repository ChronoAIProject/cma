import { act, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AgentLifecycleOperationEvent, AgentLifecycleOperationView } from '../api/client';
import { AgentProvisioningProgress } from './AgentProvisioningProgress';

const start = Date.parse('2026-10-14T04:00:00Z');
const event = (
  sequence: number,
  phase: string,
  status: string,
  seconds: number,
): AgentLifecycleOperationEvent => ({
  sequence,
  phase,
  status,
  message: 'Internal event details',
  occurred_at: new Date(start + seconds * 1000).toISOString(),
});
const operation = (
  events: readonly AgentLifecycleOperationEvent[],
  extra: Partial<AgentLifecycleOperationView> = {},
): AgentLifecycleOperationView => ({
  id: 'operation-one',
  status: 'running',
  phase: events.at(-1)?.phase ?? 'queued',
  events,
  ...extra,
});
const progress = (value?: AgentLifecycleOperationView) => (
  <AgentProvisioningProgress
    receipt={{ id: 'operation-one', status: 'queued' }}
    operation={value}
  />
);
const step = (name: string) => within(screen.getByText(name).closest('li')!);

afterEach(() => vi.useRealTimers());

it('waits for reported stages and preserves concurrent completion without inventing ordered progress', () => {
  const view = render(progress());
  expect(screen.getAllByText('Waiting')).toHaveLength(4);
  expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '0');
  expect(screen.getByText('Setup details').closest('details')).not.toHaveAttribute('open');
  expect(screen.queryByText('In progress')).not.toBeInTheDocument();
  view.rerender(
    progress(
      operation([
        event(1, 'sandbox_ready', 'running', 0),
        event(2, 'sandbox_ready', 'ready', 3),
        event(3, 'key_scope_ready', 'running', 3),
        event(4, 'workspace_runtime', 'running', 4),
        event(5, 'key_scope_ready', 'ready', 5),
      ]),
    ),
  );
  expect(step('Sandbox').getByText('Ready')).toBeInTheDocument();
  expect(step('Workspace provisioning').getByText('In progress')).toBeInTheDocument();
  expect(step('Key permissions').getByText('Ready')).toBeInTheDocument();
  expect(step('Agent startup').getByText('Waiting')).toBeInTheDocument();
  expect(screen.queryByText('Internal event details')).not.toBeInTheDocument();
  expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '40');
});

it('uses server event times across remounts and stops elapsed timing at recorded Agent readiness', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(start + 10_000);
  const events = [
    event(1, 'sandbox_ready', 'running', 0),
    event(2, 'sandbox_ready', 'ready', 3),
    event(3, 'agent_startup', 'running', 4),
  ];
  const first = render(progress(operation(events)));
  expect(screen.getByText('Provisioning elapsed: 10s')).toBeInTheDocument();
  expect(step('Sandbox').getByText('3s elapsed')).toBeInTheDocument();
  expect(step('Agent startup').getByText('6s elapsed')).toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(step('Agent startup').getByText('11s elapsed')).toBeInTheDocument();
  expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '20');
  first.unmount();
  const view = render(progress(operation(events)));
  expect(screen.getByText('Provisioning elapsed: 15s')).toBeInTheDocument();
  view.rerender(
    progress(
      operation([...events, event(4, 'agent_startup', 'ready', 12)], {
        status: 'succeeded',
        agent_id: 'ready-agent',
      }),
    ),
  );
  expect(screen.getByText('Provisioning elapsed: 12s')).toBeInTheDocument();
  expect(step('Agent startup').getByText('8s elapsed')).toBeInTheDocument();
  expect(vi.getTimerCount()).toBe(0);
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(screen.getByText('Provisioning elapsed: 12s')).toBeInTheDocument();
});

it('keeps completed stages while showing connection waits and failed active stages honestly', () => {
  vi.useFakeTimers();
  vi.setSystemTime(start + 10_000);
  const events = [
    event(1, 'sandbox_ready', 'running', 0),
    event(2, 'sandbox_ready', 'ready', 3),
    event(3, 'key_scope_ready', 'running', 3),
  ];
  const view = render(progress(operation(events, { status: 'waiting_for_connection' })));
  expect(step('Key permissions').getByText('Waiting for connection')).toBeInTheDocument();
  expect(step('Workspace provisioning').getByText('Waiting')).toBeInTheDocument();
  view.rerender(
    progress(
      operation(events, { status: 'failed', phase: 'failed', error: 'Permission setup failed.' }),
    ),
  );
  expect(screen.getByRole('status')).toHaveTextContent('Permission setup failed.');
  expect(step('Sandbox').getByText('Ready')).toBeInTheDocument();
  expect(step('Key permissions').getByText('Needs attention')).toBeInTheDocument();
  expect(step('Agent startup').getByText('Not reported')).toBeInTheDocument();
  expect(screen.queryByText(/Provisioning elapsed:/)).not.toBeInTheDocument();
  expect(vi.getTimerCount()).toBe(0);
});

it('supports legacy success without inferring unreported steps or durations', () => {
  render(
    progress(
      operation([event(2, 'sandbox_ready', 'ready', 3)], {
        status: 'succeeded',
        agent_id: 'legacy-agent',
      }),
    ),
  );
  expect(step('Sandbox').getByText('Ready')).toBeInTheDocument();
  expect(step('Agent startup').getByText('Ready')).toBeInTheDocument();
  expect(step('Workspace provisioning').getByText('Not reported')).toBeInTheDocument();
  expect(step('Key permissions').getByText('Not reported')).toBeInTheDocument();
  expect(screen.queryByText(/elapsed/)).not.toBeInTheDocument();
});

it('orders recorded events by sequence and hides invalid timestamps and unknown stages', () => {
  render(
    progress(
      operation([
        event(3, 'sandbox_ready', 'ready', 3),
        { ...event(1, 'sandbox_ready', 'running', 0), occurred_at: 'invalid' },
        event(2, 'unknown_backend_phase', 'running', 2),
      ]),
    ),
  );
  expect(step('Sandbox').getByText('Ready')).toBeInTheDocument();
  expect(screen.queryByText('In progress')).not.toBeInTheDocument();
  expect(screen.queryByText('unknown_backend_phase')).not.toBeInTheDocument();
  expect(screen.queryByText(/elapsed/)).not.toBeInTheDocument();
});

it('reserves the final fifth for confirmed successful completion with an Agent identifier', () => {
  const events = [
    event(1, 'sandbox_ready', 'ready', 1),
    event(2, 'workspace_runtime', 'ready', 2),
    event(3, 'key_scope_ready', 'ready', 3),
    event(4, 'agent_startup', 'ready', 4),
  ];
  const view = render(progress(operation(events)));
  expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '80');
  expect(screen.getByRole('progressbar')).toHaveAttribute(
    'aria-valuetext',
    '4 of 5 milestones confirmed',
  );
  view.rerender(progress(operation(events, { status: 'succeeded' })));
  expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '80');
  expect(screen.queryByText('Agent ready')).not.toBeInTheDocument();
  view.rerender(progress(operation(events, { status: 'succeeded', agent_id: 'confirmed-agent' })));
  expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '100');
  expect(screen.getByText('Agent ready')).toBeInTheDocument();
});
