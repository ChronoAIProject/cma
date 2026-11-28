import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { accessRequests, type AgentAccessRequest } from './accessRequests';
import { useAccessRequests } from './useAccessRequests';

const request: AgentAccessRequest = {
  id: 'request-one',
  agent_id: 'agt_one',
  status: 'applying',
  services: ['storage'],
  reason: 'Save work',
};
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('refreshes on turn start and keeps conversation updates within the polling cadence', async () => {
  vi.useFakeTimers();
  const list = vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
  const { rerender } = renderHook(
    ({ running, cursor }) => useAccessRequests('agt_one', 'owner', true, running, cursor),
    {
      initialProps: { running: false, cursor: 0 },
    },
  );
  await act(async () => {});
  expect(list).toHaveBeenCalledOnce();
  rerender({ running: true, cursor: 0 });
  await act(() => vi.advanceTimersByTimeAsync(100));
  expect(list).toHaveBeenCalledTimes(2);
  await act(() => vi.advanceTimersByTimeAsync(14_000));
  expect(list).toHaveBeenCalledTimes(5);

  rerender({ running: true, cursor: 1 });
  await act(() => vi.advanceTimersByTimeAsync(2_000));
  expect(list).toHaveBeenCalledTimes(5);
  rerender({ running: true, cursor: 1 });
  await act(() => vi.advanceTimersByTimeAsync(12_999));
  expect(list).toHaveBeenCalledTimes(5);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(list).toHaveBeenCalledTimes(6);

  rerender({ running: false, cursor: 2 });
  await act(() => vi.advanceTimersByTimeAsync(14_999));
  expect(list).toHaveBeenCalledTimes(6);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(list).toHaveBeenCalledTimes(7);
  await act(() => vi.advanceTimersByTimeAsync(29_999));
  expect(list).toHaveBeenCalledTimes(7);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(list).toHaveBeenCalledTimes(8);
});

it('retains requests on read failure but clears them across owners and ignores stale replies', async () => {
  vi.useFakeTimers();
  const list = vi.spyOn(accessRequests, 'list').mockResolvedValue([request]);
  const { result, rerender } = renderHook(
    ({ owner }) => useAccessRequests('agt_one', owner, true, false, ''),
    { initialProps: { owner: 'one' } },
  );
  await act(async () => {});
  expect(result.current.requests).toEqual([request]);
  list.mockRejectedValueOnce(new Error('unavailable'));
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(result.current.requests).toEqual([request]);
  expect(result.current.error).toBe('unavailable');
  let finish!: (value: readonly AgentAccessRequest[]) => void;
  list.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  await act(() => vi.advanceTimersByTimeAsync(2000));
  list.mockResolvedValue([]);
  rerender({ owner: 'two' });
  expect(result.current.requests).toEqual([]);
  await act(async () => {
    finish([request]);
  });
  expect(result.current.requests).toEqual([]);
  expect(result.current.error).toBeUndefined();
});

it('refreshes after a local decision and stops when disabled', async () => {
  vi.useFakeTimers();
  const list = vi.spyOn(accessRequests, 'list').mockResolvedValue([request]);
  const { result, rerender } = renderHook(
    ({ enabled }) => useAccessRequests('agt_one', 'owner', enabled, false, ''),
    { initialProps: { enabled: true } },
  );
  await act(async () => {});
  const terminal = { ...request, status: 'succeeded' as const };
  act(() => result.current.update(terminal));
  expect(result.current.requests).toEqual([terminal]);
  list.mockResolvedValue([terminal]);
  act(() => result.current.refresh());
  await act(() => vi.advanceTimersByTimeAsync(100));
  expect(list).toHaveBeenCalledTimes(2);
  rerender({ enabled: false });
  await act(() => vi.advanceTimersByTimeAsync(60000));
  expect(list).toHaveBeenCalledTimes(2);
  expect(result.current.requests).toEqual([]);
});
