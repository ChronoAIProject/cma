import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { observeAccessRequests } from './accessRequestObservation';
import type { AgentAccessRequest } from './accessRequests';

const request: AgentAccessRequest = {
  id: 'request-one',
  agent_id: 'agt_one',
  status: 'pending',
  services: ['storage'],
  reason: 'Save work',
};
let observer: ReturnType<typeof observeAccessRequests> | undefined;
beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  observer?.dispose();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

async function setup(running: boolean, requests: readonly AgentAccessRequest[] = []) {
  const read = vi.fn().mockResolvedValue(requests);
  const publish = vi.fn();
  const failed = vi.fn();
  observer = observeAccessRequests({ running: () => running, read, publish, failed });
  await vi.advanceTimersByTimeAsync(0);
  return { read, publish, failed };
}

it.each(['pending', 'connecting', 'applying'] as const)(
  'polls %s requests every two seconds until terminal',
  async (status) => {
    const { read } = await setup(false, [{ ...request, status }]);
    await vi.advanceTimersByTimeAsync(6000);
    expect(read).toHaveBeenCalledTimes(4);
    read.mockResolvedValue([{ ...request, status: 'succeeded' }]);
    await vi.advanceTimersByTimeAsync(2000);
    expect(read).toHaveBeenCalledTimes(5);
    await vi.advanceTimersByTimeAsync(29_999);
    expect(read).toHaveBeenCalledTimes(5);
    await vi.advanceTimersByTimeAsync(1);
    expect(read).toHaveBeenCalledTimes(6);
  },
);

it('backs off unchanged active turns at 2, 4, 8 and 15 seconds, then resets for pending requests', async () => {
  const { read } = await setup(true);
  for (const [index, delay] of [2000, 4000, 8000, 15000, 15000].entries()) {
    await vi.advanceTimersByTimeAsync(delay - 1);
    expect(read).toHaveBeenCalledTimes(index + 1);
    await vi.advanceTimersByTimeAsync(1);
    expect(read).toHaveBeenCalledTimes(index + 2);
  }
  read.mockResolvedValue([request]);
  observer!.refresh();
  await vi.advanceTimersByTimeAsync(2000);
  expect(read).toHaveBeenCalledTimes(7);
  await vi.advanceTimersByTimeAsync(2000);
  expect(read).toHaveBeenCalledTimes(8);
});

it('uses a thirty-second idle fallback and coalesces foreground events into an immediate read', async () => {
  const { read } = await setup(false);
  await vi.advanceTimersByTimeAsync(59_999);
  expect(read).toHaveBeenCalledTimes(2);
  for (let i = 0; i < 20; i++) window.dispatchEvent(new Event('focus'));
  await vi.advanceTimersByTimeAsync(100);
  expect(read).toHaveBeenCalledTimes(3);
});

it('keeps the adaptive polling window under continuous conversation event hints', async () => {
  const { read, publish } = await setup(true);
  await vi.advanceTimersByTimeAsync(14_000);
  expect(read).toHaveBeenCalledTimes(4);
  read.mockResolvedValue([request]);
  for (let i = 0; i < 14; i++) {
    observer!.hint();
    await vi.advanceTimersByTimeAsync(1000);
  }
  expect(read).toHaveBeenCalledTimes(4);
  await vi.advanceTimersByTimeAsync(1000);
  expect(read).toHaveBeenCalledTimes(5);
  expect(publish).toHaveBeenLastCalledWith([request]);
  await vi.advanceTimersByTimeAsync(2000);
  expect(read).toHaveBeenCalledTimes(6);
});

it('retains displayed requests on failure and backs off 2, 4, 8, 10 seconds despite refresh hints', async () => {
  const { read, publish, failed } = await setup(false, [request]);
  read.mockRejectedValue(new Error('503 unavailable'));
  await vi.advanceTimersByTimeAsync(2000);
  for (const delay of [2000, 4000, 8000, 10000]) {
    const count = read.mock.calls.length;
    observer!.refresh();
    observer!.hint();
    await vi.advanceTimersByTimeAsync(delay - 1);
    expect(read).toHaveBeenCalledTimes(count);
    await vi.advanceTimersByTimeAsync(1);
    expect(read).toHaveBeenCalledTimes(count + 1);
  }
  expect(publish).toHaveBeenCalledOnce();
  expect(publish).toHaveBeenLastCalledWith([request]);
  expect(failed).toHaveBeenCalledTimes(5);
  read.mockResolvedValue([request]);
  await vi.advanceTimersByTimeAsync(10_000);
  expect(publish).toHaveBeenCalledTimes(2);
  await vi.advanceTimersByTimeAsync(2000);
  expect(publish).toHaveBeenCalledTimes(3);
});

it('does no reads while hidden and resumes immediately on visibility', async () => {
  const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
  const { read } = await setup(false);
  await vi.advanceTimersByTimeAsync(120_000);
  expect(read).not.toHaveBeenCalled();
  visibility.mockReturnValue('visible');
  document.dispatchEvent(new Event('visibilitychange'));
  await vi.advanceTimersByTimeAsync(100);
  expect(read).toHaveBeenCalledOnce();
  visibility.mockReturnValue('hidden');
  document.dispatchEvent(new Event('visibilitychange'));
  await vi.advanceTimersByTimeAsync(120_000);
  expect(read).toHaveBeenCalledOnce();
});

it('does not overlap a slow read or publish a result after disposal', async () => {
  let finish!: (requests: readonly AgentAccessRequest[]) => void;
  const read = vi.fn(
    () =>
      new Promise<readonly AgentAccessRequest[]>((resolve) => {
        finish = resolve;
      }),
  );
  const publish = vi.fn();
  const failed = vi.fn();
  observer = observeAccessRequests({ running: () => true, read, publish, failed });
  observer.hint();
  observer.refresh();
  await vi.advanceTimersByTimeAsync(60_000);
  expect(read).toHaveBeenCalledOnce();
  observer.dispose();
  finish([request]);
  await vi.advanceTimersByTimeAsync(0);
  expect(publish).not.toHaveBeenCalled();
  expect(failed).not.toHaveBeenCalled();
});
