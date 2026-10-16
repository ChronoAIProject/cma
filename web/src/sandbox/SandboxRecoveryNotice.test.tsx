import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { SandboxDetail, SandboxLifecycle } from '../api/types';
import { sandboxFixture } from '../test/hierarchy';
import { SandboxRecoveryNotice } from './SandboxRecoveryNotice';

const detectedAt = '2026-10-16T08:13:55Z';
function recovering(
  cause: NonNullable<SandboxLifecycle['interruption']>['cause'] = 'out_of_memory',
): SandboxDetail {
  return {
    ...sandboxFixture('reviving'),
    lifecycle: {
      pause: { allowed: false },
      revive: { allowed: false },
      interruption: { cause, detectedAt, memoryLimitMib: 8192 },
      operation: { purpose: 'revive', phase: 'checkpoint_verification', startedAt: detectedAt },
    },
  };
}
function activated(): SandboxDetail {
  return {
    ...sandboxFixture(),
    lifecycle: {
      ...sandboxFixture().lifecycle!,
      interruption: recovering().lifecycle!.interruption,
    },
  };
}
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('reports confirmed memory exhaustion and changes only with recorded recovery stages', () => {
  vi.useFakeTimers();
  const sandbox = recovering();
  const view = render(<SandboxRecoveryNotice sandbox={sandbox} />);
  expect(screen.getByRole('status')).toHaveTextContent('Recovering your sandbox');
  expect(screen.getByText(/ran out of memory. Configured memory: 8 GiB/)).toBeInTheDocument();
  expect(screen.getByText('Checking your saved work')).toBeInTheDocument();
  act(() => vi.advanceTimersByTime(60_000));
  expect(screen.getByText('Checking your saved work')).toBeInTheDocument();
  expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
  view.rerender(
    <SandboxRecoveryNotice
      sandbox={{
        ...sandbox,
        lifecycle: {
          ...sandbox.lifecycle!,
          operation: { ...sandbox.lifecycle!.operation!, phase: 'conversation_reopen' },
        },
      }}
    />,
  );
  expect(screen.getByText('Reopening your agents')).toBeInTheDocument();
  expect(screen.queryByText('Checking your saved work')).not.toBeInTheDocument();
});

it.each(['runtime_restarted', 'runtime_unavailable'] as const)(
  'never infers OOM from %s',
  (cause) => {
    render(<SandboxRecoveryNotice sandbox={recovering(cause)} />);
    expect(screen.queryByText(/ran out of memory/)).not.toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('Recovering your sandbox');
    if (cause === 'runtime_unavailable')
      expect(screen.getByText(/cause has not been confirmed/)).toBeInTheDocument();
  },
);

it('does not reuse a historical OOM as the cause of a later manual resume', () => {
  const sandbox = recovering();
  render(
    <SandboxRecoveryNotice
      sandbox={{
        ...sandbox,
        lifecycle: {
          ...sandbox.lifecycle!,
          operation: { ...sandbox.lifecycle!.operation!, startedAt: '2026-10-16T09:00:00Z' },
        },
      }}
    />,
  );
  expect(screen.getByRole('status')).toHaveTextContent('Resuming your sandbox');
  expect(screen.queryByText(/ran out of memory/)).not.toBeInTheDocument();
});

it('preserves a last stage during stale reads and never claims completion from them', () => {
  const view = render(<SandboxRecoveryNotice sandbox={recovering()} />);
  view.rerender(<SandboxRecoveryNotice sandbox={activated()} stale />);
  expect(screen.queryByText('Sandbox ready')).not.toBeInTheDocument();
  view.rerender(<SandboxRecoveryNotice sandbox={activated()} />);
  expect(screen.getByRole('status')).toHaveTextContent('Sandbox ready');
});

it('shows useful recovery failure information without promising files were recovered', () => {
  render(
    <SandboxRecoveryNotice
      sandbox={{
        ...recovering(),
        status: 'failed',
        lifecycle: { ...recovering().lifecycle!, operation: undefined },
        failure: {
          class: 'restore_failed',
          detail: 'Home verification failed.',
          remediation:
            'The saved home could not be verified. Contact support before deleting this Sandbox.',
        },
      }}
    />,
  );
  expect(screen.getByRole('status')).toHaveTextContent('Sandbox recovery needs attention');
  expect(screen.getByRole('alert')).toHaveTextContent('saved home could not be verified');
  expect(screen.queryByText('Sandbox ready')).not.toBeInTheDocument();
});

it('does not attribute an unrelated later failure to a historical interruption', () => {
  render(
    <SandboxRecoveryNotice
      sandbox={{
        ...activated(),
        status: 'failed',
        failure: { class: 'teardown_failed', detail: 'A later deletion failed.' },
      }}
      showHistory
    />,
  );
  expect(screen.queryByRole('region', { name: 'Sandbox recovery' })).not.toBeInTheDocument();
  expect(screen.getByText('Last sandbox interruption').closest('details')).not.toHaveAttribute(
    'open',
  );
});

it('expires success feedback and never repeats it after navigation or a new active read', () => {
  vi.useFakeTimers();
  const view = render(<SandboxRecoveryNotice sandbox={recovering()} />);
  view.rerender(<SandboxRecoveryNotice sandbox={activated()} />);
  expect(screen.getByRole('status')).toHaveTextContent('Sandbox ready');
  act(() => vi.advanceTimersByTime(8_000));
  act(() => vi.advanceTimersByTime(200));
  expect(screen.queryByRole('region', { name: 'Sandbox recovery' })).not.toBeInTheDocument();
  view.rerender(<SandboxRecoveryNotice sandbox={activated()} />);
  expect(screen.queryByText('Sandbox ready')).not.toBeInTheDocument();
  view.unmount();
  render(<SandboxRecoveryNotice sandbox={activated()} />);
  expect(screen.queryByText('Sandbox ready')).not.toBeInTheDocument();
});

it('allows dismissing success and keeps the interruption available only in details', () => {
  vi.useFakeTimers();
  const view = render(<SandboxRecoveryNotice sandbox={recovering()} showHistory />);
  view.rerender(<SandboxRecoveryNotice sandbox={activated()} showHistory />);
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss recovery status' }));
  act(() => vi.advanceTimersByTime(200));
  expect(screen.queryByText('Sandbox ready')).not.toBeInTheDocument();
  expect(screen.getByText('Last sandbox interruption').closest('details')).not.toHaveAttribute(
    'open',
  );
});

it('cannot carry a success notice across Sandbox identities and cancels timers on unmount', () => {
  vi.useFakeTimers();
  const view = render(<SandboxRecoveryNotice sandbox={recovering()} />);
  view.rerender(<SandboxRecoveryNotice sandbox={{ ...activated(), id: 'sbx_other' }} />);
  expect(screen.queryByText('Sandbox ready')).not.toBeInTheDocument();
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it('supports older responses without interruption metadata and exposes details without writing', () => {
  const onDetails = vi.fn();
  render(<SandboxRecoveryNotice sandbox={sandboxFixture('reviving')} onDetails={onDetails} />);
  expect(screen.getByRole('status')).toHaveTextContent('Resuming your sandbox');
  expect(screen.getByText('Waiting for the next recorded recovery stage')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Sandbox details' }));
  expect(onDetails).toHaveBeenCalledOnce();
});
