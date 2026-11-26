import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import {
  parkedRuntimeResultFixture,
  parkedRuntimeUpdateFixture,
  workspaceFixture,
} from '../test/hierarchy';
import { LifecycleProgress } from './LifecycleProgress';
import { SandboxPage } from './SandboxPage';
import { SandboxRecoveryNotice } from './SandboxRecoveryNotice';
import { Tree } from '../shell/Tree';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
const tick = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));
const permissions = {
  createWorkspace: false,
  createAgent: false,
  pauseSandbox: false,
  deleteSandbox: false,
  removeWorkspace: false,
  stopAgent: false,
  deleteAgent: false,
  exportHistory: true,
};

it('replaces parked revival recovery animation with static attention and observes a later retry', () => {
  const sandbox = parkedRuntimeResultFixture('revive');
  const view = render(<SandboxRecoveryNotice sandbox={sandbox} />);
  expect(screen.queryByText(/Resuming —|recovery is continuing/)).not.toBeInTheDocument();
  expect(screen.getByRole('region', { name: 'Sandbox recovery' })).toHaveAttribute(
    'data-state',
    'failed',
  );
  expect(screen.getByText('Runtime update needs attention')).toBeVisible();
  expect(screen.getByText(/Last confirmed stage/)).toBeVisible();
  view.rerender(
    <SandboxRecoveryNotice
      sandbox={{
        ...sandbox,
        lifecycle: {
          ...sandbox.lifecycle!,
          result: {
            ...sandbox.lifecycle!.result!,
            state: 'in_progress',
            code: undefined,
            continuing: true,
          },
        },
      }}
    />,
  );
  expect(screen.getByText('Resuming — about 30 seconds')).toBeVisible();
});

it('keeps the recorded stage and request time without ticking or promising continued execution', async () => {
  vi.useFakeTimers();
  const readSandboxEvents = vi
    .fn()
    .mockResolvedValueOnce([])
    .mockResolvedValueOnce([
      {
        sandboxId: 'sbx_1',
        seq: 1,
        appendedAt: '2026-11-19T00:01:00Z',
        kind: {
          type: 'work_step_failed',
          step: 'run_lifecycle_operation',
          failure: { detail: 'private error' },
        },
      },
    ])
    .mockResolvedValue([]);
  const view = render(
    <LifecycleProgress
      sandbox={parkedRuntimeResultFixture('revive')}
      stale
      client={agentClient({ readSandboxEvents })}
    />,
  );
  await tick();
  expect(screen.getByText('Runtime update needs attention')).toBeVisible();
  expect(screen.getByText('Last confirmed stage')).toBeInTheDocument();
  expect(screen.queryByText('In progress')).not.toBeInTheDocument();
  expect(screen.queryByText(/Preparation continues|Elapsed:/)).not.toBeInTheDocument();
  expect(screen.getByText(/Started:/)).toBeInTheDocument();
  expect(screen.getByText(/Status updates interrupted/)).toBeVisible();
  await tick(4999);
  expect(readSandboxEvents).toHaveBeenCalledTimes(1);
  await tick(1);
  expect(readSandboxEvents).toHaveBeenCalledTimes(3);
  expect(screen.queryByText('private error')).not.toBeInTheDocument();
  expect(screen.getByRole('region', { name: 'Recorded activity' })).toHaveTextContent(
    'needs attention',
  );
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it('renders the authorized sidebar observation without a revival animation or task-failure invention', async () => {
  const sandbox = parkedRuntimeResultFixture('revive');
  const view = render(
    <Tree
      viewerId="owner"
      items={[{ sandbox, workspaces: [workspaceFixture()] }]}
      route={{ kind: 'sandbox', sandboxId: sandbox.id }}
      permissions={permissions}
      onNavigate={vi.fn()}
      onAction={vi.fn()}
    />,
  );
  await act(async () => {});
  expect(screen.getAllByText('Runtime update needs attention').length).toBeGreaterThan(0);
  expect(view.container.querySelector('[data-reviving="true"]')).toBeNull();
  expect(screen.queryByText('Resuming')).not.toBeInTheDocument();
});

it('Review runtime update opens the existing owner panel without submitting a command', async () => {
  vi.useFakeTimers();
  const sandbox = { ...parkedRuntimeUpdateFixture(), owner: 'owner' };
  const updateSandboxRuntime = vi.fn();
  const client = agentClient({ readSandbox: async () => sandbox, updateSandboxRuntime });
  const draw = (viewerId: string) => (
    <SandboxPage
      client={client}
      sandboxId={sandbox.id}
      viewerId={viewerId}
      revision={0}
      permissions={permissions}
      busy={false}
      onAction={vi.fn()}
      onNavigate={vi.fn()}
    />
  );
  const view = render(draw('owner'));
  await tick();
  fireEvent.click(screen.getByRole('tab', { name: 'Activity' }));
  fireEvent.click(screen.getByRole('button', { name: 'Review runtime update' }));
  await tick(20);
  expect(screen.getByRole('tab', { name: 'Overview' })).toHaveAttribute('aria-selected', 'true');
  expect(screen.getByRole('region', { name: 'Runtime' })).toBeVisible();
  expect(updateSandboxRuntime).not.toHaveBeenCalled();
  view.rerender(draw('another-owner'));
  await tick();
  expect(screen.queryByRole('button', { name: 'Review runtime update' })).not.toBeInTheDocument();
});
