import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { ApiError } from '@chronoai/cma-sdk';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import type { SandboxDetail } from '../api/types';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { PUBLIC_RECORD, publicClient } from '../test/public';
import { Progress } from '../public/Progress';
import { LifecycleProgress, elapsedDuration } from './LifecycleProgress';
import { LaunchSandboxProgress } from './LaunchSandboxProgress';

const start = '2026-09-08T09:27:18Z';
const observation = '2026-09-08T09:30:18Z';
const restoring = (phase = 'home_restore'): SandboxDetail => ({
  ...sandboxFixture('reviving'),
  lifecycle: {
    pause: { allowed: false },
    revive: { allowed: false },
    operation: { purpose: 'revive', phase, startedAt: start },
  },
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('shows server stages and a ticking elapsed time without inventing file transfers', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(observation);
  render(<LifecycleProgress sandbox={restoring()} observedAt={observation} />);
  expect(screen.getByText('Elapsed: 3m 0s')).toBeInTheDocument();
  expect(screen.getByText(/Checking your files and installed software/)).toBeInTheDocument();
  expect(screen.queryByText(/Downloading|unpacking|S3/)).not.toBeInTheDocument();
  expect(screen.getByText(/Last checked/).querySelector('time')).toHaveAttribute(
    'datetime',
    observation,
  );
  fireEvent.click(screen.getByText('Activity details'));
  expect(screen.getAllByText('Done')).toHaveLength(3);
  expect(screen.getAllByText('In progress')).toHaveLength(1);
  expect(screen.getAllByText('Waiting')).toHaveLength(3);
  await act(() => vi.advanceTimersByTimeAsync(600_000));
  expect(screen.getByText('Elapsed: 13m 0s')).toBeInTheDocument();
  expect(screen.getAllByText('Done')).toHaveLength(3);
  expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
});

it('uses the recorded start across remounts and clears preparation after activation', () => {
  vi.useFakeTimers();
  vi.setSystemTime(observation);
  const first = render(<LifecycleProgress sandbox={restoring()} />);
  first.unmount();
  const second = render(<LifecycleProgress sandbox={restoring('conversation_reopen')} />);
  expect(screen.getByText('Elapsed: 3m 0s')).toBeInTheDocument();
  expect(screen.getAllByText('Done')).toHaveLength(4);
  second.rerender(<LifecycleProgress sandbox={sandboxFixture()} />);
  expect(screen.queryByRole('region', { name: 'Sandbox progress' })).not.toBeInTheDocument();
  expect(vi.getTimerCount()).toBe(0);
});

it('does not leak unknown phases or infer completed stages from an unknown phase', async () => {
  render(<LifecycleProgress sandbox={restoring('unrecognized_wire_phase')} stale />);
  expect(screen.getByText('Preparing your sandbox')).toBeInTheDocument();
  expect(screen.queryByText('unrecognized_wire_phase')).not.toBeInTheDocument();
  expect(screen.queryByText('Done')).not.toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent('Status updates interrupted');
  await act(async () => {});
});

it('shows cancellation without falsely completing the normal pause stages', async () => {
  const sandbox = restoring();
  render(
    <LifecycleProgress
      sandbox={{
        ...sandbox,
        status: 'pausing',
        lifecycle: {
          ...sandbox.lifecycle!,
          operation: { purpose: 'pause', phase: 'resume_writers', startedAt: start },
        },
      }}
    />,
  );
  expect(screen.getAllByText('Canceling pause')).toHaveLength(2);
  expect(screen.queryByText('Done')).not.toBeInTheDocument();
  expect(screen.getAllByText('In progress')).toHaveLength(1);
  await act(async () => {});
});

it('formats elapsed time without negative, invalid or wrapped-hour values', () => {
  expect(elapsedDuration('invalid', Date.now())).toBeUndefined();
  expect(elapsedDuration(start, Date.parse(start) - 5000)).toBe('0m 0s');
  expect(elapsedDuration(start, Date.parse(start) + 3_665_000)).toBe('1h 1m 5s');
});

it('retains the last stage during an outage, recovers and aborts on unmount', async () => {
  vi.useFakeTimers();
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce(restoring())
    .mockRejectedValueOnce(new Error('private diagnostic must not be displayed'))
    .mockResolvedValue(restoring('conversation_reopen'));
  const client = agentClient({ readSandbox });
  const view = render(
    <CmaClientProvider client={client}>
      <LaunchSandboxProgress sandboxId="sbx_1" />
    </CmaClientProvider>,
  );
  await act(async () => {});
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(screen.getByRole('status')).toHaveTextContent('Status updates interrupted');
  expect(screen.getByText('Last confirmed stage')).toBeInTheDocument();
  expect(screen.queryByText(/private diagnostic/)).not.toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  expect(screen.getAllByText('Done')).toHaveLength(4);
  const signal = readSandbox.mock.calls.at(-1)?.[1].signal as AbortSignal;
  view.unmount();
  expect(signal.aborted).toBe(true);
  expect(vi.getTimerCount()).toBe(0);
});

it.each([401, 403, 404])('clears last progress after HTTP %i', async (status) => {
  vi.useFakeTimers();
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce(restoring())
    .mockRejectedValue(new ApiError(status, 'Not available'));
  render(
    <CmaClientProvider client={agentClient({ readSandbox })}>
      <LaunchSandboxProgress sandboxId="sbx_1" />
    </CmaClientProvider>,
  );
  await act(async () => {});
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(screen.queryByRole('region', { name: 'Sandbox progress' })).not.toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent('Sandbox progress is unavailable');
});

it('never overlaps reads, skips hidden pages and rejects another sandbox', async () => {
  vi.useFakeTimers();
  let resolve: (value: SandboxDetail) => void = () => {};
  const readSandbox = vi.fn(
    () =>
      new Promise<SandboxDetail>((done) => {
        resolve = done;
      }),
  );
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
  render(
    <CmaClientProvider client={agentClient({ readSandbox })}>
      <LaunchSandboxProgress sandboxId="sbx_1" />
    </CmaClientProvider>,
  );
  await act(() => vi.advanceTimersByTimeAsync(10_000));
  fireEvent(document, new Event('visibilitychange'));
  expect(readSandbox).toHaveBeenCalledTimes(1);
  await act(async () => resolve(restoring()));
  hidden.mockReturnValue(true);
  await act(() => vi.advanceTimersByTimeAsync(10_000));
  expect(readSandbox).toHaveBeenCalledTimes(1);
  hidden.mockReturnValue(false);
  fireEvent(document, new Event('visibilitychange'));
  expect(readSandbox).toHaveBeenCalledTimes(2);
  await act(async () => resolve({ ...restoring(), id: 'sbx_wrong' }));
  expect(screen.queryByRole('region', { name: 'Sandbox progress' })).not.toBeInTheDocument();
});

it('shows sandbox progress inside the preparing row of the actual Launch journey', async () => {
  const client = publicClient();
  vi.mocked(client.requests.read).mockResolvedValue({
    ...PUBLIC_RECORD,
    state: 'sandbox_pending',
    blocked_reason: { type: 'sandbox_paused_reviving' },
    resolution: { ...PUBLIC_RECORD.resolution, sandbox: { state: 'reused', id: 'sbx_1' } },
  });
  render(
    <CmaClientProvider client={agentClient({ readSandbox: async () => restoring() })}>
      <Progress client={client} id={PUBLIC_RECORD.id} onNavigate={vi.fn()} />
    </CmaClientProvider>,
  );
  const region = await screen.findByRole('region', { name: 'Sandbox progress' });
  const row = screen.getByRole('heading', { name: 'Sandbox ready' }).closest('li')!;
  expect(row).toContainElement(region);
  expect(
    within(region).getByText(/Checking your files and installed software/),
  ).toBeInTheDocument();
  expect(client.requests.start).not.toHaveBeenCalled();
});
