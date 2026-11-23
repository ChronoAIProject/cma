import { act, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError, ProtocolError, TransportError } from '@cma/client';
import type { AgentLifecycleOperationView, CmaWebClient } from '../api/client';
import { agentClient } from '../test/agent';
import { AgentLifecycleCard } from './AgentLifecycleCard';
import { lifecycleStorageKey } from './lifecycleIntent';

beforeEach(() => {
  vi.useFakeTimers();
  sessionStorage.clear();
  sessionStorage.setItem(
    lifecycleStorageKey('owner'),
    JSON.stringify({
      version: 1,
      intent: {
        key: 'retained-key',
        body: { name: 'Retained agent' },
        receipt: { id: 'retained-operation' },
      },
    }),
  );
});
afterEach(() => {
  vi.useRealTimers();
  sessionStorage.clear();
});

const pending: AgentLifecycleOperationView = {
  id: 'retained-operation',
  status: 'running',
  phase: 'sandbox_ready',
  events: [],
};

function show(read: NonNullable<CmaWebClient['readAgentLifecycle']>) {
  const create = vi.fn();
  const opened = vi.fn();
  const view = render(
    <AgentLifecycleCard
      client={agentClient({ createAgentLifecycle: create, readAgentLifecycle: read })}
      viewerId="owner"
      onClose={vi.fn()}
      onCreated={opened}
    />,
  );
  return { ...view, create, opened };
}

async function advance(milliseconds: number) {
  await act(async () => vi.advanceTimersByTimeAsync(milliseconds));
}

it('opens within the next fast read when Agent startup follows a slow earlier phase', async () => {
  let operation = pending;
  const read = vi.fn(async () => operation);
  const { create, opened, unmount } = show(read);
  await advance(10_500);
  expect(opened).not.toHaveBeenCalled();

  operation = { ...pending, phase: 'agent_startup', agent_id: 'starting-agent' };
  await advance(500);
  expect(screen.getByRole('button', { name: 'Open agent' })).toBeDisabled();
  expect(opened).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toContain('retained-key');

  operation = { ...operation, status: 'succeeded', agent_id: 'ready-agent' };
  await advance(249);
  expect(opened).not.toHaveBeenCalled();
  await advance(1);
  expect(opened).toHaveBeenCalledExactlyOnceWith({ id: 'ready-agent', status: 'ready' });
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBeNull();
  const reads = read.mock.calls.length;
  await advance(60_000);
  expect(opened).toHaveBeenCalledOnce();
  expect(read).toHaveBeenCalledTimes(reads);
  expect(create).not.toHaveBeenCalled();
  unmount();
});

it('bounds fast reads for an unchanged phase even when its event history advances', async () => {
  const read = vi.fn(async () => ({
    ...pending,
    events: [
      {
        sequence: Date.now(),
        phase: pending.phase,
        status: 'running',
        message: 'Still preparing',
        occurred_at: new Date().toISOString(),
      },
    ],
  }));
  const { opened, unmount } = show(read);
  await advance(10_000);
  const reads = read.mock.calls.length;
  await advance(999);
  expect(read).toHaveBeenCalledTimes(reads);
  await advance(1);
  expect(read).toHaveBeenCalledTimes(reads + 1);
  expect(opened).not.toHaveBeenCalled();
  unmount();
});

it('keeps one status read in flight and ignores success arriving after the dialog closes', async () => {
  let complete: ((value: AgentLifecycleOperationView) => void) | undefined;
  const read = vi.fn<NonNullable<CmaWebClient['readAgentLifecycle']>>(
    () =>
      new Promise<AgentLifecycleOperationView>((resolve) => {
        complete = resolve;
      }),
  );
  const { create, opened, unmount } = show(read);
  await advance(60_000);
  expect(read).toHaveBeenCalledOnce();
  const signal = read.mock.calls[0]?.[1]?.signal;
  unmount();
  expect(signal?.aborted).toBe(true);
  await act(async () => {
    complete?.({ ...pending, status: 'succeeded', agent_id: 'ready-agent' });
  });
  await advance(60_000);
  expect(read).toHaveBeenCalledOnce();
  expect(opened).not.toHaveBeenCalled();
  expect(create).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toContain('retained-key');
});

it('does not add an idle interval after a slow successful status read', async () => {
  const starts: number[] = [];
  let inFlight = 0;
  let maximumInFlight = 0;
  const read = vi.fn(async () => {
    starts.push(Date.now());
    maximumInFlight = Math.max(maximumInFlight, ++inFlight);
    await new Promise((resolve) => setTimeout(resolve, 350));
    inFlight -= 1;
    return starts.length === 1
      ? pending
      : { ...pending, status: 'succeeded' as const, agent_id: 'ready-agent' };
  });
  const { create, opened } = show(read);
  await advance(349);
  expect(read).toHaveBeenCalledOnce();
  expect(opened).not.toHaveBeenCalled();
  await advance(2);
  expect(read).toHaveBeenCalledTimes(2);
  expect(starts[1]! - starts[0]!).toBeLessThanOrEqual(351);
  expect(maximumInFlight).toBe(1);
  await advance(350);
  expect(opened).toHaveBeenCalledExactlyOnceWith({ id: 'ready-agent', status: 'ready' });
  await advance(5000);
  expect(read).toHaveBeenCalledTimes(2);
  expect(create).not.toHaveBeenCalled();
});

it('recovers transient status failures automatically and opens the original operation once', async () => {
  const read = vi
    .fn()
    .mockRejectedValueOnce(new TransportError('Network unavailable'))
    .mockRejectedValueOnce(new ApiError(503, 'Temporarily unavailable'))
    .mockResolvedValueOnce(pending)
    .mockResolvedValue({ ...pending, status: 'succeeded', agent_id: 'ready-agent' });
  const { create, opened } = show(read);
  await advance(499);
  expect(read).toHaveBeenCalledOnce();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toContain('retained-key');
  await advance(1);
  expect(read).toHaveBeenCalledTimes(2);
  await advance(999);
  expect(read).toHaveBeenCalledTimes(2);
  await advance(1);
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(opened).not.toHaveBeenCalled();
  await advance(250);
  expect(opened).toHaveBeenCalledExactlyOnceWith({ id: 'ready-agent', status: 'ready' });
  expect(read.mock.calls.every(([id]) => id === pending.id)).toBe(true);
  expect(create).not.toHaveBeenCalled();
});

it('caps transient retry delay at five seconds and stops pending retries on unmount', async () => {
  const read = vi.fn().mockRejectedValue(new ApiError(429, 'Try later'));
  const { unmount, create } = show(read);
  await advance(12_500);
  expect(read).toHaveBeenCalledTimes(6);
  await advance(4_999);
  expect(read).toHaveBeenCalledTimes(6);
  await advance(1);
  expect(read).toHaveBeenCalledTimes(7);
  unmount();
  await advance(30_000);
  expect(read).toHaveBeenCalledTimes(7);
  expect(create).not.toHaveBeenCalled();
});

it.each([
  new ApiError(401, 'Sign in again'),
  new ApiError(403, 'No longer authorized'),
  new ApiError(404, 'Not accessible'),
  new ProtocolError('Invalid operation identity'),
])('holds recovery after $message without erasing the original command', async (error) => {
  const read = vi.fn().mockResolvedValueOnce(pending).mockRejectedValue(error);
  const { create, opened } = show(read);
  await advance(250);
  expect(screen.getByRole('alert')).toHaveTextContent(error.message);
  await advance(30_000);
  expect(read).toHaveBeenCalledTimes(2);
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toContain('retained-key');
  expect(opened).not.toHaveBeenCalled();
  expect(create).not.toHaveBeenCalled();
});
