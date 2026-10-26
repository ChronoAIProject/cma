import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import type { SandboxDetail } from '../api/types';
import { SandboxActionStatus, type SandboxActionRequest } from './SandboxActionStatus';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const request: SandboxActionRequest = {
  action: { kind: 'pause_sandbox', sandboxId: 'sbx_1' },
  key: 'pause-once',
  phase: 'accepted',
};

it.each(['pause_sandbox', 'revive_sandbox'] as const)(
  'shows %s submission before acceptance and confirms only the settled public state',
  async (kind) => {
    vi.useFakeTimers();
    const resume = kind === 'revive_sandbox';
    let sandbox = sandboxFixture(resume ? 'paused' : 'active');
    const readSandbox = vi.fn(async () => sandbox);
    const client = agentClient({ readSandbox });
    const onSettled = vi.fn();
    const onNavigate = vi.fn();
    const props = {
      client,
      request: { ...request, action: { kind, sandboxId: 'sbx_1' } },
      onSettled,
      onNavigate,
      onClose: vi.fn(),
    };
    const view = render(
      <SandboxActionStatus {...props} request={{ ...props.request, phase: 'submitting' }} />,
    );
    expect(screen.getByRole('status')).toHaveTextContent('Submitting');
    expect(readSandbox).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'View Sandbox progress' }));
    expect(onNavigate).toHaveBeenCalledWith({ kind: 'sandbox', sandboxId: 'sbx_1' });
    view.rerender(<SandboxActionStatus {...props} />);
    await act(async () => {});
    expect(screen.getByRole('status')).toHaveTextContent('request accepted');
    expect(onSettled).not.toHaveBeenCalled();
    const target = sandboxFixture(resume ? 'active' : 'paused');
    sandbox = {
      ...target,
      lifecycle: {
        ...target.lifecycle!,
        operation: {
          purpose: resume ? 'revive' : 'pause',
          phase: 'resume_agents',
          startedAt: new Date().toISOString(),
        },
      },
    };
    await act(() => vi.advanceTimersByTimeAsync(250));
    expect(onSettled).not.toHaveBeenCalled();
    sandbox = target;
    await act(() => vi.advanceTimersByTimeAsync(250));
    expect(onSettled).toHaveBeenCalledExactlyOnceWith(true);
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(readSandbox).toHaveBeenCalledTimes(3);
  },
);

it('bounds fast reads to ten seconds, never overlaps, and defers hidden-page reads', async () => {
  vi.useFakeTimers();
  const readSandbox = vi.fn(async () => sandboxFixture('pausing'));
  const onSettled = vi.fn();
  render(
    <SandboxActionStatus
      client={agentClient({ readSandbox })}
      request={request}
      onSettled={onSettled}
      onClose={vi.fn()}
    />,
  );
  await act(async () => {});
  await act(() => vi.advanceTimersByTimeAsync(10_000));
  expect(readSandbox).toHaveBeenCalledTimes(41);
  await act(() => vi.advanceTimersByTimeAsync(1499));
  expect(readSandbox).toHaveBeenCalledTimes(41);
  let resolve: (sandbox: SandboxDetail) => void = () => {};
  readSandbox.mockImplementationOnce(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  await act(() => vi.advanceTimersByTimeAsync(1));
  await act(() => vi.advanceTimersByTimeAsync(5000));
  fireEvent(document, new Event('visibilitychange'));
  expect(readSandbox).toHaveBeenCalledTimes(42);
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  await act(async () => resolve(sandboxFixture('pausing')));
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(readSandbox).toHaveBeenCalledTimes(42);
  hidden.mockReturnValue(false);
  await act(async () => fireEvent(document, new Event('visibilitychange')));
  expect(readSandbox).toHaveBeenCalledTimes(43);
  expect(onSettled).not.toHaveBeenCalled();
});

it('backs off on transient read failure and never reports completion after lost access', async () => {
  vi.useFakeTimers();
  const readSandbox = vi
    .fn()
    .mockRejectedValueOnce(new Error('offline'))
    .mockRejectedValueOnce(new ApiError(403, 'Denied'));
  const onSettled = vi.fn();
  render(
    <SandboxActionStatus
      client={agentClient({ readSandbox })}
      request={request}
      onSettled={onSettled}
      onClose={vi.fn()}
    />,
  );
  await act(async () => {});
  expect(screen.getByRole('alert')).toHaveTextContent('completion is not confirmed');
  await act(() => vi.advanceTimersByTimeAsync(4999));
  expect(readSandbox).toHaveBeenCalledTimes(1);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(onSettled).toHaveBeenCalledExactlyOnceWith(false);
  expect(screen.getByRole('alert')).toHaveTextContent('no longer accessible');
  await act(() => vi.advanceTimersByTimeAsync(10_000));
  expect(readSandbox).toHaveBeenCalledTimes(2);
});

it('keeps accepted progress optional and observes the target after dismissing its status', async () => {
  vi.useFakeTimers();
  let sandbox = sandboxFixture('pausing');
  const onSettled = vi.fn();
  const onClose = vi.fn();
  const onNavigate = vi.fn();
  const props = {
    client: agentClient({ readSandbox: vi.fn(async () => sandbox) }),
    request,
    onSettled,
    onClose,
    onNavigate,
  };
  const view = render(<SandboxActionStatus {...props} />);
  await act(async () => {});
  expect(screen.getByRole('status')).toHaveTextContent(/^Pause request accepted\.$/);
  expect(screen.getByText('Waiting for the Sandbox to pause.')).not.toBeVisible();
  expect(screen.getByRole('button', { name: 'View Sandbox progress' })).not.toBeVisible();
  fireEvent.click(screen.getByText('Operation progress'));
  expect(screen.getByText('Waiting for the Sandbox to pause.')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'View Sandbox progress' }));
  expect(onNavigate).toHaveBeenCalledExactlyOnceWith({ kind: 'sandbox', sandboxId: 'sbx_1' });
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss status' }));
  expect(onClose).toHaveBeenCalledOnce();
  view.rerender(<SandboxActionStatus {...props} request={{ ...request, hidden: true }} />);
  expect(screen.queryByRole('region', { name: 'Sandbox request' })).not.toBeInTheDocument();
  expect(onSettled).not.toHaveBeenCalled();
  sandbox = sandboxFixture('paused');
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(onSettled).toHaveBeenCalledExactlyOnceWith(true);
});
