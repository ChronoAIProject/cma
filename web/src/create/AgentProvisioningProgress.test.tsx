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

it('reports desktop preparation separately and never infers a live frame from readiness', () => {
  const view = render(progress(operation([event(1, 'desktop_preparation', 'running', 0)])));
  expect(step('Desktop preparation').getByText('In progress')).toBeInTheDocument();
  expect(screen.queryByText('Live')).not.toBeInTheDocument();
  view.rerender(
    progress(
      operation([
        event(1, 'desktop_preparation', 'running', 0),
        event(2, 'desktop_preparation', 'ready', 4),
      ]),
    ),
  );
  expect(step('Desktop preparation').getByText('Ready')).toBeInTheDocument();
  expect(screen.queryByText('Live')).not.toBeInTheDocument();
});

it('waits for reported stages and preserves concurrent completion without inventing ordered progress', () => {
  const view = render(progress());
  expect(screen.getAllByText('Waiting')).toHaveLength(4);
  expect(
    within(screen.getByRole('list', { name: 'Agent setup steps' }))
      .getAllByRole('listitem')
      .map((item) => item.querySelector('strong')?.textContent),
  ).toEqual(['Sandbox', 'Workspace', 'Service access', 'Agent startup']);
  expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
  expect(screen.queryByText(/%/)).not.toBeInTheDocument();
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
  expect(step('Workspace').getByText('In progress')).toBeInTheDocument();
  expect(step('Service access').getByText('Ready')).toBeInTheDocument();
  expect(step('Agent startup').getByText('Waiting')).toBeInTheDocument();
  expect(screen.queryByText('Internal event details')).not.toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent('Workspace · In progress');
  expect(screen.getByText('Workspace').closest('li')).toHaveAttribute('aria-current', 'step');
  expect(screen.getByText('Sandbox').closest('li')).not.toHaveAttribute('aria-current');
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
  expect(step('Service access').getByText('Waiting for connection')).toBeInTheDocument();
  expect(step('Workspace').getByText('Waiting')).toBeInTheDocument();
  view.rerender(
    progress(
      operation(events, { status: 'failed', phase: 'failed', error: 'Permission setup failed.' }),
    ),
  );
  expect(screen.getByRole('status')).toHaveTextContent('Permission setup failed.');
  expect(step('Sandbox').getByText('Ready')).toBeInTheDocument();
  expect(step('Service access').getByText('Needs attention')).toBeInTheDocument();
  expect(
    screen.getByText('Service access').closest('li')!.querySelector('.statedot'),
  ).toHaveAttribute('data-state', 'error');
  expect(
    screen.getByRole('heading', { name: 'Agent provisioning needs attention' }),
  ).toBeInTheDocument();
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
  expect(step('Workspace').getByText('Not reported')).toBeInTheDocument();
  expect(step('Service access').getByText('Not reported')).toBeInTheDocument();
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

it('says Agent ready only after successful completion with an Agent identifier', () => {
  const events = [
    event(1, 'sandbox_ready', 'ready', 1),
    event(2, 'workspace_runtime', 'ready', 2),
    event(3, 'key_scope_ready', 'ready', 3),
    event(4, 'agent_startup', 'ready', 4),
  ];
  const view = render(progress(operation(events)));
  expect(screen.getAllByText('Ready')).toHaveLength(4);
  expect(screen.getByRole('heading', { name: 'Preparing your Agent' })).toBeInTheDocument();
  view.rerender(progress(operation(events, { status: 'succeeded' })));
  expect(screen.getByRole('heading', { name: 'Preparing your Agent' })).toBeInTheDocument();
  expect(screen.queryByText('Agent ready')).not.toBeInTheDocument();
  view.rerender(progress(operation(events, { status: 'succeeded', agent_id: 'confirmed-agent' })));
  expect(screen.getByRole('heading', { name: 'Agent ready' })).toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent('Your Agent is ready to open.');
  expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
});

it('reinforces each step state with its dot tone', () => {
  vi.useFakeTimers();
  vi.setSystemTime(start + 10_000);
  render(
    progress(
      operation(
        [
          event(1, 'sandbox_ready', 'ready', 1),
          event(2, 'workspace_runtime', 'running', 2),
          event(3, 'key_scope_ready', 'running', 3),
        ],
        { status: 'waiting_for_connection' },
      ),
    ),
  );
  const dot = (name: string) =>
    screen.getByText(name).closest('li')!.querySelector('.statedot')!.getAttribute('data-state');
  expect(dot('Sandbox')).toBe('done');
  expect(dot('Workspace')).toBe('warning');
  expect(dot('Service access')).toBe('warning');
  expect(dot('Agent startup')).toBe('neutral');
});
